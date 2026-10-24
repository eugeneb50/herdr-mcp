pub mod agents;
pub mod clipboard;
pub mod herdr_client;
pub mod outbound;
pub mod panes;
pub mod persistence;
pub mod profiles;
pub mod runs;
pub mod scheduler;
pub mod security;
pub mod server;
pub mod templates;
pub mod triggers;
pub mod variables;
pub mod web;

/// Lock guards that shrug off poisoning: the data behind every lock here
/// stays consistent across a panic (single inserts, removes, swaps).
pub(crate) fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn read<T>(l: &std::sync::RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn write<T>(l: &std::sync::RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(std::sync::PoisonError::into_inner)
}

// Convenience re-exports so consumers don't need deep paths
pub use herdr_client::{AgentRegistry, HerdrClient};
pub use persistence::Persistence;
pub use server::HerdrMcpServer;
pub use server::start_http;
pub use variables::{Recipe, RecipeStep, ScheduledRecipe, extract_variables};
