//! A collection's flow: nodes joined by edges, run as a graph.
//!
//! One `start` node; its edges fan out (branches run concurrently). A node
//! with several incoming edges waits for each of them to fire or to be
//! ruled out (a `branch` took another port), so it is also the join. An
//! edge back to a node already upstream is a loop: taking it runs that part
//! of the flow again (`run.pass` counts the passes). A node whose port has
//! no edge ends that path; the run ends when no path is left.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::playground::display_value;
use crate::{Extract, RecipeStep};

/// Node runs one run may start in total, every loop pass counted, unless
/// the flow sets its own `max_steps`.
pub const DEFAULT_MAX_STEPS: u32 = 1000;

/// The start node's id in a flow built by [`Flow::chain`].
pub const START: &str = "start";

fn default_max_steps() -> u32 {
    DEFAULT_MAX_STEPS
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Flow {
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    /// Node runs one run may start in total, every loop pass counted; a run
    /// that would go over fails.
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
}

impl Default for Flow {
    fn default() -> Self {
        Self::chain(Vec::new())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Node {
    /// Unique in the flow; later nodes read this one's result as
    /// `{{ id.path }}`.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Where the editor draws it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Point>,
    #[serde(flatten)]
    pub kind: NodeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeKind {
    /// Where a run begins. Port `out`.
    Start,
    /// Call a herdr-mcp tool. Ports `out`, and `error`: with an edge on
    /// `error`, a failure (after the retries) takes it instead of failing
    /// the run.
    Tool {
        tool: String,
        /// The tool's arguments; strings may hold `{{ ... }}` placeholders.
        #[serde(default)]
        params: Value,
        /// Applied in order once the call succeeds.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extract: Vec<Extract>,
        #[serde(default, skip_serializing_if = "Retry::is_none")]
        retry: Retry,
        /// Each attempt fails after this long.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
    },
    /// Check rules in order; the first that holds picks its port (`1`, `2`,
    /// …), none picks `else`.
    Branch { rules: Vec<Rule> },
    /// Run another saved collection as a child run in this run's workspace,
    /// with its own variables, the globals and `inputs` (placeholders
    /// filled from this run), and wait for it. The result is
    /// `{run_id, status, error, variables}` (`variables`: what the child
    /// extracted). Ports `out`, `error` (the child did not complete).
    Run {
        collection: Uuid,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        inputs: BTreeMap<String, Value>,
    },
    /// Sleep, then port `out`.
    Wait { ms: u64 },
    /// The run fails with `message` (empty: the error names the node).
    Fail {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        message: String,
    },
}

/// Attempts after the first; `delay_ms` between them.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct Retry {
    pub times: u32,
    #[serde(default)]
    pub delay_ms: u64,
}

impl Retry {
    pub fn is_none(&self) -> bool {
        self.times == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Edge {
    pub from: String,
    #[serde(default, skip_serializing_if = "Port::is_out")]
    pub port: Port,
    pub to: String,
}

/// An output of a node: `out`, `error`, a branch rule's number (`1`, …) or
/// `else`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Port {
    #[default]
    Out,
    Error,
    /// A branch rule, from 1.
    Rule(usize),
    Else,
}

impl schemars::JsonSchema for Port {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Port".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "`out` (default), `error`, a branch rule's number (`1`, …) or `else`",
        })
    }
}

impl Port {
    pub fn is_out(&self) -> bool {
        *self == Self::Out
    }
}

impl std::fmt::Display for Port {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Out => f.write_str("out"),
            Self::Error => f.write_str("error"),
            Self::Rule(n) => write!(f, "{n}"),
            Self::Else => f.write_str("else"),
        }
    }
}

impl From<Port> for String {
    fn from(p: Port) -> Self {
        p.to_string()
    }
}

impl TryFrom<String> for Port {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        match s.as_str() {
            "out" | "" => Ok(Self::Out),
            "error" => Ok(Self::Error),
            "else" => Ok(Self::Else),
            n => match n.parse() {
                Ok(n) if n > 0 => Ok(Self::Rule(n)),
                _ => Err(format!("`{s}` is not a port (out, error, 1…, else)")),
            },
        }
    }
}

