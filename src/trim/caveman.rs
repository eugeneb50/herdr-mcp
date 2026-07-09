//! caveman — style compressor (port of the `caveman` skill rules).
//!
//! Collapses natural-language prose while keeping all technical substance
//! verbatim (code, API names, error strings, commit types). Deterministic,
//! pure string ops — no LLM, no network.
//!
//! Char-space: ASCII/Latin prose only. This is deliberately disjoint from
//! PFC1's Cherokee syllabary (U+13A0–U+13FF), so the two compressors
//! never collide and compose cleanly (caveman first, then pfc1).

/// Intensity level. `wenyan-*` levels require a `zh` corpus and, without one,
/// return `skipped = true` (see `CompressionPlan` Open Items §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CavemanLevel {
    Lite,
    Full,
    Ultra,
    WenyanLite,
    WenyanFull,
    WenyanUltra,
}

impl CavemanLevel {
    pub fn parse(s: &str) -> Option<CavemanLevel> {
        match s.to_ascii_lowercase().as_str() {
            "lite" | "caveman:lite" => Some(CavemanLevel::Lite),
            "full" | "caveman:full" | "" => Some(CavemanLevel::Full),
            "ultra" | "caveman:ultra" => Some(CavemanLevel::Ultra),
            "wenyan-lite" | "wenyan_lite" | "caveman:wenyan-lite" => Some(CavemanLevel::WenyanLite),
            "wenyan-full" | "wenyan_full" | "caveman:wenyan-full" => Some(CavemanLevel::WenyanFull),
            "wenyan-ultra" | "wenyan_ultra" | "caveman:wenyan-ultra" => Some(CavemanLevel::WenyanUltra),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            CavemanLevel::Lite => "caveman:lite",
            CavemanLevel::Full => "caveman:full",
            CavemanLevel::Ultra => "caveman:ultra",
            CavemanLevel::WenyanLite => "caveman:wenyan-lite",
            CavemanLevel::WenyanFull => "caveman:wenyan-full",
            CavemanLevel::WenyanUltra => "caveman:wenyan-ultra",
        }
    }

    fn is_wenyan(&self) -> bool {
        matches!(
            self,
            CavemanLevel::WenyanLite | CavemanLevel::WenyanFull | CavemanLevel::WenyanUltra
        )
    }
}

/// Result of a caveman compress. `skipped` is set when the auto-clarity guard
/// or a missing wenyan corpus prevents safe compression.
#[derive(Debug, Clone)]
pub struct CavemanResult {
    pub output: String,
    pub skipped: bool,
    pub reason: Option<String>,
}

// Multi-word pleasantries removed verbatim (case-insensitive).
const MULTI_WORD_PLEASANTRIES: &[&str] = &[
    "of course", "happy to", "glad to", "no problem", "you're welcome",
    "you are welcome", "let me know", "feel free", "i would be happy to",
];

// Single-word tokens dropped (case-insensitive bare-word match; technical terms
// are preserved because they contain interior uppercase / digits / separators).
const PLEASANTRIES: &[&str] = &[
    "sure", "certainly", "actually", "basically", "really", "simply",
];
const FILLERS: &[&str] = &[
    "just", "really", "basically", "actually", "simply", "literally",
    "essentially", "merely",
];
const ARTICLES: &[&str] = &["a", "an", "the"];
const HEDGES: &[&str] = &[
    "maybe", "perhaps", "seems", "appears", "i think", "i believe",
    "in my opinion", "might", "possibly",
];

// Verbatim phrases -> short synonyms (applied before token dropping).
const SYNONYMS: &[(&str, &str)] = &[
    ("in order to", "to"),
    ("a large number of", "many"),
    ("a majority of", "most"),
    ("due to the fact that", "because"),
    ("is able to", "can"),
    ("are able to", "can"),
    ("make use of", "use"),
    ("in the event that", "if"),
    ("in spite of the fact that", "although"),
    ("implement a solution for", "implement"),
    ("provides the ability to", "lets"),
    ("a significant amount of", "much"),
];

