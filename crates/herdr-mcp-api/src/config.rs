//! Runtime config wire types (`/api/config`).

use serde::{Deserialize, Serialize};

/// Clipboard backend commands for the `clipboard_set` / `clipboard_get` tools.
/// When both are set (non-empty) the server runs them instead of its built-in
/// OS clipboard (e.g. to force `xsel`); otherwise it uses the built-in one.
/// Settings (web/TUI) saves them; there is no env override, so what Settings
/// shows is what runs.
///
/// Also the `[clipboard]` table of the config file (`herdr-mcp-core` re-exports
/// it), hence the kebab-case keys on the wire too.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ClipboardConfig {
    /// Command that reads clipboard contents from stdin (copy).
    #[serde(default)]
    pub copy_command: Option<String>,

    /// Command that writes clipboard contents to stdout (paste).
    #[serde(default)]
    pub paste_command: Option<String>,
}

impl ClipboardConfig {
    /// The (copy, paste) commands when both are set and non-empty, else
    /// `None`: the built-in OS clipboard.
    pub fn commands(&self) -> Option<(&str, &str)> {
        non_empty(&self.copy_command).zip(non_empty(&self.paste_command))
    }
}

fn non_empty(command: &Option<String>) -> Option<&str> {
    command.as_deref().map(str::trim).filter(|c| !c.is_empty())
}

/// `GET /api/config` and `PUT /api/config/clipboard` response: the bridge's
/// runtime configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigResponse {
    pub data_dir: String,
    pub herdr_socket: String,
    pub herdr_bin: String,
    /// The clipboard commands the server is using now.
    pub clipboard: ClipboardConfig,
    /// Config file that `PUT /api/config/clipboard` writes to.
    pub config_path: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_need_both_non_empty() {
        let cfg = |copy: Option<&str>, paste: Option<&str>| ClipboardConfig {
            copy_command: copy.map(Into::into),
            paste_command: paste.map(Into::into),
        };
        assert_eq!(
            cfg(Some(" xsel -i "), Some("xsel -o")).commands(),
            Some(("xsel -i", "xsel -o"))
        );
        assert_eq!(cfg(Some("xsel -i"), None).commands(), None);
        assert_eq!(cfg(Some("xsel -i"), Some("  ")).commands(), None);
        assert_eq!(cfg(None, None).commands(), None);
    }
}
