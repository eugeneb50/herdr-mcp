//! The playground's model, shared by the web UI and the TUI. Collections are
//! saved recipes and their requests are the flow's tool nodes; this module
//! holds what both front ends need to show and edit them: the tool catalog
//! and herdr's method schema as forms, raw form values to params and back,
//! result paths for extract rules, flow edits and layout, and the trigger
//! editor's text form.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

use crate::{
    ApiClient, Edge, Flow, Node, NodeKind, Point, Port, Recipe, RecipeStep, RunRecord,
    UpdateRecipeRequest, Uuid, schema_type,
};

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

/// A request as a list names it: the herdr method it calls, the HTTP
/// method and URL (`GET example.com/x`), the LLM model, else its tool.
pub fn tool_label(step: &RecipeStep) -> String {
    let param = |k| {
        step.params
            .get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let label = match step.tool.as_str() {
        HERDR_API => param("method").map(str::to_string),
        HTTP_REQUEST => param("url")
            .map(|url| url.split_once("://").map_or(url, |(_, rest)| rest))
            .filter(|url| !url.is_empty())
            .map(|url| {
                let method = param("method").unwrap_or("GET").to_uppercase();
                format!("{method} {url}")
            }),
        LLM_CALL => param("model").map(|m| format!("LLM {m}")),
        _ => None,
    };
    label.unwrap_or_else(|| step.tool.clone())
}

const NEW_COLLECTION: &str = "New collection";

/// The saved collections as `(id, name)`: what a `run` node names.
pub fn collections(recipes: &[Recipe]) -> Vec<(Uuid, String)> {
    recipes
        .iter()
        .filter(|r| !r.is_template)
        .map(|r| (r.id, r.name.clone()))
        .collect()
}

/// `New collection`, numbered past the collections already named so.
pub fn fresh_collection_name(recipes: &[Recipe]) -> String {
    let taken = recipes
        .iter()
        .filter_map(|r| match r.name.strip_prefix(NEW_COLLECTION)? {
            "" => Some(1),
            n => n.strip_prefix(' ')?.parse::<usize>().ok(),
        })
        .max();
    match taken {
        None => NEW_COLLECTION.to_string(),
        Some(n) => format!("{NEW_COLLECTION} {}", n + 1),
    }
}

/// How many of the newest runs the Runs views list.
pub const RUNS_SHOWN: usize = 200;

/// A send's headline facts: its status, the HTTP status when the request
/// was `http_request`, and how long the run took (`completed · 200 · 312 ms`).
pub fn send_summary(run: &RunRecord, step_id: &str) -> String {
    let mut out = vec![run.status.as_str().to_string()];
    let http = run.flow.node(step_id).and_then(Node::tool_name) == Some(HTTP_REQUEST);
    if http
        && let Some(status) = run
            .results
            .get(step_id)
            .and_then(crate::tool_payload)
            .and_then(|p| p.get("status")?.as_u64())
    {
        out.push(status.to_string());
    }
    if let Some(done) = run.finished_at {
        let ms = (done - run.started_at).num_milliseconds().max(0);
        out.push(if ms < 1000 {
            format!("{ms} ms")
        } else {
            format!("{:.1} s", ms as f64 / 1000.0)
        });
    }
    out.join(" · ")
}

/// `{prefix}N`, one past the highest N among `flow`'s node ids.
pub fn fresh_node_id(flow: &Flow, prefix: &str) -> String {
    let n = flow
        .nodes
        .iter()
        .filter_map(|s| s.id.strip_prefix(prefix)?.parse::<usize>().ok())
        .max()
        .unwrap_or(0);
    format!("{prefix}{}", n + 1)
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

// ── editing flows ──────────────────────────────────────────────────────

/// Apply `edit` to collection `id`'s flow, read fresh so nothing else is
/// clobbered, and save it; what `edit` returned.
pub async fn edit_flow<T>(
    client: &ApiClient,
    id: Uuid,
    edit: impl FnOnce(&mut Flow) -> Result<T, String>,
) -> Result<T, String> {
    let mut recipe = client.get_recipe(id).await.map_err(|e| e.to_string())?;
    let out = edit(&mut recipe.flow)?;
    let req = UpdateRecipeRequest {
        flow: Some(recipe.flow),
        ..Default::default()
    };
    client
        .update_recipe(id, &req)
        .await
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Put `node` in place of node `old_id`, renaming its edges and dropping
/// those from ports it no longer has (a branch's removed rule).
pub fn replace_node(flow: &mut Flow, old_id: &str, node: Node) -> Result<(), String> {
    if node.id != old_id && flow.node(&node.id).is_some() {
        return Err(format!("this flow already has a node `{}`", node.id));
    }
    let Some(slot) = flow.nodes.iter_mut().find(|n| n.id == old_id) else {
        return Err(format!("no node `{old_id}` (deleted?)"));
    };
    for e in &mut flow.edges {
        if e.from == old_id {
            e.from.clone_from(&node.id);
        }
        if e.to == old_id {
            e.to.clone_from(&node.id);
        }
    }
    let ports = node.kind.ports();
    flow.edges
        .retain(|e| e.from != node.id || ports.contains(&e.port));
    *slot = node;
    Ok(())
}

/// Add a node of `kind` named `{prefix}N` at `at`, wired from `after`'s
/// port when given; its id.
pub fn add_node(
    flow: &mut Flow,
    prefix: &str,
    kind: NodeKind,
    at: Point,
    after: Option<(&str, Port)>,
) -> String {
    let id = fresh_node_id(flow, prefix);
    flow.nodes.push(Node {
        id: id.clone(),
        description: None,
        at: Some(at),
        kind,
    });
    if let Some((from, port)) = after {
        connect(flow, from, port, &id);
    }
    id
}

/// Remove node `id` and its edges.
pub fn remove_node(flow: &mut Flow, id: &str) {
    flow.nodes.retain(|n| n.id != id);
    flow.edges.retain(|e| e.from != id && e.to != id);
}

/// Add the edge `from`.`port` → `to` unless the flow has it.
pub fn connect(flow: &mut Flow, from: &str, port: Port, to: &str) {
    let edge = Edge {
        from: from.into(),
        port,
        to: to.into(),
    };
    if !flow.edges.contains(&edge) {
        flow.edges.push(edge);
    }
}

/// Node width and height on the canvas, and the gap between columns and rows.
pub const NODE_W: f32 = 180.0;
pub const NODE_H: f32 = 56.0;
const GAP_X: f32 = 80.0;
const GAP_Y: f32 = 36.0;

/// Where to draw each node: its `at`, or a column by its longest forward
/// path from start (loops ignored) and a row in that column. Nodes start
/// cannot reach go in a column after the rest.
pub fn layout(flow: &Flow) -> HashMap<String, Point> {
    let shape = flow.shape();
    let mut depth: HashMap<&str, usize> = HashMap::new();
    if let Some(start) = flow.start() {
        depth.insert(&start.id, 0);
    }
    // Longest path by relaxation: a forward graph has no cycle, so at most
    // one pass per node.
    for _ in 0..flow.nodes.len() {
        let mut changed = false;
        for (i, e) in flow.edges.iter().enumerate() {
            if shape.back.contains(&i) {
                continue;
            }
            if let Some(&d) = depth.get(e.from.as_str())
                && depth.get(e.to.as_str()).is_none_or(|&t| t < d + 1)
            {
                depth.insert(&e.to, d + 1);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let stray = depth.values().max().map_or(0, |d| d + 1);
    let mut rows: HashMap<usize, usize> = HashMap::new();
    flow.nodes
        .iter()
        .map(|n| {
            let at = n.at.unwrap_or_else(|| {
                let col = depth.get(n.id.as_str()).copied().unwrap_or(stray);
                let row = rows.entry(col).or_default();
                *row += 1;
                Point {
                    x: col as f32 * (NODE_W + GAP_X),
                    y: (*row - 1) as f32 * (NODE_H + GAP_Y),
                }
            });
            (n.id.clone(), at)
        })
        .collect()
}

/// Focus `pane` in herdr (`herdr_api` `pane.focus`); what to tell the user.
pub async fn focus_pane(client: &ApiClient, pane: &str) -> Result<String, String> {
    let args = serde_json::json!({ "method": "pane.focus", "params": { "pane_id": pane } });
    match client.call_tool(HERDR_API, &args).await {
        Ok(r) if !crate::tool_is_error(&r) => Ok(format!("focused pane {pane} in herdr")),
        Ok(r) => Err(crate::tool_text(&r)),
        Err(e) => Err(format!("cannot focus {pane}: {e}")),
    }
}

// ── the map ────────────────────────────────────────────────────────────

/// What starts collections on the map: a herdr event, a cron schedule, or
/// another collection's run node.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MapSource {
    Trigger(String),
    Schedule(String),
    Collection(Uuid),
}

/// `from` starts collection `to`, which may be missing (deleted).
#[derive(Debug, Clone, PartialEq)]
pub struct MapLink {
    pub from: MapSource,
    pub to: Uuid,
    /// The trigger's filter, `off` for a disabled schedule, the run node's
    /// id.
    pub label: String,
}

/// The `(collection, cron, enabled)` of each schedule in `list_schedules`'
/// payload.
pub fn schedules(payload: &Value) -> Vec<(Uuid, String, bool)> {
    payload["schedules"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some((
                s["recipe_id"].as_str()?.parse().ok()?,
                s["cron_schedule"].as_str()?.to_string(),
                s["enabled"].as_bool().unwrap_or(true),
            ))
        })
        .collect()
}

/// Every trigger, schedule and run node, as links into the collection each
/// starts.
pub fn map_links(recipes: &[Recipe], schedules: &[(Uuid, String, bool)]) -> Vec<MapLink> {
    let mut out = Vec::new();
    for r in recipes {
        for t in &r.triggers {
            out.push(MapLink {
                from: MapSource::Trigger(t.on.clone()),
                to: r.id,
                label: when_text(&t.when),
            });
        }
        for n in &r.flow.nodes {
            if let NodeKind::Run { collection, .. } = n.kind {
                out.push(MapLink {
                    from: MapSource::Collection(r.id),
                    to: collection,
                    label: n.id.clone(),
                });
            }
        }
    }
    out.extend(schedules.iter().map(|(to, cron, on)| MapLink {
        from: MapSource::Schedule(cron.clone()),
        to: *to,
        label: if *on { String::new() } else { "off".into() },
    }));
    out
}

/// Each collection's column: one past the deepest collection that runs it,
/// loops (a link back to a collection that leads to it) ignored.
pub fn map_columns(recipes: &[Recipe], links: &[MapLink]) -> HashMap<Uuid, usize> {
    let mut seen: HashMap<Uuid, Visit> = recipes.iter().map(|r| (r.id, Visit::New)).collect();
    let mut back = HashSet::new();
    for r in recipes {
        if seen[&r.id] == Visit::New {
            find_loops(r.id, links, &mut seen, &mut back);
        }
    }
    let mut col: HashMap<Uuid, usize> = recipes.iter().map(|r| (r.id, 0)).collect();
    // Longest path by relaxation; without the loops there is no cycle.
    for _ in 0..recipes.len() {
        let mut changed = false;
        for (i, l) in links.iter().enumerate() {
            if let MapSource::Collection(from) = l.from
                && !back.contains(&i)
                && let (Some(&f), Some(&t)) = (col.get(&from), col.get(&l.to))
                && t < f + 1
            {
                col.insert(l.to, f + 1);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    col
}

#[derive(Clone, Copy, PartialEq)]
enum Visit {
    New,
    Open,
    Done,
}

/// Depth-first from `id` over hand-offs: a link into a collection still
/// open (on the path) is a loop.
fn find_loops(
    id: Uuid,
    links: &[MapLink],
    seen: &mut HashMap<Uuid, Visit>,
    back: &mut HashSet<usize>,
) {
    seen.insert(id, Visit::Open);
    for (i, l) in links.iter().enumerate() {
        if l.from != MapSource::Collection(id) {
            continue;
        }
        match seen.get(&l.to) {
            Some(Visit::Open) => {
                back.insert(i);
            }
            Some(Visit::New) => find_loops(l.to, links, seen, back),
            Some(Visit::Done) | None => {}
        }
    }
    seen.insert(id, Visit::Done);
}

// ── variables ──────────────────────────────────────────────────────────
// The variables tab edits globals and every collection's own variables;
// a rename (`old` differs from `key`) drops the old name.

/// Save global `key`, dropping `old` when it was renamed.
pub async fn save_global(
    client: &ApiClient,
    old: Option<&str>,
    key: &str,
    value: Value,
) -> Result<(), String> {
    let req = crate::SaveVariableRequest {
        key: key.to_string(),
        value,
        ..Default::default()
    };
    client
        .save_variable(&req)
        .await
        .map_err(|e| e.to_string())?;
    match old.filter(|o| *o != key) {
        Some(old) => client.delete_variable(old).await.map_err(|e| e.to_string()),
        None => Ok(()),
    }
}

/// Save collection `id`'s variable `key`, dropping `old` when it was
/// renamed; the collection is read fresh so nothing else is clobbered.
pub async fn save_collection_var(
    client: &ApiClient,
    id: Uuid,
    old: Option<&str>,
    key: &str,
    value: Value,
) -> Result<(), String> {
    let mut recipe = client.get_recipe(id).await.map_err(|e| e.to_string())?;
    if let Some(old) = old {
        recipe.variables.remove(old);
    }
    recipe.variables.insert(key.to_string(), value);
    set_collection_vars(client, id, recipe.variables).await
}

/// Remove collection `id`'s variable `key`.
pub async fn delete_collection_var(client: &ApiClient, id: Uuid, key: &str) -> Result<(), String> {
    let mut recipe = client.get_recipe(id).await.map_err(|e| e.to_string())?;
    recipe.variables.remove(key);
    set_collection_vars(client, id, recipe.variables).await
}

async fn set_collection_vars(
    client: &ApiClient,
    id: Uuid,
    variables: HashMap<String, Value>,
) -> Result<(), String> {
    let req = UpdateRecipeRequest {
        variables: Some(variables),
        ..Default::default()
    };
    client
        .update_recipe(id, &req)
        .await
        .map(drop)
        .map_err(|e| e.to_string())
}

/// A collection's id, name and variables (name, value as a form shows it).
pub type CollectionVars = (Uuid, String, Vec<(String, String)>);

/// Every collection with its variables, by collection name.
pub fn collection_vars(recipes: &[Recipe]) -> Vec<CollectionVars> {
    let mut out: Vec<_> = recipes
        .iter()
        .map(|r| (r.id, r.name.clone(), sorted_vars(&r.variables)))
        .collect();
    out.sort_by_key(|c| c.1.to_lowercase());
    out
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
            "id": Uuid::nil(), "name": name, "description": null, "flow": { "nodes": [] },
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
    fn collection_names_count_past_the_taken_ones() {
        assert_eq!(
            fresh_collection_name(&[recipe("a", None)]),
            "New collection"
        );
        let rs = [
            recipe("New collection", None),
            recipe("New collections", None),
        ];
        assert_eq!(fresh_collection_name(&rs), "New collection 2");
        let rs = [
            recipe("New collection", None),
            recipe("New collection 4", None),
        ];
        assert_eq!(fresh_collection_name(&rs), "New collection 5");
    }

    #[test]
    fn labels_name_what_a_request_calls() {
        let step = |tool: &str, params: Value| RecipeStep {
            tool: tool.into(),
            params,
            ..Default::default()
        };
        let http = step(
            HTTP_REQUEST,
            serde_json::json!({ "method": "post", "url": "https://example.com/x" }),
        );
        assert_eq!(tool_label(&http), "POST example.com/x");
        let blank = step(HTTP_REQUEST, serde_json::json!({ "url": "https://" }));
        assert_eq!(tool_label(&blank), HTTP_REQUEST);
        let llm = step(LLM_CALL, serde_json::json!({ "model": "m1" }));
        assert_eq!(tool_label(&llm), "LLM m1");
        let bare = step(LLM_CALL, serde_json::json!({ "model": "" }));
        assert_eq!(tool_label(&bare), LLM_CALL);
        let herdr = step(HERDR_API, serde_json::json!({ "method": "pane.read" }));
        assert_eq!(tool_label(&herdr), "pane.read");
    }

    #[test]
    fn send_summary_has_http_status_and_time() {
        let started: chrono::DateTime<chrono::Utc> = "2026-10-19T00:00:00Z".parse().unwrap();
        let payload = serde_json::json!({ "status": 404, "ok": false }).to_string();
        let mut run = RunRecord {
            id: Uuid::nil(),
            recipe_id: None,
            name: None,
            session_id: "w1".into(),
            flow: Flow::chain(vec![RecipeStep {
                id: "get".into(),
                tool: HTTP_REQUEST.into(),
                ..Default::default()
            }]),
            origin: Default::default(),
            pass: 1,
            status: crate::RunStatus::Completed,
            active: Vec::new(),
            results: [(
                "get".to_string(),
                serde_json::json!({ "content": [{ "type": "text", "text": payload }] }),
            )]
            .into(),
            variables: HashMap::new(),
            failed_node: None,
            error: None,
            started_at: started,
            finished_at: Some(started + chrono::Duration::milliseconds(312)),
        };
        assert_eq!(send_summary(&run, "get"), "completed · 404 · 312 ms");
        run.flow = Flow::chain(vec![RecipeStep {
            id: "get".into(),
            tool: "list_panes".into(),
            ..Default::default()
        }]);
        run.finished_at = Some(started + chrono::Duration::milliseconds(1420));
        assert_eq!(send_summary(&run, "get"), "completed · 1.4 s");
    }

    #[test]
    fn the_map_links_triggers_schedules_and_hand_offs() {
        let now: chrono::DateTime<chrono::Utc> = "2026-11-13T00:00:00Z".parse().unwrap();
        let recipe = |n: u128| Recipe {
            id: Uuid::from_u128(n),
            name: n.to_string(),
            description: None,
            flow: Flow::default(),
            variables: HashMap::new(),
            created_at: now,
            updated_at: now,
            is_template: false,
            category: None,
            triggers: Vec::new(),
            trail: Default::default(),
        };
        let (mut a, mut b, mut c) = (recipe(1), recipe(2), recipe(3));
        let gone = Uuid::from_u128(4);
        a.triggers.push(crate::RecipeTrigger {
            on: "pane.created".into(),
            when: serde_json::json!({"pane.tab_id": "w1:t1"})
                .as_object()
                .cloned()
                .unwrap(),
        });
        let run = |to: Uuid| NodeKind::Run {
            collection: to,
            inputs: Default::default(),
        };
        let at = Point { x: 0.0, y: 0.0 };
        add_node(&mut a.flow, "run", run(b.id), at, None);
        add_node(&mut b.flow, "run", run(c.id), at, None);
        add_node(&mut b.flow, "run", run(gone), at, None);
        let payload = serde_json::json!({ "schedules": [
            { "recipe_id": c.id, "cron_schedule": "0 * * * * *", "enabled": false },
            { "recipe_id": "not a uuid", "cron_schedule": "x" },
        ]});
        let sched = schedules(&payload);
        assert_eq!(sched, vec![(c.id, "0 * * * * *".to_string(), false)]);

        let recipes = vec![a.clone(), b.clone(), c.clone()];
        let links = map_links(&recipes, &sched);
        assert_eq!(links.len(), 5);
        assert!(links.contains(&MapLink {
            from: MapSource::Trigger("pane.created".into()),
            to: a.id,
            label: "pane.tab_id=w1:t1".into(),
        }));
        assert!(links.contains(&MapLink {
            from: MapSource::Schedule("0 * * * * *".into()),
            to: c.id,
            label: "off".into(),
        }));
        assert!(
            links
                .iter()
                .any(|l| l.to == gone && l.from == MapSource::Collection(b.id))
        );

        let cols = map_columns(&recipes, &links);
        assert_eq!((cols[&a.id], cols[&b.id], cols[&c.id]), (0, 1, 2));

        // A loop back to a (or c to itself) changes nothing.
        add_node(&mut c.flow, "run", run(a.id), at, None);
        add_node(&mut c.flow, "run", run(c.id), at, None);
        let recipes = vec![a.clone(), b.clone(), c.clone()];
        let cols = map_columns(&recipes, &map_links(&recipes, &sched));
        assert_eq!((cols[&a.id], cols[&b.id], cols[&c.id]), (0, 1, 2));
    }

    #[test]
    fn editing_and_laying_out_a_flow() {
        let step = |id: &str| RecipeStep {
            id: id.into(),
            tool: "list_panes".into(),
            ..Default::default()
        };
        let mut flow = Flow::chain(vec![step("a"), step("b")]);
        let origin = Point { x: 0.0, y: 0.0 };
        let wait = add_node(
            &mut flow,
            "wait",
            NodeKind::Wait { ms: 1 },
            origin,
            Some(("start", Port::Out)),
        );
        assert_eq!(wait, "wait1");
        connect(&mut flow, "wait1", Port::Out, "b");
        connect(&mut flow, "wait1", Port::Out, "b");
        connect(&mut flow, "b", Port::Out, "a");
        assert_eq!(flow.edges.len(), 5, "no duplicate edge: {:?}", flow.edges);
        flow.nodes.iter_mut().find(|n| n.id == "wait1").unwrap().at = None;
        flow.nodes.push(Node::tool(step("stray")));

        let at = layout(&flow);
        let col = |id: &str| at[id].x / (NODE_W + GAP_X);
        // b is two hops away by way of a; the loop back to a is ignored.
        assert_eq!(
            (col("start"), col("a"), col("wait1"), col("b")),
            (0.0, 1.0, 1.0, 2.0)
        );
        assert_eq!(col("stray"), 3.0);
        assert_ne!(at["a"].y, at["wait1"].y);

        remove_node(&mut flow, "b");
        assert!(flow.edges.iter().all(|e| e.from != "b" && e.to != "b"));

        // A branch losing its last rule loses that port's edges.
        let rule = crate::Rule {
            checks: Vec::new(),
            any: false,
        };
        let branch = add_node(
            &mut flow,
            "branch",
            NodeKind::Branch {
                rules: vec![rule.clone(), rule.clone()],
            },
            origin,
            None,
        );
        connect(&mut flow, &branch, Port::Rule(1), "a");
        connect(&mut flow, &branch, Port::Rule(2), "wait1");
        let mut fewer = flow.node(&branch).unwrap().clone();
        fewer.kind = NodeKind::Branch { rules: vec![rule] };
        replace_node(&mut flow, &branch, fewer).unwrap();
        let from_branch: Vec<_> = flow.edges.iter().filter(|e| e.from == branch).collect();
        assert_eq!(from_branch.len(), 1);
        assert_eq!(from_branch[0].port, Port::Rule(1));
    }

    #[test]
    fn request_ids_and_params() {
        let step = |id: &str| RecipeStep {
            id: id.into(),
            tool: "list_panes".into(),
            ..Default::default()
        };
        let mut flow = Flow::chain(vec![step("request2"), step("spawn")]);
        assert_eq!(fresh_node_id(&flow, "request"), "request3");
        replace_node(&mut flow, "request2", Node::tool(step("ls"))).unwrap();
        assert_eq!(flow.edges[0].to, "ls");
        assert_eq!(flow.edges[1].from, "ls");
        assert!(replace_node(&mut flow, "ls", Node::tool(step("spawn"))).is_err());
        assert!(replace_node(&mut flow, "gone", Node::tool(step("x"))).is_err());

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