// Prose-only abbreviations (ultra). Never applied to code symbols.
const PROSE_ABBREV: &[(&str, &str)] = &[
    ("database", "DB"),
    ("authentication", "auth"),
    ("configuration", "config"),
    ("request", "req"),
    ("response", "res"),
    ("function", "fn"),
    ("implementation", "impl"),
    ("parameter", "param"),
    ("return", "ret"),
    ("value", "val"),
];

// Causal phrases -> arrow (ultra). No surrounding spaces: the replacement
// " → " supplies them, and word boundaries are enforced by replace_whole_words.
const CAUSAL_ARROWS: &[(&str, &str)] = &[
    ("because", "→"),
    ("Because", "→"),
    ("so that", "→"),
    ("So that", "→"),
    ("therefore", "→"),
    ("Therefore", "→"),
    ("thus", "→"),
    ("Thus", "→"),
    ("as a result", "→"),
    ("As a result", "→"),
    ("consequently", "→"),
    ("Consequently", "→"),
];

// Auto-clarity: do not compress these (security / irreversible / ambiguous).
const DESTRUCTIVE: &[&str] = &[
    "warning", "permanently delete", "cannot be undone", "irreversible",
    "drop column", "drop table", "rm -rf", "force push", "hard reset",
    "delete all", "wipe", "danger", "caution",
];

fn split_code_fences(text: &str) -> Vec<(bool, String)> {
    // Returns segments: (is_code, content). A code fence (` ``` `) and its
    // closing fence stay together in one code segment; prose is segmented by
    // line so blank lines and code boundaries are preserved.
    let mut out = Vec::new();
    let mut in_code = false;
    let mut buf = String::new();
    for line in text.split('\n') {
        let is_fence = line.trim_start().starts_with("```");
        if is_fence {
            buf.push_str(line);
            buf.push('\n');
            if in_code {
                out.push((true, std::mem::take(&mut buf)));
                in_code = false;
            } else {
                in_code = true;
            }
            continue;
        }
        buf.push_str(line);
        buf.push('\n');
        if !in_code && !buf.trim().is_empty() {
            out.push((false, std::mem::take(&mut buf)));
        }
    }
    if !buf.is_empty() {
        out.push((in_code, buf));
    }
    out
}

/// True when `token` looks like technical content we must never rewrite.
fn is_technical(token: &str) -> bool {
    let bare = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.' && c != '/');
    if bare.is_empty() {
        return false;
    }
    if bare.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    // interior uppercase (CamelCase) or all-caps acronym
    let bytes: Vec<char> = bare.chars().collect();
    let has_interior_upper = bytes
        .iter()
        .enumerate()
        .any(|(i, c)| i > 0 && c.is_ascii_uppercase());
    if has_interior_upper {
        return true;
    }
    if bare.len() <= 3 && bare.chars().all(|c| c.is_ascii_uppercase()) {
        return true; // API, DB, CLI
    }
    bare.contains(['.', '_', '/', '-', ':'])
}

/// Split a token into (leading punctuation, core word, trailing punctuation).
fn split_punct(token: &str) -> (&str, &str, &str) {
    let lead_end: usize = token
        .chars()
        .take_while(|c| !c.is_alphanumeric() && *c != '_')
        .map(|c| c.len_utf8())
        .sum();
    let lead = &token[..lead_end];
    let rest = &token[lead_end..];
    let trail_start: usize = rest
        .chars()
        .rev()
        .take_while(|c| !c.is_alphanumeric() && *c != '_')
        .map(|c| c.len_utf8())
        .sum();
    let trail = &rest[rest.len() - trail_start..];
    let core = &rest[..rest.len() - trail_start];
    (lead, core, trail)
}

