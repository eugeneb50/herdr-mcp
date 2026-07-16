//! Keybinding configuration for the TUI dashboard.
//!
//! Actions map to key combos via string format: `"ctrl-q"`, `"up"`, `"page_up"`, etc.
//! Configurable in the `[keybindings]` section of `herdr-mcp.toml`.

use crossterm::event::{KeyCode, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::fmt;

/// A parsed key combination (KeyCode + modifiers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCombo {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl KeyCombo {
    pub fn matches(self, code: KeyCode, mods: KeyModifiers) -> bool {
        self.code == code && self.mods == mods
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            parts.push("ctrl".into());
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            parts.push("shift".into());
        }
        if self.mods.contains(KeyModifiers::ALT) {
            parts.push("alt".into());
        }
        match self.code {
            KeyCode::Char(c) => parts.push(c.to_string()),
            KeyCode::Up => parts.push("up".into()),
            KeyCode::Down => parts.push("down".into()),
            KeyCode::Left => parts.push("left".into()),
            KeyCode::Right => parts.push("right".into()),
            KeyCode::Home => parts.push("home".into()),
            KeyCode::End => parts.push("end".into()),
            KeyCode::PageUp => parts.push("page_up".into()),
            KeyCode::PageDown => parts.push("page_down".into()),
            KeyCode::Tab => parts.push("tab".into()),
            KeyCode::Enter => parts.push("enter".into()),
            KeyCode::Esc => parts.push("esc".into()),
            KeyCode::BackTab => parts.push("backtab".into()),
            _ => parts.push("?".into()),
        }
        write!(f, "{}", parts.join("-"))
    }
}

/// Parse a keybinding string like `"ctrl-q"` into a `KeyCombo`.
///
/// Format: `[ctrl-][shift-][alt-]<key>`.
/// Special keys: `up`, `down`, `left`, `right`, `home`, `end`, `page_up`,
/// `page_down`, `tab`, `backtab`, `enter`, `esc`.
pub fn parse_keybinding(s: &str) -> Result<KeyCombo, String> {
    let mut mods = KeyModifiers::NONE;
    let mut remaining = s;

    // Parse modifier prefixes (case-insensitive).
    loop {
        let lower = remaining.to_lowercase();
        if lower.starts_with("ctrl-") {
            mods |= KeyModifiers::CONTROL;
            remaining = &remaining[5..];
        } else if lower.starts_with("shift-") {
            mods |= KeyModifiers::SHIFT;
            remaining = &remaining[6..];
        } else if lower.starts_with("alt-") {
            mods |= KeyModifiers::ALT;
            remaining = &remaining[4..];
        } else {
            break;
        }
    }

    let code = match remaining {
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "page_up" => KeyCode::PageUp,
        "page_down" => KeyCode::PageDown,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "enter" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "" => return Err(format!("empty key in binding: \"{s}\"")),
        c if c.len() == 1 => KeyCode::Char(c.chars().next().unwrap()),
        other => return Err(format!("unknown key: \"{other}\" in binding \"{s}\"")),
    };

    Ok(KeyCombo { code, mods })
}

/// TUI dashboard keybinding configuration.
///
/// All values are strings in `"ctrl-q"` format. Defaults are hardcoded;
/// users can override any binding in the `[keybindings]` section of
/// `herdr-mcp.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Keybindings {
    // Global
    pub quit: String,
    pub refresh: String,
    pub copy: String,
    pub paste: String,
    pub cut: String,

    // Tab switching
    pub tab_1: String,
    pub tab_2: String,
    pub tab_3: String,
    pub tab_4: String,
    pub tab_5: String,

    // Within-frame navigation
    pub nav_up: String,
    pub nav_down: String,
    pub nav_home: String,
    pub nav_end: String,
    pub nav_page_up: String,
    pub nav_page_down: String,

    // Between-frame navigation
    pub frame_next: String,
    pub frame_prev: String,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self {
            quit: "ctrl-q".into(),
            refresh: "ctrl-r".into(),
            copy: "ctrl-c".into(),
            paste: "ctrl-v".into(),
            cut: "ctrl-x".into(),
            tab_1: "ctrl-1".into(),
            tab_2: "ctrl-2".into(),
            tab_3: "ctrl-3".into(),
            tab_4: "ctrl-4".into(),
            tab_5: "ctrl-5".into(),
            nav_up: "up".into(),
            nav_down: "down".into(),
            nav_home: "home".into(),
            nav_end: "end".into(),
            nav_page_up: "page_up".into(),
            nav_page_down: "page_down".into(),
            frame_next: "tab".into(),
            frame_prev: "backtab".into(),
        }
    }
}

