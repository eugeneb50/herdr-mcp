use serde_json::Value;

pub(super) fn item_id(item: &Value) -> Option<&str> {
    item["id"].as_str().filter(|id| !id.is_empty())
}

pub(super) fn selected_row(rows: &[Value], selected: Option<&str>) -> Option<usize> {
    let selected = selected?;
    rows.iter().position(|row| item_id(row) == Some(selected))
}

/// Whether the item can be cancelled and whether it can be retried.
pub(super) fn actions(status: &str) -> (bool, bool) {
    (
        matches!(status, "queued" | "running"),
        matches!(status, "dead" | "cancelled"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_only_nonempty_string_ids() {
        assert_eq!(item_id(&json!({"id": "item-1"})), Some("item-1"));
        for item in [json!({}), json!({"id": ""}), json!({"id": 42})] {
            assert_eq!(item_id(&item), None);
        }
    }

    #[test]
    fn selected_row_follows_id_across_refresh() {
        let a = json!({"id": "a"});
        let b = json!({"id": "b"});
        assert_eq!(selected_row(&[a.clone(), b.clone()], Some("a")), Some(0));
        assert_eq!(selected_row(&[b.clone(), a], Some("a")), Some(1));
        assert_eq!(selected_row(&[b], Some("a")), None);
    }

    #[test]
    fn actions_follow_queue_status() {
        for status in ["queued", "running"] {
            assert_eq!(actions(status), (true, false));
        }
        for status in ["dead", "cancelled"] {
            assert_eq!(actions(status), (false, true));
        }
        for status in ["done", "unknown"] {
            assert_eq!(actions(status), (false, false));
        }
    }
}
