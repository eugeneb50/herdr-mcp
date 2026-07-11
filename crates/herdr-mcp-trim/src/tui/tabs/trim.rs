//! Trim tab: savings dashboard, per-pane bars, active policies, diagnose.

use std::fmt::Write;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use serde_json::Value;

use crate::tui::App;
use crate::tui::render::{amber, bar, bold, emerald, fmt_bytes, move_to, muted, truncate};

/// Returns `true` if the key was consumed.
pub async fn handle_key(app: &mut App, code: KeyCode, _mods: KeyModifiers) -> Result<bool> {
    match code {
        KeyCode::Char('d') => {
            if let Some(http) = app.http.clone() {
                match http.trim_diagnose().await {
                    Ok(v) => {
                        app.trim.diagnose = Some(v);
                        app.status_msg = "diagnose ok".into();
                    }
                    Err(e) => {
                        app.status_msg = format!("diagnose failed: {e}");
                    }
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub fn render(f: &mut String, app: &App) {
    let row = 4u16;
    let _ = write!(f, "{}", move_to(2, row));
    let _ = write!(f, "{}   {} (d=diagnose)\r\n", bold("Trim Dashboard"), muted("message-trim savings"));

    let trim = app.trim_status.as_ref();
    if let Some(t) = trim {
        let savings = t
            .get("workspace_savings_pct")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let net = t.get("net_saved_bytes").and_then(|v| v.as_u64());
        let gross = t.get("gross_saved_bytes").and_then(|v| v.as_u64());
        let total = t.get("total_input_bytes").and_then(|v| v.as_u64());
        let msgs = t.get("messages_trimmed").and_then(|v| v.as_u64());

        let _ = write!(f, "{}", move_to(2, row + 2));
        let _ = write!(
            f,
            "  savings {}  net {}  gross {}  input {}  {} msgs\r\n",
            emerald(&format!("{}%", savings.round() as i64)),
            muted(&net.map(|n| fmt_bytes(n as usize)).unwrap_or_default()),
            muted(&gross.map(|n| fmt_bytes(n as usize)).unwrap_or_default()),
            muted(&total.map(|n| fmt_bytes(n as usize)).unwrap_or_default()),
            muted(&msgs.map(|n| n.to_string()).unwrap_or_default()),
        );

        // per-pane bars
        let _ = write!(f, "{}", move_to(2, row + 4));
        let _ = write!(f, "{}\r\n", bold("Per-pane"));
        if let Some(panes) = t.get("per_pane").and_then(|p| p.as_object()) {
            let mut entries: Vec<_> = panes
                .iter()
                .map(|(k, v)| {
                    let net = v
                        .get("net_saved_bytes")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0) as usize;
                    (k.clone(), net)
                })
                .collect();
            entries.sort_by(|a, b| b.1.cmp(&a.1));
            let max = entries.first().map(|(_, n)| *n).unwrap_or(1).max(1);
            for (i, (pane, net)) in entries.iter().take(12).enumerate() {
                let _ = write!(f, "{}", move_to(2, row + 5 + i as u16));
                let _ = write!(
                    f,
                    "  {} {}\r\n",
                    muted(&truncate(pane, 14)),
                    &format!("{} {}", bar(*net, max, 28), emerald(&fmt_bytes(*net))),
                );
            }
        }

        // active policies
        let pol_row = row + 18;
        let _ = write!(f, "{}", move_to(2, pol_row));
        let _ = write!(f, "{}\r\n", bold("Active policies"));
        if let Some(policies) = t.get("active_policies").and_then(|p| p.as_object()) {
            if policies.is_empty() {
                let _ = write!(f, "{}", move_to(2, pol_row + 1));
                let _ = write!(f, "{}\r\n", muted("(none)"));
            } else {
                for (i, (pane, stages)) in policies.iter().take(8).enumerate() {
                    let _ = write!(f, "{}", move_to(2, pol_row + 1 + i as u16));
                    let stage_list = match stages {
                        Value::Array(a) => a
                            .iter()
                            .filter_map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(","),
                        _ => String::new(),
                    };
                    let _ = write!(
                        f,
                        "  {} {}\r\n",
                        muted(&truncate(pane, 14)),
                        emerald(&truncate(&stage_list, 50)),
                    );
                }
            }
        }
    } else {
        // File fallback from the legacy stats reader.
        let _ = write!(f, "{}", move_to(2, row + 2));
        if let Some(s) = &app.trim_stats_file {
            let pct = s.savings_pct();
            let _ = write!(
                f,
                "  file stats: savings {}  net {}  gross {}  {} msgs\r\n",
                emerald(&format!("{}%", pct.round() as i64)),
                muted(&fmt_bytes(s.net_saved_bytes)),
                muted(&fmt_bytes(s.gross_saved_bytes)),
                muted(&s.messages_trimmed.to_string()),
            );
            let mut panes: Vec<_> = s.per_pane.iter().collect();
            panes.sort_by_key(|(_, p)| std::cmp::Reverse(p.net_saved_bytes));
            let max = panes
                .first()
                .map(|(_, p)| p.net_saved_bytes.max(1))
                .unwrap_or(1);
            for (i, (pane, p)) in panes.iter().take(12).enumerate() {
                let _ = write!(f, "{}", move_to(2, row + 4 + i as u16));
                let _ = write!(
                    f,
                    "  {} {}\r\n",
                    muted(&truncate(pane, 14)),
                    &format!(
                        "{} {}",
                        bar(p.net_saved_bytes, max, 28),
                        emerald(&fmt_bytes(p.net_saved_bytes))
                    ),
                );
            }
        } else {
            let _ = write!(f, "{}\r\n", muted("No trim stats yet. Run `herdr-mcp trim` or send a trimmed message."));
        }
    }

    // diagnose result
    let d_row = row + 20;
    if let Some(d) = &app.trim.diagnose {
        let _ = write!(f, "{}", move_to(2, d_row));
        let _ = write!(f, "{}\r\n", bold("Diagnose"));
        let pretty = serde_json::to_string_pretty(d).unwrap_or_else(|_| d.to_string());
        for (i, line) in pretty.lines().take(8).enumerate() {
            let _ = write!(f, "{}", move_to(2, d_row + 1 + i as u16));
            let _ = write!(f, "{}\r\n", amber(&truncate(line, 100)));
        }
    }
}
