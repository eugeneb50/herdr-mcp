//! Tunable settings: keybindings, timeouts, limits and intervals.
//!
//! [`SPECS`] is the single source of truth: every knob's key, default,
//! bounds and help text. The bridge stores overrides in the config file's
//! `[settings.<section>]` tables and serves them at `/api/settings`; the TUI
//! and web Settings pages render and edit them from the same table. Any
//! override that is missing, out of range, unparseable or conflicting falls
//! back to the built-in default, the known-good configuration.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// What a setting holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// Milliseconds.
    Millis,
    /// A plain count (lines, attempts).
    Count,
    /// Key chords bound to a TUI action, e.g. `["alt+right", "ctrl+tab"]`.
    Keys,
}

/// A setting's value: a number for [`Kind::Millis`] / [`Kind::Count`], key
/// chords for [`Kind::Keys`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Number(u64),
    Keys(Vec<String>),
}

impl Value {
    /// A value as typed in a Settings field: a whole number (`120_000`
    /// works), or comma-separated key chords. Only the shape is checked
    /// here; [`Settings::set`] checks the rest.
    pub fn parse(kind: Kind, text: &str) -> Result<Self, String> {
        match kind {
            Kind::Millis | Kind::Count => text
                .trim()
                .replace('_', "")
                .parse::<u64>()
                .map(Self::Number)
                .map_err(|_| format!("{:?} is not a whole number", text.trim())),
            Kind::Keys => Ok(Self::Keys(
                text.split(',')
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                    .map(str::to_string)
                    .collect(),
            )),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{n}"),
            Self::Keys(k) => f.write_str(&k.join(", ")),
        }
    }
}

/// A built-in default (`const`-friendly form of [`Value`]).
#[derive(Debug, Clone, Copy)]
pub enum DefaultValue {
    Number(u64),
    Keys(&'static [&'static str]),
}

impl DefaultValue {
    fn value(self) -> Value {
        match self {
            Self::Number(n) => Value::Number(n),
            Self::Keys(k) => Value::Keys(k.iter().map(|s| (*s).to_string()).collect()),
        }
    }
}

/// One setting: `key` is `<section>.<name>`, as stored under
/// `[settings.<section>]` in the config file.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    pub default: DefaultValue,
    /// Inclusive bounds for numbers.
    pub min: u64,
    pub max: u64,
    /// Read once at startup: a change applies after a restart.
    pub restart: bool,
}

impl Spec {
    /// The `[settings.<section>]` table this lives in.
    pub fn section(&self) -> &'static str {
        self.key.split_once('.').map_or(self.key, |(s, _)| s)
    }
}

const fn ms(
    key: &'static str,
    label: &'static str,
    help: &'static str,
    d: u64,
    min: u64,
    max: u64,
) -> Spec {
    Spec {
        key,
        label,
        help,
        kind: Kind::Millis,
        default: DefaultValue::Number(d),
        min,
        max,
        restart: false,
    }
}

const fn count(
    key: &'static str,
    label: &'static str,
    help: &'static str,
    d: u64,
    min: u64,
    max: u64,
) -> Spec {
    Spec {
        kind: Kind::Count,
        ..ms(key, label, help, d, min, max)
    }
}

const fn keys(
    key: &'static str,
    label: &'static str,
    help: &'static str,
    d: &'static [&'static str],
) -> Spec {
    Spec {
        key,
        label,
        help,
        kind: Kind::Keys,
        default: DefaultValue::Keys(d),
        min: 0,
        max: 0,
        restart: false,
    }
}

const fn restart(mut s: Spec) -> Spec {
    s.restart = true;
    s
}

