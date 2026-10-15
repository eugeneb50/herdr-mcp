//! Runtime config wire types (`/api/config`).

use serde::{Deserialize, Serialize};

/// Explicit clipboard backend commands for the `clipboard_set` / `clipboard_get`
/// tools. When both `copy-command` and `paste-command` are set (non-empty) they
/// override platform auto-detection (e.g. force `xsel` instead of `wl-copy` on
/// Linux). The `HERDR_MCP_CLIPBOARD_COPY` / `HERDR_MCP_CLIPBOARD_PASTE` env vars
/// still take precedence over this setting.
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
