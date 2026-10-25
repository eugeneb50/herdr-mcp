//! A collection's end gate: what a run does after its last request.
//!
//! The gate is an ordered list of rules; the first whose checks hold decides
//! (end, restart from the first request, or fail). No rule matching ends the
//! run as completed. The evaluator is here, not in the server, so the UIs can
//! preview which rule a finished run would take.

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    /// Stop; the run fails with `message`.
    Fail { message: String },
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
                Some(Value::String(s)) => s.contains(&text(&self.value)),
                Some(Value::Array(items)) => items.iter().any(|i| loose_eq(i, &self.value)),
                Some(Value::Object(map)) => map.contains_key(&text(&self.value)),
                _ => false,
            },
        }
    }
}

/// A value as placeholder text: strings bare, null empty, the rest as JSON.
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn loose_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(_), Value::Number(_) | Value::Bool(_))
        | (Value::Number(_) | Value::Bool(_), Value::String(_)) => text(a).trim() == text(b).trim(),
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
