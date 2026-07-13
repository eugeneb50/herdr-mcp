pub mod config;
pub mod error;

// Selective re-exports — no glob `use config::*`
pub use config::{
    CliOverrides, ClipboardConfig, Config, PersistentConfig, generate_default_config,
};
pub use error::{Context, Result, bail};
