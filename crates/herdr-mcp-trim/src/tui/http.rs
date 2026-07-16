//! HTTP client for the herdr-mcp TUI dashboard.
//!
//! Talks to the local herdr-mcp HTTP bridge (`/api/*`) on behalf of the
//! interactive playground, recipe builder, trim dashboard, variables, and
//! settings tabs. Keeps a single `reqwest::Client` and a configured base URL.

use anyhow::{Context, Result};
use serde_json::Value;

/// Thin async HTTP client over the herdr-mcp HTTP bridge.
#[derive(Clone)]
pub struct HttpClient {
    base: String,
    inner: reqwest::Client,
    data_dir: std::path::PathBuf,
}

impl HttpClient {
    /// Build a client for `http://localhost:{port}`.
    pub fn new(port: u16, data_dir: std::path::PathBuf) -> Result<Self> {
        let inner = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("building reqwest client")?;
        Ok(Self {
            base: format!("http://localhost:{port}"),
            inner,
            data_dir,
        })
    }

    /// Write a debug line to the TUI log file (no stderr output).
    fn log_debug(&self, msg: &str) {
        super::log_debug(&self.data_dir, msg);
    }

    /// Retarget the client at a different bridge port.
    pub fn set_port(&mut self, port: u16) {
        self.base = format!("http://localhost:{port}");
    }

    /// Cheap liveness probe against `GET /api/health`.
    pub async fn health(&self) -> Result<Value> {
        let url = format!("{}/api/health", self.base);
        let v = self
            .inner
            .get(&url)
            .send()
            .await
            .context("GET /api/health")?
            .json::<Value>()
            .await
            .context("parsing /api/health body")?;
        Ok(v)
    }

