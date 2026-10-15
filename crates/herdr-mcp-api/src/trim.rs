//! Trim savings and diagnose wire types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Per-pane savings breakdown returned by `trim_status`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PaneStatus {
    pub gross_saved_bytes: usize,
    pub net_saved_bytes: usize,
    pub messages_trimmed: u64,
}

/// Aggregate trim savings across a workspace (or all workspaces).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrimStatusResponse {
    pub workspace_net_pct: f64,
    pub workspace_savings_pct: f64,
    pub gross_saved_bytes: usize,
    pub net_saved_bytes: usize,
    pub total_input_bytes: usize,
    pub messages_trimmed: u64,
    pub per_pane: HashMap<String, PaneStatus>,
    pub active_policies: HashMap<String, Vec<String>>,
}

/// A single round-trip sample used by `trim_diagnose`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleRoundtrip {
    pub input: String,
    pub compressed: String,
    pub decompressed: String,
    pub matches: bool,
}

/// End-to-end readiness report returned by `trim_diagnose`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnoseReport {
    pub pipeline_roundtrip_ok: bool,
    pub pfc1_memory_valid: bool,
    pub active_policies: usize,
    pub badge_reachable: bool,
    pub sample_roundtrip: Option<SampleRoundtrip>,
}
