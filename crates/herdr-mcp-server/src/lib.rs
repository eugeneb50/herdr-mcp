pub mod agents;
pub mod clipboard;
pub mod herdr_client;
pub mod panes;
pub mod persistence;
pub mod profiles;
pub mod runs;
pub mod scheduler;
pub mod server;
pub mod templates;
pub mod triggers;
pub mod variables;

// Convenience re-exports so consumers don't need deep paths
pub use herdr_client::{AgentRegistry, HerdrClient};
pub use persistence::Persistence;
pub use server::HerdrMcpServer;
pub use server::start_http;
pub use variables::{Recipe, RecipeStep, ScheduledRecipe, extract_variables};
