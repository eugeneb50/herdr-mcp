use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{error::Error, fmt, io, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Queued,
    Running,
    Done,
    Dead,
    Cancelled,
}

impl ItemStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Dead => "dead",
            Self::Cancelled => "cancelled",
        }
    }
}

impl FromStr for ItemStatus {
    type Err = serde::de::value::Error;

    fn from_str(status: &str) -> Result<Self, Self::Err> {
        Self::deserialize(serde::de::value::StrDeserializer::new(status))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Item {
    pub id: String,
    pub queue: String,
    pub seq: i64,
    pub status: ItemStatus,
    pub attempts: u32,
    pub claims: i64,
    pub max_attempts: u32,
    pub dedupe_key: Option<String>,
    pub coalesce_key: Option<String>,
    pub payload: Map<String, Value>,
    pub requester: Option<String>,
    pub waiters: Vec<String>,
    pub run_id: Option<String>,
    pub delivery: Option<String>,
    pub lease_until: Option<i64>,
    pub result: Option<Value>,
    pub enqueued_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct QueueEvent {
    pub item_id: String,
    pub seq: i64,
    pub at: i64,
    pub kind: String,
    pub data: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct QueueDepth {
    pub queue: String,
    pub queued: i64,
    pub running: i64,
    pub dead: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Push {
    pub queue: String,
    pub payload: Map<String, Value>,
    pub dedupe_key: Option<String>,
    pub coalesce_key: Option<String>,
    pub requester: Option<String>,
    pub max_attempts: u32,
    pub max_depth: Option<u32>,
}

#[derive(Debug)]
pub enum Pushed {
    New(Item),
    Deduped(Item),
    Coalesced(Item),
}

#[derive(Debug)]
pub enum Outcome {
    Done(Value),
    Cancelled,
    Failed(String),
    Dead(String),
}

#[derive(Debug)]
pub enum QueueError {
    Full {
        depth: i64,
    },
    UnsupportedSchema {
        version: i64,
    },
    InvalidQueueName(String),
    InvalidMaxAttempts,
    InvalidLease,
    InvalidDeliveryData,
    NotFound(String),
    InvalidStatus {
        id: String,
        status: ItemStatus,
    },
    DedupeConflict {
        id: String,
    },
    /// `push_unique`: another item already has this key.
    KeyTaken {
        key: String,
    },
    /// `push_unique`: the delivery this reply answers already has `reply`.
    AlreadyAnswered {
        reply: String,
    },
    Io(io::Error),
    Sqlite(rusqlite::Error),
}

impl fmt::Display for QueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full { depth } => write!(f, "queue is full (depth {depth})"),
            Self::UnsupportedSchema { version } => {
                write!(f, "unsupported queue schema version {version}")
            }
            Self::InvalidQueueName(name) => write!(f, "invalid queue name {name:?}"),
            Self::InvalidMaxAttempts => f.write_str("max attempts must be at least one"),
            Self::InvalidLease => {
                f.write_str("lease must be positive and fit in a Unix millisecond timestamp")
            }
            Self::InvalidDeliveryData => f.write_str("delivery data must be a JSON object"),
            Self::NotFound(id) => write!(f, "no such item {id}"),
            Self::InvalidStatus { id, status } => write!(f, "item {id} is {}", status.as_str()),
            Self::DedupeConflict { id } => {
                write!(f, "unfinished item {id} already owns the dedupe key")
            }
            Self::KeyTaken { key } => write!(f, "key {key} is already taken"),
            Self::AlreadyAnswered { reply } => {
                write!(f, "this delivery already has its reply ({reply})")
            }
            Self::Io(error) => write!(f, "{error}"),
            Self::Sqlite(error) => write!(f, "{error}"),
        }
    }
}

impl Error for QueueError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for QueueError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for QueueError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

/// Identifies one claim even when the retry budget is refunded or reset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Lease {
    pub id: String,
    pub claim: i64,
}

/// Fencing identity for one continuous period of mailbox ownership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Consumer {
    pub queue: String,
    pub owner: String,
    pub token: i64,
}

/// Stored mailbox lease, including its expiry in Unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConsumerRow {
    #[serde(flatten)]
    pub consumer: Consumer,
    pub until: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Fenced<T> {
    Applied(T),
    Stale,
}