pub const PANE_EXEC_MS: &str = "timeouts.pane-exec-ms";
pub const HERDR_API_MS: &str = "timeouts.herdr-api-ms";
pub const AGENT_WAIT_MS: &str = "timeouts.agent-wait-ms";
pub const AGENT_PROMPT_MS: &str = "timeouts.agent-prompt-ms";
pub const AGENT_START_MS: &str = "timeouts.agent-start-ms";
pub const TUI_REQUEST_MS: &str = "timeouts.tui-request-ms";
pub const PANE_EXEC_LINES: &str = "limits.pane-exec-lines";
pub const REPLY_FALLBACK_LINES: &str = "limits.reply-fallback-lines";
pub const WATCHER_GIVE_UP: &str = "limits.watcher-give-up";
pub const RECONNECT_ATTEMPTS: &str = "limits.reconnect-attempts";
pub const WATCHER_POLL_MS: &str = "intervals.watcher-poll-ms";
pub const RECONNECT_BACKOFF_MS: &str = "intervals.reconnect-backoff-ms";
pub const TUI_REFRESH_MS: &str = "intervals.tui-refresh-ms";
pub const TUI_PANE_MS: &str = "intervals.tui-pane-ms";

/// Every setting, in display order.
pub const SPECS: &[Spec] = &[
    ms(
        PANE_EXEC_MS,
        "pane_exec timeout",
        "How long pane_exec waits for its command when the call passes no timeout_ms.",
        120_000,
        1_000,
        3_600_000,
    ),
    ms(
        HERDR_API_MS,
        "herdr_api timeout",
        "How long herdr_api waits for herdr's answer when the call passes no timeout_ms.",
        60_000,
        1_000,
        600_000,
    ),
    ms(
        AGENT_WAIT_MS,
        "agent_spawn waits",
        "How long agent_spawn waits for each `needs` agent, and (wait_idle) for the new agent, to go idle.",
        300_000,
        1_000,
        3_600_000,
    ),
    ms(
        AGENT_PROMPT_MS,
        "agent_prompt timeout",
        "How long agent_prompt waits for the reply when the call passes no timeout_ms.",
        300_000,
        1_000,
        3_600_000,
    ),
    ms(
        AGENT_START_MS,
        "agent start timeout",
        "How long herdr may take to report a spawned native agent ready.",
        60_000,
        1_000,
        600_000,
    ),
    ms(
        TUI_REQUEST_MS,
        "TUI request timeout",
        "Per-request timeout of the TUI's bridge calls (tool calls, saves). Applies on the TUI's next connect.",
        30_000,
        1_000,
        600_000,
    ),
    count(
        PANE_EXEC_LINES,
        "pane_exec scrollback",
        "Scrollback lines pane_exec reads back to find its output, when the call passes no `lines`.",
        500,
        10,
        100_000,
    ),
    count(
        REPLY_FALLBACK_LINES,
        "reply fallback lines",
        "Lines agent_prompt keeps as the reply when it can't find where the prompt was echoed.",
        40,
        1,
        10_000,
    ),
    count(
        WATCHER_GIVE_UP,
        "watcher error limit",
        "Consecutive failed pane reads after which a command agent's watcher assumes the pane is gone and stops.",
        5,
        1,
        1_000,
    ),
    restart(count(
        RECONNECT_ATTEMPTS,
        "herdr reconnect warnings",
        "Failed herdr event-socket reconnects logged as warnings before retrying quietly (it never gives up).",
        3,
        0,
        1_000,
    )),
    ms(
        WATCHER_POLL_MS,
        "watcher poll interval",
        "How often a command agent's watcher reads its pane to decide working / idle.",
        500,
        50,
        60_000,
    ),
    restart(ms(
        RECONNECT_BACKOFF_MS,
        "herdr reconnect backoff",
        "Delay between herdr event-socket reconnect attempts.",
        500,
        50,
        60_000,
    )),
    ms(
        TUI_REFRESH_MS,
        "TUI refresh interval",
        "How often the TUI re-reads tools, variables and recipes from the bridge.",
        3_000,
        500,
        600_000,
    ),
    ms(
        TUI_PANE_MS,
        "TUI pane view interval",
        "How often the TUI's Overview re-reads the picked pane's text.",
        1_000,
        250,
        600_000,
    ),
    keys(
        "keys.tab-next",
        "next tab",
        "TUI: go to the next tab (wraps).",
        &["alt+right", "ctrl+tab"],
    ),
    keys(
        "keys.tab-prev",
        "previous tab",
        "TUI: go to the previous tab (wraps).",
        &["alt+left", "ctrl+shift+tab"],
    ),
    keys(
        "keys.tab-1",
        "Send tab",
        "TUI: jump to Send.",
        &["alt+1", "ctrl+1"],
    ),
    // Legacy terminals send Ctrl+2 as NUL, which reads as Ctrl+Space.
    keys(
        "keys.tab-2",
        "Overview tab",
        "TUI: jump to Overview.",
        &["alt+2", "ctrl+2", "ctrl+space"],
    ),
    keys(
        "keys.tab-3",
        "Playground tab",
        "TUI: jump to Playground.",
        &["alt+3", "ctrl+3"],
    ),
    keys(
        "keys.tab-4",
        "Variables tab",
        "TUI: jump to Variables.",
        &["alt+4", "ctrl+4"],
    ),
    keys(
        "keys.tab-5",
        "Settings tab",
        "TUI: jump to Settings.",
        &["alt+5", "ctrl+5"],
    ),
    keys(
        "keys.refresh",
        "refresh",
        "TUI: re-read everything from the bridge now.",
        &["ctrl+r"],
    ),
    keys("keys.quit", "quit", "TUI: quit.", &["ctrl+q"]),
];

