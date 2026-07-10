//! Pipeline runner — the executable entry point for the trim layer.
//!
//! Wraps `pipeline::run` with persistent PFC1 "memory": a learned key merged
//! into the default key and saved between runs so repeated terms compress even
//! on the first occurrence in a new session. Usable from the CLI (`herdr-mcp
//! trim`), from the MCP tools, and from recipes.

use std::path::Path;

use crate::pfc1::{self, CompressionKey};
use crate::pipeline::{self, StageSpec};

/// On-disk memory file name (inside the data dir).
pub const MEMORY_FILE: &str = "pfc1_memory.json";

/// Runs the trim pipeline, owning the persistent base key.
pub struct PipelineRunner {
    base_key: CompressionKey,
    memory_path: Option<std::path::PathBuf>,
}

impl PipelineRunner {
    /// Build a runner rooted at `data_dir`, loading any existing memory.
    pub async fn new(data_dir: &Path) -> Self {
        let memory_path = data_dir.join(MEMORY_FILE);
        let seed = load_memory(&memory_path).await;
        let base_key = match seed {
            Some(s) => pfc1::merge_keys(&pfc1::default_key(), &s),
            None => pfc1::default_key(),
        };
        PipelineRunner {
            base_key,
            memory_path: Some(memory_path),
        }
    }

    /// Build a runner with an explicit in-memory base key (no persistence).
    #[allow(dead_code)]
    pub fn with_base_key(base_key: CompressionKey) -> Self {
        PipelineRunner {
            base_key,
            memory_path: None,
        }
    }

    /// Run `text` through the given stages. When a PFC1 stage produces a key,
    /// it is merged into persistent memory.
    pub async fn run(&self, text: &str, stages: &[StageSpec]) -> pipeline::PipelineResult {
        let result = pipeline::run(text, stages, &self.base_key);
        if let Some(path) = &self.memory_path
            && let Some(last) = result.stages.iter().rev().find_map(|s| s.pfc1_key.clone())
        {
            save_memory(path, &last).await;
        }
        result
    }

    /// Decompress text (PFC1 header-aware); no-op if no header present.
    pub fn decompress(&self, text: &str) -> String {
        pipeline::decompress_pfc1(text, None)
    }

    /// Current merged base key (default + memory).
    pub fn base_key(&self) -> &CompressionKey {
        &self.base_key
    }
}

/// Load a saved PFC1 memory key (symbol -> term) from `path`, if present.
pub async fn load_memory(path: &Path) -> Option<CompressionKey> {
    let content = tokio::fs::read_to_string(path).await.ok()?;
    serde_json::from_str::<CompressionKey>(&content).ok()
}

/// Merge `key` into the memory at `path` (union; new symbols win) and write back.
pub async fn save_memory(path: &Path, key: &CompressionKey) {
    let mut merged = load_memory(path).await.unwrap_or_default();
    for (k, v) in key {
        merged.insert(k.clone(), v.clone());
    }
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    if let Ok(s) = serde_json::to_string_pretty(&merged) {
        let _ = tokio::fs::write(path, s).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::parse_stage_specs;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn test_runner_new_and_base_key() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = PipelineRunner::new(tmp.path()).await;
        // base key should be seeded with the default key entries
        assert!(!runner.base_key().is_empty());
        assert!(runner.base_key().contains_key("Ꮜ"));
    }

    #[tokio::test]
    async fn test_runner_run_returns_result() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = PipelineRunner::new(tmp.path()).await;
        let stages = parse_stage_specs(&["caveman:full".to_string(), "pfc1".to_string()]).unwrap();
        let text = "the quick brown fox jumps over the lazy dog because it is slow";
        let result = runner.run(text, &stages).await;
        assert_eq!(result.input, text);
        assert!(!result.stages.is_empty());
        assert!(result.output.len() <= text.len());
    }

    #[tokio::test]
    async fn test_runner_decompress_headerless_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = PipelineRunner::new(tmp.path()).await;
        let plain = "this is plain text with no header";
        assert_eq!(runner.decompress(plain), plain);
    }

    #[tokio::test]
    async fn test_runner_run_saves_memory() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = PipelineRunner::new(tmp.path()).await;
        let stages = parse_stage_specs(&["pfc1".to_string()]).unwrap();
        // Use a long text with repeated technical terms so PFC1 produces a key.
        let text = "configuration session gateway profile ".repeat(50);
        let _ = runner.run(&text, &stages).await;
        let mem_path = tmp.path().join(MEMORY_FILE);
        assert!(tokio::fs::try_exists(&mem_path).await.unwrap());
    }

    #[tokio::test]
    async fn test_load_save_memory_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("mem.json");
        let mut key = CompressionKey::new();
        key.insert("Ꮜ".to_string(), "session".to_string());
        save_memory(&path, &key).await;
        let loaded = load_memory(&path).await.unwrap();
        assert_eq!(loaded.get("Ꮜ").map(|s| s.as_str()), Some("session"));
    }
}