    /// Find a live herdr-mcp bridge by probing candidate ports.
    ///
    /// Tries `initial` first (the explicitly configured/overridden port), then
    /// a small set of historical/default ports. Uses a short timeout so probing
    /// several closed ports fails fast instead of hanging for minutes. Returns
    /// the first port that answers `GET /api/health` with HTTP 200, or `None`.
    pub async fn discover_bridge(initial: u16) -> Option<u16> {
        let mut candidates: Vec<u16> = vec![initial, 8080, 7676, 5173];
        candidates.dedup();
        let probe = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(500))
            .build()
            .ok()?;
        for port in candidates {
            let url = format!("http://localhost:{port}/api/health");
            if probe
                .get(&url)
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                return Some(port);
            }
        }
        None
    }

    /// List every MCP tool with its JSON schema.
    pub async fn list_tools(&self) -> Result<Value> {
        let url = format!("{}/api/tools", self.base);
        let v = self
            .inner
            .get(&url)
            .send()
            .await
            .context("GET /api/tools")?
            .json::<Value>()
            .await
            .context("parsing /api/tools body")?;
        Ok(v)
    }

    /// Invoke a tool by name with the given params object.
    pub async fn call_tool(&self, name: &str, params: Value) -> Result<Value> {
        let url = format!("{}/api/tools/{name}", self.base);
        let v = self
            .inner
            .post(&url)
            .json(&params)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?
            .json::<Value>()
            .await
            .with_context(|| format!("parsing {url} body"))?;
        Ok(v)
    }

    /// Run a multi-step recipe (variable interpolation server-side).
    pub async fn run_recipe(&self, recipe: Value) -> Result<Value> {
        let url = format!("{}/api/recipe", self.base);
        let v = self
            .inner
            .post(&url)
            .json(&recipe)
            .send()
            .await
            .context("POST /api/recipe")?
            .json::<Value>()
            .await
            .context("parsing /api/recipe body")?;
        Ok(v)
    }

    /// List saved recipes.
    pub async fn list_recipes(&self) -> Result<Value> {
        let url = format!("{}/api/recipes", self.base);
        let v = self
            .inner
            .get(&url)
            .send()
            .await
            .context("GET /api/recipes")?
            .json::<Value>()
            .await
            .context("parsing /api/recipes body")?;
        Ok(v)
    }

    /// Save (create or update) a recipe.
    pub async fn save_recipe(&self, recipe: Value) -> Result<Value> {
        let url = format!("{}/api/recipes", self.base);
        let v = self
            .inner
            .post(&url)
            .json(&recipe)
            .send()
            .await
            .context("POST /api/recipes")?
            .json::<Value>()
            .await
            .context("parsing /api/recipes body")?;
        Ok(v)
    }

    /// Run a saved recipe by id.
    pub async fn run_recipe_by_id(&self, id: &str) -> Result<Value> {
        let url = format!("{}/api/recipes/{id}/run", self.base);
        let v = self
            .inner
            .post(&url)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?
            .json::<Value>()
            .await
            .with_context(|| format!("parsing {url} body"))?;
        Ok(v)
    }

    /// Aggregate trim savings (optionally scoped to a workspace).
    pub async fn trim_status(&self) -> Result<Value> {
        let url = format!("{}/api/trim/status", self.base);
        let v = self
            .inner
            .get(&url)
            .send()
            .await
            .context("GET /api/trim/status")?
            .json::<Value>()
            .await
            .context("parsing /api/trim/status body")?;
        Ok(v)
    }

    /// List all session variables.
    pub async fn list_variables(&self) -> Result<Value> {
        let url = format!("{}/api/variables", self.base);
        let v = self
            .inner
            .get(&url)
            .send()
            .await
            .context("GET /api/variables")?
            .json::<Value>()
            .await
            .context("parsing /api/variables body")?;
        Ok(v)
    }

    /// Save a variable (key + value).
    pub async fn save_variable(&self, body: Value) -> Result<Value> {
        let url = format!("{}/api/variables", self.base);
        let v = self
            .inner
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("POST /api/variables")?
            .json::<Value>()
            .await
            .context("parsing /api/variables body")?;
        Ok(v)
    }

    /// Delete a variable by key.
    pub async fn delete_variable(&self, key: &str) -> Result<()> {
        let url = format!("{}/api/variables/{key}", self.base);
        let _ = self
            .inner
            .delete(&url)
            .send()
            .await
            .with_context(|| format!("DELETE {url}"))?;
        Ok(())
    }

    /// End-to-end trim readiness check.
    pub async fn trim_diagnose(&self) -> Result<Value> {
        let url = format!("{}/api/trim/diagnose", self.base);
        let v = self
            .inner
            .post(&url)
            .send()
            .await
            .context("POST /api/trim/diagnose")?
            .json::<Value>()
            .await
            .context("parsing /api/trim/diagnose body")?;
        Ok(v)
    }

    /// Fire a savings summary notification.
    pub async fn trim_summary(&self) -> Result<Value> {
        let url = format!("{}/api/trim/summary", self.base);
        let v = self
            .inner
            .post(&url)
            .send()
            .await
            .context("POST /api/trim/summary")?
            .json::<Value>()
            .await
            .context("parsing /api/trim/summary body")?;
        Ok(v)
    }

    /// Open a live trim dashboard in a new herdr pane.
    pub async fn trim_dashboard_open(&self) -> Result<Value> {
        let url = format!("{}/api/trim/dashboard/open", self.base);
        let v = self
            .inner
            .post(&url)
            .send()
            .await
            .context("POST /api/trim/dashboard/open")?
            .json::<Value>()
            .await
            .context("parsing /api/trim/dashboard/open body")?;
        Ok(v)
    }

    /// Read the trim policy for a target pane (role or pane id).
    pub async fn trim_policy_get(&self, target: &str) -> Result<Value> {
        let params = serde_json::json!({ "target": target });
        self.call_tool("trim_policy_get", params).await
    }

    /// Set (or clear with `policy: null`) the trim policy for a target pane.
    pub async fn trim_policy_set(&self, target: &str, policy: Value) -> Result<Value> {
        let params = serde_json::json!({ "target": target, "policy": policy });
        self.call_tool("trim_policy_set", params).await
    }

    /// Write `text` to the system clipboard via the bridge's `clipboard_set` tool.
    pub async fn clipboard_set(&self, text: &str) -> Result<Value> {
        let url = format!("{}/api/tools/clipboard_set", self.base);
        let body = serde_json::json!({ "text": text });
        self.log_debug(&format!(
            "[http] clipboard_set -> {url} ({} chars)",
            text.len()
        ));
        let resp = self
            .inner
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("POST /api/tools/clipboard_set")?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .await
            .context("parsing /api/tools/clipboard_set body")?;
        self.log_debug(&format!("[http] clipboard_set <- status={status} body={v}"));
        if !status.is_success() {
            anyhow::bail!("clipboard_set HTTP {status}: {v}");
        }
        if v.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false) {
            let detail = v
                .pointer("/content/0/text")
                .and_then(|x| x.as_str())
                .unwrap_or("tool reported is_error");
            anyhow::bail!("clipboard_set tool error: {detail}");
        }
        Ok(v)
    }

    /// Read text from the system clipboard via the bridge's `clipboard_get` tool.
    pub async fn clipboard_get(&self) -> Result<String> {
        let url = format!("{}/api/tools/clipboard_get", self.base);
        self.log_debug(&format!("[http] clipboard_get -> {url}"));
        let resp = self
            .inner
            .post(&url)
            .json(&serde_json::Value::Object(Default::default()))
            .send()
            .await
            .context("POST /api/tools/clipboard_get")?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .await
            .context("parsing /api/tools/clipboard_get body")?;
        self.log_debug(&format!("[http] clipboard_get <- status={status} body={v}"));
        if !status.is_success() {
            anyhow::bail!("clipboard_get HTTP {status}: {v}");
        }
        Ok(parse_clipboard_text(&v))
    }
}

