pub mod config;
pub mod error;

// Selective re-exports — no glob `use config::*`
pub use config::{Config, CliOverrides, generate_default_config};
pub use error::{Result, bail, Context};
