//! Recipe, execution and template wire types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::{Gate, RecipeTrigger};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// herdr events that start a run of this recipe.
    #[serde(default)]
    pub triggers: Vec<RecipeTrigger>,
    /// What a full run does after the last request (end, restart, fail).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<Gate>,
    /// How long its runs are kept, and its failure snapshots.
    #[serde(default, skip_serializing_if = "RunTrail::is_default")]
    pub trail: RunTrail,
}

/// A collection's own run-history settings. A field left `None` uses the
/// global `runs.*` setting; 0 means no limit (no snapshot for
/// `snapshot_lines`).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct RunTrail {
    /// Finished runs of this collection kept; older ones are deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_runs: Option<u32>,
    /// Finished runs older than this many days are deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_days: Option<u32>,
    /// Lines of the pane a failed run last used, saved into its trail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_lines: Option<u32>,
}

impl RunTrail {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// One request in a collection (recipe): a tool call, plus the variables
/// its result sets for later requests.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RecipeStep {
    /// Unique within the recipe; later steps read this one's result as
    /// `{{ id.path }}`.
    pub id: String,
    /// The herdr-mcp tool to call.
    pub tool: String,
    /// The tool's arguments; strings may hold `{{ ... }}` placeholders.
    pub params: serde_json::Value,
    pub description: Option<String>,
    /// Applied in order once the step succeeds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extract: Vec<Extract>,
}

/// Save part of a step's result as a variable: `path` is followed into the
/// result like a `{{ step.path }}` placeholder (`panes[0].pane_id`; empty
/// for the whole text the tool returned).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Extract {
    pub path: String,
    pub var: String,
    #[serde(default)]
    pub scope: VarScope,
}

/// Where an [`Extract`] saves its variable. Either way, later steps of the
/// same run see it at once.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum VarScope {
    /// `/api/variables`: every run and request sees it.
    #[default]
    Global,
    /// The recipe's own `variables` (needs a saved recipe).
    Collection,
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
    #[serde(default)]
    pub triggers: Vec<RecipeTrigger>,
    #[serde(default)]
    pub gate: Option<Gate>,
    #[serde(default)]
    pub trail: RunTrail,
}

/// `PUT /api/recipes/{id}` body — every field optional (partial update).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateRecipeRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub steps: Option<Vec<RecipeStep>>,
    pub variables: Option<HashMap<String, serde_json::Value>>,
    pub triggers: Option<Vec<RecipeTrigger>>,
    /// The folder the playground files it under; `""` clears it.
    pub category: Option<String>,
    /// A gate without rules clears it.
    pub gate: Option<Gate>,
    /// Replaces the collection's run-history settings.
    pub trail: Option<RunTrail>,
}
