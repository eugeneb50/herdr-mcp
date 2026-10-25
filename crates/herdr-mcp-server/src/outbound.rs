//! Outbound HTTP: the `http_request` tool and the OpenAI-compatible
//! `llm_call` / `llm_models` tools (OpenRouter, OpenAI, Ollama, LM Studio, …).

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, StatusCode, Url};
use serde_json::{Value, json};

/// OpenRouter: the default `base_url` of the LLM tools.
pub const OPENROUTER: &str = "https://openrouter.ai/api/v1";

/// Redirects followed before giving up.
const MAX_REDIRECTS: usize = 10;

/// Where outbound requests may go (`[security]`): checked for the URL's
/// host, every address it resolves to, and every redirect hop.
#[derive(Debug, Clone)]
pub struct Guard {
    /// `host` or `*.suffix`, lowercase; empty allows any host.
    pub allow_hosts: Vec<String>,
    pub block_private: bool,
    pub max_response_bytes: u64,
    /// The bridge's port. Loopback on it is always refused, so a request
    /// can't drive the bridge's own API (and through it, the panes).
    pub bridge_port: Option<u16>,
}

impl Default for Guard {
    /// The `[security]` defaults, no bridge port.
    fn default() -> Self {
        Self::new(&herdr_mcp_core::SecurityConfig::default(), None)
    }
}

impl Guard {
    pub fn new(sec: &herdr_mcp_core::SecurityConfig, bridge_port: Option<u16>) -> Self {
        Self {
            allow_hosts: sec
                .allow_hosts
                .iter()
                .map(|h| h.trim().to_ascii_lowercase())
                .collect(),
            block_private: sec.block_private,
            max_response_bytes: sec.max_response_bytes,
            bridge_port,
        }
    }

    fn listed(&self, host: &str) -> bool {
        self.allow_hosts.iter().any(|p| match p.strip_prefix("*.") {
            Some(suffix) => host
                .strip_suffix(suffix)
                .is_some_and(|rest| rest.ends_with('.')),
            None => host == p,
        })
    }

    fn check_host(&self, host: &str) -> anyhow::Result<()> {
        if !self.allow_hosts.is_empty() && !self.listed(host) {
            bail!("{host} is not in [security] allow-hosts");
        }
        Ok(())
    }

    /// Whether `host` may be reached at `ip:port`; `Err` says why not.
    fn check_addr(&self, host: &str, ip: IpAddr, port: u16) -> Result<(), String> {
        let ip = ip.to_canonical();
        if (ip.is_loopback() || ip.is_unspecified()) && Some(port) == self.bridge_port {
            return Err(format!("{host}:{port} is herdr-mcp's own bridge"));
        }
        if self.listed(host) {
            return Ok(());
        }
        if ip.is_unspecified() || ip.is_multicast() || link_local(ip) {
            return Err(format!(
                "{host} is {ip}, a link-local or metadata address (add the host to [security] allow-hosts)"
            ));
        }
        if self.block_private && (ip.is_loopback() || private(ip)) {
            return Err(format!(
                "{host} is {ip}, a private address ([security] block-private)"
            ));
        }
        Ok(())
    }
}

fn link_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        // fd00:ec2::254 is AWS's IPv6 instance-metadata address.
        IpAddr::V6(v6) => {
            v6.is_unicast_link_local()
                || v6 == std::net::Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254)
        }
    }
}

fn private(ip: IpAddr) -> bool {
    match ip {
        // 100.64.0.0/10: carrier-grade NAT (and Tailscale).
        IpAddr::V4(v4) => v4.is_private() || (v4.octets()[0] == 100 && v4.octets()[1] & 0xc0 == 64),
        IpAddr::V6(v6) => v6.is_unique_local(),
    }
}

/// Resolves names for one request hop, dropping addresses [`Guard`]
/// refuses, so a name can't pass the check and then connect elsewhere.
struct Resolver {
    guard: Arc<Guard>,
    port: u16,
}