/// Chords the TUI's tabs and editors use themselves (Playground run/save,
/// clear, sub-tabs, Variables edit/delete, copy/cut/paste…). Binding an
/// action to one would break the tab or be shadowed by it, so they are
/// refused, except as the action's own default (`ctrl+r` for refresh).
pub const RESERVED: &[&str] = &[
    "ctrl+b",
    "ctrl+c",
    "ctrl+d",
    "ctrl+e",
    "ctrl+j",
    "ctrl+k",
    "ctrl+l",
    "ctrl+n",
    "ctrl+o",
    "ctrl+r",
    "ctrl+s",
    "ctrl+t",
    "ctrl+v",
    "ctrl+w",
    "ctrl+x",
    "ctrl+enter",
    "ctrl+shift+r",
];

/// The spec for `key`.
pub fn spec(key: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.key == key)
}

/// A key the TUI can bind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A printable character, lowercase for letters (`shift` carries case).
    Char(char),
    F(u8),
    Tab,
    Enter,
    Esc,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
}

const NAMED: &[(&str, Key)] = &[
    ("tab", Key::Tab),
    ("enter", Key::Enter),
    ("esc", Key::Esc),
    ("backspace", Key::Backspace),
    ("delete", Key::Delete),
    ("insert", Key::Insert),
    ("home", Key::Home),
    ("end", Key::End),
    ("pageup", Key::PageUp),
    ("pagedown", Key::PageDown),
    ("up", Key::Up),
    ("down", Key::Down),
    ("left", Key::Left),
    ("right", Key::Right),
    ("space", Key::Char(' ')),
];

/// A key plus modifiers, e.g. `ctrl+shift+tab`. Written lowercase,
/// modifiers first in the order ctrl, alt, shift ([`fmt::Display`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: Key,
}

