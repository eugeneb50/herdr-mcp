pub mod config;
pub mod error;

// Selective re-exports — no glob `use config::*`
pub use config::{CliOverrides, Config, PersistentConfig, generate_default_config};
pub use error::{Context, Result, bail};
