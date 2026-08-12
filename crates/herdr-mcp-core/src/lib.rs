pub mod config;
pub mod error;
pub mod keybindings;
pub mod session_context;

// Selective re-exports — no glob `use config::*`
pub use config::{
    CliOverrides, ClipboardConfig, Config, PersistentConfig, config_file_or_default,
    find_config_file_path, generate_default_config, upsert_clipboard_in_file,
};
pub use error::{Context, Result, bail};
pub use keybindings::{KeyCombo, Keybindings, parse_keybinding};
pub use session_context::inside_herdr;
