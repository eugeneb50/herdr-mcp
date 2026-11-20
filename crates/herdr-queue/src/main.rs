use clap::{Args, Parser, Subcommand};
use herdr_queue::{
    AuditError, Fenced, Item, ItemStatus, Lease, Outcome, Push, Pushed, Queue, QueueError,
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::{
    env,
    error::Error,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

const DEFAULT_LEASE_MS: u64 = 60_000;

#[derive(Parser)]
#[command(about = "A durable SQLite work queue", args_override_self = true)]
struct Cli {
    #[arg(long, global = true, env = "HERDR_QUEUE_DB")]
    db: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct LeaseArgs {
    #[arg(long)]
    id: String,
    #[arg(long)]
    lease: i64,
}

impl From<LeaseArgs> for Lease {
    fn from(args: LeaseArgs) -> Self {
        Self {
            id: args.id,
            claim: args.lease,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    Push {
        #[arg(long)]
        queue: String,
        #[arg(long, value_parser = |s: &str| serde_json::from_str::<Map<String, Value>>(s))]
        payload: Map<String, Value>,
        #[arg(long)]
        dedupe: Option<String>,
        #[arg(long)]
        coalesce: Option<String>,
        #[arg(long)]
        requester: Option<String>,
        #[arg(long, default_value_t = 3)]
        max_attempts: u32,
        #[arg(long)]
        max_depth: Option<u32>,
    },
    Claim {
        #[arg(long)]
        queue: String,
        #[arg(long, default_value_t = DEFAULT_LEASE_MS)]
        lease_ms: u64,
    },
    Done {
        #[command(flatten)]
        lease: LeaseArgs,
        #[arg(long, value_parser = |s: &str| serde_json::from_str::<Value>(s))]
        result: Option<Value>,
    },
    Fail {
        #[command(flatten)]
        lease: LeaseArgs,
        #[arg(long)]
        error: String,
    },
    Release {
        #[command(flatten)]
        lease: LeaseArgs,
    },
    Heartbeat {
        #[command(flatten)]
        lease: LeaseArgs,
        #[arg(long, default_value_t = DEFAULT_LEASE_MS)]
        lease_ms: u64,
    },
    Reap {
        #[arg(long)]
        queue: Option<String>,
    },
    List {
        #[arg(long)]
        queue: Option<String>,
        #[arg(long)]
        status: Option<ItemStatus>,
        #[arg(long, default_value_t = u32::MAX)]
        limit: u32,
    },
    Events {
        #[arg(long)]
        id: String,
    },
    Cancel {
        #[arg(long)]
        id: String,
    },
    Retry {
        #[arg(long)]
        id: String,
    },
    Depth,
    Prune {
        #[arg(long, allow_hyphen_values = true)]
        finished_before_ms: i64,
    },
    Audit {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        driver: Option<PathBuf>,
        #[arg(long)]
        max: Option<usize>,
    },
}

#[derive(Serialize)]
struct CliItem<'a> {
    #[serde(flatten)]
    item: &'a Item,
    #[serde(skip_serializing_if = "Option::is_none")]
    lease: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pushed: Option<&'static str>,
}

impl<'a> From<&'a Item> for CliItem<'a> {
    fn from(item: &'a Item) -> Self {
        Self {
            item,
            lease: None,
            pushed: None,
        }
    }
}

fn applied<T>(fenced: Fenced<T>) -> Result<T, io::Error> {
    match fenced {
        Fenced::Applied(value) => Ok(value),
        Fenced::Stale => Err(io::Error::other("stale lease")),
    }
}

fn default_db() -> Result<PathBuf, io::Error> {
    let base = match env::var_os("XDG_DATA_HOME").filter(|var| !var.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => env::home_dir()
            .ok_or_else(|| io::Error::other("no home directory, pass --db"))?
            .join(".local/share"),
    };
    Ok(base.join("herdr-queue/queue.db"))
}

fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    // Audit writes its own text output to stdout; it must not go through the
    // JSON writer below, or the output won't be byte-identical to Python.
    if let Command::Audit { db, driver, max } = &cli.command {
        herdr_queue::run_audit(db, *max, driver.as_deref())?;
        return Ok(());
    }

    let db = match cli.db {
        Some(path) => path,
        None => default_db()?,
    };
    let mut db = Queue::open(db)?;
    let now = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
    let output = match cli.command {
        Command::Push {
            queue,
            payload,
            dedupe,
            coalesce,
            requester,
            max_attempts,
            max_depth,
        } => {
            let push = Push {
                queue,
                payload,
                dedupe_key: dedupe,
                coalesce_key: coalesce,
                requester,
                max_attempts,
                max_depth,
            };
            let (item, pushed) = match db.push(push, now)? {
                Pushed::New(item) => (item, "new"),
                Pushed::Deduped(item) => (item, "deduped"),
                Pushed::Coalesced(item) => (item, "coalesced"),
            };
            json!(CliItem {
                pushed: Some(pushed),
                ..CliItem::from(&item)
            })
        }
        Command::Claim { queue, lease_ms } => {
            let claimed = db.claim(&queue, lease_ms, now)?;
            json!(claimed.as_ref().map(|(item, token)| CliItem {
                lease: Some(token.claim),
                ..CliItem::from(item)
            }))
        }
        Command::Done { lease, result } => {
            let item = applied(db.finish(
                &lease.into(),
                Outcome::Done(result.unwrap_or(Value::Null)),
                now,
            )?)?;
            json!(CliItem::from(&item))
        }
        Command::Fail { lease, error } => {
            let item = applied(db.finish(&lease.into(), Outcome::Failed(error), now)?)?;
            json!(CliItem::from(&item))
        }
        Command::Release { lease } => {
            applied(db.release(&lease.into(), now)?)?;
            json!(true)
        }
        Command::Heartbeat { lease, lease_ms } => {
            applied(db.heartbeat(&lease.into(), lease_ms, now)?)?;
            json!(true)
        }
        Command::Reap { queue } => {
            let items = match queue {
                Some(queue) => db.reap_queue(&queue, now)?,
                None => db.reap(now)?,
            };
            json!(items.iter().map(CliItem::from).collect::<Vec<_>>())
        }
        Command::List {
            queue,
            status,
            limit,
        } => {
            let items = db.list(queue.as_deref(), status, limit)?;
            json!(items.iter().map(CliItem::from).collect::<Vec<_>>())
        }
        Command::Events { id } => {
            let events = db.item(&id)?.map(|(_, events)| events).unwrap_or_default();
            json!(events)
        }
        Command::Cancel { id } => json!(CliItem::from(&db.cancel(&id, now)?)),
        Command::Retry { id } => json!(CliItem::from(&db.retry(&id, now)?)),
        Command::Depth => json!(db.depths()?),
        Command::Prune { finished_before_ms } => json!(db.prune(finished_before_ms)?),
        Command::Audit { .. } => unreachable!("audit handled before match"),
    };
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &output)?;
    writeln!(stdout)?;
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if matches!(
                error.downcast_ref::<AuditError>(),
                Some(AuditError::Violations(_))
            ) {
                return ExitCode::FAILURE;
            }
            eprintln!("herdr-queue: {error}");
            if matches!(
                error.downcast_ref::<QueueError>(),
                Some(QueueError::Full { .. })
            ) {
                ExitCode::from(3)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}
