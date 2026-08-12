/// Session-context detection for herdr-mcp (monolith version).
///
/// herdr sets `HERDR_ENV=1` on every pane process (see herdr's `src/pane.rs`
/// and `src/integration/env.rs`). This is herdr's own canonical "inside herdr"
/// marker — used by herdr's `running_inside_herdr()` and
/// `should_block_nested()`. We mirror that check here.
///
/// All functions in this module read the environment on demand. No state is
/// stored, avoiding DRY violations — the environment is the single source of
/// truth.

/// The env var herdr sets on every pane process.
pub const HERDR_ENV_VAR: &str = "HERDR_ENV";
/// The value herdr sets (always `"1"`).
pub const HERDR_ENV_VALUE: &str = "1";

/// Returns `true` if this process was launched inside a herdr pane.
///
/// Mirrors herdr's own `running_inside_herdr()` check in `src/update.rs`.
pub fn inside_herdr() -> bool {
    matches!(
        std::env::var(HERDR_ENV_VAR).ok().as_deref(),
        Some(HERDR_ENV_VALUE)
    )
}

/// The herdr pane id this process is running in, if any.
///
/// Set by herdr on managed panes (`HERDR_PANE_ID`). Returns `None` for popup
/// panes (which use `OmitPane`) and when running outside herdr entirely.
pub fn pane_id() -> Option<String> {
    std::env::var("HERDR_PANE_ID").ok().filter(|s| !s.is_empty())
}

/// The herdr tab id this process is running in, if any.
pub fn tab_id() -> Option<String> {
    std::env::var("HERDR_TAB_ID").ok().filter(|s| !s.is_empty())
}

/// The herdr workspace id this process is running in, if any.
pub fn workspace_id() -> Option<String> {
    std::env::var("HERDR_WORKSPACE_ID")
        .ok()
        .filter(|s| !s.is_empty())
}

/// The named herdr session this process belongs to, if any.
///
/// Set by herdr when using named sessions (`herdr session attach <name>`).
/// Returns `None` for the default session.
pub fn session_name() -> Option<String> {
    std::env::var("HERDR_SESSION")
        .ok()
        .filter(|s| !s.is_empty() && s != "default")
}

/// The `HERDR_SOCKET_PATH` override, if set.
///
/// herdr injects this on pane processes pointing at the active API socket.
/// Users can also set it manually, so this is a *hint* — not a reliable
/// indicator of "inside herdr" (use [`inside_herdr`] for that).
pub fn socket_path_override() -> Option<std::path::PathBuf> {
    std::env::var("HERDR_SOCKET_PATH")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    /// Env vars are process-global; serialize tests that touch them.
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    fn lock() -> &'static Mutex<()> {
        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn inside_herdr_true_when_set_to_1() {
        let _g = lock().lock().unwrap();
        // Save and restore
        let saved = std::env::var(HERDR_ENV_VAR).ok();
        unsafe { std::env::set_var(HERDR_ENV_VAR, HERDR_ENV_VALUE); }
        assert!(inside_herdr());
        // Restore
        match saved {
            Some(v) => unsafe { std::env::set_var(HERDR_ENV_VAR, v); },
            None => unsafe { std::env::remove_var(HERDR_ENV_VAR); },
        }
    }

    #[test]
    fn inside_herdr_false_when_absent() {
        let _g = lock().lock().unwrap();
        let saved = std::env::var(HERDR_ENV_VAR).ok();
        unsafe { std::env::remove_var(HERDR_ENV_VAR); }
        assert!(!inside_herdr());
        match saved {
            Some(v) => unsafe { std::env::set_var(HERDR_ENV_VAR, v); },
            None => {}
        }
    }

    #[test]
    fn inside_herdr_false_when_wrong_value() {
        let _g = lock().lock().unwrap();
        let saved = std::env::var(HERDR_ENV_VAR).ok();
        unsafe { std::env::set_var(HERDR_ENV_VAR, "0"); }
        assert!(!inside_herdr());
        unsafe { std::env::set_var(HERDR_ENV_VAR, "production"); }
        assert!(!inside_herdr());
        match saved {
            Some(v) => unsafe { std::env::set_var(HERDR_ENV_VAR, v); },
            None => unsafe { std::env::remove_var(HERDR_ENV_VAR); },
        }
    }

    #[test]
    fn pane_id_returns_some_when_set() {
        let _g = lock().lock().unwrap();
        let saved = std::env::var("HERDR_PANE_ID").ok();
        unsafe { std::env::set_var("HERDR_PANE_ID", "p1"); }
        assert_eq!(pane_id().as_deref(), Some("p1"));
        match saved {
            Some(v) => unsafe { std::env::set_var("HERDR_PANE_ID", v); },
            None => unsafe { std::env::remove_var("HERDR_PANE_ID"); },
        }
    }

    #[test]
    fn pane_id_returns_none_when_empty() {
        let _g = lock().lock().unwrap();
        let saved = std::env::var("HERDR_PANE_ID").ok();
        unsafe { std::env::set_var("HERDR_PANE_ID", ""); }
        assert_eq!(pane_id(), None);
        match saved {
            Some(v) => unsafe { std::env::set_var("HERDR_PANE_ID", v); },
            None => unsafe { std::env::remove_var("HERDR_PANE_ID"); },
        }
    }

    #[test]
    fn session_name_filters_default() {
        let _g = lock().lock().unwrap();
        let saved = std::env::var("HERDR_SESSION").ok();
        unsafe { std::env::set_var("HERDR_SESSION", "default"); }
        assert_eq!(session_name(), None);
        unsafe { std::env::set_var("HERDR_SESSION", "work"); }
        assert_eq!(session_name().as_deref(), Some("work"));
        match saved {
            Some(v) => unsafe { std::env::set_var("HERDR_SESSION", v); },
            None => unsafe { std::env::remove_var("HERDR_SESSION"); },
        }
    }

    #[test]
    fn socket_path_override_parses_path() {
        let _g = lock().lock().unwrap();
        let saved = std::env::var("HERDR_SOCKET_PATH").ok();
        unsafe { std::env::set_var("HERDR_SOCKET_PATH", "/tmp/test.sock"); }
        assert_eq!(
            socket_path_override(),
            Some(std::path::PathBuf::from("/tmp/test.sock"))
        );
        match saved {
            Some(v) => unsafe { std::env::set_var("HERDR_SOCKET_PATH", v); },
            None => unsafe { std::env::remove_var("HERDR_SOCKET_PATH"); },
        }
    }
}