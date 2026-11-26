//! Queries and statistics for mailboxes, stored as `mail-<label>` queues.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use crate::recipe::QueueItemResponse as MailItemResponse;

/// Query for a page of mailbox items with their event histories.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailListQuery {
    /// Exact recipient label.
    pub to: Option<String>,
    /// Queue status (queued, running, done, dead, or cancelled).
    pub status: Option<String>,
    /// Maximum number of results. Defaults to 100 and is capped at 1000.
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailItemQuery {
    /// Exact queue item ID, as returned in the list's item object.
    pub id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailStatsQuery {
    /// Count events at or after this Unix epoch millisecond.
    #[schemars(range(min = 0))]
    pub since: Option<i64>,
}

/// Gauges and last activity describe the retained mailbox. Counters and
/// latency samples honor the requested window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailLabelStats {
    pub to: String,
    pub depth: u64,
    pub in_flight: u64,
    pub delivered: u64,
    pub replied: u64,
    pub failed: u64,
    pub dead: u64,
    pub median_reply_ms: Option<f64>,
    pub p90_reply_ms: Option<i64>,
    pub last_activity_at: Option<i64>,
}
