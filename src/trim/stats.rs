//! Savings statistics persisted per workspace.
//!
//! Mirrors llmtrim-herdr's honest accounting: net = gross − header overhead.
//! Two distinct metrics are tracked:
//! - `workspace_net_pct()` (efficiency): what fraction of *gross* savings
//!   survived the PFC1 header tax.
//! - `savings_pct()` (savings): what fraction of *total input bytes* were
//!   actually saved on the wire. This is the number shown on the badge.

use std::collections::HashMap;
use std::path::Path;
use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Per-pane savings breakdown.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PaneStat {
    pub gross_saved_bytes: usize,
    pub net_saved_bytes: usize,
    pub messages_trimmed: u64,
    pub last_trimmed_at: i64,
}

/// Workspace-level cumulative savings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrimStats {
    pub gross_saved_bytes: usize,
    pub net_saved_bytes: usize,
    pub messages_trimmed: u64,
    /// Total input bytes that were passed through the trim layer. Needed to
    /// compute the user-facing "savings %" (gross / input), distinct from the
    /// "net efficiency %" (net / gross).
    #[serde(default)]
    pub total_input_bytes: usize,
    #[serde(default)]
    pub per_pane: HashMap<String, PaneStat>,
}

impl TrimStats {
    /// Efficiency: what fraction of gross savings survived the header tax.
    pub fn workspace_net_pct(&self) -> f64 {
        if self.gross_saved_bytes == 0 {
            0.0
        } else {
            (self.net_saved_bytes as f64 / self.gross_saved_bytes as f64) * 100.0
        }
    }

    /// Savings: what fraction of total input bytes were actually saved on the wire.
    /// This is the metric surfaced on the pane badge (e.g. "-12%").
    pub fn savings_pct(&self) -> f64 {
        if self.total_input_bytes == 0 {
            0.0
        } else {
            (self.gross_saved_bytes as f64 / self.total_input_bytes as f64) * 100.0
        }
    }

    pub fn record_trim(
        &mut self,
        pane_id: &str,
        input_bytes: usize,
        output_bytes: usize,
        header_bytes: usize,
    ) {
        let gross_saved = input_bytes.saturating_sub(output_bytes);
        let net_saved = gross_saved.saturating_sub(header_bytes);

        self.gross_saved_bytes = self.gross_saved_bytes.saturating_add(gross_saved);
        self.net_saved_bytes = self.net_saved_bytes.saturating_add(net_saved);
        self.total_input_bytes = self.total_input_bytes.saturating_add(input_bytes);
        self.messages_trimmed = self.messages_trimmed.saturating_add(1);

        let pane = self.per_pane.entry(pane_id.to_string()).or_default();
        pane.gross_saved_bytes = pane.gross_saved_bytes.saturating_add(gross_saved);
        pane.net_saved_bytes = pane.net_saved_bytes.saturating_add(net_saved);
        pane.messages_trimmed = pane.messages_trimmed.saturating_add(1);
        pane.last_trimmed_at = chrono::Utc::now().timestamp();
    }
}

impl PaneStat {
    pub fn net_pct(&self) -> f64 {
        if self.gross_saved_bytes == 0 {
            0.0
        } else {
            (self.net_saved_bytes as f64 / self.gross_saved_bytes as f64) * 100.0
        }
    }
}

/// Path for a workspace's trim stats file inside the data dir.
fn stats_path(data_dir: &Path, workspace_id: &str) -> std::path::PathBuf {
    data_dir
        .join("sessions")
        .join(format!("{workspace_id}.trim_stats.json"))
}

/// Load a workspace's trim stats. Returns an empty default if the file is
/// missing (not an error — stats accrue from zero on a fresh workspace).
pub async fn load_stats(data_dir: &Path, workspace_id: &str) -> TrimStats {
    let path = stats_path(data_dir, workspace_id);
    match tokio::fs::read_to_string(&path).await {
        Ok(content) => serde_json::from_str::<TrimStats>(&content).unwrap_or_default(),
        Err(_) => TrimStats::default(),
    }
}

/// Persist a workspace's trim stats. Creates the parent `sessions/` dir if needed.
pub async fn save_stats(
    data_dir: &Path,
    workspace_id: &str,
    stats: &TrimStats,
) -> anyhow::Result<()> {
    let path = stats_path(data_dir, workspace_id);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    let json = serde_json::to_string_pretty(stats)
        .context(format!("serializing trim stats for workspace {workspace_id}"))?;
    tokio::fs::write(&path, json)
        .await
        .context(format!("writing trim stats to {}", path.display()))?;
    Ok(())
}
