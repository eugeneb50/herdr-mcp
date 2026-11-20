use crate::{Capsule, CapsuleConfig};
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path};
const SAFE: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_@%+=:,./-";
const DEFAULT_SIGIL: &str = "\u{13c9}";
const DEFAULT_STATUS: [(&str, &str); 5] = [
    ("done", "\u{13e0}"),
    ("fail", "\u{13e1}"),
    ("block", "\u{13e2}"),
    ("wip", "\u{13e3}"),
    ("task", "\u{13e4}"),
];
const DEFAULT_KIND: [(&str, &str); 9] = [
    ("file", "\u{13d0}"),
    ("dir", "\u{13d1}"),
    ("log", "\u{13d2}"),
    ("branch", "\u{13d3}"),
    ("commit", "\u{13d4}"),
    ("run", "\u{13d5}"),
    ("pane", "\u{13d6}"),
    ("url", "\u{13d7}"),
    ("capsules", "\u{13da}"),
];
pub fn shlex_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| SAFE.contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}
fn check_mark(c: &Capsule) -> String {
    match &c.check {
        None => String::new(),
        Some(k) if k.exit == 0 => " ok".to_string(),
        Some(k) => format!(" chk:{}", k.exit),
    }
}
pub fn head(c: &Capsule, skin: &str) -> String {
    let re =
        c.re.as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!(" re:{s}"))
            .unwrap_or_default();
    let (sigil, status, quote) = match skin {
        "raw" => (">".to_string(), c.status.clone(), ""),
        "glyph" => (
            DEFAULT_SIGIL.to_string(),
            DEFAULT_STATUS
                .iter()
                .find(|(k, _)| *k == c.status)
                .map(|(_, v)| v.to_string())
                .unwrap_or_else(|| c.status.clone()),
            "",
        ),
        _ => ("CAP".to_string(), c.status.clone(), "\""),
    };
    format!(
        "{sigil} {} {} {status} {}{re}{} {quote}{}{quote}",
        c.id,
        c.task,
        c.from,
        check_mark(c),
        c.summary
    )
}
fn kmark(kind: &str, skin: &str) -> String {
    if skin == "glyph" {
        DEFAULT_KIND
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, v)| v.to_string())
            .unwrap_or_else(|| kind.to_string())
    } else {
        kind.to_string()
    }
}
pub fn line(c: &Capsule, skin: &str) -> String {
    let mut parts = vec![head(c, skin)];
    for a in &c.artifacts {
        parts.push(format!("{}:{}", kmark(&a.kind, skin), a.reference));
    }
    parts.join(" ")
}
fn digest(c: &Capsule) -> Vec<String> {
    let Some(index) = c.artifacts.iter().find(|a| a.kind == "capsules") else {
        return vec!["  rollup: no capsules artifact".to_string()];
    };
    let path = Path::new(&index.reference);
    let Ok(text) = fs::read_to_string(path) else {
        return vec![format!(
            "  rollup: index unreadable ({}): no such file",
            path.display()
        )];
    };
    let entries: Vec<Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if entries.is_empty() {
        return vec![format!("  rollup: index empty: {}", path.display())];
    }
    let mut stat: BTreeMap<String, usize> = BTreeMap::new();
    for e in &entries {
        if let Some(s) = e.get("status").and_then(Value::as_str) {
            *stat.entry(s.to_string()).or_default() += 1;
        }
    }
    let tally = stat
        .iter()
        .map(|(k, v)| format!("{v} {k}"))
        .collect::<Vec<_>>()
        .join(" \u{b7} ");
    let first = entries[0].get("at").and_then(Value::as_str).unwrap_or("");
    let last = entries[entries.len() - 1]
        .get("at")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut out = vec![format!(
        "  rollup {}: {tally} \u{b7} span {}→{}",
        entries.len(),
        first.chars().skip(5).take(5).collect::<String>(),
        last.chars().skip(5).take(5).collect::<String>()
    )];
    for e in &entries[..entries.len() - 1] {
        if matches!(
            e.get("status").and_then(Value::as_str),
            Some("fail" | "block")
        ) {
            out.push(format!(
                "  lesson {} {}: {}",
                e.get("status").and_then(Value::as_str).unwrap_or(""),
                e.get("id").and_then(Value::as_str).unwrap_or(""),
                e.get("summary").and_then(Value::as_str).unwrap_or("")
            ));
        }
    }
    let closer = &entries[entries.len() - 1];
    out.push(format!(
        "  closed {}: {}",
        closer.get("id").and_then(Value::as_str).unwrap_or(""),
        closer.get("summary").and_then(Value::as_str).unwrap_or("")
    ));
    out
}
pub fn card(c: &Capsule, skin: &str) -> String {
    let mut out = vec![head(c, skin)];
    for a in &c.artifacts {
        let mut extra = Vec::new();
        if let Some(s) = &a.size {
            extra.push(format!("[{s}]"));
        }
        if let Some(n) = &a.note {
            extra.push(n.clone());
        }
        let suffix = extra.join(" ");
        let rendered = format!("  {}:{} {suffix}", kmark(&a.kind, skin), a.reference);
        out.push(rendered.trim_end().to_string());
    }
    if let Some(check) = &c.check
        && let Some(why) = &check.why
    {
        for w in why {
            let clipped: String = w.trim().chars().take(160).collect();
            out.push(format!("  ! {clipped}"));
        }
    }
    if c.rollup.is_some() {
        out.extend(digest(c));
    }
    out.join("\n")
}
pub fn delivery_text(c: &Capsule, cfg: &CapsuleConfig) -> String {
    let rendered = line(c, "raw");
    let raw = rendered.strip_prefix("> ").unwrap_or(&rendered);
    let head = format!("Capsule {} for you: {}", c.id, raw);
    let q = shlex_quote;
    let read = format!(
        " Read it: {} card {} --to {}",
        q(&cfg.exe),
        q(&c.id),
        q(&c.to)
    );
    if c.status != "task" {
        return head + &read;
    }
    let check = c
        .reply_check
        .as_deref()
        .map(|x| format!(" --check {}", q(x)))
        .unwrap_or_default();
    let report = q(&format!(
        "file={}/{}/{}-report.md::report",
        cfg.work.display(),
        c.task,
        c.id
    ));
    let reply = format!(
        " When done or stuck, reply with ONE command: {} send --re {} --to {} --task {} --status done|fail|block --summary \"<=120 chars\" --artifact {report}{check} ; then reply with its CAP line only and stop.",
        q(&cfg.exe),
        q(&c.id),
        q(c.reply_to.as_deref().unwrap_or("lead")),
        q(&c.task)
    );
    head + &read + &reply
}
pub fn pack_summary(closer: &str, n: usize, k: usize, name: &str) -> String {
    let head = format!("rollup {n} \u{b7} ");
    let tail = format!(
        " \u{b7} {k} lesson{} \u{b7} {name}",
        if k == 1 { "" } else { "s" }
    );
    let budget = 120usize.saturating_sub(head.chars().count() + tail.chars().count());
    let chars: Vec<char> = closer.trim().chars().collect();
    let mut s: String = if chars.len() > budget {
        let cut = &chars[..budget];
        match cut.iter().rposition(|c| *c == ' ') {
            Some(pos) => cut[..pos].iter().collect(),
            None => cut.iter().collect(),
        }
    } else {
        chars.iter().collect()
    };
    s = s.trim_end_matches([' ', '\u{b7}', ',', ';']).to_string();
    format!("{head}{s}{tail}")
}
fn fill(template: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (key, value) in pairs {
        out = out.replace(key, value);
    }
    out
}
pub fn brief(exe: &str, work: &str, to: &str, task: &str, check: &str, note: &str) -> String {
    fill(
        include_str!("brief.md"),
        &[
            ("{exe}", exe),
            ("{work}", work),
            ("{to}", to),
            ("{task}", task),
            ("{check}", check),
            ("{max}", "120"),
            ("{check_note}", note),
        ],
    )
}
pub fn short_brief(exe: &str, to: &str, task: &str, check: &str) -> String {
    fill(
        include_str!("short.md"),
        &[
            ("{exe}", exe),
            ("{to}", to),
            ("{task}", task),
            ("{check}", check),
        ],
    )
}
pub fn howto(exe: &str) -> String {
    include_str!("howto.md").replace("{exe}", exe)
}
fn vget<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}
pub fn legend(home: &Path) -> String {
    let path = home.join("glyphs.json");
    let key: Value = fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let sigil = vget(&key, "sigil").unwrap_or(DEFAULT_SIGIL);
    let row = |sec: &str, defs: &[(&str, &str)]| {
        defs.iter()
            .map(|(k, d)| format!("{} {k}", key.get(sec).and_then(|s| vget(s, k)).unwrap_or(d)))
            .collect::<Vec<_>>()
            .join("  ")
    };
    format!(
        "{sigil}  capsule   ({})\nstatus:  {}\nkind:    {}",
        path.display(),
        row("status", &DEFAULT_STATUS),
        row("kind", &DEFAULT_KIND)
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    fn cap() -> Capsule {
        Capsule {
            v: 2,
            id: "rotisu".into(),
            task: "t1".into(),
            from: "tohitu".into(),
            to: "builder".into(),
            status: "task".into(),
            summary: "build it".into(),
            ..Capsule::default()
        }
    }
    #[test]
    fn quoted_shell_words_are_safe() {
        assert_eq!(shlex_quote("lead"), "lead");
        assert_eq!(shlex_quote("sh /x/check.sh"), "'sh /x/check.sh'");
        assert_eq!(shlex_quote("x'y"), "'x'\\''y'");
    }
    #[test]
    fn cap_and_raw_lines_have_the_same_field_order() {
        assert_eq!(
            head(&cap(), "cap"),
            "CAP rotisu t1 task tohitu \"build it\""
        );
        assert_eq!(head(&cap(), "raw"), "> rotisu t1 task tohitu build it");
    }
    #[test]
    fn brief_fills_every_placeholder() {
        let out = brief("X", "W", "L", "t", " --check \"c\"", "\n- note");
        assert!(out.starts_with("## Report: capsule (required)\n"));
        assert!(out.contains("X send --to L --task t"));
        assert!(out.contains("--check \"c\""));
        assert!(out.contains("W/<task>/<id>-report.md"));
        assert!(out.contains("done needs >=1.\n- note"));
        assert!(!out.contains('{'));
        let s = short_brief("X", "L", "t", "");
        assert!(s.starts_with("Report: `X send --to L --task t"));
        assert!(s.ends_with("Reply only its CAP line.\n"));
    }
    #[test]
    fn howto_has_one_exe_placeholder_source() {
        let s = howto("X");
        assert!(s.contains("`X card <id>`") && s.contains("`X legend`") && !s.contains("{exe}"));
    }
    #[test]
    fn legend_uses_the_local_key() {
        let d = tempfile::tempdir().unwrap();
        let key = r#"{"sigil":"S","status":{"done":"D"},"kind":{"file":"F"}}"#;
        let p = d.path().join("glyphs.json");
        fs::write(&p, key).unwrap();
        let s: Vec<_> = legend(d.path()).lines().map(str::to_string).collect();
        assert_eq!(s[0], format!("S  capsule   ({})", p.display()));
        assert_eq!(s[1], "status:  D done  Ꮱ fail  Ꮲ block  Ꮳ wip  Ꮴ task");
        assert!(s[2].contains("F file") && s[2].ends_with("Ꮪ capsules"));
    }
}
