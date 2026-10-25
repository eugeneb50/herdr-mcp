//! The playground's model, shared by the web UI and the TUI. Collections are
//! saved recipes and their requests are the recipe's steps; this module
//! holds what both front ends need to show and edit them: the tool catalog
//! and herdr's method schema as forms, raw form values to params and back,
//! result paths for extract rules, and the trigger editor's text form.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

use crate::{ApiClient, Recipe, RecipeStep, UpdateRecipeRequest, Uuid, schema_type};

/// The tool whose requests call any herdr socket method.
pub const HERDR_API: &str = "herdr_api";
pub const HTTP_REQUEST: &str = "http_request";
pub const LLM_CALL: &str = "llm_call";

/// A value as a form shows it: strings as-is, null empty, the rest as JSON.
pub fn display_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    }
}

/// A variable typed in a form: JSON when it parses (`3`, `true`, `[1]`),
/// else the text.
pub fn parse_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

// ── tools ──────────────────────────────────────────────────────────────

/// One tool of the catalog (`GET /api/tools`).
#[derive(Clone, Debug, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub schema: Value,
    /// The tool's `_meta.category` from its server definition.
    pub category: String,
}

pub fn tool_list(tools: &[Value]) -> Vec<ToolInfo> {
    tools
        .iter()
        .filter_map(|t| {
            Some(ToolInfo {
                name: t.get("name")?.as_str()?.to_string(),
                description: t
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string(),
                schema: t.get("inputSchema").cloned().unwrap_or(Value::Null),
                category: t
                    .pointer("/_meta/category")
                    .and_then(Value::as_str)
                    .unwrap_or("Other")
                    .to_string(),
            })
        })
        .collect()
}

/// Tools grouped by category, categories and tools sorted by name.
pub fn tool_groups(tools: &[ToolInfo]) -> Vec<(String, Vec<ToolInfo>)> {
    let mut groups: BTreeMap<String, Vec<ToolInfo>> = BTreeMap::new();
    for t in tools {
        groups
            .entry(t.category.clone())
            .or_default()
            .push(t.clone());
    }
    groups
        .into_iter()
        .map(|(label, mut tools)| {
            tools.sort_by(|a, b| a.name.cmp(&b.name));
            (label, tools)
        })
        .collect()
}

/// A JSON schema's properties and required names.
pub fn schema_parts(schema: &Value) -> (HashMap<String, Value>, HashSet<String>) {
    let props = schema
        .get("properties")
        .and_then(|p| p.as_object())
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    let required = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    (props, required)
}

// ── herdr methods ──────────────────────────────────────────────────────

/// One herdr socket API method, from the installed herdr's schema
/// (`GET /api/herdr/schema`). Nothing is hand-listed, so the methods always
/// match the herdr version that is running.
#[derive(Clone, Debug, PartialEq)]
pub struct Method {
    pub name: String,
    pub props: HashMap<String, Value>,
    pub required: HashSet<String>,
}

/// The methods in herdr's request schema, with flattened params.
pub fn methods(schema: &Value) -> Vec<Method> {
    let request = &schema["schemas"]["request"];
    let defs = &request["$defs"];
    let Some(variants) = request["oneOf"].as_array() else {
        return Vec::new();
    };
    variants
        .iter()
        .filter_map(|v| {
            let name = v.pointer("/properties/method/const")?.as_str()?.to_string();
            let params = v
                .pointer("/properties/params")
                .map(|p| resolve(p, defs))
                .unwrap_or(Value::Null);
            let props = params["properties"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, p)| (k.clone(), flatten(p, defs)))
                        .collect()
                })
                .unwrap_or_default();
            let required = params["required"]
                .as_array()
                .map(|r| {
                    r.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            Some(Method {
                name,
                props,
                required,
            })
        })
        .collect()
}

/// Follow `$ref`s and `anyOf [X, null]` to the schema that says what X is.
fn resolve(schema: &Value, defs: &Value) -> Value {
    if let Some(name) = schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| r.rsplit('/').next())
    {
        return resolve(&defs[name], defs);
    }
    if let Some(any) = schema.get("anyOf").and_then(Value::as_array)
        && let Some(inner) = any
            .iter()
            .find(|s| s.get("type") != Some(&Value::from("null")))
    {
        return resolve(inner, defs);
    }
    schema.clone()
}