impl reqwest::dns::Resolve for Resolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let guard = self.guard.clone();
        let port = self.port;
        Box::pin(async move {
            let host = name.as_str().to_ascii_lowercase();
            let mut refused = None;
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
                .await?
                .filter(|a| match guard.check_addr(&host, a.ip(), port) {
                    Ok(()) => true,
                    Err(e) => {
                        refused = Some(e);
                        false
                    }
                })
                .collect();
            if addrs.is_empty() {
                return Err(refused
                    .unwrap_or_else(|| format!("{host}: no addresses"))
                    .into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// A request body.
pub enum Body {
    None,
    Text(String),
    Json(Value),
}

/// A response: status, headers and the body as text.
struct Fetched {
    status: StatusCode,
    headers: HeaderMap,
    text: String,
}

/// Send `method url` with `headers` and `body`, following redirects by
/// hand so [`Guard`] checks every hop: 303 (and 301/302 after a POST)
/// turn into a body-less GET, and credentials are dropped when a hop
/// changes origin. A body over `max_response_bytes` is an error.
async fn fetch(
    guard: &Arc<Guard>,
    mut method: Method,
    mut url: Url,
    mut headers: HeaderMap,
    mut body: Body,
    timeout: Duration,
) -> anyhow::Result<Fetched> {
    for _ in 0..=MAX_REDIRECTS {
        if !matches!(url.scheme(), "http" | "https") {
            bail!("{url}: only http and https URLs");
        }
        let host = url
            .host_str()
            .ok_or_else(|| anyhow!("{url}: no host"))?
            .trim_matches(['[', ']'])
            .to_ascii_lowercase();
        let port = url
            .port_or_known_default()
            .ok_or_else(|| anyhow!("{url}: no port"))?;
        guard.check_host(&host)?;
        // IP literals skip the resolver.
        if let Ok(ip) = host.parse::<IpAddr>() {
            guard.check_addr(&host, ip, port).map_err(|e| anyhow!(e))?;
        }
        let mut builder = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .dns_resolver(Arc::new(Resolver {
                guard: guard.clone(),
                port,
            }))
            .user_agent(concat!("herdr-mcp/", env!("CARGO_PKG_VERSION")));
        if guard.block_private {
            // A proxy would connect for us, past the address checks.
            builder = builder.no_proxy();
        }
        let mut req = builder
            .build()?
            .request(method.clone(), url.clone())
            .headers(headers.clone());
        req = match &body {
            Body::None => req,
            Body::Text(text) => req.body(text.clone()),
            Body::Json(json) => req.json(json),
        };
        let mut resp = req
            .send()
            .await
            .with_context(|| format!("{method} {url}"))?;
        let status = resp.status();
        if status.is_redirection()
            && let Some(location) = resp.headers().get(header::LOCATION)
        {
            let next = url
                .join(location.to_str().context("redirect Location")?)
                .context("redirect Location")?;
            if status == StatusCode::SEE_OTHER && method != Method::HEAD
                || matches!(status.as_u16(), 301 | 302) && method == Method::POST
            {
                method = Method::GET;
                body = Body::None;
                headers.remove(header::CONTENT_TYPE);
            }
            if next.origin() != url.origin() {
                for h in [
                    header::AUTHORIZATION,
                    header::COOKIE,
                    header::PROXY_AUTHORIZATION,
                ] {
                    headers.remove(h);
                }
            }
            url = next;
            continue;
        }
        let headers = resp.headers().clone();
        let mut buf = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            if (buf.len() + chunk.len()) as u64 > guard.max_response_bytes {
                bail!(
                    "{url}: the response is over {} bytes ([security] max-response-bytes)",
                    guard.max_response_bytes
                );
            }
            buf.extend_from_slice(&chunk);
        }
        return Ok(Fetched {
            status,
            headers,
            text: String::from_utf8_lossy(&buf).into_owned(),
        });
    }
    bail!("{url}: more than {MAX_REDIRECTS} redirects")
}

/// One HTTP request as `http_request` describes it.
pub struct Request<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: &'a BTreeMap<String, String>,
    pub query: &'a BTreeMap<String, String>,
    pub body: Body,
    pub timeout: Duration,
}

/// Send `req`. Returns `{status, ok, headers, body}`, `body` parsed as JSON
/// when it is JSON, else text. A non-2xx answer is a result, not an error.
pub async fn send(guard: &Arc<Guard>, req: Request<'_>) -> anyhow::Result<Value> {
    let method = Method::from_bytes(req.method.to_ascii_uppercase().as_bytes())
        .map_err(|_| anyhow!("bad HTTP method {:?}", req.method))?;
    let mut url = Url::parse(req.url).with_context(|| format!("url {:?}", req.url))?;
    if !req.query.is_empty() {
        url.query_pairs_mut().extend_pairs(req.query);
    }
    let mut headers = HeaderMap::new();
    for (k, v) in req.headers {
        headers.insert(
            HeaderName::from_bytes(k.as_bytes()).with_context(|| format!("header {k:?}"))?,
            HeaderValue::from_str(v).with_context(|| format!("header {k:?}"))?,
        );
    }
    let resp = fetch(guard, method, url, headers, req.body, req.timeout).await?;
    let headers: BTreeMap<String, String> = resp
        .headers
        .iter()
        .map(|(k, v)| {
            (
                k.to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    let body = serde_json::from_str(&resp.text).unwrap_or(Value::String(resp.text));
    Ok(json!({
        "status": resp.status.as_u16(),
        "ok": resp.status.is_success(),
        "headers": headers,
        "body": body,
    }))
}

/// The API key for `base_url`: `api_key` itself, else the environment
/// variable `api_key_env`, else the provider's usual one
/// (`OPENROUTER_API_KEY`, `OPENAI_API_KEY`). `Ok(None)`: send no key (a
/// local server). A variable not in `allowed` (`[security] key-env`) is
/// an error.
pub fn api_key(
    base_url: &str,
    api_key: Option<&str>,
    api_key_env: Option<&str>,
    allowed: &[String],
) -> anyhow::Result<Option<String>> {
    if let Some(key) = api_key.filter(|k| !k.trim().is_empty()) {
        return Ok(Some(key.trim().to_string()));
    }
    let Some(var) = api_key_env.or_else(|| default_key_env(base_url)) else {
        return Ok(None);
    };
    if !allowed.iter().any(|a| a == var) {
        bail!("api_key_env {var} is not in [security] key-env");
    }
    Ok(std::env::var(var).ok().filter(|k| !k.trim().is_empty()))
}

/// The environment variable a provider's key usually lives in, by the
/// URL's exact host: a key must never follow a look-alike URL.
pub fn default_key_env(base_url: &str) -> Option<&'static str> {
    match Url::parse(base_url).ok()?.host_str()? {
        "openrouter.ai" => Some("OPENROUTER_API_KEY"),
        "api.openai.com" => Some("OPENAI_API_KEY"),
        _ => None,
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
    guard: &Arc<Guard>,
    base_url: &str,
    path: &str,
    key: Option<&str>,
    body: Option<&Value>,
    timeout: Duration,
) -> anyhow::Result<Value> {
    let raw = format!("{}/{path}", base_url.trim_end_matches('/'));
    let url = Url::parse(&raw).with_context(|| format!("base_url {base_url:?}"))?;
    let mut headers = HeaderMap::new();
    if let Some(key) = key {
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {key}")).context("the API key")?,
        );
    }
    // OpenRouter's app attribution; other APIs ignore it.
    headers.insert("x-title", HeaderValue::from_static("herdr-mcp"));
    let (method, body) = match body {
        Some(b) => (Method::POST, Body::Json(b.clone())),
        None => (Method::GET, Body::None),
    };
    let resp = fetch(guard, method, url, headers, body, timeout).await?;
    let json: Value = serde_json::from_str(&resp.text).unwrap_or(Value::String(resp.text));
    if !resp.status.is_success() || json.get("error").is_some_and(|e| !e.is_null()) {
        let msg = json["error"]["message"]
            .as_str()
            .or_else(|| json["error"].as_str())
            .map(str::to_string)
            .unwrap_or_else(|| json.to_string());
        bail!("{raw}: {}: {msg}", resp.status);
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
    fn key_comes_from_the_call_then_an_allowed_env_and_only_exact_hosts() {
        let allowed = herdr_mcp_core::SecurityConfig::default().key_env;
        let key = |base: &str, k: Option<&str>, env: Option<&str>| api_key(base, k, env, &allowed);
        assert_eq!(
            key(OPENROUTER, Some(" k "), None).unwrap(),
            Some("k".into())
        );
        assert_eq!(default_key_env(OPENROUTER), Some("OPENROUTER_API_KEY"));
        assert_eq!(
            default_key_env("https://api.openai.com/v1"),
            Some("OPENAI_API_KEY")
        );
        // Look-alikes get no provider key.
        for lookalike in [
            "https://openrouter.ai.evil.example/api/v1",
            "https://evil.example/openrouter.ai/v1",
            "http://127.0.0.1:9/api.openai.com",
        ] {
            assert_eq!(default_key_env(lookalike), None, "{lookalike}");
        }
        assert_eq!(key("http://localhost:11434/v1", None, None).unwrap(), None);
        let err = key(OPENROUTER, None, Some("AWS_SECRET_ACCESS_KEY")).unwrap_err();
        assert!(err.to_string().contains("key-env"), "{err}");
    }

    #[test]
    fn guard_refuses_the_bridge_metadata_and_private_addresses() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let open = Guard {
            bridge_port: Some(7676),
            ..Guard::default()
        };
        assert!(open.check_addr("localhost", ip("127.0.0.1"), 7676).is_err());
        assert!(open.check_addr("x", ip("::ffff:127.0.0.1"), 7676).is_err());
        assert!(open.check_addr("localhost", ip("127.0.0.1"), 11434).is_ok());
        assert!(open.check_addr("x", ip("169.254.169.254"), 80).is_err());
        assert!(open.check_addr("x", ip("fd00:ec2::254"), 80).is_err());
        assert!(open.check_addr("x", ip("10.0.0.5"), 80).is_ok());
        let closed = Guard {
            block_private: true,
            allow_hosts: vec!["*.example.com".into(), "localhost".into()],
            ..open.clone()
        };
        assert!(closed.check_addr("x", ip("10.0.0.5"), 80).is_err());
        assert!(closed.check_addr("x", ip("100.100.1.1"), 80).is_err());
        assert!(closed.check_addr("x", ip("127.0.0.1"), 80).is_err());
        assert!(closed.check_addr("localhost", ip("127.0.0.1"), 80).is_ok());
        assert!(
            closed
                .check_addr("localhost", ip("127.0.0.1"), 7676)
                .is_err()
        );
        assert!(closed.check_host("api.example.com").is_ok());
        assert!(closed.check_host("example.com").is_err());
        assert!(closed.check_host("evilexample.com").is_err());
        assert!(Guard::default().check_host("anything.test").is_ok());
    }

    /// A one-shot HTTP server on a free port answering `status` with
    /// `extra` headers and `reply`; the URL and a handle resolving to the
    /// raw request it got.
    async fn serve(
        status: &'static str,
        extra: String,
        reply: String,
    ) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0; 8192];
            let n = sock.read(&mut buf).await.unwrap();
            let len = reply.len();
            let resp = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{extra}content-length: {len}\r\nconnection: close\r\n\r\n{reply}"
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        (url, handle)
    }

    async fn serve_once(reply: &str) -> (String, tokio::task::JoinHandle<String>) {
        serve("200 OK", String::new(), reply.to_string()).await
    }

    /// `send` a GET of `url` with `headers`.
    async fn get(
        guard: &Arc<Guard>,
        url: &str,
        headers: &BTreeMap<String, String>,
    ) -> anyhow::Result<Value> {
        let req = Request {
            method: "get",
            url,
            headers,
            query: &BTreeMap::new(),
            body: Body::None,
            timeout: Duration::from_secs(5),
        };
        send(guard, req).await
    }

    #[tokio::test]
    async fn send_passes_method_headers_query_and_json_body() {
        let (url, got) = serve_once(r#"{"x":1}"#).await;
        let headers = BTreeMap::from([("X-Test".to_string(), "yes".to_string())]);
        let query = BTreeMap::from([("q".to_string(), "a b".to_string())]);
        let out = send(
            &Arc::default(),
            Request {
                method: "post",
                url: &format!("{url}/path"),
                headers: &headers,
                query: &query,
                body: Body::Json(json!({ "k": "v" })),
                timeout: Duration::from_secs(5),
            },
        )
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
    async fn redirects_are_checked_and_drop_credentials_across_origins() {
        let guard = Arc::new(Guard {
            max_response_bytes: 1024,
            ..Guard::default()
        });
        let (target, got) = serve_once("{}").await;
        let (start, _) = serve(
            "302 Found",
            format!("location: {target}/next\r\n"),
            String::new(),
        )
        .await;
        let auth = BTreeMap::from([("Authorization".to_string(), "Bearer sk-1".to_string())]);
        let out = get(&guard, &start, &auth).await.unwrap();
        assert_eq!(out["status"], 200);
        let raw = got.await.unwrap().to_ascii_lowercase();
        assert!(raw.starts_with("get /next "), "{raw}");
        assert!(!raw.contains("authorization"), "{raw}");

        let (start, _) = serve(
            "302 Found",
            "location: http://169.254.169.254/latest/meta-data\r\n".into(),
            String::new(),
        )
        .await;
        let err = get(&guard, &start, &BTreeMap::new()).await.unwrap_err();
        assert!(format!("{err:#}").contains("metadata"), "{err:#}");

        let (big, _) = serve_once(&"x".repeat(2048)).await;
        let err = get(&guard, &big, &BTreeMap::new()).await.unwrap_err();
        assert!(format!("{err:#}").contains("max-response-bytes"), "{err:#}");
    }

    #[tokio::test]
    async fn api_sends_the_key_and_reads_the_reply() {
        let (url, got) = serve_once(
            r#"{"model":"m","choices":[{"message":{"content":"hi"},"finish_reason":"stop"}]}"#,
        )
        .await;
        let body = json!({ "model": "m", "messages": [] });
        let resp = api(
            &Arc::default(),
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
