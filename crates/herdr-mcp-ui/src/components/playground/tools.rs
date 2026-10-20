//! The tool catalog (`GET /api/tools`) as the playground's pickers and forms
//! use it.

use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Clone, PartialEq)]
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
    let mut groups: std::collections::BTreeMap<String, Vec<ToolInfo>> = Default::default();
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
