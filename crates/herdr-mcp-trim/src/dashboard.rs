//! Live ANSI dashboard of trim savings across all workspaces.
//!
//! `herdr-mcp trim dashboard` opens a full-screen TUI that scans
//! `data_dir/sessions/*.trim_stats.json` every 2s and renders net savings %,
//! per-pane bars, and the active-policy table. Quit with `q` or `Ctrl-C`.

use std::io::{self, Write};
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};

use crate::stats;

/// Bar widths and palette (24-bit ANSI).
const BAR_WIDTH: usize = 32;

/// Run the live dashboard. Blocks until the user quits.
pub async fn run(data_dir: &Path) -> Result<()> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, Hide)?;

    // Restore terminal on any exit path (normal return or early `?`).
    let restore = RestoreTerm;
    let _ = &restore;

    loop {
        render(&mut out, data_dir).await?;

        // Wait up to 2s, or until a quit key is pressed.
        let start = Instant::now();
        let timeout = std::time::Duration::from_secs(2);
        let mut quit = false;
        while start.elapsed() < timeout {
            if event::poll(std::time::Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    match key.code {
                        KeyCode::Char('q') => {
                            quit = true;
                            break;
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            quit = true;
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
        if quit {
            break;
        }
    }

    drop(restore);
    Ok(())
}

/// RAII guard that restores the terminal to its normal state.
struct RestoreTerm;

impl Drop for RestoreTerm {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = disable_raw_mode();
        let _ = execute!(out, LeaveAlternateScreen, Show);
    }
}

/// Scan all workspaces and render a single frame.
async fn render(out: &mut io::Stdout, data_dir: &Path) -> Result<()> {
    let sessions = data_dir.join("sessions");
    let mut workspaces: Vec<String> = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(&sessions).await {
        while let Ok(Some(e)) = entries.next_entry().await {
            let fname = e.file_name().to_string_lossy().to_string();
            if fname.ends_with(".trim_stats.json") {
                workspaces.push(fname.trim_end_matches(".trim_stats.json").to_string());
            }
        }
    }
    workspaces.sort();

    let mut frame = String::new();
    frame.push_str(&format!("\x1b[2J\x1b[H")); // clear + home
    frame.push_str("\x1b[1;36mherdr-mcp Trim Dashboard\x1b[0m  (q to quit)\r\n");
    frame.push_str("\x1b[90m─────────────────────────────────────────────────────────\x1b[0m\r\n");

    if workspaces.is_empty() {
        frame.push_str("\x1b[90mno trim stats yet — send a trimmed message or run `herdr-mcp trim`\x1b[0m\r\n");
    }

    for ws in &workspaces {
        let s = stats::load_stats(data_dir, ws).await;
        let net_pct = s.savings_pct();
        let badge = if net_pct > 0.0 {
            format!("\x1b[32m-{net_pct:.1}%\x1b[0m")
        } else {
            "\x1b[90m—\x1b[0m".to_string()
        };
        frame.push_str(&format!(
            "\x1b[1m{ws}\x1b[0m  net {badge}  \x1b[90mgross {}B  net {}B  {} msgs\x1b[0m\r\n",
            s.gross_saved_bytes, s.net_saved_bytes, s.messages_trimmed
        ));

        // Per-pane bars.
        let mut panes: Vec<_> = s.per_pane.iter().collect();
        panes.sort_by(|a, b| b.1.net_saved_bytes.cmp(&a.1.net_saved_bytes));
        let max_net = panes
            .first()
            .map(|(_, p)| p.net_saved_bytes.max(1))
            .unwrap_or(1);
        for (pane, p) in &panes {
            let filled = (p.net_saved_bytes * BAR_WIDTH as usize) / max_net;
            let bar = "█".repeat(filled) + &"░".repeat(BAR_WIDTH - filled);
            frame.push_str(&format!(
                "  \x1b[90m{pane:<14}\x1b[0m {bar} \x1b[90m{}B\x1b[0m\r\n",
                p.net_saved_bytes
            ));
        }
        frame.push_str("\r\n");
    }

    write!(out, "{frame}")?;
    out.flush()?;
    Ok(())
}
