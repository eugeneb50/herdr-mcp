pub mod config;
pub mod error;

// Selective re-exports — no glob `use config::*`
pub use config::{CliOverrides, Config, generate_default_config};
pub use error::{Context, Result, bail};
