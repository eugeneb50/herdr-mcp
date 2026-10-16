//! Variable, playground and tool-catalog wire types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::recipe::RecipeStep;

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

/// Playground last-used UI state (tool selection + field values + builder).
/// Source of truth: `data/sessions/{ws}.playground.json` on the server.
/// TUI and web resolve it on demand via `GET /api/playground`; never cached.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlaygroundSnapshot {
    pub workspace_id: String,
    pub tool_name: Option<String>,
    pub field_values: HashMap<String, String>,
    pub builder_recipe_name: String,
    pub builder_steps: Vec<RecipeStep>,
    pub updated_at: DateTime<Utc>,
}

/// `GET /api/tools` response. Each tool is an MCP tool definition (name,
/// description, JSON `inputSchema`) — kept as JSON since schemas are dynamic.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolsResponse {
    pub tools: Vec<serde_json::Value>,
}
