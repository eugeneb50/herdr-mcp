pub mod variables;
pub mod persistence;
pub mod scheduler;
pub mod templates;
pub mod herdr_client;
pub mod server;

// Convenience re-exports so consumers don't need deep paths
pub use server::HerdrMcpServer;
pub use server::start_http;
pub use herdr_client::{HerdrClient, AgentRegistry};
pub use persistence::Persistence;
pub use variables::{
    Recipe, RecipeStep, ExecutionResult, ExecutionStatus,
    ScheduledRecipe, extract_variables,
};
