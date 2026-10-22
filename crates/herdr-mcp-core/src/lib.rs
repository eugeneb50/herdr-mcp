pub mod config;
pub mod session_context;

// Selective re-exports — no glob `use config::*`
pub use config::{
    CliOverrides, ClipboardConfig, Config, PLUGIN_ID, config_file_or_default, herdr_bin,
    reset_settings_in_file, set_clipboard_in_file, set_setting_in_file,
};
pub use session_context::inside_herdr;
