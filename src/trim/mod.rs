//! Message-trim layer for herdr-mcp.
//!
//! Two deterministic, reversible compressors that never collide:
//! - `caveman` — style compressor (ASCII/Latin prose only)
//! - `pfc1` — phonetic/frequency key dictionary (Cherokee syllabary U+13A0–U+13FF)
//!
//! They compose through an ordered [`pipeline`] and are wired into the MCP tools
//! and the a2a primitives (`agent_message` / `agent_read`) via per-pane
//! [`policy`]. [`eval`] lets the model verify its own savings.

pub mod caveman;
pub mod eval;
pub mod pfc1;
pub mod pipeline;
pub mod policy;
pub mod runner;