/// One param as the playground form understands it: a JSON type, an enum,
/// or `object` (entered as JSON), with its description and default.
fn flatten(prop: &Value, defs: &Value) -> Value {
    let target = resolve(prop, defs);
    let consts: Vec<Value> = target["oneOf"]
        .as_array()
        .map(|o| o.iter().filter_map(|v| v.get("const").cloned()).collect())
        .unwrap_or_default();
    let mut out = serde_json::Map::new();
    if let Some(e) = target.get("enum") {
        out.insert("type".into(), "string".into());
        out.insert("enum".into(), e.clone());
    } else if !consts.is_empty() {
        out.insert("type".into(), "string".into());
        out.insert("enum".into(), Value::Array(consts));
    } else if target.get("properties").is_some() || target.get("oneOf").is_some() {
        out.insert("type".into(), "object".into());
    } else {
        out.insert(
            "type".into(),
            target.get("type").cloned().unwrap_or("string".into()),
        );
    }
    let mut desc = [prop.get("description"), target.get("description")]
        .into_iter()
        .flatten()
        .find_map(Value::as_str)
        .unwrap_or("")
        .to_string();
    let kind = schema_type(&Value::Object(out.clone())).to_string();
    if kind == "object" || kind == "array" {
        desc = format!("{desc} (JSON)").trim().to_string();
    }
    if let Some(d) = prop.get("default").or(target.get("default")) {
        out.insert("default".into(), d.clone());
        desc = format!("{desc} — default {d}")
            .trim_start_matches(" — ")
            .to_string();
    }
    out.insert("description".into(), desc.into());
    Value::Object(out)
}

/// Method names containing `filter`, grouped by their prefix (`pane`,
/// `tab`, …) in schema order.
pub fn method_groups(methods: &[Method], filter: &str) -> Vec<(String, Vec<String>)> {
    let needle = filter.to_lowercase();
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for m in methods.iter().filter(|m| m.name.contains(&needle)) {
        let group = m.name.split('.').next().unwrap_or("").to_string();
        match groups.iter_mut().find(|(g, _)| *g == group) {
            Some((_, names)) => names.push(m.name.clone()),
            None => groups.push((group, vec![m.name.clone()])),
        }
    }
    groups
}

/// herdr events a trigger can name: every subscription type in the
/// installed herdr's schema.
pub fn event_names(schema: &Value) -> Vec<String> {
    schema
        .pointer("/schemas/request/$defs/Subscription/oneOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| s.pointer("/properties/type/const")?.as_str())
        .map(str::to_string)
        .collect()
}

// ── forms ──────────────────────────────────────────────────────────────

/// Tool arguments from raw form values, each converted per its schema type
/// (`{{ }}` placeholders stay strings for the server to resolve). A value
/// the schema doesn't know (or with no schema loaded) reads as JSON, as
/// [`raw_values`] wrote it. Blank fields are left out, so optional params
/// fall back to their defaults.
pub fn build_params(
    properties: &HashMap<String, Value>,
    values: &HashMap<String, String>,
) -> Value {
    let mut map = serde_json::Map::new();
    for (k, raw) in values {
        if raw.trim().is_empty() {
            continue;
        }
        let value = match properties.get(k) {
            Some(p) => convert_param(schema_type(p), raw),
            None => parse_value(raw),
        };
        map.insert(k.clone(), value);
    }
    Value::Object(map)
}

/// Convert a raw string field to a JSON value per schema type. Arrays take
/// JSON (`["Enter"]`) or a comma list (`Enter, ctrl+c`).
pub fn convert_param(schema_type: &str, raw: &str) -> Value {
    match schema_type {
        "array" => serde_json::from_str::<Value>(raw)
            .ok()
            .filter(Value::is_array)
            .unwrap_or_else(|| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| Value::String(s.to_string()))
                    .collect()
            }),
        "object" => serde_json::from_str(raw).unwrap_or(Value::String(raw.to_string())),
        "boolean" => Value::Bool(matches!(raw, "true" | "1" | "yes")),
        "integer" | "number" => raw
            .trim()
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| {
                raw.trim()
                    .parse::<f64>()
                    .map(|f| serde_json::json!(f))
                    .unwrap_or(Value::String(raw.to_string()))
            }),
        _ => Value::String(raw.to_string()),
    }
}

