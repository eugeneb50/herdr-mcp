//! A collection's end gate: what a run does after its last request.
//!
//! The gate is an ordered list of rules; the first whose checks hold decides
//! (end, restart from the first request, or fail). No rule matching ends the
//! run as completed. Rules also have a one-line text form ([`rule_text`],
//! [`parse_rule`]) the UIs edit and the run trail prints.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::playground::display_value;

/// Requests one run may execute in total, every pass of a restart loop
/// counted, unless the gate sets its own `max_steps`.
pub const DEFAULT_MAX_STEPS: u32 = 1000;

fn default_max_steps() -> u32 {
    DEFAULT_MAX_STEPS
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Gate {
    /// Checked in order after the last request; the first match decides.
    pub rules: Vec<GateRule>,
    /// Requests one run may execute in total, every restart pass counted;
    /// a run that would go over fails.
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GateRule {
    /// All must hold (`any`: at least one). None: the rule always matches,
    /// so it works as the final "else".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<Check>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub any: bool,
    pub then: GateAction,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum GateAction {
    /// Stop; the run completes.
    End,
    /// Run the collection again from its first request, after `delay_ms`.
    Restart {
        #[serde(default, skip_serializing_if = "is_zero")]
        delay_ms: u64,
    },
    /// Stop; the run fails with `message` (empty: the error names the rule).
    Fail {
        #[serde(default)]
        message: String,
    },
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// One check as the gate evaluated it: what `path` resolved to (`found`)
/// and whether the check held. Recorded so a run's trail shows why the
/// gate chose its rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CheckResult {
    pub rule: usize,
    pub path: String,
    pub op: Op,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found: Option<Value>,
    pub held: bool,
}

impl Gate {
    /// The index of the first rule that matches, and every check of the
    /// rules up to it as evaluated. `lookup` resolves a check's path in the
    /// run's scope.
    pub fn evaluate(
        &self,
        lookup: impl Fn(&str) -> Option<Value>,
    ) -> (Option<usize>, Vec<CheckResult>) {
        let mut evaluated = Vec::new();
        for (i, rule) in self.rules.iter().enumerate() {
            let held: Vec<bool> = rule
                .checks
                .iter()
                .map(|c| {
                    let found = lookup(c.path.trim());
                    let held = c.holds(found.clone());
                    evaluated.push(CheckResult {
                        rule: i,
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
                return (Some(i), evaluated);
            }
        }
        (None, evaluated)
    }
}

impl Check {
    /// Whether the check holds for `left`, the value its path resolved to.
    pub fn holds(&self, left: Option<Value>) -> bool {
        let blank = matches!(&left, None | Some(Value::Null))
            || matches!(&left, Some(Value::String(s)) if s.is_empty());
        match self.op {
            Op::Exists => !blank,
            Op::Missing => blank,
            Op::Eq => loose_eq(left.as_ref().unwrap_or(&Value::Null), &self.value),
            Op::Ne => !loose_eq(left.as_ref().unwrap_or(&Value::Null), &self.value),
            Op::Lt => compare(left.as_ref(), &self.value).is_some_and(|o| o.is_lt()),
            Op::Gt => compare(left.as_ref(), &self.value).is_some_and(|o| o.is_gt()),
            Op::Contains => match left {
                Some(Value::String(s)) => s.contains(&display_value(&self.value)),
                Some(Value::Array(items)) => items.iter().any(|i| loose_eq(i, &self.value)),
                Some(Value::Object(map)) => map.contains_key(&display_value(&self.value)),
                _ => false,
            },
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

    /// An op as typed in a rule; `==` `=` `!=` `<` `>` and `null` (for
    /// `missing`) are accepted too.
    fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "eq" | "==" | "=" => Op::Eq,
            "ne" | "!=" => Op::Ne,
            "lt" | "<" => Op::Lt,
            "gt" | ">" => Op::Gt,
            "contains" => Op::Contains,
            "exists" => Op::Exists,
            "missing" | "null" => Op::Missing,
            _ => return None,
        })
    }
}

/// A rule as one line, the form the TUI edits:
/// `read.text contains "error" or code gt 399 → fail agent errored`,
/// `run.pass lt 5 → restart 2000ms`, `always → end`.
pub fn rule_text(rule: &GateRule) -> String {
    let checks = if rule.checks.is_empty() {
        "always".to_string()
    } else {
        rule.checks
            .iter()
            .map(|c| {
                if c.op.takes_value() {
                    format!("{} {} {}", c.path, c.op.name(), c.value)
                } else {
                    format!("{} {}", c.path, c.op.name())
                }
            })
            .collect::<Vec<_>>()
            .join(if rule.any { " or " } else { " and " })
    };
    format!("{checks} → {}", action_text(&rule.then))
}

/// An action as words: `end`, `restart`, `restart 2000ms`, `fail`, `fail <message>`.
pub fn action_text(action: &GateAction) -> String {
    match action {
        GateAction::End => "end".into(),
        GateAction::Restart { delay_ms: 0 } => "restart".into(),
        GateAction::Restart { delay_ms } => format!("restart {delay_ms}ms"),
        GateAction::Fail { message } if message.is_empty() => "fail".into(),
        GateAction::Fail { message } => format!("fail {message}"),
    }
}

/// The inverse of [`rule_text`]. Values are JSON when they parse as JSON
/// (`3`, `true`, `"a b"`), else the bare word.
pub fn parse_rule(text: &str) -> Result<GateRule, String> {
    let (left, right) = split_arrow(text)
        .ok_or("a rule is `<checks> → <action>`, e.g. `count.value lt 3 → restart 500ms`")?;
    let then = parse_action(right.trim())?;
    let words = tokens(left.trim())?;
    if words.is_empty() || matches!(words.as_slice(), [w] if w == "always" || w == "else") {
        return Ok(GateRule {
            checks: Vec::new(),
            any: false,
            then,
        });
    }
    let mut checks = Vec::new();
    let mut joins = Vec::new();
    for part in words.split(|w| w == "and" || w == "or") {
        let [path, op, rest @ ..] = part else {
            return Err(format!(
                "`{}`: a check is `<path> <op> [value]`",
                part.join(" ")
            ));
        };
        let op = Op::parse(op).ok_or_else(|| {
            format!(
                "`{op}` is not an operator ({})",
                Op::ALL.map(Op::name).join(", ")
            )
        })?;
        let value = match (op.takes_value(), rest) {
            (false, []) => Value::Null,
            (false, _) => return Err(format!("`{}` takes no value", op.name())),
            (true, []) => return Err(format!("`{path} {}` needs a value", op.name())),
            (true, words) => {
                let raw = words.join(" ");
                serde_json::from_str(&raw).unwrap_or(Value::String(raw))
            }
        };
        checks.push(Check {
            path: path.clone(),
            op,
            value,
        });
    }
    joins.extend(words.iter().filter(|w| *w == "and" || *w == "or"));
    if joins.windows(2).any(|w| w[0] != w[1]) {
        return Err("use either `and` or `or` in one rule, not both".into());
    }
    Ok(GateRule {
        checks,
        any: joins.first().is_some_and(|j| *j == "or"),
        then,
    })
}

fn parse_action(text: &str) -> Result<GateAction, String> {
    let (word, rest) = text.split_once(' ').unwrap_or((text, ""));
    let rest = rest.trim();
    match word {
        "end" if rest.is_empty() => Ok(GateAction::End),
        "restart" => {
            let ms = rest.trim_end_matches("ms").trim();
            let delay_ms = if ms.is_empty() {
                0
            } else {
                ms.replace('_', "")
                    .parse()
                    .map_err(|_| format!("`{rest}`: restart takes a delay like `500ms`"))?
            };
            Ok(GateAction::Restart { delay_ms })
        }
        "fail" => Ok(GateAction::Fail {
            message: rest.to_string(),
        }),
        _ => Err(format!(
            "`{text}`: the action is `end`, `restart [500ms]` or `fail [message]`"
        )),
    }
}

/// `text` split at its first `→` (or `->`) outside quotes.
fn split_arrow(text: &str) -> Option<(&str, &str)> {
    let mut quoted = false;
    for (i, c) in text.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '→' if !quoted => return Some((&text[..i], &text[i + c.len_utf8()..])),
            '-' if !quoted && text[i..].starts_with("->") => {
                return Some((&text[..i], &text[i + 2..]));
            }
            _ => {}
        }
    }
    None
}

/// Words split on spaces; a `"…"` (JSON string) stays one word, quotes kept.
fn tokens(text: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for c in text.chars() {
        if quoted {
            word.push(c);
            match c {
                '\\' if !escaped => escaped = true,
                '"' if !escaped => quoted = false,
                _ => escaped = false,
            }
        } else if c == '"' {
            word.push(c);
            quoted = true;
        } else if c.is_whitespace() {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
        } else {
            word.push(c);
        }
    }
    if quoted {
        return Err("a quote is not closed".into());
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

    fn rule(checks: Vec<Check>, then: GateAction) -> GateRule {
        GateRule {
            checks,
            any: false,
            then,
        }
    }

    #[test]
    fn ops_compare_loosely() {
        let c = |op, v| check("x", op, v);
        assert!(c(Op::Eq, json!(200)).holds(Some(json!("200"))));
        assert!(c(Op::Eq, json!("ok")).holds(Some(json!("ok"))));
        assert!(c(Op::Eq, json!(1)).holds(Some(json!(1.0))));
        assert!(c(Op::Ne, json!("a")).holds(None));
        assert!(c(Op::Eq, Value::Null).holds(None));
        assert!(c(Op::Lt, json!(5)).holds(Some(json!("3"))));
        assert!(!c(Op::Lt, json!(5)).holds(Some(json!("abc"))));
        assert!(!c(Op::Gt, json!(5)).holds(None));
        assert!(c(Op::Contains, json!("err")).holds(Some(json!("an error"))));
        assert!(c(Op::Contains, json!(2)).holds(Some(json!([1, "2"]))));
        assert!(c(Op::Contains, json!("k")).holds(Some(json!({ "k": 1 }))));
        for blank in [None, Some(Value::Null), Some(json!(""))] {
            assert!(c(Op::Missing, Value::Null).holds(blank.clone()));
            assert!(!c(Op::Exists, Value::Null).holds(blank));
        }
        assert!(c(Op::Exists, Value::Null).holds(Some(json!(0))));
    }

    #[test]
    fn first_matching_rule_decides() {
        let gate = Gate {
            rules: vec![
                rule(
                    vec![check("out", Op::Missing, Value::Null)],
                    GateAction::Restart { delay_ms: 10 },
                ),
                GateRule {
                    checks: vec![
                        check("out", Op::Contains, json!("error")),
                        check("code", Op::Gt, json!(399)),
                    ],
                    any: true,
                    then: GateAction::Fail {
                        message: "bad".into(),
                    },
                },
                rule(vec![], GateAction::End),
            ],
            max_steps: DEFAULT_MAX_STEPS,
        };
        let decide = |scope: Value| gate.evaluate(|p| scope.get(p).cloned()).0;
        assert_eq!(decide(json!({})), Some(0));
        assert_eq!(decide(json!({ "out": "error!" })), Some(1));
        assert_eq!(decide(json!({ "out": "fine", "code": 500 })), Some(1));
        assert_eq!(decide(json!({ "out": "fine", "code": 200 })), Some(2));
        let empty = Gate {
            rules: vec![],
            max_steps: 1,
        };
        assert_eq!(empty.evaluate(|_| None), (None, vec![]));
        // Every check of the rules tried is recorded, with what it found.
        let (rule, checks) = gate.evaluate(|p| (p == "out").then(|| json!("fine")));
        assert_eq!(rule, Some(2));
        let found: Vec<_> = checks
            .iter()
            .map(|c| (c.rule, c.path.as_str(), c.found.clone(), c.held))
            .collect();
        assert_eq!(
            found,
            [
                (0, "out", Some(json!("fine")), false),
                (1, "out", Some(json!("fine")), false),
                (1, "code", None, false),
            ]
        );
    }

    #[test]
    fn rule_text_round_trips() {
        for text in [
            "always → end",
            "count.value lt 3 → restart 500ms",
            "read.text missing → restart",
            r#"read.text contains "an error" or code gt 399 → fail agent errored"#,
            r#"a eq "ok" and b ne true and c exists → end"#,
        ] {
            let rule = parse_rule(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(rule_text(&rule), text);
        }
        let rule = parse_rule("x == ok -> fail").unwrap();
        assert_eq!(rule.checks[0].value, json!("ok"));
        assert_eq!(rule_text(&rule), r#"x eq "ok" → fail"#);
        assert_eq!(parse_rule("else -> end").unwrap().checks, vec![]);
        for bad in [
            "x lt 3",
            "x around 3 → end",
            "x lt → end",
            "x exists 3 → end",
            "a exists and b exists or c exists → end",
            "x eq 1 → jump",
            r#"x eq "open → end"#,
        ] {
            assert!(parse_rule(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn gate_round_trips_with_defaults() {
        let gate: Gate = serde_json::from_value(json!({
            "rules": [
                { "checks": [{ "path": "a", "op": "missing" }], "then": { "action": "restart" } },
                { "then": { "action": "fail", "message": "no" } }
            ]
        }))
        .unwrap();
        assert_eq!(gate.max_steps, DEFAULT_MAX_STEPS);
        assert_eq!(gate.rules[0].then, GateAction::Restart { delay_ms: 0 });
        let back = serde_json::to_value(&gate).unwrap();
        assert_eq!(
            back["rules"][0],
            json!({ "checks": [{ "path": "a", "op": "missing" }], "then": { "action": "restart" } })
        );
    }
}