impl NodeKind {
    /// The ports this node may fire.
    pub fn ports(&self) -> Vec<Port> {
        match self {
            Self::Start | Self::Wait { .. } => vec![Port::Out],
            Self::Tool { .. } | Self::Run { .. } => vec![Port::Out, Port::Error],
            Self::Branch { rules } => (1..=rules.len())
                .map(Port::Rule)
                .chain([Port::Else])
                .collect(),
            Self::Fail { .. } => Vec::new(),
        }
    }

    /// `tool`, `branch`, … as on the wire.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Tool { .. } => "tool",
            Self::Branch { .. } => "branch",
            Self::Run { .. } => "run",
            Self::Wait { .. } => "wait",
            Self::Fail { .. } => "fail",
        }
    }
}

impl Node {
    /// A tool node from a step.
    pub fn tool(step: RecipeStep) -> Self {
        Self {
            id: step.id,
            description: step.description,
            at: None,
            kind: NodeKind::Tool {
                tool: step.tool,
                params: step.params,
                extract: step.extract,
                retry: Retry::default(),
                timeout_ms: None,
            },
        }
    }

    /// A tool node as a step (its id, tool, params, extracts), for the
    /// request editor.
    pub fn step(&self) -> Option<RecipeStep> {
        match &self.kind {
            NodeKind::Tool {
                tool,
                params,
                extract,
                ..
            } => Some(RecipeStep {
                id: self.id.clone(),
                tool: tool.clone(),
                params: params.clone(),
                description: self.description.clone(),
                extract: extract.clone(),
            }),
            _ => None,
        }
    }

    /// The tool a tool node calls.
    pub fn tool_name(&self) -> Option<&str> {
        match &self.kind {
            NodeKind::Tool { tool, .. } => Some(tool),
            _ => None,
        }
    }
}

impl Flow {
    /// `start` → `steps[0]` → `steps[1]` → …: a list of requests as a flow.
    pub fn chain(steps: Vec<RecipeStep>) -> Self {
        let mut nodes = vec![Node {
            id: START.into(),
            description: None,
            at: None,
            kind: NodeKind::Start,
        }];
        nodes.extend(steps.into_iter().map(Node::tool));
        let edges = nodes
            .windows(2)
            .map(|w| Edge {
                from: w[0].id.clone(),
                port: Port::Out,
                to: w[1].id.clone(),
            })
            .collect();
        Self {
            nodes,
            edges,
            max_steps: DEFAULT_MAX_STEPS,
        }
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn start(&self) -> Option<&Node> {
        self.nodes.iter().find(|n| n.kind == NodeKind::Start)
    }

    /// The tool nodes, in flow order.
    pub fn tools(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter().filter(|n| n.tool_name().is_some())
    }

    /// The collections its `run` nodes start.
    pub fn collections(&self) -> impl Iterator<Item = Uuid> + '_ {
        self.nodes.iter().filter_map(|n| match n.kind {
            NodeKind::Run { collection, .. } => Some(collection),
            _ => None,
        })
    }

