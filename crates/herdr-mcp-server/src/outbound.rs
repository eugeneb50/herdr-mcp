//! Outbound HTTP: the `http_request` tool and the OpenAI-compatible
//! `llm_call` / `llm_models` tools (OpenRouter, OpenAI, Ollama, LM Studio, …).

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use serde_json::{Value, json};

/// OpenRouter: the default `base_url` of the LLM tools.
pub const OPENROUTER: &str = "https://openrouter.ai/api/v1";

/// One HTTP request as `http_request` describes it.
pub struct Request<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: &'a BTreeMap<String, String>,
    pub query: &'a BTreeMap<String, String>,
    /// A string is sent as is; anything else as JSON.
    pub body: Option<&'a Value>,
    pub timeout: Duration,
}

/// Send `req`. Returns `{status, ok, headers, body}`, `body` parsed as JSON
/// when it is JSON, else text. A non-2xx answer is a result, not an error.
pub async fn send(req: Request<'_>) -> anyhow::Result<Value> {
    let method = reqwest::Method::from_bytes(req.method.to_ascii_uppercase().as_bytes())
        .map_err(|_| anyhow!("bad HTTP method {:?}", req.method))?;
    let mut builder = client(req.timeout)?
        .request(method, req.url)
        .query(&req.query);
    for (k, v) in req.headers {
        builder = builder.header(k, v);
    }
    builder = match req.body {
        None | Some(Value::Null) => builder,
        Some(Value::String(text)) => builder.body(text.clone()),
        Some(json) => builder.json(json),
    };
    let resp = builder
        .send()
        .await
        .with_context(|| format!("{} {}", req.method, req.url))?;
    let status = resp.status();
    let headers: BTreeMap<String, String> = resp
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    let text = resp.text().await?;
    let body = serde_json::from_str(&text).unwrap_or(Value::String(text));
    Ok(json!({
        "status": status.as_u16(),
        "ok": status.is_success(),
        "headers": headers,
        "body": body,
    }))
}

fn client(timeout: Duration) -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(concat!("herdr-mcp/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// The API key for `base_url`: `api_key` itself, else the environment
/// variable `api_key_env`, else the provider's usual one
/// (`OPENROUTER_API_KEY`, `OPENAI_API_KEY`). `None`: send no key (a local
/// server).
pub fn api_key(base_url: &str, api_key: Option<&str>, api_key_env: Option<&str>) -> Option<String> {
    if let Some(key) = api_key.filter(|k| !k.trim().is_empty()) {
        return Some(key.trim().to_string());
    }
    let var = api_key_env.or_else(|| default_key_env(base_url))?;
    std::env::var(var).ok().filter(|k| !k.trim().is_empty())
}

/// The environment variable a provider's key usually lives in.
pub fn default_key_env(base_url: &str) -> Option<&'static str> {
    if base_url.contains("openrouter.ai") {
        Some("OPENROUTER_API_KEY")
    } else if base_url.contains("api.openai.com") {
        Some("OPENAI_API_KEY")
    } else {
        None
    }
}

/// What `llm_call` asks for.
pub struct Chat<'a> {
    pub model: &'a str,
    pub system: Option<&'a str>,
    pub prompt: Option<&'a str>,
    /// Earlier turns (`[{role, content}]`), before `prompt`.
    pub messages: &'a [Value],
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    /// Merged into the request body last (e.g. `response_format`, `top_p`).
    pub extra: Option<&'a serde_json::Map<String, Value>>,
}

/// The `chat/completions` request body for `chat`.
pub fn chat_body(chat: &Chat<'_>) -> anyhow::Result<Value> {
    let mut messages = Vec::new();
    if let Some(system) = chat.system.filter(|s| !s.trim().is_empty()) {
        messages.push(json!({ "role": "system", "content": system }));
    }
    messages.extend(chat.messages.iter().cloned());
    if let Some(prompt) = chat.prompt.filter(|p| !p.trim().is_empty()) {
        messages.push(json!({ "role": "user", "content": prompt }));
    }
    if messages.iter().all(|m| m["role"] == "system") {
        bail!("nothing to send: give a prompt or messages");
    }
    let mut body = json!({ "model": chat.model, "messages": messages });
    if let Some(t) = chat.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(n) = chat.max_tokens {
        body["max_tokens"] = json!(n);
    }
    if let Some(extra) = chat.extra {
        for (k, v) in extra {
            body[k] = v.clone();
        }
    }
    Ok(body)
}

/// POST `body` to `{base_url}/{path}` (or GET with no body) as an
/// OpenAI-compatible API; the JSON answer, or the API's error message.
pub async fn api(
    base_url: &str,
    path: &str,
    key: Option<&str>,
    body: Option<&Value>,
    timeout: Duration,
) -> anyhow::Result<Value> {
    let url = format!("{}/{path}", base_url.trim_end_matches('/'));
    let http = client(timeout)?;
    let mut req = match body {
        Some(b) => http.post(&url).json(b),
        None => http.get(&url),
    };
    if let Some(key) = key {
        req = req.bearer_auth(key);
    }
    // OpenRouter's app attribution; other APIs ignore it.
    req = req.header("X-Title", "herdr-mcp");
    let resp = req.send().await.with_context(|| url.clone())?;
    let status = resp.status();
    let text = resp.text().await?;
    let json: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    if !status.is_success() || json.get("error").is_some_and(|e| !e.is_null()) {
        let msg = json["error"]["message"]
            .as_str()
            .or_else(|| json["error"].as_str())
            .map(str::to_string)
            .unwrap_or_else(|| json.to_string());
        bail!("{url}: {status}: {msg}");
    }
    Ok(json)
}

