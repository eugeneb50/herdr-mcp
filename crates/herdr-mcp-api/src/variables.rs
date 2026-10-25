//! Variable and tool-catalog wire types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

/// `POST /api/variables` body.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SaveVariableRequest {
    pub key: String,
    #[serde(default)]
    pub value: serde_json::Value,
    #[serde(default)]
    pub id: Option<Uuid>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub execution_id: Option<String>,
}

/// `GET /api/variables/auto` response: live panes, registry agents and (for
/// a workspace) its session variables, resolved on request. `workspace_id`
/// is empty when none was asked for.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutoVariablesResponse {
    pub workspace_id: String,
    pub variables: HashMap<String, serde_json::Value>,
}

/// `POST /api/variables/resolve` body: `value` with its `{{ }}` placeholders
/// resolved in the scope a run would start with — `variables`, then the
/// recipe's (`recipe_id`), then globals, then live agents.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResolveRequest {
    #[serde(default)]
    pub recipe_id: Option<Uuid>,
    /// herdr workspace id, as for a run.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub variables: HashMap<String, serde_json::Value>,
    pub value: serde_json::Value,
}

/// `POST /api/variables/resolve` response.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResolveResponse {
    pub value: serde_json::Value,
    /// Placeholders nothing in scope resolves (left as written), e.g. a
    /// step result that only exists during a run.
    pub unresolved: Vec<String>,
}

/// `GET /api/tools` response. Each tool is an MCP tool definition (name,
/// description, JSON `inputSchema`) — kept as JSON since schemas are dynamic.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolsResponse {
    pub tools: Vec<serde_json::Value>,
}
