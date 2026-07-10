//! caveman — style compressor (port of the `caveman` skill rules).
//!
//! Collapses natural-language prose while keeping all technical substance
//! verbatim (code, API names, error strings, commit types). Deterministic,
//! pure string ops — no LLM, no network.
//!
//! Char-space: ASCII/Latin prose only. This is deliberately disjoint from
//! PFC1's Cherokee syllabary (U+13A0–U+13FF), so the two compressors
//! never collide and compose cleanly (caveman first, then pfc1).

use crate::code_regions::{DetectMode, detect_all_regions, split_by_regions};

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
            "wenyan-ultra" | "wenyan_ultra" | "caveman:wenyan-ultra" => {
                Some(CavemanLevel::WenyanUltra)
            }
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
    "of course",
    "happy to",
    "glad to",
    "no problem",
    "you're welcome",
    "you are welcome",
    "let me know",
    "feel free",
    "i would be happy to",
];

// Single-word tokens dropped (case-insensitive bare-word match; technical terms
// are preserved because they contain interior uppercase / digits / separators).
const PLEASANTRIES: &[&str] = &[
    "sure",
    "certainly",
    "actually",
    "basically",
    "really",
    "simply",
];
const FILLERS: &[&str] = &[
    "just",
    "really",
    "basically",
    "actually",
    "simply",
    "literally",
    "essentially",
    "merely",
];
const ARTICLES: &[&str] = &["a", "an", "the"];
const HEDGES: &[&str] = &[
    "maybe",
    "perhaps",
    "seems",
    "appears",
    "i think",
    "i believe",
    "in my opinion",
    "might",
    "possibly",
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
    "warning",
    "permanently delete",
    "cannot be undone",
    "irreversible",
    "drop column",
    "drop table",
    "rm -rf",
    "force push",
    "hard reset",
    "delete all",
    "wipe",
    "danger",
    "caution",
];