    /// What makes this flow unrunnable, one line each; empty when it runs.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let starts = self
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Start)
            .count();
        if starts != 1 {
            out.push(format!("a flow has one start node, this one has {starts}"));
        }
        let mut seen = HashSet::new();
        for n in &self.nodes {
            if let Err(e) = check_id(&n.id) {
                out.push(e);
            } else if !seen.insert(n.id.as_str()) {
                out.push(format!("two nodes are called `{}`", n.id));
            }
            if let NodeKind::Tool { tool, .. } = &n.kind
                && tool.trim().is_empty()
            {
                out.push(format!("`{}` names no tool", n.id));
            }
        }
        let mut edges = HashSet::new();
        for e in &self.edges {
            let Some(from) = self.node(&e.from) else {
                out.push(format!("an edge leaves `{}`, which is not a node", e.from));
                continue;
            };
            match self.node(&e.to) {
                None => out.push(format!("an edge goes to `{}`, which is not a node", e.to)),
                Some(to) if to.kind == NodeKind::Start => {
                    out.push(format!(
                        "`{}` → `{}`: nothing goes into the start",
                        e.from, e.to
                    ));
                }
                Some(_) => {}
            }
            if !from.kind.ports().contains(&e.port) {
                out.push(format!(
                    "`{}` ({}) has no port `{}`",
                    e.from,
                    from.kind.name(),
                    e.port
                ));
            }
            if !edges.insert(e) {
                out.push(format!("`{}` {} → `{}` twice", e.from, e.port, e.to));
            }
        }
        out
    }

    /// The nodes a run can reach from the start.
    pub fn reachable(&self) -> HashSet<&str> {
        let mut seen = HashSet::new();
        let mut todo: Vec<&str> = self.start().map(|n| n.id.as_str()).into_iter().collect();
        while let Some(id) = todo.pop() {
            if seen.insert(id) {
                todo.extend(
                    self.edges
                        .iter()
                        .filter(|e| e.from == id)
                        .map(|e| e.to.as_str()),
                );
            }
        }
        seen
    }

    /// The flow's shape for running it: which edges loop back, and each
    /// node's forward in-edges.
    pub fn shape(&self) -> Shape {
        // Depth-first from the start; an edge to a node on the current path
        // is a loop. Edges of unreachable nodes are forward.
        let out_edges = |id: &str| -> Vec<usize> {
            self.edges
                .iter()
                .enumerate()
                .filter(|(_, e)| e.from == id)
                .map(|(i, _)| i)
                .collect()
        };
        let mut back = HashSet::new();
        let mut done: HashSet<&str> = HashSet::new();
        if let Some(start) = self.start() {
            let mut on_path: HashSet<&str> = HashSet::new();
            let mut stack: Vec<(&str, Vec<usize>)> =
                vec![(start.id.as_str(), out_edges(&start.id))];
            on_path.insert(&start.id);
            while let Some((id, pending)) = stack.last_mut() {
                let Some(i) = pending.first().copied() else {
                    on_path.remove(*id);
                    done.insert(*id);
                    stack.pop();
                    continue;
                };
                pending.remove(0);
                let to = self.edges[i].to.as_str();
                if on_path.contains(to) {
                    back.insert(i);
                } else if !done.contains(to) && self.node(to).is_some() {
                    on_path.insert(to);
                    stack.push((to, out_edges(to)));
                }
            }
        }
        let mut inputs: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, e) in self.edges.iter().enumerate() {
            if !back.contains(&i) {
                inputs.entry(e.to.clone()).or_default().push(i);
            }
        }
        Shape { back, inputs }
    }
}

/// See [`Flow::shape`].
#[derive(Debug, Clone, Default)]
pub struct Shape {
    /// Indexes into `edges` of the edges that loop back.
    pub back: HashSet<usize>,
    /// Each node's incoming edges that do not loop back.
    pub inputs: HashMap<String, Vec<usize>>,
}

impl Shape {
    /// The nodes downstream of `from` (itself included) along forward
    /// edges: what a loop back to `from` runs again.
    pub fn region<'a>(&self, flow: &'a Flow, from: &'a str) -> HashSet<&'a str> {
        let mut seen = HashSet::new();
        let mut todo = vec![from];
        while let Some(id) = todo.pop() {
            if seen.insert(id) {
                todo.extend(
                    flow.edges
                        .iter()
                        .enumerate()
                        .filter(|(i, e)| e.from == id && !self.back.contains(i))
                        .map(|(_, e)| e.to.as_str()),
                );
            }
        }
        seen
    }
}

/// A node id must work as the head of a `{{ id.path }}` placeholder.
fn check_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("a node has no id".into());
    }
    if id == "run" {
        return Err("`run` is taken (`run.pass`); name the node something else".into());
    }
    if !id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!(
            "`{id}`: a node id is letters, digits, `_` and `-` (it heads `{{{{ {id}.path }}}}`)"
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Rule {
    /// All must hold (`any`: at least one). None: the rule always holds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<Check>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub any: bool,
}

/// `path` resolved like a `{{ path }}` placeholder (`list.panes[0].pane_id`,
/// a variable, `run.pass`), compared with `value`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Check {
    pub path: String,
    pub op: Op,
    /// Unused by `exists` / `missing`.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// Equal; a string and a number or bool compare by their text (`"200"`
    /// equals `200`).
    Eq,
    Ne,
    /// Both sides read as numbers; false when either is not one.
    Lt,
    Gt,
    /// A substring of a string, an element of an array, a key of an object.
    Contains,
    /// Resolves to something other than null or "".
    Exists,
    /// Unresolved, null or "".
    Missing,
}