/// A params object as the form edits it: strings as-is, the rest as JSON.
pub fn raw_values(params: &Value) -> HashMap<String, String> {
    params
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), display_value(v)))
                .collect()
        })
        .unwrap_or_default()
}

/// Variables as editable `(name, value)` rows, sorted by name.
pub fn sorted_vars(vars: &HashMap<String, Value>) -> Vec<(String, String)> {
    let mut out: Vec<_> = vars
        .iter()
        .map(|(k, v)| (k.clone(), display_value(v)))
        .collect();
    out.sort();
    out
}

// ── collections ────────────────────────────────────────────────────────

/// Recipes by folder (`category`; `""` first, for none), each sorted by name.
pub fn folders(recipes: &[Recipe]) -> Vec<(String, Vec<Recipe>)> {
    let mut by: BTreeMap<String, Vec<Recipe>> = BTreeMap::new();
    for r in recipes {
        let folder = r.category.clone().unwrap_or_default().trim().to_string();
        by.entry(folder).or_default().push(r.clone());
    }
    by.into_iter()
        .map(|(f, mut rs)| {
            rs.sort_by_key(|r| r.name.to_lowercase());
            (f, rs)
        })
        .collect()
}

/// A request as a list names it: the herdr method it calls, else its tool.
pub fn tool_label(step: &RecipeStep) -> String {
    match step.params.get("method").and_then(Value::as_str) {
        Some(m) if step.tool == HERDR_API => m.to_string(),
        _ => step.tool.clone(),
    }
}

/// `requestN`, one past the highest N among `steps`.
pub fn fresh_request_id(steps: &[RecipeStep]) -> String {
    let n = steps
        .iter()
        .filter_map(|s| s.id.strip_prefix("request")?.parse::<usize>().ok())
        .max()
        .unwrap_or(0);
    format!("request{}", n + 1)
}

/// Request ids are used as `{{ id.path }}`: letters, digits, `_` and `-`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The herdr method a `herdr_api` request calls (`workspace.list` for a
/// new one).
pub fn request_method(step: &RecipeStep) -> String {
    step.params
        .get("method")
        .and_then(Value::as_str)
        .filter(|_| step.tool == HERDR_API)
        .unwrap_or("workspace.list")
        .to_string()
}

/// A request's params as the form edits them: the method's params for
/// `herdr_api`, else the tool's.
pub fn request_values(step: &RecipeStep) -> HashMap<String, String> {
    raw_values(if step.tool == HERDR_API {
        &step.params["params"]
    } else {
        &step.params
    })
}

/// The form schema of a request: the herdr method's params for
/// `herdr_api`, else the tool's.
pub fn request_schema(
    tools: &[ToolInfo],
    methods: &[Method],
    tool: &str,
    method: &str,
) -> (HashMap<String, Value>, HashSet<String>) {
    if tool == HERDR_API {
        methods
            .iter()
            .find(|m| m.name == method)
            .map(|m| (m.props.clone(), m.required.clone()))
            .unwrap_or_default()
    } else {
        tools
            .iter()
            .find(|t| t.name == tool)
            .map(|t| schema_parts(&t.schema))
            .unwrap_or_default()
    }
}

/// The params a request sends: `{method, params}` for `herdr_api`.
pub fn request_params(
    props: &HashMap<String, Value>,
    tool: &str,
    method: &str,
    values: &HashMap<String, String>,
) -> Value {
    let params = build_params(props, values);
    if tool == HERDR_API {
        serde_json::json!({ "method": method, "params": params })
    } else {
        params
    }
}

/// Every leaf of `v` (and every empty container) with the path an
/// [`crate::Extract`] follows to it, in document order; at most `limit`.
pub fn result_paths(v: &Value, limit: usize) -> Vec<(String, Value)> {
    fn walk(v: &Value, path: String, out: &mut Vec<(String, Value)>, limit: usize) {
        if out.len() >= limit {
            return;
        }
        match v {
            Value::Object(m) if !m.is_empty() => {
                for (k, child) in m {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    walk(child, p, out, limit);
                }
            }
            Value::Array(a) if !a.is_empty() => {
                for (i, child) in a.iter().enumerate() {
                    walk(child, format!("{path}[{i}]"), out, limit);
                }
            }
            _ => out.push((path, v.clone())),
        }
    }
    let mut out = Vec::new();
    walk(v, String::new(), &mut out, limit);
    out
}