impl Chord {
    /// Parse `ctrl+shift+tab`, `Alt+Right`, `f5`, `backtab` (= shift+tab).
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim().to_ascii_lowercase();
        let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let Some(name) = parts.pop().filter(|n| !n.is_empty()) else {
            return Err(format!("{text:?}: no key"));
        };
        let mut chord = Chord {
            ctrl: false,
            alt: false,
            shift: false,
            key: Key::Tab,
        };
        for m in parts {
            match m {
                "ctrl" | "control" => chord.ctrl = true,
                "alt" | "meta" | "option" => chord.alt = true,
                "shift" => chord.shift = true,
                other => return Err(format!("{text:?}: unknown modifier {other:?}")),
            }
        }
        chord.key = if name == "backtab" {
            chord.shift = true;
            Key::Tab
        } else if let Some((_, k)) = NAMED.iter().find(|(n, _)| *n == name) {
            *k
        } else if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
            if !(1..=12).contains(&n) {
                return Err(format!("{text:?}: function keys are f1-f12"));
            }
            Key::F(n)
        } else {
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_graphic() => Key::Char(c),
                _ => return Err(format!("{text:?}: unknown key {name:?}")),
            }
        };
        Ok(chord)
    }

    /// Would pressing this type text or edit a field? Only chords with Ctrl
    /// or Alt, and the function keys, are safe as global shortcuts.
    pub fn types_text(&self) -> bool {
        !(self.ctrl || self.alt || matches!(self.key, Key::F(_)))
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, m) in [
            (self.ctrl, "ctrl+"),
            (self.alt, "alt+"),
            (self.shift, "shift+"),
        ] {
            if on {
                f.write_str(m)?;
            }
        }
        match self.key {
            Key::Char(' ') => f.write_str("space"),
            Key::Char(c) => write!(f, "{c}"),
            Key::F(n) => write!(f, "f{n}"),
            k => {
                let name = NAMED.iter().find(|(_, v)| *v == k).map_or("?", |(n, _)| n);
                f.write_str(name)
            }
        }
    }
}

/// The overrides in effect; anything not here is its [`Spec::default`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct Settings(BTreeMap<String, Value>);

impl Settings {
    /// Overrides from the config file (`[settings.<section>] name = …`),
    /// keeping only valid ones. Each entry dropped (and so back on its
    /// default) is described in the returned warnings.
    pub fn from_file(
        tables: &BTreeMap<String, BTreeMap<String, serde_json::Value>>,
    ) -> (Self, Vec<String>) {
        let mut out = Self::default();
        let mut warnings = Vec::new();
        let mut drop = |key: &str, e: String| {
            warnings.push(format!("settings: {key}: {e}; using the default"));
        };
        for (section, entries) in tables {
            for (name, raw) in entries {
                let key = format!("{section}.{name}");
                match serde_json::from_value::<Value>(raw.clone()) {
                    Ok(v) => {
                        out.0.insert(key, v);
                    }
                    Err(_) => drop(
                        &key,
                        format!("expected a number or a list of keys, got {raw}"),
                    ),
                }
            }
        }
        // Check each override against all the others, not just the ones
        // before it, so a rebinding the API accepted loads whatever order
        // the keys sort in. Dropping one can free or retake a chord, so
        // recheck until every one passes.
        loop {
            let checked: Result<BTreeMap<String, Value>, (String, String)> = out
                .0
                .iter()
                .map(|(k, v)| match out.check(k, v.clone()) {
                    Ok(v) => Ok((k.clone(), v)),
                    Err(e) => Err((k.clone(), e)),
                })
                .collect();
            match checked {
                Ok(all) => {
                    out.0 = all;
                    break;
                }
                Err((key, e)) => {
                    out.0.remove(&key);
                    drop(&key, e);
                }
            }
        }
        (out, warnings)
    }

    /// Validate `value` for `key` against the spec and the other bindings
    /// in effect, and store it (normalized: chords in canonical form).
    pub fn set(&mut self, key: &str, value: Value) -> Result<(), String> {
        let value = self.check(key, value)?;
        self.0.insert(key.to_string(), value);
        Ok(())
    }

    /// Drop `key`'s override: back to its default. `false` if unknown.
    pub fn reset(&mut self, key: &str) -> bool {
        self.0.remove(key);
        spec(key).is_some()
    }