/// One check as a branch evaluated it: what `path` resolved to (`found`)
/// and whether it held. Recorded so the trail shows why a port was taken.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CheckResult {
    /// The rule's number, from 1.
    pub rule: usize,
    pub path: String,
    pub op: Op,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found: Option<Value>,
    pub held: bool,
}

/// Evaluate a branch's `rules` in order: the port of the first that holds
/// (`else` when none does) and every check tried on the way. `lookup`
/// resolves a check's path in the run's scope.
pub fn decide(rules: &[Rule], lookup: impl Fn(&str) -> Option<Value>) -> (Port, Vec<CheckResult>) {
    let mut tried = Vec::new();
    for (i, rule) in rules.iter().enumerate() {
        let held: Vec<bool> = rule
            .checks
            .iter()
            .map(|c| {
                let found = lookup(c.path.trim());
                let held = c.holds(found.as_ref());
                tried.push(CheckResult {
                    rule: i + 1,
                    path: c.path.clone(),
                    op: c.op,
                    value: c.value.clone(),
                    found,
                    held,
                });
                held
            })
            .collect();
        let matched = if rule.any {
            held.is_empty() || held.contains(&true)
        } else {
            !held.contains(&false)
        };
        if matched {
            return (Port::Rule(i + 1), tried);
        }
    }
    (Port::Else, tried)
}

impl Check {
    /// Whether the check holds for `left`, the value its path resolved to.
    pub fn holds(&self, left: Option<&Value>) -> bool {
        let blank = matches!(left, None | Some(Value::Null))
            || matches!(left, Some(Value::String(s)) if s.is_empty());
        match self.op {
            Op::Exists => !blank,
            Op::Missing => blank,
            Op::Eq => loose_eq(left.unwrap_or(&Value::Null), &self.value),
            Op::Ne => !loose_eq(left.unwrap_or(&Value::Null), &self.value),
            Op::Lt => compare(left, &self.value).is_some_and(|o| o.is_lt()),
            Op::Gt => compare(left, &self.value).is_some_and(|o| o.is_gt()),
            Op::Contains => match left {
                Some(Value::String(s)) => s.contains(&display_value(&self.value)),
                Some(Value::Array(items)) => items.iter().any(|i| loose_eq(i, &self.value)),
                Some(Value::Object(map)) => map.contains_key(&display_value(&self.value)),
                _ => false,
            },
        }
    }

    /// `path op value` as the trail and the editor print it.
    pub fn text(&self) -> String {
        if self.op.takes_value() {
            format!("{} {} {}", self.path, self.op.name(), self.value)
        } else {
            format!("{} {}", self.path, self.op.name())
        }
    }
}

fn loose_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(_), Value::Number(_) | Value::Bool(_))
        | (Value::Number(_) | Value::Bool(_), Value::String(_)) => {
            display_value(a).trim() == display_value(b).trim()
        }
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn compare(left: Option<&Value>, right: &Value) -> Option<std::cmp::Ordering> {
    number(left?)?.partial_cmp(&number(right)?)
}

impl Op {
    pub const ALL: [Op; 7] = [
        Op::Eq,
        Op::Ne,
        Op::Lt,
        Op::Gt,
        Op::Contains,
        Op::Exists,
        Op::Missing,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Op::Eq => "eq",
            Op::Ne => "ne",
            Op::Lt => "lt",
            Op::Gt => "gt",
            Op::Contains => "contains",
            Op::Exists => "exists",
            Op::Missing => "missing",
        }
    }

    /// `exists` / `missing` compare with nothing.
    pub fn takes_value(self) -> bool {
        !matches!(self, Op::Exists | Op::Missing)
    }
}

