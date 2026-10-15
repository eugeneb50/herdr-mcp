//! Recipe, execution and template wire types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<RecipeStep>,
    pub variables: HashMap<String, serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub is_template: bool,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeStep {
    pub id: String,
    pub tool: String,
    pub params: serde_json::Value,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ExecutionStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub id: Uuid,
    pub recipe_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: ExecutionStatus,
    pub results: HashMap<String, serde_json::Value>,
    pub variables: HashMap<String, serde_json::Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub variables: Vec<TemplateVariable>,
    pub steps: Vec<RecipeStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateVariable {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub default_value: Option<serde_json::Value>,
    #[serde(default)]
    pub required: bool,
}

/// `POST /api/recipe` body — run an ad-hoc recipe.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecipeRequest {
    #[serde(default)]
    pub name: Option<String>,
    /// herdr workspace id (e.g. "w1"). Variables are scoped to this workspace so
    /// recipes can chain across its panes/tabs.
    #[serde(default)]
    pub session_id: Option<String>,
    pub steps: Vec<RecipeStep>,
}

/// `POST /api/recipe` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeResponse {
    pub results: HashMap<String, serde_json::Value>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `POST /api/recipes` body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRecipeRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub steps: Vec<RecipeStep>,
    #[serde(default)]
    pub variables: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub category: Option<String>,
}

/// `PUT /api/recipes/{id}` body — every field optional (partial update).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateRecipeRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub steps: Option<Vec<RecipeStep>>,
    pub variables: Option<HashMap<String, serde_json::Value>>,
}

/// `GET /api/recipes/{id}/export` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportScriptResponse {
    pub script: String,
}