impl Keybindings {
    /// Resolve a keybinding string to a parsed `KeyCombo`.
    pub fn resolve(action: &str) -> Option<KeyCombo> {
        let s = match action {
            "quit" => &Self::default().quit,
            "refresh" => &Self::default().refresh,
            "copy" => &Self::default().copy,
            "paste" => &Self::default().paste,
            "cut" => &Self::default().cut,
            "tab_1" => &Self::default().tab_1,
            "tab_2" => &Self::default().tab_2,
            "tab_3" => &Self::default().tab_3,
            "tab_4" => &Self::default().tab_4,
            "tab_5" => &Self::default().tab_5,
            "nav_up" => &Self::default().nav_up,
            "nav_down" => &Self::default().nav_down,
            "nav_home" => &Self::default().nav_home,
            "nav_end" => &Self::default().nav_end,
            "nav_page_up" => &Self::default().nav_page_up,
            "nav_page_down" => &Self::default().nav_page_down,
            "frame_next" => &Self::default().frame_next,
            "frame_prev" => &Self::default().frame_prev,
            _ => return None,
        };
        parse_keybinding(s).ok()
    }

    /// Resolve a specific action from this (possibly customized) config.
    pub fn get(&self, action: &str) -> Option<KeyCombo> {
        let s = match action {
            "quit" => &self.quit,
            "refresh" => &self.refresh,
            "copy" => &self.copy,
            "paste" => &self.paste,
            "cut" => &self.cut,
            "tab_1" => &self.tab_1,
            "tab_2" => &self.tab_2,
            "tab_3" => &self.tab_3,
            "tab_4" => &self.tab_4,
            "tab_5" => &self.tab_5,
            "nav_up" => &self.nav_up,
            "nav_down" => &self.nav_down,
            "nav_home" => &self.nav_home,
            "nav_end" => &self.nav_end,
            "nav_page_up" => &self.nav_page_up,
            "nav_page_down" => &self.nav_page_down,
            "frame_next" => &self.frame_next,
            "frame_prev" => &self.frame_prev,
            _ => return None,
        };
        parse_keybinding(s).ok()
    }

    /// Check if a key event matches a named action.
    pub fn matches(&self, action: &str, code: KeyCode, mods: KeyModifiers) -> bool {
        self.get(action)
            .map(|kc| kc.matches(code, mods))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ctrl_char() {
        let kc = parse_keybinding("ctrl-q").unwrap();
        assert_eq!(kc.code, KeyCode::Char('q'));
        assert!(kc.mods.contains(KeyModifiers::CONTROL));
    }

    #[test]
    fn parse_plain_key() {
        let kc = parse_keybinding("up").unwrap();
        assert_eq!(kc.code, KeyCode::Up);
        assert_eq!(kc.mods, KeyModifiers::NONE);
    }

    #[test]
    fn parse_shift_tab() {
        let kc = parse_keybinding("backtab").unwrap();
        assert_eq!(kc.code, KeyCode::BackTab);
    }

    #[test]
    fn parse_ctrl_digit() {
        let kc = parse_keybinding("ctrl-3").unwrap();
        assert_eq!(kc.code, KeyCode::Char('3'));
        assert!(kc.mods.contains(KeyModifiers::CONTROL));
    }

    #[test]
    fn parse_page_up() {
        let kc = parse_keybinding("page_up").unwrap();
        assert_eq!(kc.code, KeyCode::PageUp);
    }

    #[test]
    fn parse_empty_fails() {
        assert!(parse_keybinding("").is_err());
    }

    #[test]
    fn parse_unknown_key_fails() {
        assert!(parse_keybinding("f12").is_err());
    }

    #[test]
    fn default_keybindings_valid() {
        let kb = Keybindings::default();
        // Every default binding should parse successfully.
        assert!(kb.get("quit").is_some());
        assert!(kb.get("refresh").is_some());
        assert!(kb.get("copy").is_some());
        assert!(kb.get("paste").is_some());
        assert!(kb.get("cut").is_some());
        assert!(kb.get("nav_up").is_some());
        assert!(kb.get("nav_down").is_some());
        assert!(kb.get("frame_next").is_some());
        assert!(kb.get("frame_prev").is_some());
    }

    #[test]
    fn matches_works() {
        let kb = Keybindings::default();
        assert!(kb.matches("quit", KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert!(!kb.matches("quit", KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(kb.matches("nav_up", KeyCode::Up, KeyModifiers::NONE));
    }

    #[test]
    fn keycombo_display() {
        let kc = KeyCombo {
            code: KeyCode::Char('q'),
            mods: KeyModifiers::CONTROL,
        };
        assert_eq!(format!("{kc}"), "ctrl-q");

        let kc2 = KeyCombo {
            code: KeyCode::Up,
            mods: KeyModifiers::NONE,
        };
        assert_eq!(format!("{kc2}"), "up");
    }
}
