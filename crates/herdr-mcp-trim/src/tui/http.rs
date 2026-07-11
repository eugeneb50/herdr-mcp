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
}

impl HttpClient {
    /// Build a client for `http://localhost:{port}`.
    pub fn new(port: u16) -> Result<Self> {
        let inner = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("building reqwest client")?;
        Ok(Self {
            base: format!("http://localhost:{port}"),
            inner,
        })
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
            if probe.get(&url).send().await.is_ok_and(|r| r.status().is_success()) {
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
}
