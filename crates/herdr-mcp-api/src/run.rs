//! Background recipe runs (`/api/runs`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::RecipeStep;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
    /// The record says running/paused but no live run owns it (server
    /// restarted mid-run). Reported on read; agents may still be in their panes.
    Interrupted,
}

impl RunStatus {
    /// True once the run can no longer change.
    pub fn is_finished(self) -> bool {
        !matches!(self, Self::Running | Self::Paused)
    }
}

/// One run of a recipe: what ran, where it got to, and what it produced.
/// Stored at `data/runs/{id}/run.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: Uuid,
    /// The saved recipe this run came from; `None` for ad-hoc runs.
    pub recipe_id: Option<Uuid>,
    pub name: Option<String>,
    /// herdr workspace the run is scoped to.
    pub session_id: String,
    /// Snapshot of the steps as they were when the run started.
    pub steps: Vec<RecipeStep>,
    pub status: RunStatus,
    /// Step being executed (or paused before) right now.
    pub current_step: Option<String>,
    /// Per-step tool results (or `{error, statusCode}`), keyed by step id.
    pub results: HashMap<String, serde_json::Value>,
    /// Interpolation scope at the end of the run.
    pub variables: HashMap<String, serde_json::Value>,
    pub failed_step: Option<String>,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// `POST /api/runs` body: a saved recipe (`recipe_id`) or ad-hoc `steps`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StartRunRequest {
    #[serde(default)]
    pub recipe_id: Option<Uuid>,
    #[serde(default)]
    pub steps: Option<Vec<RecipeStep>>,
    #[serde(default)]
    pub name: Option<String>,
    /// herdr workspace id; defaults to the recipe's workspace hint, else "w1".
    #[serde(default)]
    pub session_id: Option<String>,
}

/// `pause` / `resume` / `cancel` for `POST /api/runs/{id}/{action}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunAction {
    Pause,
    Resume,
    Cancel,
}

impl RunAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Cancel => "cancel",
        }
    }
}
