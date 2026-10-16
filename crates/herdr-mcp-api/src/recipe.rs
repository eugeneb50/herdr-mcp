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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipeStep {
    pub id: String,
    pub tool: String,
    pub params: serde_json::Value,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipeTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub variables: Vec<TemplateVariable>,
    pub steps: Vec<RecipeStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateVariable {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub default_value: Option<serde_json::Value>,
    #[serde(default)]
    pub required: bool,
}

/// `POST /api/templates/{id}/instantiate` body: values for the template's
/// variables (missing ones take their defaults) and an optional name. The
/// response is the filled-in [`Recipe`], not saved.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstantiateTemplateRequest {
    #[serde(default)]
    pub variables: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub name: Option<String>,
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
