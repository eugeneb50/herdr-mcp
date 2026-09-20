//! Shared persistence types for the TUI dashboard.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A persistent variable stored in the herdr-mcp data directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableStore {
    pub id: Uuid,
    pub session_id: Option<Uuid>,
    pub execution_id: Option<Uuid>,
    pub key: String,
    pub value: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
