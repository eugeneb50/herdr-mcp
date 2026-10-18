//! Pane dropdowns for any `pane_id` / `label` field: two `<datalist>`s built
//! from the auto variables (`GET /api/variables/auto`), which a text input
//! joins with `list: pane_list(name)`.

use dioxus::prelude::*;
use serde_json::Value;
use std::collections::HashMap;

use crate::client::use_api;
use crate::vars::{auto_panes, pane_names, pane_text};

const PANE_IDS: &str = "pane-ids";
const PANE_LABELS: &str = "pane-labels";

/// The datalist a field named `name` suggests from (`""`: none).
pub fn pane_list(name: &str) -> &'static str {
    if name == "label" {
        PANE_LABELS
    } else if name.contains("pane") {
        PANE_IDS
    } else {
        ""
    }
}

/// Both datalists from already-fetched auto variables.
#[component]
pub fn PaneOptions(auto: HashMap<String, Value>) -> Element {
    rsx! {
        datalist { id: PANE_IDS,
            for p in auto_panes(&auto) {
                option { value: "{p.pane_id}", "{pane_text([&p.label, &p.agent, &p.cwd])}" }
            }
        }
        datalist { id: PANE_LABELS,
            for (name, id) in pane_names(&auto) {
                option { value: "{name}", "{id}" }
            }
        }
    }
}

/// [`PaneOptions`] fetching the auto variables itself (suggestions are
/// optional: none if the call fails).
#[component]
pub fn LivePaneOptions() -> Element {
    let client = use_api();
    let auto = use_resource(move || {
        let client = client();
        async move { client.auto_variables(None).await.unwrap_or_default() }
    });
    let vars = auto
        .read()
        .as_ref()
        .map(|a| a.variables.clone())
        .unwrap_or_default();
    rsx! { PaneOptions { auto: vars } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_map_to_lists() {
        assert_eq!(pane_list("pane_id"), PANE_IDS);
        assert_eq!(pane_list("source_pane"), PANE_IDS);
        assert_eq!(pane_list("label"), PANE_LABELS);
        assert_eq!(pane_list("text"), "");
    }
}
