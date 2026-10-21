//! herdr-mcp-api — the wire contract of the herdr-mcp HTTP bridge (`/api/*`).
//!
//! Single source of truth for every type that crosses the bridge. The server
//! (`herdr-mcp-server`) re-exports these instead of defining its own; the web
//! UI and the TUI talk to the bridge through [`ApiClient`].
//!
//! Leaf crate: compiles for native and `wasm32-unknown-unknown`.

pub mod agents;
pub mod client;
pub mod config;
pub mod herdr;
pub mod profile;
pub mod recipe;
pub mod run;
pub mod variables;

pub use agents::{AgentHandle, AgentsResponse};
pub use client::{ApiClient, ApiError, schema_type, tool_is_error, tool_payload, tool_text};
pub use config::{ClipboardConfig, ConfigResponse};
pub use herdr::{
    HerdrEvent, HerdrPane, HerdrSession, PaneFilter, PanesResponse, RecipeTrigger, SessionsResponse,
};
pub use profile::AgentProfile;
pub use recipe::{
    CreateRecipeRequest, InstantiateTemplateRequest, Recipe, RecipeStep, RecipeTemplate,
    TemplateVariable, UpdateRecipeRequest,
};
pub use run::{RunAction, RunEvent, RunEventKind, RunRecord, RunStatus, StartRunRequest};
/// Re-exported so clients name recipe/execution ids without their own uuid dep.
pub use uuid::Uuid;
pub use variables::{
    AutoVariablesResponse, PlaygroundSnapshot, SaveVariableRequest, ToolsResponse, VariableStore,
};