    /// Is `key` overridden?
    pub fn is_set(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    /// `key`'s value in effect (its override, else its default).
    pub fn value(&self, key: &str) -> Option<Value> {
        let spec = spec(key)?;
        Some(
            self.0
                .get(key)
                .cloned()
                .unwrap_or_else(|| spec.default.value()),
        )
    }

    /// A number setting in effect; 0 for an unknown key (a test pins every
    /// key constant to a spec).
    pub fn number(&self, key: &str) -> u64 {
        match self.value(key) {
            Some(Value::Number(n)) => n,
            _ => 0,
        }
    }

    /// A millisecond setting as a [`std::time::Duration`].
    pub fn duration(&self, key: &str) -> std::time::Duration {
        std::time::Duration::from_millis(self.number(key))
    }

    /// The `keys.*` action `chord` is bound to, if any (e.g. `"tab-next"`).
    pub fn action_for(&self, chord: &Chord) -> Option<&'static str> {
        SPECS
            .iter()
            .filter(|s| s.kind == Kind::Keys)
            .find(|s| self.chords(s.key).contains(chord))
            .map(|s| s.key.trim_start_matches("keys."))
    }

    /// The chords bound to `key` (parsed; stored ones are valid).
    pub fn chords(&self, key: &str) -> Vec<Chord> {
        match self.value(key) {
            Some(Value::Keys(k)) => k.iter().filter_map(|c| Chord::parse(c).ok()).collect(),
            _ => Vec::new(),
        }
    }

    fn check(&self, key: &str, value: Value) -> Result<Value, String> {
        let spec = spec(key).ok_or_else(|| format!("unknown setting {key:?}"))?;
        match (spec.kind, value) {
            (Kind::Millis | Kind::Count, Value::Number(n)) => {
                if (spec.min..=spec.max).contains(&n) {
                    Ok(Value::Number(n))
                } else {
                    Err(format!("{n} is outside {}..={}", spec.min, spec.max))
                }
            }
            (Kind::Keys, Value::Keys(list)) => self.check_keys(spec, &list).map(Value::Keys),
            (Kind::Keys, Value::Number(_)) => Err("expected a list of keys".into()),
            (_, Value::Keys(_)) => Err("expected a number".into()),
        }
    }

    fn check_keys(&self, spec: &Spec, list: &[String]) -> Result<Vec<String>, String> {
        if list.is_empty() {
            return Err("bind at least one key (reset to go back to the default)".into());
        }
        let own_default: Vec<String> = match spec.default.value() {
            Value::Keys(k) => k,
            Value::Number(_) => Vec::new(),
        };
        let mut out: Vec<String> = Vec::new();
        for text in list {
            let chord = Chord::parse(text)?;
            let name = chord.to_string();
            if chord.types_text() {
                return Err(format!("{name}: plain keys type text; add ctrl or alt"));
            }
            // Terminals report the shifted character itself (`}`, not
            // shift+`]`), so the TUI never sees Shift on a non-letter.
            if chord.shift && matches!(chord.key, Key::Char(c) if !c.is_ascii_alphabetic()) {
                return Err(format!(
                    "{name}: never fires; write the shifted character instead (alt+}} for alt+shift+])"
                ));
            }
            if RESERVED.contains(&name.as_str()) && !own_default.contains(&name) {
                return Err(format!("{name}: a tab already uses it"));
            }
            let taken = SPECS
                .iter()
                .filter(|s| s.kind == Kind::Keys && s.key != spec.key)
                .find(|s| self.chords(s.key).contains(&chord));
            if let Some(other) = taken {
                return Err(format!("{name}: already bound to {}", other.label));
            }
            if !out.contains(&name) {
                out.push(name);
            }
        }
        Ok(out)
    }
}

/// One row of `GET /api/settings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingInfo {
    pub key: String,
    pub section: String,
    pub label: String,
    pub help: String,
    pub kind: Kind,
    pub default: Value,
    /// In effect now: the override, else the default.
    pub value: Value,
    pub overridden: bool,
    pub min: u64,
    pub max: u64,
    pub restart: bool,
}