/// Drop whole-word tokens that match any entry in `drop` (case-insensitive),
/// skipping technical tokens. Leading/trailing punctuation is preserved so
/// dropping "the." keeps the period.
fn drop_words(prose: &str, drop: &[&str]) -> String {
    let mut out = String::with_capacity(prose.len());
    for token in prose.split(' ') {
        if token.is_empty() {
            continue;
        }
        let (lead, core, trail) = split_punct(token);
        let lower = core.to_ascii_lowercase();
        let is_drop = drop.iter().any(|d| {
            // multi-word phrases handled separately; here only single words
            if d.contains(' ') {
                return false;
            }
            &lower == *d
        });
        if is_drop && !is_technical(token) {
            // Drop the bare word but keep its punctuation, attached to neighbors.
            if !out.is_empty() && out.ends_with(' ') {
                out.truncate(out.len() - 1);
            }
            if !lead.is_empty() {
                out.push_str(lead);
                out.push(' ');
            }
            if !trail.is_empty() {
                out.push_str(trail);
                out.push(' ');
            }
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(token);
    }
    out
}

/// Apply phrase replacements (synonyms / causal arrows / abbrev). Uses the
/// word-boundary-aware `replace_whole_words` so identifiers embedded in prose
/// (e.g. `user_database`, `user-database`) are never corrupted.
fn apply_phrases(prose: &str, pairs: &[(&str, &str)]) -> String {
    let mut s = prose.to_string();
    for (from, to) in pairs {
        s = crate::trim::pfc1::replace_whole_words(&s, from, to);
    }
    s
}

/// Remove multi-word pleasantry phrases (lite+). Collapses the resulting gap.
fn remove_phrases(prose: &str, phrases: &[&str]) -> String {
    let mut s = prose.to_string();
    for p in phrases {
        // case-insensitive: lowercase the haystack for matching, but we can't
        // easily rebuild case; replace both exact and Title-case variants.
        s = s.replace(p, " ");
        let titled: String = p
            .split_whitespace()
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str().to_lowercase().as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        s = s.replace(&titled, " ");
    }
    // collapse repeated spaces
    while s.contains("  ") {
        s = s.replace("  ", " ");
    }
    s
}

/// Main entry. `level` selects the transform; returns the (possibly skipped) result.
pub fn compress(text: &str, level: CavemanLevel) -> CavemanResult {
    if level.is_wenyan() {
        return CavemanResult {
            output: text.to_string(),
            skipped: true,
            reason: Some(
                "wenyan levels require a configured zh corpus (not available); returned verbatim".into(),
            ),
        };
    }

    let lower = text.to_ascii_lowercase();
    if DESTRUCTIVE.iter().any(|d| lower.contains(d)) {
        return CavemanResult {
            output: text.to_string(),
            skipped: true,
            reason: Some("auto-clarity: destructive/security content; returned verbatim".into()),
        };
    }

    let segments = split_code_fences(text);
    let mut out = String::with_capacity(text.len());

    for (is_code, content) in segments {
        if is_code {
            out.push_str(&content);
            continue;
        }
        let mut prose = content.trim_end_matches('\n').to_string();

        // Synonym phrases (full + ultra).
        if level != CavemanLevel::Lite {
            prose = apply_phrases(&prose, SYNONYMS);
        }
        // Causal arrows + prose abbrev (ultra only).
        if level == CavemanLevel::Ultra {
            for (arrow, repl) in CAUSAL_ARROWS {
                prose = crate::trim::pfc1::replace_whole_words(&prose, arrow, &format!(" {repl} "));
            }
            prose = apply_phrases(&prose, PROSE_ABBREV);
        }

        // Multi-word pleasantries (lite+).
        prose = remove_phrases(&prose, MULTI_WORD_PLEASANTRIES);

        // Token dropping.
        let mut drop: Vec<&str> = Vec::new();
        drop.extend_from_slice(PLEASANTRIES);
        drop.extend_from_slice(FILLERS);
        if level != CavemanLevel::Lite {
            drop.extend_from_slice(ARTICLES);
        }
        if level == CavemanLevel::Ultra {
            drop.extend_from_slice(HEDGES);
        }
        prose = drop_words(&prose, &drop);

        out.push_str(prose.trim());
        out.push('\n');
    }

    let result = out.trim_end_matches('\n').to_string();
    CavemanResult {
        output: result,
        skipped: false,
        reason: None,
    }
}
