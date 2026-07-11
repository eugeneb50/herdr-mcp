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
