#![forbid(unsafe_code)]
pub mod capsule;
mod check;
mod config;
pub mod format;
pub mod id;
pub mod store;
pub use capsule::{
    Artifact, Capsule, Check, KINDS, Rollup, SKINS, STATUSES, artifact, no_control_chars,
    normalize_summary, report_path_parts,
};
pub use check::{check_files, decisive, reply_check_of, run_check};
pub use config::CapsuleConfig;
pub use format::{card, delivery_text, head, line, pack_summary, report_path, shlex_quote};
pub use id::{capsule_id, confusable, mint, syllables};
use std::path::PathBuf;
pub use store::Store;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("capsule: {0}")]
    Msg(String),
    #[error("capsule: {0}")]
    Queue(#[from] herdr_queue::QueueError),
    #[error("capsule: {0}")]
    Io(#[from] std::io::Error),
    #[error("capsule: {0}")]
    Json(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
pub fn fail(msg: impl Into<String>) -> Error {
    Error::Msg(msg.into())
}
/// Python's `!r` for a plain string: single quotes unless that would need more
/// escaping than double quotes (`repr("a'b") == '"a\\'b"'`).
pub fn py_repr(s: &str) -> String {
    if s.contains('\'') && !s.contains('"') {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