/// Extract clipboard text from a `clipboard_get` tool result.
///
/// The bridge serializes a rmcp `CallToolResult` whose `content` is an array of
/// `{ "type": "text", "text": "..." }` items. We concatenate every `text`
/// item (joined by newlines). As a fallback we return the first string found
/// anywhere in the payload, or an empty string.
pub fn parse_clipboard_text(v: &Value) -> String {
    let mut out = String::new();
    if let Some(items) = v.get("content").and_then(|c| c.as_array()) {
        for item in items {
            if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(t);
            }
        }
    }
    if !out.is_empty() {
        return out;
    }
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    // No extractable text content.
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Discovery must finish quickly (the probe client uses a 500ms timeout per
    // candidate port) rather than hanging for the full 30s client timeout.
    #[tokio::test]
    async fn discover_bridge_does_not_hang() {
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            HttpClient::discover_bridge(1),
        )
        .await;
        assert!(res.is_ok(), "discover_bridge hung");
    }

    #[test]
    fn parse_clipboard_text_from_content_array() {
        let v = serde_json::json!({
            "content": [
                { "type": "text", "text": "hello" },
                { "type": "text", "text": "world" }
            ],
            "is_error": false
        });
        assert_eq!(parse_clipboard_text(&v), "hello\nworld");
    }

    #[test]
    fn parse_clipboard_text_empty_when_no_text() {
        let v = serde_json::json!({ "content": [{ "type": "image", "data": "x" }] });
        assert_eq!(parse_clipboard_text(&v), "");
    }

    #[test]
    fn parse_clipboard_text_multiline_roundtrip() {
        let v = serde_json::json!({
            "content": [
                { "type": "text", "text": "line one" },
                { "type": "text", "text": "line two" }
            ]
        });
        assert_eq!(parse_clipboard_text(&v), "line one\nline two");
    }

    #[test]
    fn http_client_log_debug_writes_to_file() {
        let tmp = tempfile::tempdir().unwrap();
        let client = HttpClient::new(1, tmp.path().to_path_buf()).unwrap();
        client.log_debug("http client test msg");
        let log_path = tmp.path().join("tui-debug.log");
        let contents =
            std::fs::read_to_string(&log_path).expect("log file should be created by HttpClient");
        assert!(
            contents.contains("http client test msg"),
            "log should contain the message"
        );
    }
}
