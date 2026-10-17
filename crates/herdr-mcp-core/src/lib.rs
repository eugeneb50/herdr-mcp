pub mod config;
pub mod session_context;

// Selective re-exports — no glob `use config::*`
pub use config::{
    CliOverrides, ClipboardConfig, Config, config_file_or_default, herdr_bin,
    upsert_clipboard_in_file,
};
pub use session_context::inside_herdr;
