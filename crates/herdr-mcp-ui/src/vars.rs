//! Client-side variable helpers: `{{name}}` substitution (preview only —
//! actual resolution happens server-side), display formatting, auto-var
//! grouping. Port of `VariableStore.tsx` semantics + `api/variables.ts`.

use serde_json::Value;
use std::collections::HashMap;

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
