//! Client-side variable helpers: `{{name}}` substitution (preview only —
//! actual resolution happens server-side), display formatting, auto-var
//! grouping, and the pane choices pane fields offer. Port of
//! `VariableStore.tsx` semantics + `api/variables.ts`.

use herdr_mcp_api::HerdrPane;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

/// Render any JSON value as a plain string (never `[object Object]`).
pub fn display_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    }
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Client-side preview substitution: `{{name}}` / `{{a.b}}` resolve
/// user-first, then auto; unknown names pass through untouched.
/// Mirrors `/\{\{(\w[\w.]*\w|\w)\}\}/g`.
pub fn substitute(
    template: &str,
    user: &HashMap<String, String>,
    auto: &HashMap<String, String>,
) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        match after.find("}}") {
            Some(close) => {
                let name = &after[..close];
                if valid_name(name) {
                    let resolved = user
                        .get(name)
                        .or_else(|| auto.get(name))
                        .cloned()
                        .unwrap_or_else(|| format!("{{{{{name}}}}}"));
                    out.push_str(&resolved);
                } else {
                    out.push_str("{{");
                    out.push_str(name);
                    out.push_str("}}");
                }
                rest = &after[close + 2..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn valid_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !is_word_char(first) {
        return false;
    }
    if name.len() == 1 {
        return true;
    }
    if !name.chars().all(|c| is_word_char(c) || c == '.') {
        return false;
    }
    is_word_char(name.chars().last().unwrap())
}

/// A dotted-prefix group: (prefix, [(short_key, value)]).
pub type VarGroup = (String, Vec<(String, String)>);

/// Split auto vars into dotted-prefix groups + ungrouped aliases, sorted.
/// Port of the `AutoVarsTable` grouping.
pub fn group_auto(entries: &[(String, String)]) -> (Vec<VarGroup>, Vec<(String, String)>) {
    let mut groups: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut ungrouped = Vec::new();
    for (k, v) in entries {
        match k.find('.') {
            Some(dot) if dot > 0 => groups
                .entry(k[..dot].to_string())
                .or_default()
                .push((k[dot + 1..].to_string(), v.clone())),
            _ => ungrouped.push((k.clone(), v.clone())),
        }
    }
    let mut grouped: Vec<_> = groups.into_iter().collect();
    grouped.sort_by(|a, b| a.0.cmp(&b.0));
    ungrouped.sort_by(|a, b| a.0.cmp(&b.0));
    (grouped, ungrouped)
}

/// The live panes among the auto variables (`GET /api/variables/auto`
/// keys each pane by its own id), labeled ones first, then by id.
pub fn auto_panes(auto: &HashMap<String, Value>) -> Vec<HerdrPane> {
    let mut panes: Vec<HerdrPane> = auto
        .iter()
        .filter(|(k, v)| v["pane_id"].as_str() == Some(k.as_str()))
        .filter_map(|(_, v)| serde_json::from_value(v.clone()).ok())
        .collect();
    panes.sort_by(|a, b| (a.label.is_none(), &a.pane_id).cmp(&(b.label.is_none(), &b.pane_id)));
    panes
}

/// herdr pane labels — what the tools' `label` parameter resolves — with
/// the pane id each names, sorted by label. Registry roles are left out: a
/// `label` that is only a role does not resolve.
pub fn pane_names(auto: &HashMap<String, Value>) -> Vec<(String, String)> {
    let names: BTreeMap<_, _> = auto
        .values()
        .filter_map(|v| {
            Some((
                v["label"].as_str()?.to_string(),
                v["pane_id"].as_str()?.to_string(),
            ))
        })
        .collect();
    names.into_iter().collect()
}

/// `builder · claude · /home/me/app` from the parts a pane has.
pub fn pane_text<'a>(parts: impl IntoIterator<Item = &'a Option<String>>) -> String {
    parts
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn maps() -> (HashMap<String, String>, HashMap<String, String>) {
        (
            [("a".to_string(), "USER".to_string())].into(),
            [
                ("a".to_string(), "AUTO".to_string()),
                ("pane.x".to_string(), "out".to_string()),
            ]
            .into(),
        )
    }

    #[test]
    fn substitute_user_wins_and_unknown_passes_through() {
        let (u, a) = maps();
        assert_eq!(substitute("{{a}}!", &u, &a), "USER!");
        assert_eq!(substitute("{{pane.x}}", &u, &a), "out");
        assert_eq!(substitute("{{missing}}", &u, &a), "{{missing}}");
        assert_eq!(substitute("no vars", &u, &a), "no vars");
        assert_eq!(substitute("{{bad name}}", &u, &a), "{{bad name}}");
        assert_eq!(substitute("{{unclosed", &u, &a), "{{unclosed");
    }

    #[test]
    fn display_value_never_object_object() {
        assert_eq!(display_value(&Value::Null), "");
        assert_eq!(display_value(&serde_json::json!({"x": 1})), r#"{"x":1}"#);
    }

    #[test]
    fn panes_and_names_from_auto_vars() {
        let pane = |id: &str, label: Option<&str>| {
            serde_json::json!({
                "pane_id": id, "workspace_id": "w1", "tab_id": "w1:t1", "label": label
            })
        };
        let auto: HashMap<String, Value> = [
            ("w1:p1".to_string(), pane("w1:p1", None)),
            ("w1:p2".to_string(), pane("w1:p2", Some("builder"))),
            ("reviewer".to_string(), pane("w1:p1", None)),
            ("greeting".to_string(), Value::String("hi".into())),
        ]
        .into();
        let ids: Vec<_> = auto_panes(&auto).into_iter().map(|p| p.pane_id).collect();
        assert_eq!(ids, ["w1:p2", "w1:p1"]);
        assert_eq!(
            pane_names(&auto),
            [("builder".to_string(), "w1:p2".to_string())]
        );
        assert_eq!(
            pane_text([&Some("a".into()), &None, &Some("b".into())]),
            "a · b"
        );
    }

    #[test]
    fn group_auto_splits_on_first_dot() {
        let entries = vec![
            ("b.y".to_string(), "2".to_string()),
            ("alias".to_string(), "0".to_string()),
            ("b.x".to_string(), "1".to_string()),
        ];
        let (grouped, ungrouped) = group_auto(&entries);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].0, "b");
        assert_eq!(ungrouped.len(), 1);
    }
}
