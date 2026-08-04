//! Shared colour palette + style helpers for the herdr-mcp TUI.
//!
//! Mirrors the emerald/neutral palette from the legacy web UI and the
//! `panel_block` / `detail_line` patterns borrowed from zerocode's
//! `theme.rs` (bordered panels + label/value rows).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Block;

/// Brand emerald accent (web UI green).
pub const EMBER: Color = Color::Rgb(16, 185, 129);
/// Dim border / secondary text.
pub const DIM: Color = Color::Rgb(115, 115, 115);
/// Muted neutral body text.
pub const MUTED: Color = Color::Rgb(163, 163, 163);
/// Amber for warnings.
pub const AMBER: Color = Color::Rgb(245, 158, 11);
/// Red for errors.
pub const RED: Color = Color::Rgb(239, 68, 68);
/// Selection background (slate).
pub const SELECT_BG: Color = Color::Rgb(30, 41, 59);

pub fn accent_style() -> Style {
    Style::default().fg(EMBER)
}

pub fn title_style() -> Style {
    Style::default().fg(EMBER).add_modifier(Modifier::BOLD)
}

pub fn dim_style() -> Style {
    Style::default().fg(DIM)
}

pub fn muted_style() -> Style {
    Style::default().fg(MUTED)
}

pub fn body_style() -> Style {
    Style::default()
}

pub fn warn_style() -> Style {
    Style::default().fg(AMBER)
}

pub fn error_style() -> Style {
    Style::default().fg(RED)
}

pub fn selected_style() -> Style {
    Style::default()
        .bg(SELECT_BG)
        .fg(EMBER)
        .add_modifier(Modifier::BOLD)
}

/// A bordered panel with a styled title (port of zerocode `theme::panel_block`).
///
/// Returns a `Block` with `Borders::ALL`; title is rendered bold emerald if
/// non-empty. Render the block over `area` then use `block.inner(area)` for
/// the content rectangle.
pub fn panel_block(title: &str) -> Block<'static> {
    let mut block = Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(dim_style());
    if !title.is_empty() {
        block = block.title(Span::styled(title.to_string(), title_style()));
    }
    block
}

/// A bordered panel with accent-coloured border to indicate focus.
///
/// Same structure as [`panel_block`] but uses [`accent_style()`] for the
/// border instead of [`dim_style()`].
pub fn focused_panel_block(title: &str) -> Block<'static> {
    let mut block = Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(accent_style());
    if !title.is_empty() {
        block = block.title(Span::styled(title.to_string(), title_style()));
    }
    block
}

/// A labelled value row (port of zerocode `dashboard.rs::detail_line`).
///
/// `label` is left-padded to `label_w` display cells and rendered in the dim
/// style; `value` is rendered in the body style.
pub fn detail_line(label: &str, value: &str, label_w: usize) -> Line<'static> {
    use unicode_width::UnicodeWidthStr;
    let pad = label_w.saturating_sub(label.width());
    Line::from(vec![
        Span::styled(format!("{}{}", label, " ".repeat(pad)), dim_style()),
        Span::styled(value.to_string(), body_style()),
    ])
}
