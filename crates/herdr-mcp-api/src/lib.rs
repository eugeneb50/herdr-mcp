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
pub mod flow;
pub mod herdr;
pub mod playground;
pub mod profile;
pub mod recipe;
pub mod run;
pub mod settings;
pub mod variables;

pub use agents::{AgentHandle, AgentsResponse};
pub use client::{ApiClient, ApiError, schema_type, tool_is_error, tool_payload, tool_text};
pub use config::{ClipboardConfig, ConfigResponse};
pub use flow::{Check, CheckResult, Edge, Flow, Node, NodeKind, Op, Point, Port, Retry, Rule};
pub use herdr::{
    EventLevel, HerdrEvent, HerdrPane, HerdrSession, PaneFilter, PanesResponse, RecipeTrigger,
    SessionsResponse,
};
pub use profile::AgentProfile;
pub use recipe::{
    CreateRecipeRequest, Extract, InstantiateTemplateRequest, Recipe, RecipeStep, RecipeTemplate,
    RunTrail, TemplateVariable, UpdateRecipeRequest, VarScope,
};
pub use run::{
    NodeState, RunAction, RunEvent, RunEventKind, RunOrigin, RunRecord, RunStatus, StartRunRequest,
    node_states, trail_text,
};
pub use settings::{Chord, SettingInfo, Settings, SettingsResponse};
/// Re-exported so clients name recipe/execution ids without their own uuid dep.
pub use uuid::Uuid;

/// The bridge token's file in the data dir (`[security]`: required on a
/// non-loopback bind or with `require-token`). Send it as
/// `Authorization: Bearer <token>`, or `?token=` where headers can't go.
pub const TOKEN_FILE: &str = "bridge.token";
pub use variables::{
    AutoVariablesResponse, ResolveRequest, ResolveResponse, SaveVariableRequest, ToolsResponse,
    VariableStore,
};

/// The web UI's address on `localhost:port`, carrying `token` in the
/// fragment (never sent to the server or written to its logs) for the page
/// to keep and send.
pub fn web_url(port: u16, token: Option<&str>) -> String {
    match token {
        Some(t) => format!("http://localhost:{port}/#token={t}"),
        None => format!("http://localhost:{port}"),
    }
}
