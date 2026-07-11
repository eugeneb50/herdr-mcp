//! Rendering helpers for the herdr-mcp TUI.
//!
//! All ANSI escape construction lives here so tab renderers can stay small.
//! Colors mirror the Tailwind neutral/emerald palette from the legacy web UI.

use std::fmt::Write;

/// ANSI reset.
pub const RESET: &str = "\x1b[0m";

/// 24-bit foreground RGB.
pub fn fg(r: u8, g: u8, b: u8) -> String {
    format!("\x1b[38;2;{r};{g};{b}m")
}

/// 24-bit background RGB.
pub fn bg(r: u8, g: u8, b: u8) -> String {
    format!("\x1b[48;2;{r};{g};{b}m")
}

/// Bold.
pub fn bold(s: &str) -> String {
    format!("\x1b[1m{s}{RESET}")
}

/// Dim text (neutral 500-ish).
pub fn dim(s: &str) -> String {
    format!("\x1b[2;38;2;115;115;115m{s}{RESET}")
}

/// Emerald accent (brand green from the web UI).
pub fn emerald(s: &str) -> String {
    format!("\x1b[38;2;16;185;129m{s}{RESET}")
}

/// Muted neutral text.
pub fn muted(s: &str) -> String {
    format!("\x1b[38;2;163;163;163m{s}{RESET}")
}

/// Amber for warnings.
pub fn amber(s: &str) -> String {
    format!("\x1b[38;2;245;158;11m{s}{RESET}")
}

/// Red for errors.
pub fn red(s: &str) -> String {
    format!("\x1b[38;2;239;68;68m{s}{RESET}")
}

/// Move cursor to `col`, `row` (1-based).
pub fn move_to(col: u16, row: u16) -> String {
    format!("\x1b[{row};{col}H")
}

/// Horizontal bar of a given fraction with block characters.
pub fn bar(value: usize, max: usize, width: usize) -> String {
    let max = max.max(1);
    let filled = if value == 0 {
        0
    } else {
        ((value as f64 / max as f64) * width as f64).round() as usize
    }
    .min(width);
    let bar = "█".repeat(filled) + &"░".repeat(width - filled);
    bar
}

/// Draw a titled box border with the given inner width.
pub fn box_border(out: &mut String, w: usize, title: Option<&str>) {
    let top = match title {
        Some(t) => format!("┌─ {t} "),
        None => "┌".to_string(),
    };
    let pad = w.saturating_sub(top.chars().count() + 1);
    let _ = write!(out, "{top}{}", "─".repeat(pad));
    let _ = write!(out, "┐\r\n");
}

/// Draw the bottom border of a box.
pub fn box_bottom(out: &mut String, w: usize, footer: Option<&str>) {
    let bot = match footer {
        Some(t) => format!("└─ {t} "),
        None => "└".to_string(),
    };
    let pad = w.saturating_sub(bot.chars().count() + 1);
    let _ = write!(out, "{bot}{}", "─".repeat(pad));
    let _ = write!(out, "┘\r\n");
}

/// Truncate a string to `max` display cells (accounting for char count, not grapheme width).
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t = String::new();
    for (i, c) in s.chars().enumerate() {
        if i + 1 >= max {
            t.push('…');
            break;
        }
        t.push(c);
    }
    t
}

/// Pretty-print bytes like the web UI (K/M suffixes).
pub fn fmt_bytes(n: usize) -> String {
    if n >= 1_048_576 {
        format!("{:.1}M", n as f64 / 1_048_576.0)
    } else if n >= 1024 {
        format!("{:.1}K", n as f64 / 1024.0)
    } else {
        n.to_string()
    }
}
