//! herdr-mcp-api — the wire contract of the herdr-mcp HTTP bridge (`/api/*`).
//!
//! Single source of truth for every type that crosses the bridge. The server
//! (`herdr-mcp-server`), trim (`herdr-mcp-trim`) and proxy (`herdr-mcp-proxy`)
//! crates re-export these instead of defining their own; the web UI and the
//! TUI talk to the bridge through [`ApiClient`].
//!
//! Leaf crate: compiles for native and `wasm32-unknown-unknown`.

pub mod agents;
pub mod client;
pub mod config;
pub mod recipe;
pub mod trim;
pub mod variables;

pub use agents::{AgentHandle, AgentsResponse, ProxyPolicy, TrimDirection, TrimPolicy};
pub use client::{ApiClient, ApiError, tool_is_error, tool_text};
pub use config::{ClipboardConfig, ConfigResponse};
pub use recipe::{
    CreateRecipeRequest, ExecutionResult, ExecutionStatus, ExportScriptResponse, Recipe,
    RecipeRequest, RecipeResponse, RecipeStep, RecipeTemplate, TemplateVariable,
    UpdateRecipeRequest,
};
pub use trim::{DiagnoseReport, PaneStatus, SampleRoundtrip, TrimStatusResponse};
/// Re-exported so clients name recipe/execution ids without their own uuid dep.
pub use uuid::Uuid;
pub use variables::{
    AutoVariablesResponse, PlaygroundSnapshot, SaveVariableRequest, ToolsResponse, VariableStore,
};