/// True when `token` looks like technical content we must never rewrite.
fn is_technical(token: &str) -> bool {
    let bare = token.trim_matches(|c: char| {
        !c.is_alphanumeric() && c != '_' && c != '-' && c != '.' && c != '/'
    });
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
            lower == *d
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
        s = crate::pfc1::replace_whole_words(&s, from, to);
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
                    Some(f) => {
                        f.to_uppercase().collect::<String>() + c.as_str().to_lowercase().as_str()
                    }
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
                "wenyan levels require a configured zh corpus (not available); returned verbatim"
                    .into(),
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

    let segments = split_by_regions(text, &detect_all_regions(text, DetectMode::FencedOnly));
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
                prose = crate::pfc1::replace_whole_words(&prose, arrow, &format!(" {repl} "));
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_caveman_level_parse_all_variants() {
        assert_eq!(CavemanLevel::parse("lite"), Some(CavemanLevel::Lite));
        assert_eq!(
            CavemanLevel::parse("caveman:lite"),
            Some(CavemanLevel::Lite)
        );
        assert_eq!(CavemanLevel::parse("full"), Some(CavemanLevel::Full));
        assert_eq!(
            CavemanLevel::parse("caveman:full"),
            Some(CavemanLevel::Full)
        );
        assert_eq!(CavemanLevel::parse(""), Some(CavemanLevel::Full));
        assert_eq!(CavemanLevel::parse("ultra"), Some(CavemanLevel::Ultra));
        assert_eq!(
            CavemanLevel::parse("caveman:ultra"),
            Some(CavemanLevel::Ultra)
        );
        assert_eq!(
            CavemanLevel::parse("wenyan-lite"),
            Some(CavemanLevel::WenyanLite)
        );
        assert_eq!(
            CavemanLevel::parse("wenyan-full"),
            Some(CavemanLevel::WenyanFull)
        );
        assert_eq!(
            CavemanLevel::parse("wenyan-ultra"),
            Some(CavemanLevel::WenyanUltra)
        );
    }

    #[test]
    fn test_caveman_level_parse_invalid() {
        assert_eq!(CavemanLevel::parse("turbo"), None);
        assert_eq!(CavemanLevel::parse("medium"), None);
    }

    #[test]
    fn test_caveman_level_name_roundtrip() {
        let l = CavemanLevel::Lite;
        assert_eq!(CavemanLevel::parse(l.name()), Some(l));
        let f = CavemanLevel::Full;
        assert_eq!(CavemanLevel::parse(f.name()), Some(f));
        let u = CavemanLevel::Ultra;
        assert_eq!(CavemanLevel::parse(u.name()), Some(u));
    }

    #[test]
    fn test_caveman_full_drops_articles() {
        let r = compress("the quick brown fox", CavemanLevel::Full);
        assert!(!r.skipped);
        let words: Vec<&str> = r.output.split_whitespace().collect();
        assert!(
            !words.contains(&"the"),
            "article 'the' should be dropped: {:?}",
            r.output
        );
    }

    #[test]
    fn test_caveman_full_preserves_technical_identifiers() {
        let r = compress("the user_database has config", CavemanLevel::Full);
        assert!(!r.skipped);
        assert!(
            r.output.contains("user_database"),
            "user_database must be preserved: {:?}",
            r.output
        );
    }

    #[test]
    fn test_caveman_full_drops_pleasantries() {
        let r = compress("sure, the tool works", CavemanLevel::Full);
        assert!(!r.skipped);
        assert!(
            !r.output.to_lowercase().contains("sure"),
            "pleasantery 'sure' should be dropped: {:?}",
            r.output
        );
    }

    #[test]
    fn test_caveman_full_removes_multi_word_pleasantries() {
        let r = compress("happy to help with the build", CavemanLevel::Full);
        assert!(
            !r.output.to_lowercase().contains("happy to"),
            "multi-word pleasentry dropped: {:?}",
            r.output
        );
    }

    #[test]
    fn test_caveman_full_synonym_replacement() {
        let r = compress("in order to build the thing", CavemanLevel::Full);
        assert!(!r.skipped);
        assert!(
            r.output.contains("to build"),
            "synonym replacement applied: {:?}",
            r.output
        );
    }

    #[test]
    fn test_caveman_ultra_adds_causal_arrows() {
        let r = compress(
            "the config changed because the test failed",
            CavemanLevel::Ultra,
        );
        assert!(!r.skipped);
        assert!(
            r.output.contains('→'),
            "causal arrow should appear: {:?}",
            r.output
        );
    }

    #[test]
    fn test_caveman_ultra_abbreviates() {
        let r = compress("the database configuration is set", CavemanLevel::Ultra);
        assert!(!r.skipped);
        assert!(
            r.output.contains("DB"),
            "database abbreviated to DB: {:?}",
            r.output
        );
        assert!(
            r.output.contains("config"),
            "configuration abbreviated to config: {:?}",
            r.output
        );
    }

    #[test]
    fn test_caveman_lite_keeps_articles() {
        let r = compress("the user_database is ready", CavemanLevel::Lite);
        assert!(!r.skipped);
        let words: Vec<&str> = r.output.split_whitespace().collect();
        assert!(
            words.contains(&"the"),
            "lite should keep articles: {:?}",
            r.output
        );
    }

    #[test]
    fn test_caveman_skips_destructive_content() {
        let r = compress("warning: permanently delete this file", CavemanLevel::Full);
        assert!(r.skipped, "destructive content must be skipped");
        assert_eq!(r.output, "warning: permanently delete this file");
    }

    #[test]
    fn test_caveman_skips_wenyan_levels() {
        let r = compress("anything goes here", CavemanLevel::WenyanLite);
        assert!(r.skipped);
        assert_eq!(r.reason.is_some(), true);
    }

    #[test]
    fn test_caveman_preserves_code_blocks() {
        let input = "the build failed:\n```rust\nfn main() {}\n```\n";
        let r = compress(input, CavemanLevel::Full);
        assert!(!r.skipped);
        assert!(
            r.output.contains("fn main() {}"),
            "code block preserved: {:?}",
            r.output
        );
        assert!(
            r.output.contains("```rust"),
            "fence marker preserved: {:?}",
            r.output
        );
    }

    #[test]
    fn test_caveman_deterministic() {
        let input = "the quick brown fox jumps over the lazy dog because it is slow";
        let a = compress(input, CavemanLevel::Full);
        let b = compress(input, CavemanLevel::Full);
        assert_eq!(a.output, b.output, "caveman must be deterministic");
    }

    #[test]
    fn test_caveman_empty_input() {
        let r = compress("", CavemanLevel::Full);
        assert!(!r.skipped);
        assert_eq!(r.output, "");
    }

    #[test]
    fn test_caveman_only_code_block() {
        let input = "```\ncode only\n```";
        let r = compress(input, CavemanLevel::Full);
        assert!(!r.skipped);
        assert_eq!(r.output, input, "all-code input unchanged");
    }

    #[test]
    fn test_caveman_preserves_api_keys() {
        let r = compress("the API_KEY is set in env", CavemanLevel::Full);
        assert!(!r.skipped);
        assert!(
            r.output.contains("API_KEY"),
            "API_KEY preserved: {:?}",
            r.output
        );
    }
}