/// `{text, model, finish_reason, usage}` from a `chat/completions` answer.
pub fn chat_reply(resp: &Value) -> anyhow::Result<Value> {
    let choice = &resp["choices"][0];
    let text = choice["message"]["content"]
        .as_str()
        .ok_or_else(|| anyhow!("the answer has no message: {resp}"))?;
    Ok(json!({
        "text": text,
        "model": resp["model"],
        "finish_reason": choice["finish_reason"],
        "usage": resp["usage"],
    }))
}

/// `[{id, name, context_length}]` from a `models` answer, sorted by id.
pub fn model_list(resp: &Value) -> Vec<Value> {
    let mut models: Vec<Value> = resp["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m["id"].as_str()?;
            Some(json!({
                "id": id,
                "name": m["name"].as_str().unwrap_or(id),
                "context_length": m["context_length"],
            }))
        })
        .collect();
    models.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    models
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn chat_body_orders_system_history_prompt_and_merges_extra() {
        let history = [json!({ "role": "user", "content": "hi" })];
        let extra = json!({ "top_p": 0.5 });
        let body = chat_body(&Chat {
            model: "m",
            system: Some("be brief"),
            prompt: Some("go"),
            messages: &history,
            temperature: Some(0.2),
            max_tokens: Some(10),
            extra: extra.as_object(),
        })
        .unwrap();
        assert_eq!(
            body,
            json!({
                "model": "m",
                "messages": [
                    { "role": "system", "content": "be brief" },
                    { "role": "user", "content": "hi" },
                    { "role": "user", "content": "go" },
                ],
                "temperature": 0.2,
                "max_tokens": 10,
                "top_p": 0.5,
            })
        );
        let empty = Chat {
            model: "m",
            system: Some("s"),
            prompt: None,
            messages: &[],
            temperature: None,
            max_tokens: None,
            extra: None,
        };
        assert!(chat_body(&empty).is_err());
    }

    #[test]
    fn replies_and_models_are_read_from_openai_shapes() {
        let resp = json!({
            "model": "m1",
            "choices": [{ "message": { "content": "ok" }, "finish_reason": "stop" }],
            "usage": { "total_tokens": 3 },
        });
        assert_eq!(
            chat_reply(&resp).unwrap(),
            json!({ "text": "ok", "model": "m1", "finish_reason": "stop", "usage": { "total_tokens": 3 } })
        );
        assert!(chat_reply(&json!({ "choices": [] })).is_err());
        let models =
            json!({ "data": [{ "id": "b" }, { "id": "a", "name": "A", "context_length": 8 }] });
        assert_eq!(
            model_list(&models),
            [
                json!({ "id": "a", "name": "A", "context_length": 8 }),
                json!({ "id": "b", "name": "b", "context_length": null }),
            ]
        );
    }

    #[test]
    fn key_comes_from_the_call_then_the_env() {
        assert_eq!(api_key(OPENROUTER, Some(" k "), None), Some("k".into()));
        assert_eq!(default_key_env(OPENROUTER), Some("OPENROUTER_API_KEY"));
        assert_eq!(
            default_key_env("https://api.openai.com/v1"),
            Some("OPENAI_API_KEY")
        );
        assert_eq!(api_key("http://localhost:11434/v1", None, None), None);
    }

    /// A one-shot HTTP server on a free port answering `reply`; the URL and
    /// a handle resolving to the raw request it got.
    async fn serve_once(reply: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0; 8192];
            let n = sock.read(&mut buf).await.unwrap();
            let body = reply.len();
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {body}\r\nconnection: close\r\n\r\n{reply}"
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        (url, handle)
    }

    #[tokio::test]
    async fn send_passes_method_headers_query_and_json_body() {
        let (url, got) = serve_once(r#"{"x":1}"#).await;
        let headers = BTreeMap::from([("X-Test".to_string(), "yes".to_string())]);
        let query = BTreeMap::from([("q".to_string(), "a b".to_string())]);
        let body = json!({ "k": "v" });
        let out = send(Request {
            method: "post",
            url: &format!("{url}/path"),
            headers: &headers,
            query: &query,
            body: Some(&body),
            timeout: Duration::from_secs(5),
        })
        .await
        .unwrap();
        assert_eq!(out["status"], 200);
        assert_eq!(out["body"], json!({ "x": 1 }));
        let raw = got.await.unwrap().to_ascii_lowercase();
        assert!(raw.starts_with("post /path?q=a+b "), "{raw}");
        assert!(raw.contains("x-test: yes"), "{raw}");
        assert!(raw.ends_with(r#"{"k":"v"}"#), "{raw}");
    }

    #[tokio::test]
    async fn api_sends_the_key_and_reads_the_reply() {
        let (url, got) = serve_once(
            r#"{"model":"m","choices":[{"message":{"content":"hi"},"finish_reason":"stop"}]}"#,
        )
        .await;
        let body = json!({ "model": "m", "messages": [] });
        let resp = api(
            &url,
            "chat/completions",
            Some("sk-1"),
            Some(&body),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(chat_reply(&resp).unwrap()["text"], "hi");
        let raw = got.await.unwrap().to_ascii_lowercase();
        assert!(raw.starts_with("post /chat/completions "), "{raw}");
        assert!(raw.contains("authorization: bearer sk-1"), "{raw}");
    }
}