/// `GET /api/settings`, and the answer to every settings write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsResponse {
    pub settings: Vec<SettingInfo>,
    /// Config file the writes go to.
    pub config_path: String,
    /// Overrides in the file that were invalid and are on their defaults.
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl SettingsResponse {
    /// Every spec with its value in `settings`.
    pub fn new(settings: &Settings, config_path: String, warnings: Vec<String>) -> Self {
        let settings = SPECS
            .iter()
            .map(|s| SettingInfo {
                key: s.key.into(),
                section: s.section().into(),
                label: s.label.into(),
                help: s.help.into(),
                kind: s.kind,
                default: s.default.value(),
                value: settings.value(s.key).unwrap_or_else(|| s.default.value()),
                overridden: settings.is_set(s.key),
                min: s.min,
                max: s.max,
                restart: s.restart,
            })
            .collect();
        Self {
            settings,
            config_path,
            warnings,
        }
    }

    /// The overrides this response reports.
    pub fn overrides(&self) -> Settings {
        Settings(
            self.settings
                .iter()
                .filter(|s| s.overridden)
                .map(|s| (s.key.clone(), s.value.clone()))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_constant_has_a_spec_and_defaults_validate() {
        for key in [
            PANE_EXEC_MS,
            HERDR_API_MS,
            AGENT_WAIT_MS,
            AGENT_PROMPT_MS,
            AGENT_START_MS,
            TUI_REQUEST_MS,
            PANE_EXEC_LINES,
            REPLY_FALLBACK_LINES,
            WATCHER_GIVE_UP,
            RECONNECT_ATTEMPTS,
            WATCHER_POLL_MS,
            RECONNECT_BACKOFF_MS,
            TUI_REFRESH_MS,
            TUI_PANE_MS,
        ] {
            assert!(spec(key).is_some(), "{key}");
        }
        // The known-good configuration: every default passes its own checks,
        // with all the other defaults in effect.
        let mut s = Settings::default();
        for spec in SPECS {
            s.set(spec.key, spec.default.value())
                .unwrap_or_else(|e| panic!("{}: {e}", spec.key));
            assert!(spec.key.contains('.'), "{}", spec.key);
        }
    }

    #[test]
    fn chords_parse_and_print_canonically() {
        for (input, canon) in [
            ("Ctrl+Shift+Tab", "ctrl+shift+tab"),
            ("shift+ctrl+tab", "ctrl+shift+tab"),
            ("backtab", "shift+tab"),
            ("alt+Right", "alt+right"),
            ("ctrl+space", "ctrl+space"),
            ("F5", "f5"),
            ("alt+]", "alt+]"),
        ] {
            assert_eq!(Chord::parse(input).unwrap().to_string(), canon, "{input}");
        }
        for bad in ["", "ctrl+", "hyper+a", "f13", "alt+right+x", "ctrl+ab"] {
            assert!(Chord::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn typed_values_parse_per_kind() {
        assert_eq!(
            Value::parse(Kind::Millis, " 120_000 "),
            Ok(Value::Number(120_000))
        );
        assert!(Value::parse(Kind::Count, "ten").is_err());
        assert!(Value::parse(Kind::Millis, "-5").is_err());
        assert_eq!(
            Value::parse(Kind::Keys, "alt+right, ctrl+tab,,"),
            Ok(Value::Keys(vec!["alt+right".into(), "ctrl+tab".into()]))
        );
    }

    #[test]
    fn numbers_stay_in_bounds() {
        let mut s = Settings::default();
        assert!(s.set(PANE_EXEC_MS, Value::Number(5_000)).is_ok());
        assert_eq!(s.duration(PANE_EXEC_MS).as_millis(), 5_000);
        assert!(s.set(PANE_EXEC_MS, Value::Number(5)).is_err());
        assert!(
            s.set(PANE_EXEC_MS, Value::Keys(vec!["alt+x".into()]))
                .is_err()
        );
        assert!(s.set("timeouts.nope", Value::Number(5_000)).is_err());
        assert!(s.reset(PANE_EXEC_MS));
        assert_eq!(s.number(PANE_EXEC_MS), 120_000);
    }

    #[test]
    fn keys_refuse_typing_reserved_and_taken_chords() {
        let mut s = Settings::default();
        let keys = |k: &[&str]| Value::Keys(k.iter().map(|s| (*s).to_string()).collect());
        assert!(
            s.set("keys.tab-next", keys(&["n"]))
                .unwrap_err()
                .contains("type text")
        );
        assert!(
            s.set("keys.tab-next", keys(&["ctrl+s"]))
                .unwrap_err()
                .contains("tab already")
        );
        assert!(
            s.set("keys.tab-next", keys(&["alt+left"]))
                .unwrap_err()
                .contains("previous tab")
        );
        assert!(s.set("keys.tab-next", keys(&[])).is_err());
        for dead in ["alt+shift+]", "ctrl+shift+space", "alt+shift+1"] {
            assert!(
                s.set("keys.tab-next", keys(&[dead]))
                    .unwrap_err()
                    .contains("never fires"),
                "{dead}"
            );
        }
        assert!(s.set("keys.tab-next", keys(&["alt+shift+n"])).is_ok());
        // Own default may be reserved; a free chord is fine and normalized.
        assert!(s.set("keys.refresh", keys(&["ctrl+r"])).is_ok());
        s.set("keys.tab-next", keys(&["Alt+]", "f6", "alt+]"]))
            .unwrap();
        assert_eq!(s.value("keys.tab-next"), Some(keys(&["alt+]", "f6"])));
        let f6 = Chord::parse("f6").unwrap();
        assert_eq!(s.action_for(&f6), Some("tab-next"));
        // The default chord is free again once overridden.
        assert_eq!(s.action_for(&Chord::parse("alt+right").unwrap()), None);
    }

    /// A rebinding the API accepted loads back from the file whatever
    /// order its keys sort in: `tab-1` takes `alt+right` after `tab-next`
    /// gave it up, and `tab-1` sorts first.
    #[test]
    fn file_overrides_load_in_any_order() {
        let keys = |k: &[&str]| Value::Keys(k.iter().map(|s| (*s).to_string()).collect());
        let mut live = Settings::default();
        live.set("keys.tab-next", keys(&["alt+n"])).unwrap();
        live.set("keys.tab-1", keys(&["alt+right"])).unwrap();
        let file: BTreeMap<String, BTreeMap<String, serde_json::Value>> =
            serde_json::from_value(serde_json::json!({
                "keys": {"tab-1": ["alt+right"], "tab-next": ["alt+n"]},
            }))
            .unwrap();
        let (loaded, warnings) = Settings::from_file(&file);
        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(loaded, live);
    }

    #[test]
    fn invalid_file_entries_fall_back_to_defaults() {
        let file: BTreeMap<String, BTreeMap<String, serde_json::Value>> =
            serde_json::from_value(serde_json::json!({
                "timeouts": {"pane-exec-ms": 9_000, "herdr-api-ms": 1, "typo-ms": 5_000},
                "keys": {"quit": ["ctrl+q", "alt+q"], "tab-next": "alt+right"},
            }))
            .unwrap();
        let (s, warnings) = Settings::from_file(&file);
        assert_eq!(s.number(PANE_EXEC_MS), 9_000);
        assert_eq!(s.number(HERDR_API_MS), 60_000);
        assert_eq!(s.chords("keys.quit").len(), 2);
        assert_eq!(
            s.value("keys.tab-next"),
            spec("keys.tab-next").map(|p| p.default.value())
        );
        assert_eq!(warnings.len(), 3, "{warnings:?}");
    }

    #[test]
    fn response_round_trips_overrides() {
        let mut s = Settings::default();
        s.set(TUI_REFRESH_MS, Value::Number(1_000)).unwrap();
        let r = SettingsResponse::new(&s, "c.toml".into(), Vec::new());
        assert_eq!(r.settings.len(), SPECS.len());
        assert_eq!(r.overrides(), s);
        let back: SettingsResponse =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }
}