/// A `run` node's inputs as `key=value …`, the form the editor edits.
pub fn inputs_text(inputs: &BTreeMap<String, Value>) -> String {
    inputs
        .iter()
        .map(|(k, v)| match v {
            Value::String(s) if !s.contains(char::is_whitespace) && !s.is_empty() => {
                format!("{k}={s}")
            }
            v => format!("{k}={v}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The inverse of [`inputs_text`]: values are JSON when they parse
/// (`3`, `true`, `"a b"`), else text.
pub fn parse_inputs(text: &str) -> Result<BTreeMap<String, Value>, String> {
    words(text)?
        .iter()
        .map(|kv| {
            let (k, v) = kv
                .split_once('=')
                .filter(|(k, _)| !k.is_empty())
                .ok_or_else(|| format!("`{kv}`: an input is `key=value`"))?;
            let v = serde_json::from_str(v).unwrap_or_else(|_| Value::String(v.to_string()));
            Ok((k.to_string(), v))
        })
        .collect()
}

/// Split on whitespace, keeping a `"…"` JSON string (escapes and all) in
/// one word.
fn words(text: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                word.push('"');
                let mut escaped = false;
                loop {
                    let c = chars.next().ok_or("a `\"` is never closed")?;
                    word.push(c);
                    match c {
                        '\\' if !escaped => escaped = true,
                        '"' if !escaped => break,
                        _ => escaped = false,
                    }
                }
            }
            c if c.is_whitespace() => {
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn check(path: &str, op: Op, value: Value) -> Check {
        Check {
            path: path.into(),
            op,
            value,
        }
    }

    fn flow(value: Value) -> Flow {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn ops_compare_loosely() {
        let c = |op, v| check("x", op, v);
        assert!(c(Op::Eq, json!(200)).holds(Some(&json!("200"))));
        assert!(c(Op::Eq, json!("ok")).holds(Some(&json!("ok"))));
        assert!(c(Op::Eq, json!(1)).holds(Some(&json!(1.0))));
        assert!(c(Op::Ne, json!("a")).holds(None));
        assert!(c(Op::Eq, Value::Null).holds(None));
        assert!(c(Op::Lt, json!(5)).holds(Some(&json!("3"))));
        assert!(!c(Op::Lt, json!(5)).holds(Some(&json!("abc"))));
        assert!(!c(Op::Gt, json!(5)).holds(None));
        assert!(c(Op::Contains, json!("err")).holds(Some(&json!("an error"))));
        assert!(c(Op::Contains, json!(2)).holds(Some(&json!([1, "2"]))));
        assert!(c(Op::Contains, json!("k")).holds(Some(&json!({ "k": 1 }))));
        for blank in [None, Some(&Value::Null), Some(&json!(""))] {
            assert!(c(Op::Missing, Value::Null).holds(blank));
            assert!(!c(Op::Exists, Value::Null).holds(blank));
        }
        assert!(c(Op::Exists, Value::Null).holds(Some(&json!(0))));
    }

    #[test]
    fn first_holding_rule_picks_the_port() {
        let rules = vec![
            Rule {
                checks: vec![check("out", Op::Missing, Value::Null)],
                any: false,
            },
            Rule {
                checks: vec![
                    check("out", Op::Contains, json!("error")),
                    check("code", Op::Gt, json!(399)),
                ],
                any: true,
            },
        ];
        let pick = |scope: Value| decide(&rules, |p| scope.get(p).cloned()).0;
        assert_eq!(pick(json!({})), Port::Rule(1));
        assert_eq!(pick(json!({ "out": "error!" })), Port::Rule(2));
        assert_eq!(pick(json!({ "out": "fine", "code": 500 })), Port::Rule(2));
        assert_eq!(pick(json!({ "out": "fine", "code": 200 })), Port::Else);
        assert_eq!(decide(&[], |_| None), (Port::Else, vec![]));
        let (_, tried) = decide(&rules, |p| (p == "out").then(|| json!("fine")));
        let found: Vec<_> = tried
            .iter()
            .map(|c| (c.rule, c.path.as_str(), c.found.clone(), c.held))
            .collect();
        assert_eq!(
            found,
            [
                (1, "out", Some(json!("fine")), false),
                (2, "out", Some(json!("fine")), false),
                (2, "code", None, false),
            ]
        );
    }

    #[test]
    fn wire_form_round_trips() {
        let value = json!({
            "nodes": [
                { "id": "start", "type": "start" },
                { "id": "ls", "type": "tool", "tool": "list_panes", "params": {},
                  "retry": { "times": 2, "delay_ms": 100 }, "at": { "x": 1.0, "y": 2.0 } },
                { "id": "ok", "type": "branch", "rules": [{ "checks": [{ "path": "ls.panes", "op": "exists" }] }] },
                { "id": "nap", "type": "wait", "ms": 50 },
                { "id": "no", "type": "fail", "message": "none" }
            ],
            "edges": [
                { "from": "start", "to": "ls" },
                { "from": "ls", "to": "ok" },
                { "from": "ok", "port": "1", "to": "nap" },
                { "from": "ok", "port": "else", "to": "no" },
                { "from": "nap", "to": "ls" }
            ]
        });
        let f = flow(value.clone());
        assert_eq!(f.max_steps, DEFAULT_MAX_STEPS);
        assert_eq!(f.edges[2].port, Port::Rule(1));
        assert!(f.problems().is_empty(), "{:?}", f.problems());
        let mut back = serde_json::to_value(&f).unwrap();
        back.as_object_mut().unwrap().remove("max_steps");
        assert_eq!(back, value);
        assert!(
            serde_json::from_value::<Edge>(json!({"from": "a", "port": "0", "to": "b"})).is_err()
        );
    }

    #[test]
    fn chain_is_start_then_each_step() {
        let step = |id: &str| RecipeStep {
            id: id.into(),
            tool: "list_panes".into(),
            ..Default::default()
        };
        let f = Flow::chain(vec![step("a"), step("b")]);
        let ids: Vec<_> = f.edges.iter().map(|e| (&*e.from, &*e.to)).collect();
        assert_eq!(ids, [("start", "a"), ("a", "b")]);
        assert!(f.problems().is_empty());
        assert_eq!(Flow::default().nodes.len(), 1);
    }

    #[test]
    fn problems_name_what_is_wrong() {
        let f = flow(json!({
            "nodes": [
                { "id": "start", "type": "start" },
                { "id": "start", "type": "start" },
                { "id": "run", "type": "wait", "ms": 1 },
                { "id": "a.b", "type": "tool", "tool": "" },
                { "id": "f", "type": "fail" }
            ],
            "edges": [
                { "from": "f", "to": "start" },
                { "from": "start", "port": "error", "to": "ghost" },
                { "from": "nobody", "to": "f" }
            ]
        }));
        let p = f.problems().join("\n");
        for needle in [
            "one start node, this one has 2",
            "two nodes are called `start`",
            "`run` is taken",
            "`a.b`: a node id",
            "names no tool",
            "nothing goes into the start",
            "`f` (fail) has no port `out`",
            "`ghost`, which is not a node",
            "`start` (start) has no port `error`",
            "`nobody`, which is not a node",
        ] {
            assert!(p.contains(needle), "{needle} not in:\n{p}");
        }
    }

    #[test]
    fn shape_finds_loops_and_joins() {
        // start → a → b → c → a (loop); start → x → c (join of b and x).
        let f = flow(json!({
            "nodes": [
                { "id": "start", "type": "start" },
                { "id": "a", "type": "wait", "ms": 0 },
                { "id": "b", "type": "wait", "ms": 0 },
                { "id": "c", "type": "branch", "rules": [{}] },
                { "id": "x", "type": "wait", "ms": 0 },
                { "id": "lost", "type": "wait", "ms": 0 }
            ],
            "edges": [
                { "from": "start", "to": "a" },
                { "from": "a", "to": "b" },
                { "from": "b", "to": "c" },
                { "from": "c", "port": "1", "to": "a" },
                { "from": "start", "to": "x" },
                { "from": "x", "to": "c" }
            ]
        }));
        let shape = f.shape();
        assert_eq!(shape.back, HashSet::from([3]));
        assert_eq!(shape.inputs["c"], [2, 5]);
        assert_eq!(shape.inputs["a"], [0]);
        let region = shape.region(&f, "a");
        assert_eq!(region, HashSet::from(["a", "b", "c"]));
        assert!(!f.reachable().contains("lost"));
    }

    #[test]
    fn inputs_round_trip() {
        let text = r#"msg="a b" n=3 pane={{list.panes[0].pane_id}}"#;
        let inputs = parse_inputs(text).unwrap();
        assert_eq!(inputs["n"], json!(3));
        assert_eq!(inputs["msg"], json!("a b"));
        assert_eq!(parse_inputs(&inputs_text(&inputs)).unwrap(), inputs);
        assert!(parse_inputs("nokey").is_err());
        assert!(parse_inputs(r#"a="open"#).is_err());
    }
}
