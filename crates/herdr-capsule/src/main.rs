use anyhow::{Context, Error as AppError, Result as AppResult};
use clap::{Parser, Subcommand};
use herdr_capsule::format as fmt;
use herdr_capsule::{
    Artifact, Capsule, CapsuleConfig, Check, Rollup, SKINS, STATUSES, Store, artifact, card,
    check_files, decisive, fail, line, no_control_chars, normalize_summary, reply_check_of,
    run_check,
};
use serde_json::{Value, json};
use std::{collections::HashSet, fs, io::Write, path::Path, thread, time::Duration};
#[derive(Parser)]
#[command(name = "capsule", about = "short messages that point at work")]
struct Cli {
    #[command(subcommand)]
    cmd: Commands,
}
#[derive(Subcommand)]
enum Commands {
    Send(#[command(flatten)] Box<SendSpec>),
    /// Answer a delivered task: `capsule reply <id>@<delivery> done "summary"`.
    /// Recipient, task and report artifact come from the task itself.
    Reply {
        /// `<capsule id>@<delivery>` exactly as the delivery text gives it.
        #[arg(value_name = "id@delivery")]
        token: String,
        #[arg(value_parser = ["done", "fail", "block"], ignore_case = true)]
        status: String,
        /// At most 120 chars; the words may be unquoted.
        #[arg(required = true, num_args = 1..)]
        summary: Vec<String>,
        /// Extra artifacts (KIND=REF); the report file is added when it exists.
        #[arg(long)]
        artifact: Vec<String>,
        #[arg(long, value_parser = SKINS)]
        skin: Option<String>,
    },
    Brief {
        #[arg(long, default_value = "lead")]
        to: String,
        #[arg(long)]
        task: String,
        #[arg(long)]
        check: Option<String>,
        #[arg(long)]
        short: bool,
    },
    Howto,
    Whoami,
    Legend,
    Line {
        #[arg(value_name = "ref")]
        reference: String,
        #[arg(long, default_value = "lead")]
        to: String,
        #[arg(long, value_parser = SKINS)]
        skin: Option<String>,
    },
    Card {
        #[arg(value_name = "ref")]
        reference: String,
        #[arg(long, default_value = "lead")]
        to: String,
        #[arg(long, value_parser = SKINS)]
        skin: Option<String>,
    },
    Json {
        #[arg(value_name = "ref")]
        reference: String,
        #[arg(long, default_value = "lead")]
        to: String,
    },
    List {
        #[arg(long, default_value = "lead")]
        to: String,
        #[arg(long)]
        all: bool,
        #[arg(long, value_parser = SKINS)]
        skin: Option<String>,
    },
    Wait {
        #[arg(long, default_value = "lead")]
        to: String,
        #[arg(long, default_value_t = 1800)]
        timeout: i64,
        #[arg(long, value_parser = SKINS)]
        skin: Option<String>,
    },
    Watch {
        #[arg(long, default_value = "lead")]
        to: String,
        #[arg(long)]
        max: Option<usize>,
        #[arg(long, value_parser = SKINS)]
        skin: Option<String>,
    },
    Rollup {
        #[arg(long, default_value = "lead")]
        to: String,
        #[arg(long)]
        task: String,
        #[arg(long)]
        send: bool,
    },
    Verify {
        #[arg(value_name = "ref")]
        reference: String,
        #[arg(long, default_value = "lead")]
        to: String,
    },
}
#[derive(Default, clap::Args)]
struct SendSpec {
    #[arg(long, default_value = "lead")]
    to: String,
    #[arg(long)]
    task: String,
    #[arg(long, value_parser = STATUSES)]
    status: String,
    #[arg(long)]
    summary: String,
    #[arg(long)]
    artifact: Vec<String>,
    #[arg(long)]
    check: Option<String>,
    #[arg(long)]
    re: Option<String>,
    /// The delivery this reply answers (from the delivery text).
    #[arg(long, requires = "re")]
    delivery: Option<String>,
    #[arg(long)]
    rollup: Option<String>,
    #[arg(long)]
    reply_to: Option<String>,
    #[arg(long)]
    reply_check: Option<String>,
    #[arg(long, value_parser = SKINS)]
    skin: Option<String>,
}
fn err(message: impl Into<String>) -> AppError {
    AppError::from(fail(message))
}
fn skin_of(cli_skin: Option<&str>, cfg: &CapsuleConfig) -> AppResult<String> {
    let skin = cli_skin
        .or(cfg.skin.as_deref())
        .unwrap_or("cap")
        .to_string();
    if !SKINS.contains(&skin.as_str()) {
        return Err(err(format!(
            "skin {} unknown; want {}",
            herdr_capsule::py_repr(&skin),
            SKINS.join(" ")
        )));
    }
    Ok(skin)
}
fn queue_name(to: &str) -> AppResult<String> {
    let valid = to
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && to
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if !valid {
        return Err(err(format!(
            "bad mailbox name {}: want [a-z0-9][a-z0-9_-]*",
            herdr_capsule::py_repr(to)
        )));
    }
    Ok(format!("mail-{to}"))
}
fn callsign(cfg: &CapsuleConfig) -> AppResult<String> {
    if let Some(from) = &cfg.from {
        return Ok(from.clone());
    }
    let pane = cfg.pane.as_deref().unwrap_or("shell");
    let path = cfg.home.join(".callsigns.json");
    let mut known: serde_json::Map<String, Value> = if path.exists() {
        serde_json::from_str(&fs::read_to_string(&path)?)?
    } else {
        serde_json::Map::new()
    };
    if let Some(name) = known.get(pane).and_then(Value::as_str) {
        return Ok(name.to_string());
    }
    let taken: HashSet<String> = known
        .values()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let name = herdr_capsule::mint(&taken, &taken)?;
    fs::create_dir_all(&cfg.home)?;
    known.insert(pane.to_string(), Value::String(name.clone()));
    fs::write(&path, serde_json::to_string_pretty(&known)?)?;
    Ok(name)
}
/// The herdr CLI used for send's optional metadata announcement.
struct CliHerdr(String);
impl CliHerdr {
    fn output(&self, args: &[&str]) -> AppResult<std::process::Output> {
        Ok(std::process::Command::new(&self.0).args(args).output()?)
    }
    fn report_metadata(&self, workspace: &str, tokens: &[(&str, &str)]) -> AppResult<()> {
        let mut args = vec![
            "workspace",
            "report-metadata",
            workspace,
            "--source",
            "capsule",
            "--ttl-ms",
            "5000",
        ];
        let pairs: Vec<String> = tokens.iter().map(|(k, v)| format!("{k}={v}")).collect();
        for pair in &pairs {
            args.push("--token");
            args.push(pair);
        }
        let out = self.output(&args)?;
        if out.status.success() {
            Ok(())
        } else {
            let bytes = if out.stderr.is_empty() {
                &out.stdout
            } else {
                &out.stderr
            };
            let msg = String::from_utf8_lossy(bytes).trim().to_string();
            Err(err(format!("herdr workspace report-metadata: {msg}")))
        }
    }
}
/// The warning to print when the trigger event could not be announced.
fn announce(cfg: &CapsuleConfig, cap: &Capsule, herdr: &CliHerdr) -> Option<String> {
    let Some(workspace) = cfg.workspace.as_deref() else {
        return Some("not in a herdr pane: no trigger event".to_string());
    };
    let tokens = [
        ("capsule", cap.id.as_str()),
        ("task", cap.task.as_str()),
        ("status", cap.status.as_str()),
        ("from", cap.from.as_str()),
        ("to", cap.to.as_str()),
        ("path", ""),
    ];
    match herdr.report_metadata(workspace, &tokens) {
        Ok(()) => None,
        Err(e) => Some(format!("trigger event failed: {e}")),
    }
}
/// The send a `reply` stands for: the task names recipient, task and report.
fn reply_spec(
    token: &str,
    status: String,
    summary: String,
    mut artifact: Vec<String>,
    cfg: &CapsuleConfig,
) -> AppResult<SendSpec> {
    let (id, delivery) = match token.split_once('@') {
        Some((id, d)) => (id, Some(d.to_string())),
        None => (token, None),
    };
    let mut store = Store::open(&cfg.queue_db).context("open queue store")?;
    let task = store.find_capsule(id)?.ok_or_else(|| {
        err(format!(
            "no capsule {}: copy <id>@<delivery> from the delivery text",
            herdr_capsule::py_repr(id)
        ))
    })?;
    if task.status != "task" {
        return Err(err(format!(
            "{id} is a {} capsule, not a task",
            task.status
        )));
    }
    // Refuse before the reply check runs; `push_unique` enforces it again in
    // the queue transaction for a reply racing this one.
    if let Some(d) = delivery.as_deref()
        && let Some(reply) = store.find_reply(&task.id, d)?
    {
        return Err(err(format!(
            "task {id} delivery {d} already has its reply {}; nothing sent",
            reply.dedupe_key.unwrap_or(reply.id)
        )));
    }
    // The path the postman told the agent wins over this CLI's own work dir.
    let report = delivery
        .as_deref()
        .map(|d| store.delivery_report(&task.id, d))
        .transpose()?
        .flatten()
        .unwrap_or_else(|| herdr_capsule::report_path(cfg, &task.task, &task.id));
    if report.is_file() {
        artifact.insert(0, format!("file={}::report", report.display()));
    } else if status == "done" && artifact.is_empty() {
        return Err(err(format!(
            "write your report first: {} (then run this reply again)",
            report.display()
        )));
    }
    Ok(SendSpec {
        to: task.reply_to.clone().unwrap_or_else(|| task.from.clone()),
        task: task.task.clone(),
        status,
        summary,
        artifact,
        re: Some(task.id.clone()),
        delivery,
        ..SendSpec::default()
    })
}
fn send_impl(
    spec: SendSpec,
    cfg: &CapsuleConfig,
    skin: &str,
    g: &fmt::Glyphs,
) -> AppResult<String> {
    let re = spec.re.as_deref().unwrap_or("");
    let reply_to = spec.reply_to.as_deref().unwrap_or("");
    let reply_check = spec.reply_check.as_deref().unwrap_or("");
    let check_arg = spec.check.as_deref().unwrap_or("");
    let delivery = spec.delivery.as_deref().unwrap_or("");
    no_control_chars(
        &[
            ("summary", spec.summary.as_str()),
            ("task", spec.task.as_str()),
            ("to", spec.to.as_str()),
            ("re", re),
            ("reply_to", reply_to),
            ("reply_check", reply_check),
            ("check", check_arg),
            ("delivery", delivery),
        ],
        &spec.artifact,
    )?;
    herdr_capsule::report_path_parts(&[("task", spec.task.as_str())])?;
    if !reply_to.is_empty() && !herdr_queue::valid_queue_name(&format!("mail-{reply_to}")) {
        return Err(err(format!(
            "--reply-to {reply_to:?}: use a mailbox label (lowercase letters, digits, - and _)"
        )));
    }
    let summary = normalize_summary(&spec.summary);
    if summary.is_empty() {
        return Err(err("--summary is empty"));
    }
    if summary.chars().count() > cfg.summary_max {
        return Err(err(format!(
            "--summary is {} chars, max {}. Terse: drop articles and filler; detail goes in an --artifact file",
            summary.chars().count(),
            cfg.summary_max
        )));
    }
    let mut arts = Vec::new();
    for a in &spec.artifact {
        arts.push(artifact(a, cfg)?);
    }
    if (spec.status == "done" || spec.status == "task") && arts.is_empty() {
        let what = if spec.status == "done" {
            "its work"
        } else {
            "its brief"
        };
        return Err(err(format!(
            "`{}` must point at {what}: add --artifact KIND=REF",
            spec.status
        )));
    }
    if spec.status != "task" && (spec.reply_check.is_some() || spec.reply_to.is_some()) {
        return Err(err("--reply-to/--reply-check are for --status task"));
    }
    let mut roll = None;
    if let Some(index) = &spec.rollup {
        let idx = Path::new(index).canonicalize()?;
        if !idx.is_file() {
            return Err(err(format!("--rollup {index}: no such index")));
        }
        let n = fs::read_to_string(&idx)?
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count();
        if n == 0 {
            return Err(err(format!("--rollup {index}: empty index")));
        }
        if arts
            .first()
            .is_none_or(|a| a.kind != "capsules" || a.reference != idx.display().to_string())
        {
            return Err(err(
                "--rollup INDEX must be this send's FIRST artifact, kind capsules",
            ));
        }
        roll = Some(Rollup { count: n });
    }
    let mut store = Store::open(&cfg.queue_db).context("open queue store")?;
    // An agent answering its delivered task without --re would leave the task
    // open until its deadline: point it at the one reply that closes it.
    if spec.re.is_none()
        && matches!(spec.status.as_str(), "done" | "fail" | "block")
        && let Some(pane) = cfg.pane.as_deref()
        && let Some((task, delivery)) = store.open_delivery(pane)?
        && task.task == spec.task
    {
        return Err(err(format!(
            "task {} was delivered to you and is waiting for its reply; send it with: {} reply {}@{} {} \"<=120 chars>\"",
            task.task,
            herdr_capsule::shlex_quote(&cfg.exe),
            task.id,
            delivery,
            spec.status
        )));
    }
    let cid = herdr_capsule::capsule_id()?;
    let check_cmd = if spec.re.is_some() && spec.status != "task" {
        reply_check_of(&mut store, re, spec.check.as_deref())?
    } else {
        spec.check.clone()
    };
    let mut check_record = None;
    if let Some(cmd) = check_cmd {
        let (exit, output) = run_check(&cmd, cfg.check_timeout)?;
        let why = decisive(&output.lines().map(str::to_string).collect::<Vec<_>>());
        if exit != 0 && spec.status == "done" {
            for w in &why {
                eprintln!("{w}");
            }
            return Err(err(format!(
                "check failed (exit {exit}); fix it, or send --status fail/block"
            )));
        }
        check_record = Some(Check {
            cmd: cmd.clone(),
            exit,
            why: (exit != 0).then_some(why),
        });
        if exit != 0 {
            let logs = cfg.home.join("logs");
            fs::create_dir_all(&logs)?;
            let log = logs.join(format!("{cid}.log"));
            // create_new: a later send whose id collides must never overwrite this log.
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&log)?
                .write_all(output.as_bytes())?;
            arts.push(Artifact {
                kind: "log".to_string(),
                reference: log.display().to_string(),
                size: Some(herdr_capsule::capsule::size(&log)?),
                note: Some("full check output".to_string()),
                ..Artifact::default()
            });
        }
    }
    let mut cap = Capsule {
        v: 2,
        id: cid.clone(),
        task: spec.task.clone(),
        from: callsign(cfg)?,
        to: spec.to.clone(),
        status: spec.status.clone(),
        summary,
        artifacts: arts,
        re: spec.re.clone(),
        delivery: spec.delivery.clone(),
        pane: cfg.pane.clone(),
        at: send_time(cfg)?,
        ..Capsule::default()
    };
    if spec.status == "task" {
        cap.reply_to = Some(spec.reply_to.clone().unwrap_or_else(|| "lead".to_string()));
        if let Some(rc) = &spec.reply_check {
            cap.reply_check = Some(rc.clone());
            cap.reply_check_files = Some(check_files(rc)?);
        }
    }
    cap.rollup = roll;
    cap.check = check_record;
    let queue = queue_name(&spec.to)?;
    herdr_capsule::store::push_unique(
        &mut store,
        &queue,
        &mut cap,
        cfg.max_attempts,
        herdr_capsule::capsule_id,
    )?;
    if let Some(bin) = &cfg.herdr {
        let herdr = CliHerdr(bin.display().to_string());
        if let Some(warn) = announce(cfg, &cap, &herdr) {
            eprintln!("capsule: {warn}");
        }
    } else {
        eprintln!("capsule: not in a herdr pane: no trigger event");
    }
    Ok(line(&cap, skin, g))
}
fn send_time(cfg: &CapsuleConfig) -> AppResult<String> {
    if let Some(now) = &cfg.now {
        if cfg.home.to_string_lossy()
            == herdr_capsule::home_dir()
                .join("code/herdr-mcp-refs/mailbox")
                .to_string_lossy()
        {
            return Err(err("CAPSULE_NOW requires a non-default CAPSULE_HOME"));
        }
        return Ok(now.clone());
    }
    Ok(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}
fn item_capsule(item: &herdr_queue::Item) -> Option<Capsule> {
    Capsule::from_value(&Value::Object(item.payload.clone())).ok()
}
fn view(
    store: &mut Store,
    to: &str,
    reference: &str,
    skin: &str,
    mode: &str,
    g: &fmt::Glyphs,
) -> AppResult<()> {
    let queue = queue_name(to)?;
    let item = store.find_q(&queue, reference)?;
    let cap = item_capsule(&item)
        .ok_or_else(|| err(format!("item {reference} in {queue} has no payload")))?;
    match mode {
        "line" => println!("{}", line(&cap, skin, g)),
        "card" => println!("{}", card(&cap, skin, g)),
        _ => println!(
            "{}",
            herdr_capsule::capsule::json_pretty(&Value::Object(item.payload.clone()))?
        ),
    }
    Ok(())
}
/// Print one claimed item, then mark it read. An item that is not a capsule
/// is named on stdout (where the reader is looking), never dropped silently.
fn read_one(
    store: &mut Store,
    item: &herdr_queue::Item,
    lease: &herdr_queue::Lease,
    skin: &str,
    g: &fmt::Glyphs,
) -> AppResult<()> {
    // Every capsule field has a default, so a payload without an id decodes.
    match Capsule::from_value(&Value::Object(item.payload.clone())) {
        Ok(cap) if !cap.id.is_empty() => println!("{}", line(&cap, skin, g)),
        Ok(_) => println!("BAD {} in {}: not a capsule: no id", item.id, item.queue),
        Err(e) => println!("BAD {} in {}: not a capsule: {e}", item.id, item.queue),
    }
    std::io::stdout().flush()?;
    consume(store, item, lease)
}
fn consume(
    store: &mut Store,
    item: &herdr_queue::Item,
    lease: &herdr_queue::Lease,
) -> AppResult<()> {
    if !store.finish(lease)? {
        eprintln!("capsule: could not mark {} read", item.id);
    }
    Ok(())
}
fn list_cmd(store: &mut Store, to: &str, all: bool, skin: &str, g: &fmt::Glyphs) -> AppResult<()> {
    let queue = queue_name(to)?;
    let status = if all {
        None
    } else {
        Some(herdr_queue::ItemStatus::Queued)
    };
    let mut items = store.list(Some(&queue), status, u32::MAX)?;
    items.sort_by(|a, b| a.id.cmp(&b.id));
    for item in items {
        if let Some(cap) = item_capsule(&item) {
            println!("{}", line(&cap, skin, g));
        }
    }
    Ok(())
}
fn wait_cmd(
    store: &mut Store,
    to: &str,
    timeout: i64,
    skin: &str,
    g: &fmt::Glyphs,
) -> AppResult<()> {
    let queue = queue_name(to)?;
    let end = std::time::Instant::now() + Duration::from_secs(timeout.max(0) as u64);
    while std::time::Instant::now() < end {
        if let Some((item, lease)) = store.claim(&queue, 60_000)? {
            return read_one(store, &item, &lease, skin, g);
        }
        thread::sleep(Duration::from_secs(1));
    }
    Err(err(format!("nothing in {queue} after {timeout}s")))
}
fn watch_cmd(
    store: &mut Store,
    to: &str,
    max: Option<usize>,
    skin: &str,
    g: &fmt::Glyphs,
) -> AppResult<()> {
    let queue = queue_name(to)?;
    let mut seen = 0usize;
    while max.is_none_or(|n| seen < n) {
        if let Some((item, lease)) = store.claim(&queue, 60_000)? {
            read_one(store, &item, &lease, skin, g)?;
            seen += 1;
        } else {
            thread::sleep(Duration::from_secs(1));
        }
    }
    Ok(())
}
fn rollup_cmd(
    store: &mut Store,
    cfg: &CapsuleConfig,
    to: &str,
    task: &str,
    send: bool,
) -> AppResult<()> {
    let queue = queue_name(to)?;
    let mut term: Vec<_> = store
        .list(Some(&queue), Some(herdr_queue::ItemStatus::Done), u32::MAX)?
        .into_iter()
        .filter(|it| it.payload.get("task").and_then(Value::as_str) == Some(task))
        .collect();
    if term.len() < 2 {
        return Err(err(format!(
            "rollup {task}: {} terminal capsule(s) in {queue}, gate is 2; unread items stay live",
            term.len()
        )));
    }
    if !send {
        println!(
            "would archive {}: {}",
            term.len(),
            term.iter()
                .filter_map(|it| it.payload.get("id").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Ok(());
    }
    let arc = cfg.home.join("archive");
    fs::create_dir_all(&arc)?;
    let seq = fs::read_dir(&arc)?
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(&format!("{task}-"))
        })
        .count()
        + 1;
    let idx = arc
        .join(format!("{task}-{seq:03}.jsonl"))
        .canonicalize()
        .unwrap_or_else(|_| arc.join(format!("{task}-{seq:03}.jsonl")));
    term.sort_by(|a, b| a.id.cmp(&b.id));
    let mut entries = Vec::new();
    for it in &term {
        let hash = payload_sha(&it.payload)?;
        entries.push(json!({
            "id": it.payload.get("id").and_then(Value::as_str).unwrap_or(""),
            "status": it.payload.get("status").and_then(Value::as_str).unwrap_or(""),
            "at": it.payload.get("at").and_then(Value::as_str).unwrap_or(""),
            "summary": it.payload.get("summary").and_then(Value::as_str).unwrap_or(""),
            "sha": hash,
        }));
    }
    let tmp = cfg
        .home
        .join("tmp")
        .join(idx.file_name().unwrap_or_default());
    fs::create_dir_all(cfg.home.join("tmp"))?;
    let mut lines = String::new();
    for entry in &entries {
        lines.push_str(&serde_json::to_string(entry)?);
        lines.push('\n');
    }
    fs::write(&tmp, lines)?;
    fs::rename(&tmp, &idx)?;
    let lessons = entries[..entries.len() - 1]
        .iter()
        .filter(|e| {
            matches!(
                e.get("status").and_then(Value::as_str),
                Some("fail" | "block")
            )
        })
        .count();
    println!(
        "archived {} -> {}",
        entries.len(),
        idx.file_name().and_then(|n| n.to_str()).unwrap_or("")
    );
    let closer = entries[entries.len() - 1]
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("");
    let summary = fmt::pack_summary(
        closer,
        entries.len(),
        lessons,
        idx.file_name().and_then(|n| n.to_str()).unwrap_or(""),
    );
    let spec = SendSpec {
        to: to.to_string(),
        task: task.to_string(),
        status: "done".to_string(),
        summary,
        artifact: vec![format!("capsules={}::rollup-index-{seq:03}", idx.display())],
        check: Some(format!("test -s {}", idx.display())),
        re: entries[entries.len() - 1]
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string),
        rollup: Some(idx.display().to_string()),
        ..SendSpec::default()
    };
    println!("{}", send_impl(spec, cfg, "cap", &fmt::Glyphs::defaults())?);
    Ok(())
}
/// The 8-hex digest a rollup index records for each original's payload.
fn payload_sha(payload: &serde_json::Map<String, Value>) -> AppResult<String> {
    let digest = <sha2::Sha256 as sha2::Digest>::digest(serde_json::to_string(payload)?.as_bytes());
    Ok(format!("{digest:x}")[..8].to_string())
}
fn verify_cmd(store: &mut Store, to: &str, reference: &str) -> AppResult<()> {
    let queue = queue_name(to)?;
    let item = store.find_q(&queue, reference)?;
    let cap =
        item_capsule(&item).ok_or_else(|| err("verify: not a rollup capsule (no payload)"))?;
    let index = cap.artifacts.iter().find(|a| a.kind == "capsules");
    if cap.rollup.is_none() || index.is_none() {
        return Err(err(
            "verify: not a rollup capsule (no rollup field / no capsules artifact)",
        ));
    }
    let index = index.expect("checked above");
    let path = Path::new(&index.reference);
    let mut bad = Vec::new();
    let mut entries: Vec<Value> = Vec::new();
    if !path.is_file() || fs::metadata(path).map(|m| m.len() == 0).unwrap_or(true) {
        bad.push(format!("index missing or empty: {}", path.display()));
    } else {
        if index.sha.as_deref() != Some(&herdr_capsule::capsule::sha8(path)?) {
            bad.push(format!("index sha != artifact sha: {}", path.display()));
        }
        let text = fs::read_to_string(path)?;
        for (n, l) in text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
        {
            match serde_json::from_str(l) {
                Ok(entry) => entries.push(entry),
                Err(e) => bad.push(format!("index line {} is not JSON: {e}", n + 1)),
            }
        }
        if entries.len() != cap.rollup.as_ref().map(|r| r.count).unwrap_or(0) {
            bad.push(format!(
                "index has {} entries, rollup.count says {}",
                entries.len(),
                cap.rollup.as_ref().map(|r| r.count).unwrap_or(0)
            ));
        }
        for entry in &entries {
            let id = entry.get("id").and_then(Value::as_str).unwrap_or("");
            match store.items_with_id(&queue, id)?.as_slice() {
                [original] => {
                    if entry.get("sha").and_then(Value::as_str)
                        != Some(payload_sha(&original.payload)?.as_str())
                    {
                        bad.push(format!("original changed since the rollup: {id}"));
                    }
                }
                [] => bad.push(format!("original not found: {id}")),
                _ => bad.push(format!("original ambiguous: {id}")),
            }
        }
    }
    if !bad.is_empty() {
        for b in &bad {
            eprintln!("{b}");
        }
        std::process::exit(1);
    }
    println!(
        "ok {}: {} originals verified (presence+sha), count {} matches, index sha8 {:?}",
        cap.id,
        entries.len(),
        cap.rollup.as_ref().map(|r| r.count).unwrap_or(0),
        index.sha
    );
    Ok(())
}
fn real_main() -> AppResult<()> {
    let cli = Cli::parse();
    let cfg = CapsuleConfig::from_env()?;
    let g = fmt::Glyphs::load(&cfg.home);
    match cli.cmd {
        Commands::Send(spec) => {
            let skin = skin_of(spec.skin.as_deref(), &cfg)?;
            println!("{}", send_impl(*spec, &cfg, &skin, &g)?);
        }
        Commands::Reply {
            token,
            status,
            summary,
            artifact,
            skin,
        } => {
            let skin = skin_of(skin.as_deref(), &cfg)?;
            let status = status.to_ascii_lowercase();
            let spec = reply_spec(&token, status, summary.join(" "), artifact, &cfg)?;
            println!("{}", send_impl(spec, &cfg, &skin, &g)?);
        }
        Commands::Brief {
            to,
            task,
            check,
            short,
        } => {
            let chk = check.as_deref().unwrap_or_default();
            let arg = if chk.is_empty() {
                String::new()
            } else {
                format!(" --check \"{chk}\"")
            };
            if short {
                print!("{}", fmt::short_brief(&cfg.exe, &to, &task, &arg));
            } else {
                let note = if chk.is_empty() {
                    ""
                } else {
                    "\n- --check runs first; done refused if it fails -> fix, or send fail."
                };
                let wk = cfg.work.display().to_string();
                print!("{}", fmt::brief(&cfg.exe, &wk, &to, &task, &arg, note));
            }
        }
        Commands::Howto => print!("{}", fmt::howto(&cfg.exe)),
        Commands::Whoami => println!("{}", callsign(&cfg)?),
        Commands::Legend => println!("{}", fmt::legend(&cfg.home, &g)),
        Commands::Line {
            reference,
            to,
            skin,
        } => {
            let skin = skin_of(skin.as_deref(), &cfg)?;
            let mut store = Store::open(&cfg.queue_db).context("open queue store")?;
            view(&mut store, &to, &reference, &skin, "line", &g)?;
        }
        Commands::Card {
            reference,
            to,
            skin,
        } => {
            let skin = skin_of(skin.as_deref(), &cfg)?;
            let mut store = Store::open(&cfg.queue_db).context("open queue store")?;
            view(&mut store, &to, &reference, &skin, "card", &g)?;
        }
        Commands::Json { reference, to } => {
            let mut store = Store::open(&cfg.queue_db).context("open queue store")?;
            view(&mut store, &to, &reference, "cap", "json", &g)?;
        }
        Commands::List { to, all, skin } => {
            let skin = skin_of(skin.as_deref(), &cfg)?;
            let mut store = Store::open(&cfg.queue_db).context("open queue store")?;
            list_cmd(&mut store, &to, all, &skin, &g)?;
        }
        Commands::Wait { to, timeout, skin } => {
            let skin = skin_of(skin.as_deref(), &cfg)?;
            let mut store = Store::open(&cfg.queue_db).context("open queue store")?;
            wait_cmd(&mut store, &to, timeout, &skin, &g)?;
        }
        Commands::Watch { to, max, skin } => {
            let skin = skin_of(skin.as_deref(), &cfg)?;
            let mut store = Store::open(&cfg.queue_db).context("open queue store")?;
            watch_cmd(&mut store, &to, max, &skin, &g)?;
        }
        Commands::Rollup { to, task, send } => {
            let mut store = Store::open(&cfg.queue_db).context("open queue store")?;
            rollup_cmd(&mut store, &cfg, &to, &task, send)?;
        }
        Commands::Verify { reference, to } => {
            let mut store = Store::open(&cfg.queue_db).context("open queue store")?;
            verify_cmd(&mut store, &to, &reference)?;
        }
    }
    Ok(())
}
fn main() {
    if let Err(e) = real_main() {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}