/// A variable name suggested for `path`: its last key (`panes[0].pane_id` →
/// `pane_id`).
pub fn var_name(path: &str) -> String {
    let last = path
        .rsplit('.')
        .map(|s| s.split('[').next().unwrap_or(""))
        .find(|s| !s.is_empty())
        .unwrap_or("value");
    last.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// A trigger's `when` as text: `agent_status=done, pane.tab_id=w1:t1`.
pub fn when_text(when: &serde_json::Map<String, Value>) -> String {
    when.iter()
        .map(|(k, v)| format!("{k}={}", display_value(v)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Inverse of [`when_text`]: values that parse as JSON (`true`, `3`) stay
/// typed, anything else is a string.
pub fn parse_when(text: &str) -> serde_json::Map<String, Value> {
    text.split(',')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            let v = v.trim();
            let value = serde_json::from_str(v).unwrap_or_else(|_| Value::String(v.to_string()));
            Some((k.trim().to_string(), value))
        })
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

// ── saving requests ────────────────────────────────────────────────────

/// Set collection `id`'s requests to `steps`.
pub async fn set_steps(client: &ApiClient, id: Uuid, steps: Vec<RecipeStep>) -> Result<(), String> {
    let req = UpdateRecipeRequest {
        steps: Some(steps),
        ..Default::default()
    };
    client
        .update_recipe(id, &req)
        .await
        .map(drop)
        .map_err(|e| e.to_string())
}

/// Replace request `old_id` of collection `id` with `step` (appending it if
/// it is gone), reading the collection fresh so nothing else is clobbered.
pub async fn save_step(
    client: &ApiClient,
    id: Uuid,
    old_id: &str,
    step: RecipeStep,
) -> Result<(), String> {
    if !valid_id(&step.id) {
        return Err("the request name may use letters, digits, _ and - only".into());
    }
    let mut recipe = client.get_recipe(id).await.map_err(|e| e.to_string())?;
    if step.id != old_id && recipe.steps.iter().any(|s| s.id == step.id) {
        return Err(format!(
            "this collection already has a request `{}`",
            step.id
        ));
    }
    match recipe.steps.iter_mut().find(|s| s.id == old_id) {
        Some(s) => *s = step,
        None => recipe.steps.push(step),
    }
    set_steps(client, id, recipe.steps).await
}

/// Append a new `tool` request (starting from `params`) to collection
/// `id`; its id.
pub async fn add_request(
    client: &ApiClient,
    id: Uuid,
    tool: &str,
    params: Value,
) -> Result<String, String> {
    let mut recipe = client.get_recipe(id).await.map_err(|e| e.to_string())?;
    let step_id = fresh_request_id(&recipe.steps);
    recipe.steps.push(RecipeStep {
        id: step_id.clone(),
        tool: tool.into(),
        params,
        ..Default::default()
    });
    set_steps(client, id, recipe.steps).await?;
    Ok(step_id)
}

/// Remove request `step_id` from collection `id`.
pub async fn delete_step(client: &ApiClient, id: Uuid, step_id: &str) -> Result<(), String> {
    let mut recipe = client.get_recipe(id).await.map_err(|e| e.to_string())?;
    recipe.steps.retain(|s| s.id != step_id);
    set_steps(client, id, recipe.steps).await
}

// ── builders ───────────────────────────────────────────────────────────
// The `http_request` endpoint builder and the `llm_call` wizard edit the
// same raw values the plain form does, so a request can switch between
// them.

/// Methods the endpoint builder offers.
pub const HTTP_METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// `values` with `key` set to `raw` ("" removes it).
pub fn with_value(
    values: &HashMap<String, String>,
    key: &str,
    raw: String,
) -> HashMap<String, String> {
    let mut next = values.clone();
    if raw.is_empty() {
        next.remove(key);
    } else {
        next.insert(key.to_string(), raw);
    }
    next
}

/// A JSON-object param (`query`, `headers`) as name/value rows
/// (non-string values as JSON).
pub fn kv_rows(raw: &str) -> Vec<(String, String)> {
    serde_json::from_str::<serde_json::Map<String, Value>>(raw)
        .map(|m| {
            m.into_iter()
                .map(|(k, v)| {
                    let v = match v {
                        Value::String(s) => s,
                        other => other.to_string(),
                    };
                    (k, v)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Rows back to a JSON object string; rows without a name are dropped,
/// and no rows at all is "" (the param is left out).
pub fn kv_json(rows: &[(String, String)]) -> String {
    let map: serde_json::Map<String, Value> = rows
        .iter()
        .filter(|(k, _)| !k.trim().is_empty())
        .map(|(k, v)| (k.trim().to_string(), Value::String(v.clone())))
        .collect();
    if map.is_empty() {
        String::new()
    } else {
        Value::Object(map).to_string()
    }
}

/// How an `http_request` sends its body.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BodyMode {
    None,
    Json,
    Text,
}

impl BodyMode {
    pub const ALL: [BodyMode; 3] = [Self::None, Self::Json, Self::Text];

    /// The mode `values` are in.
    pub fn of(values: &HashMap<String, String>) -> Self {
        if values.contains_key("json") {
            Self::Json
        } else if values.contains_key("body") {
            Self::Text
        } else {
            Self::None
        }
    }

    /// The param holding the body.
    pub fn key(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Json => Some("json"),
            Self::Text => Some("body"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Json => "JSON",
            Self::Text => "Text",
        }
    }

    /// `values` switched to this mode, keeping the body text.
    pub fn apply(self, values: &HashMap<String, String>) -> HashMap<String, String> {
        let current = values
            .get("json")
            .or_else(|| values.get("body"))
            .cloned()
            .unwrap_or_default();
        let mut out = values.clone();
        out.remove("json");
        out.remove("body");
        match self {
            Self::Json => {
                let body = if current.is_empty() {
                    "{}".into()
                } else {
                    current
                };
                out.insert("json".into(), body);
            }
            Self::Text => {
                out.insert("body".into(), current);
            }
            Self::None => {}
        }
        out
    }
}

/// Why a JSON body won't parse yet (`None` when it does, is blank, or has
/// placeholders the server fills in first).
pub fn json_body_error(values: &HashMap<String, String>) -> Option<String> {
    let body = values.get("json")?;
    if body.trim().is_empty() || body.contains("{{") {
        return None;
    }
    serde_json::from_str::<Value>(body)
        .err()
        .map(|e| e.to_string())
}

/// An OpenAI-compatible provider preset.
#[derive(Debug, PartialEq, Eq)]
pub struct Provider {
    pub name: &'static str,
    pub base_url: &'static str,
    pub key_env: Option<&'static str>,
    pub hint: &'static str,
}

pub static PROVIDERS: [Provider; 5] = [
    Provider {
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        key_env: Some("OPENROUTER_API_KEY"),
        hint: "Hundreds of models behind one key; ids look like openai/gpt-4o-mini.",
    },
    Provider {
        name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        key_env: Some("OPENAI_API_KEY"),
        hint: "OpenAI's own API; ids look like gpt-4o-mini.",
    },
    Provider {
        name: "Ollama",
        base_url: "http://localhost:11434/v1",
        key_env: None,
        hint: "Local models; no key.",
    },
    Provider {
        name: "LM Studio",
        base_url: "http://localhost:1234/v1",
        key_env: None,
        hint: "Local models; no key.",
    },
    Provider {
        name: "Custom",
        base_url: "",
        key_env: None,
        hint: "Any OpenAI-compatible server: give its base URL (…/v1).",
    },
];

/// The index of the Custom preset in [`PROVIDERS`].
pub const CUSTOM_PROVIDER: usize = 4;

/// The preset `base_url` belongs to (OpenRouter when unset, as the tool
/// defaults to it), else Custom.
pub fn provider_of(base_url: &str) -> usize {
    let base = base_url.trim().trim_end_matches('/');
    let base = if base.is_empty() {
        PROVIDERS[0].base_url
    } else {
        base
    };
    PROVIDERS
        .iter()
        .position(|p| !p.base_url.is_empty() && p.base_url == base)
        .unwrap_or(CUSTOM_PROVIDER)
}

/// `values` switched to provider `i` (its base URL; the key starts over).
pub fn pick_provider(values: &HashMap<String, String>, i: usize) -> HashMap<String, String> {
    let mut next = with_value(values, "base_url", PROVIDERS[i].base_url.to_string());
    next.remove("api_key_env");
    next.remove("api_key");
    next
}

/// Where an `llm_call` gets its key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyMode {
    /// From herdr-mcp's environment.
    Env,
    /// From a `{{variable}}` (or pasted).
    Key,
    None,
}

impl KeyMode {
    pub const ALL: [KeyMode; 3] = [Self::Env, Self::Key, Self::None];

    /// The mode `values` are in for `provider`.
    pub fn of(values: &HashMap<String, String>, provider: &Provider) -> Self {
        let set = |k: &str| values.get(k).is_some_and(|v| !v.is_empty());
        if set("api_key") {
            Self::Key
        } else if set("api_key_env") || provider.key_env.is_some() {
            Self::Env
        } else {
            Self::None
        }
    }

    /// The param holding the key.
    pub fn key(self) -> Option<&'static str> {
        match self {
            Self::Env => Some("api_key_env"),
            Self::Key => Some("api_key"),
            Self::None => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Env => "From an environment variable",
            Self::Key => "From a variable (or pasted)",
            Self::None => "No key",
        }
    }

    /// `values` switched to this mode for `provider`.
    pub fn apply(
        self,
        values: &HashMap<String, String>,
        provider: &Provider,
    ) -> HashMap<String, String> {
        let mut next = values.clone();
        next.remove("api_key");
        next.remove("api_key_env");
        match self {
            Self::Key => {
                next.insert("api_key".into(), "{{llm_api_key}}".into());
            }
            Self::Env if provider.key_env.is_none() => {
                next.insert("api_key_env".into(), "LLM_API_KEY".into());
            }
            _ => {}
        }
        next
    }
}

/// The models `values`' provider serves (`llm_models`); a `{{variable}}`
/// key resolves like it would in a run.
pub async fn load_models(
    client: &ApiClient,
    values: &HashMap<String, String>,
) -> Result<Vec<Value>, String> {
    let mut args = serde_json::Map::new();
    for k in ["base_url", "api_key", "api_key_env"] {
        if let Some(v) = values.get(k).filter(|s| !s.is_empty()) {
            args.insert(k.into(), Value::String(v.clone()));
        }
    }
    let args = match client
        .resolve(&crate::ResolveRequest {
            value: Value::Object(args.clone()),
            ..Default::default()
        })
        .await
    {
        Ok(r) => r.value,
        Err(_) => Value::Object(args),
    };
    let r = client
        .call_tool("llm_models", &args)
        .await
        .map_err(|e| e.to_string())?;
    if crate::tool_is_error(&r) {
        let text = r
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or("failed");
        return Err(text.to_string());
    }
    Ok(crate::tool_payload(&r)
        .and_then(|p| p["models"].as_array().cloned())
        .unwrap_or_default())
}

/// Model ids matching `filter`, each with its context size ("128k ctx").
pub fn model_choices(models: &[Value], filter: &str) -> Vec<(String, String)> {
    let needle = filter.to_lowercase();
    models
        .iter()
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_string();
            let ctx = m["context_length"]
                .as_u64()
                .map(|n| format!("{}k ctx", n / 1000))
                .unwrap_or_default();
            id.to_lowercase().contains(&needle).then_some((id, ctx))
        })
        .take(200)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe(name: &str, category: Option<&str>) -> Recipe {
        serde_json::from_value(serde_json::json!({
            "id": Uuid::nil(), "name": name, "description": null, "steps": [],
            "variables": {}, "created_at": "2026-10-19T00:00:00Z",
            "updated_at": "2026-10-19T00:00:00Z", "is_template": false,
            "category": category
        }))
        .unwrap()
    }

    #[test]
    fn folders_group_by_category_unfiled_first() {
        let rs = [
            recipe("b", Some("smoke")),
            recipe("a", None),
            recipe("c", Some("smoke")),
            recipe("d", Some(" ")),
        ];
        let got: Vec<(String, Vec<String>)> = folders(&rs)
            .into_iter()
            .map(|(f, rs)| (f, rs.into_iter().map(|r| r.name).collect()))
            .collect();
        assert_eq!(
            got,
            [
                ("".to_string(), vec!["a".to_string(), "d".to_string()]),
                ("smoke".to_string(), vec!["b".to_string(), "c".to_string()]),
            ]
        );
    }

    #[test]
    fn result_paths_follow_extract_syntax() {
        let v = serde_json::json!({ "panes": [{ "pane_id": "w1:p1", "tags": [] }], "n": 2 });
        let paths: Vec<String> = result_paths(&v, 10).into_iter().map(|(p, _)| p).collect();
        assert_eq!(paths, ["n", "panes[0].pane_id", "panes[0].tags"]);
        assert_eq!(result_paths(&v, 1).len(), 1);
        let top = result_paths(&serde_json::json!([{ "id": 1 }]), 10);
        assert_eq!(top[0].0, "[0].id");
        assert_eq!(var_name("panes[0].pane_id"), "pane_id");
        assert_eq!(var_name("[0]"), "value");
        assert_eq!(var_name("a.b-c"), "b_c");
    }

    #[test]
    fn request_ids_and_params() {
        let steps = [
            RecipeStep {
                id: "request2".into(),
                ..Default::default()
            },
            RecipeStep {
                id: "spawn".into(),
                ..Default::default()
            },
        ];
        assert_eq!(fresh_request_id(&steps), "request3");
        assert!(valid_id("spawn_2-b") && !valid_id("a.b") && !valid_id(""));

        let props: HashMap<String, Value> = [(
            "lines".to_string(),
            serde_json::json!({ "type": "integer" }),
        )]
        .into();
        let values = [("lines".to_string(), "{{ n }}".to_string())].into();
        assert_eq!(
            request_params(&props, HERDR_API, "pane.read", &values),
            serde_json::json!({ "method": "pane.read", "params": { "lines": "{{ n }}" } })
        );
        let herdr = RecipeStep {
            tool: HERDR_API.into(),
            params: serde_json::json!({ "method": "pane.read", "params": { "lines": 3 } }),
            ..Default::default()
        };
        assert_eq!(request_method(&herdr), "pane.read");
        assert_eq!(request_values(&herdr)["lines"], "3");
        assert_eq!(parse_value("3"), serde_json::json!(3));
        assert_eq!(parse_value("w1:p1"), serde_json::json!("w1:p1"));
        assert_eq!(
            raw_values(&serde_json::json!({ "a": "x", "b": [1] }))["b"],
            "[1]"
        );
        assert_eq!(display_value(&Value::Null), "");
        assert_eq!(display_value(&serde_json::json!({"x": 1})), r#"{"x":1}"#);
    }

    #[test]
    fn converts_form_values_by_schema_type() {
        assert_eq!(convert_param("boolean", "true"), Value::Bool(true));
        assert_eq!(convert_param("integer", "42"), serde_json::json!(42));
        assert_eq!(
            convert_param("string", "{{pane.x}}"),
            serde_json::json!("{{pane.x}}")
        );
        assert_eq!(
            convert_param("array", "Enter, ctrl+c"),
            serde_json::json!(["Enter", "ctrl+c"])
        );
        assert_eq!(
            convert_param("array", r#"["a,b"]"#),
            serde_json::json!(["a,b"])
        );
    }

    /// Optional params (`Option<u32>`, `Option<bool>`) arrive as
    /// `["integer","null"]`; they must still be sent typed, and blanks dropped.
    #[test]
    fn build_params_types_optional_fields_and_skips_blanks() {
        let schema = serde_json::json!({
            "lines": { "type": ["integer", "null"] },
            "check": { "type": ["boolean", "null"] },
            "keys": { "type": "array", "items": { "type": "string" } },
            "pane_id": { "type": ["string", "null"] },
            "command": { "type": "string" }
        });
        let props: HashMap<String, Value> = serde_json::from_value(schema).unwrap();
        let values: HashMap<String, String> = [
            ("lines", "5"),
            ("check", "true"),
            ("keys", "Enter"),
            ("pane_id", "  "),
            ("command", "echo {{x}}"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        // Placeholders stay as written: the server resolves them.
        assert_eq!(
            build_params(&props, &values),
            serde_json::json!({
                "lines": 5, "check": true, "keys": ["Enter"], "command": "echo {{x}}"
            })
        );
        // No schema (herdr down, or a key it doesn't list): types survive
        // the round trip through the form.
        let saved = serde_json::json!({ "lines": 3, "strip": false, "pane_id": "w1:p2" });
        assert_eq!(build_params(&HashMap::new(), &raw_values(&saved)), saved);
    }

    #[test]
    fn when_round_trips_through_text() {
        let when = parse_when("agent_status=done, pane.tab_id = w1:t1, focused=true");
        assert_eq!(when["agent_status"], "done");
        assert_eq!(when["pane.tab_id"], "w1:t1");
        assert_eq!(when["focused"], true);
        assert_eq!(parse_when(&when_text(&when)), when);
        assert!(parse_when("").is_empty());
    }

    #[test]
    fn flattens_refs_nullables_enums_and_objects() {
        let schema = serde_json::json!({ "schemas": { "request": {
            "oneOf": [{ "properties": {
                "method": { "const": "pane.read" },
                "params": { "$ref": "#/schemas/request/$defs/PaneReadParams" }
            }}],
            "$defs": {
                "PaneReadParams": {
                    "properties": {
                        "pane_id": { "type": "string" },
                        "source": { "$ref": "#/schemas/request/$defs/ReadSource" },
                        "lines": { "type": ["integer", "null"] },
                        "wait": { "anyOf": [{ "$ref": "#/schemas/request/$defs/Wait" }, { "type": "null" }] },
                        "strip_ansi": { "type": "boolean", "default": true }
                    },
                    "required": ["pane_id", "source"]
                },
                "ReadSource": { "enum": ["visible", "recent"], "type": "string" },
                "Wait": { "properties": { "timeout_ms": { "type": "integer" } }, "type": "object" }
            }
        }}});
        let m = &methods(&schema)[0];
        assert_eq!(m.name, "pane.read");
        assert!(m.required.contains("source"));
        assert_eq!(
            m.props["source"]["enum"],
            serde_json::json!(["visible", "recent"])
        );
        assert_eq!(schema_type(&m.props["lines"]), "integer");
        assert_eq!(m.props["wait"]["type"], "object");
        assert!(
            m.props["strip_ansi"]["description"]
                .as_str()
                .unwrap()
                .contains("default true")
        );
    }

    #[test]
    fn kv_rows_round_trip_and_drop_blank_names() {
        let rows = kv_rows(r#"{"a":"1","n":2}"#);
        assert_eq!(rows, [("a".into(), "1".into()), ("n".into(), "2".into())]);
        let mut rows = rows;
        rows.push((" ".into(), "x".into()));
        assert_eq!(kv_json(&rows), r#"{"a":"1","n":"2"}"#);
        assert_eq!(kv_json(&[]), "");
    }

    #[test]
    fn body_mode_switches_keep_the_text() {
        let v: HashMap<String, String> = [("body".to_string(), "hi".to_string())].into();
        assert_eq!(BodyMode::of(&v), BodyMode::Text);
        let j = BodyMode::Json.apply(&v);
        assert_eq!(j.get("json").map(String::as_str), Some("hi"));
        assert!(!j.contains_key("body"));
        assert!(json_body_error(&j).is_some());
        assert!(BodyMode::None.apply(&j).is_empty());
        assert_eq!(BodyMode::Json.apply(&HashMap::new())["json"], "{}");
    }

    #[test]
    fn providers_and_key_modes() {
        assert_eq!(PROVIDERS[provider_of("")].name, "OpenRouter");
        assert_eq!(
            PROVIDERS[provider_of("https://api.openai.com/v1/")].name,
            "OpenAI"
        );
        assert_eq!(provider_of("http://box:8000/v1"), CUSTOM_PROVIDER);
        let ollama = &PROVIDERS[2];
        let v = pick_provider(&HashMap::new(), 2);
        assert_eq!(KeyMode::of(&v, ollama), KeyMode::None);
        let v = KeyMode::Env.apply(&v, ollama);
        assert_eq!(v["api_key_env"], "LLM_API_KEY");
        assert_eq!(KeyMode::of(&v, ollama), KeyMode::Env);
        let v = KeyMode::Key.apply(&v, ollama);
        assert_eq!(KeyMode::of(&v, ollama), KeyMode::Key);
        assert!(!v.contains_key("api_key_env"));
    }

    #[test]
    fn model_choices_filter_and_show_context() {
        let models = vec![
            serde_json::json!({"id": "openai/gpt-4o-mini", "context_length": 128000}),
            serde_json::json!({"id": "meta/llama"}),
        ];
        assert_eq!(
            model_choices(&models, "GPT"),
            [("openai/gpt-4o-mini".to_string(), "128k ctx".to_string())]
        );
        assert_eq!(model_choices(&models, "").len(), 2);
    }
}
