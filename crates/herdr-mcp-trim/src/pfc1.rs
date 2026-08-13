//! PFC1 — phonetic/frequency key-dictionary compressor.
//!
//! Deterministic, reversible port of `Phonetic/arean1/src/lib/pfc1-compressor.ts`.
//! Common technical terms are substituted with **Cherokee syllabary symbols**
//! (U+13A0–U+13FF, 3 UTF-8 bytes each) — a private alphabet that cannot be
//! confused with ASCII/Latin model output or with caveman-style prose, so the
//! two compressors compose without collision.
//!
//! A compressed payload carries an ASCII `generate_header` block before the body
//! so the recipient can reconstruct without holding the key in advance.

use std::collections::HashMap;

use lazy_static::lazy_static;
use serde::Serialize;

/// A PFC1 compression key: Cherokee symbol (`&str`, e.g. `"Ꮜ"`) -> term (`&str`).
pub type CompressionKey = HashMap<String, String>;

/// Statistics produced for a compress/decompress round. Mirrors the `.ts` shape.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct CompressionStats {
    pub original_size: usize,
    pub compressed_size: usize,
    pub savings: usize,
    pub ratio: f64,
    pub terms_found: usize,
    pub key_size: usize,
}

/// Cherokee syllabary block, space-free. 85 symbols.
pub const CHEROKEE_SYMBOLS_STR: &str = "ᎠᎡᎢᎣᎤᎥᎦᎧᎨᎩᎪᎫᎬᎭᎮᎯᎰᎱᎲᎳᎴᎵᎶᎷᎸᎹᎺᎻᎼᎽᎾᎿ\
     ᏀᏁᏂᏃᏄᏅᏆᏇᏈᏉᏊᏋᏌᏍᏎᏏᏐᏑᏒᏓᏔᏕᏖᏗᏘᏙᏚᏛᏜᏝᏞᏟ\
     ᏠᏡᏢᏣᏤᏥᏦᏧᏨᏩᏪᏫᏬᏭᏮᏯᏰᏱᏲᏳᏴ";

pub fn cherokee_symbols() -> Vec<char> {
    let syms: Vec<char> = CHEROKEE_SYMBOLS_STR.chars().collect();
    // Safety net: a duplicate or out-of-range symbol would silently corrupt
    // decompression. Assert 85 unique symbols in the Cherokee syllabary block
    // (U+13A0..=U+13FF).
    debug_assert_eq!(syms.len(), 85, "expected 85 Cherokee symbols");
    let lo = std::char::from_u32(0x13A0).unwrap();
    let hi = std::char::from_u32(0x13FF).unwrap();
    let mut seen = std::collections::HashSet::new();
    for &c in &syms {
        debug_assert!(lo <= c && c <= hi, "symbol {c} is outside U+13A0..=U+13FF");
        debug_assert!(seen.insert(c), "duplicate Cherokee symbol {c}");
    }
    syms
}

/// Default key — common technical terms (ported verbatim from the reference).
pub fn default_key() -> CompressionKey {
    let mut key = CompressionKey::new();
    for (sym, term) in [
        ("Ꮜ", "session"),
        ("Ꮞ", "tools"),
        ("Ꮟ", "agent"),
        ("Ꮠ", "profile"),
        ("Ꮡ", "OpenClaw"),
        ("Ꮢ", "browser"),
        ("Ꮣ", "allow"),
        ("Ꮤ", "snapshot"),
        ("Ꮥ", "config"),
        ("Ꮦ", "gateway"),
        ("Ꮧ", "default"),
        ("Ꮨ", "sandbox"),
        ("Ꮩ", "optional"),
        ("Ꮪ", "canvas"),
        ("Ꮫ", "command"),
        ("Ꮬ", "status"),
        ("Ꮭ", "action"),
        ("Ꮮ", "thread"),
        ("Ꮯ", "message"),
        ("Ꮰ", "spawn"),
        ("Ꮱ", "node"),
        ("Ꮲ", "restart"),
        ("Ꮳ", "override"),
        ("Ꮴ", "workspace"),
        ("Ꮵ", "parameter"),
        ("Ꮶ", "plugin"),
        ("Ꮷ", "enabled"),
        ("Ꮸ", "timeout"),
        ("Ꮹ", "group:"),
        ("Ꮺ", "require"),
        ("Ꮻ", "announce"),
        ("Ꮼ", "elevated"),
    ] {
        key.insert(sym.to_string(), term.to_string());
    }
    key
}

/// Merge a seed key with additional keys, returning the union (later keys win).
pub fn merge_keys(base: &CompressionKey, extra: &CompressionKey) -> CompressionKey {
    let mut out = base.clone();
    for (k, v) in extra {
        out.insert(k.clone(), v.clone());
    }
    out
}

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "with", "that", "this", "from", "have", "your", "you", "was",
    "were", "not", "but", "all", "can", "has", "had", "its", "out", "our", "into", "than", "then",
    "they", "their", "them", "here", "there", "what", "when", "where", "while", "which", "will",
    "would", "could", "about", "after", "before", "over", "under", "just", "only", "also", "more",
    "most", "some", "many", "very", "each", "other", "such",
];

#[derive(Debug, Clone, Copy)]
pub struct CompressionOptions {
    pub normalize_case: bool,
    pub filter_stopwords: bool,
    pub allow_three_char_terms: bool,
    // Phrase (n-gram) compression options
    pub enable_phrases: bool,
    pub min_phrase_words: usize,
    pub max_phrase_words: usize,
    pub min_phrase_frequency: usize,
}

impl Default for CompressionOptions {
    fn default() -> Self {
        // Lossless round-trip matters for herdr-mcp trim, so we keep case as-is
        // (case-sensitive replacement preserves original bytes on decompress).
        CompressionOptions {
            normalize_case: false,
            filter_stopwords: false,
            allow_three_char_terms: false,
            // Phrase compression defaults
            enable_phrases: true,
            min_phrase_words: 2,
            max_phrase_words: 4,
            min_phrase_frequency: 2,
        }
    }
}

/// Heuristic net-benefit of assigning a symbol to `term` given `frequency`.
///
/// Key storage cost: `"sym=term\n"` ≈ 3 bytes (symbol) + 1 (`=`) + len(term) + 1 (`\n`).
/// Space saved per occurrence: `len(term) - 3` (symbol is 3 UTF-8 bytes).
/// Net benefit: `(freq × space_saved) - key_cost`. Only keep term when net > 0.
pub fn calculate_heuristic_benefit(term: &str, frequency: usize) -> HeuristicBenefit {
    const SYMBOL_UTF8_BYTES: usize = 3;
    const KEY_OVERHEAD: usize = 5; // "=\n" plus formatting overhead

    let term_length = term.len();
    let key_cost = KEY_OVERHEAD + term_length;
    let space_saved_per_occurrence = term_length.saturating_sub(SYMBOL_UTF8_BYTES);
    let total_space_saved = frequency * space_saved_per_occurrence;
    let net_benefit = total_space_saved as isize - key_cost as isize;

    HeuristicBenefit { net_benefit }
}

pub struct HeuristicBenefit {
    pub net_benefit: isize,
}

#[derive(Debug, Clone)]
pub struct PhoneticPair {
    pub term: String,
    pub length: usize,
    pub net_benefit: isize,
}

/// Phrase (n-gram) candidate for compression.
#[derive(Debug, Clone)]
pub struct PhrasePair {
    pub phrase: String,
    pub net_benefit: isize,
}

/// Container for both single tokens and phrases from analysis.
#[derive(Debug, Clone, Default)]
pub struct AnalyzedTerms {
    pub tokens: Vec<PhoneticPair>,
    pub phrases: Vec<PhrasePair>,
}

/// Heuristic benefit for a phrase.
/// Key cost: symbol(3) + '='(1) + phrase.len() + '\n'(1) = phrase.len() + 5
/// Space saved per occurrence: phrase.len() - 3 (Cherokee symbol = 3 UTF-8 bytes)
/// Net benefit: frequency * (len - 3) - (len + 5)
pub fn calculate_phrase_benefit(phrase: &str, frequency: usize) -> HeuristicBenefit {
    const SYMBOL_UTF8_BYTES: usize = 3;
    const KEY_OVERHEAD: usize = 5; // "=\n" plus formatting overhead

    let phrase_length = phrase.len();
    let key_cost = KEY_OVERHEAD + phrase_length;
    let space_saved_per_occurrence = phrase_length.saturating_sub(SYMBOL_UTF8_BYTES);
    let net_benefit = (frequency * space_saved_per_occurrence) as isize - key_cost as isize;

    HeuristicBenefit { net_benefit }
}

// Analyze `text` for repeated technical terms and phrases worth keying.
//
// Extracts tokens `[a-zA-Z0-9_-]{3,}` and phrases (n-grams of 2-4 words with
// exact punctuation/case preserved), counts frequencies, filters by net-benefit
// heuristic, and returns both sorted by net benefit (highest first).
lazy_static! {
    static ref TOKEN_RE: regex::Regex = regex::Regex::new(r"[a-zA-Z0-9_-]{3,}").unwrap();
}

pub fn analyze_phonetic_pairs(
    text: &str,
    min_length: usize,
    min_frequency: usize,
    enable_heuristic: bool,
    options: CompressionOptions,
) -> AnalyzedTerms {
    let effective_min_length = if options.allow_three_char_terms {
        min_length.min(3)
    } else {
        min_length.max(4)
    };
    let source = if options.normalize_case {
        text.to_lowercase()
    } else {
        text.to_string()
    };

    let mut freq: HashMap<String, usize> = HashMap::new();
    for word in TOKEN_RE.find_iter(&source) {
        let w = word.as_str().to_string();
        if options.filter_stopwords && STOPWORDS.contains(&w.to_lowercase().as_str()) {
            continue;
        }
        *freq.entry(w).or_insert(0) += 1;
    }

    let mut pairs: Vec<PhoneticPair> = freq
        .into_iter()
        .filter(|(term, freq)| term.len() >= effective_min_length && *freq >= min_frequency)
        .map(|(term, frequency)| {
            let h = calculate_heuristic_benefit(&term, frequency);
            let length = term.len();
            PhoneticPair {
                term,
                length,
                net_benefit: h.net_benefit,
            }
        })
        .collect();

    if enable_heuristic {
        pairs.retain(|p| p.net_benefit > 0);
    }
    pairs.sort_by_key(|b| std::cmp::Reverse(b.net_benefit));

    // Extract phrases if enabled
    let mut phrases: Vec<PhrasePair> = Vec::new();
    if options.enable_phrases {
        let phrase_counts = extract_phrases(
            text,
            options.min_phrase_words,
            options.max_phrase_words,
            options.min_phrase_frequency,
        );

        for (phrase, _frequency) in phrase_counts {
            let h = calculate_phrase_benefit(&phrase, _frequency);
            if h.net_benefit > 0 || !enable_heuristic {
                phrases.push(PhrasePair {
                    phrase,
                    net_benefit: h.net_benefit,
                });
            }
        }

        phrases.sort_by_key(|b| std::cmp::Reverse(b.net_benefit));
    }

    AnalyzedTerms {
        tokens: pairs,
        phrases,
    }
}
/// Returns HashMap of phrase -> frequency.
fn extract_phrases(
    text: &str,
    min_words: usize,
    max_words: usize,
    min_frequency: usize,
) -> HashMap<String, usize> {
    // Tokenize: split into words and non-words (punctuation/whitespace)
    // We track word positions to reconstruct exact phrases with original punctuation
    let word_re = regex::Regex::new(r"[a-zA-Z0-9_-]+").unwrap();
    let mut word_positions = Vec::new();
    for m in word_re.find_iter(text) {
        word_positions.push((m.start(), m.end(), m.as_str().to_string()));
    }

    let mut phrase_counts: HashMap<String, usize> = HashMap::new();

    // Generate n-grams from word positions
    if word_positions.len() >= min_words {
        for window_size in min_words..=max_words {
            if word_positions.len() < window_size {
                continue;
            }
            for i in 0..=word_positions.len() - window_size {
                let window = &word_positions[i..i + window_size];

                // Reconstruct phrase with exact original text including punctuation between words
                let start = window[0].0;
                let end = window[window_size - 1].1;
                let phrase = text[start..end].to_string();

                // Filter out phrases that are just punctuation/whitespace
                if phrase.trim().is_empty() {
                    continue;
                }

                *phrase_counts.entry(phrase).or_insert(0) += 1;
            }
        }
    }

    // Filter by minimum frequency
    phrase_counts.retain(|_, &mut freq| freq >= min_frequency);
    phrase_counts
}

/// Build a compression key from analyzed terms (tokens + phrases), seeded with `existing_key`.
/// Assigns unused Cherokee symbols to the highest-benefit terms (max 80 total).
pub fn generate_compression_key(
    terms: &AnalyzedTerms,
    existing_key: &CompressionKey,
    max_terms: usize,
) -> CompressionKey {
    let mut key = existing_key.clone();
    let used: std::collections::HashSet<String> = key.keys().cloned().collect();
    let available: Vec<char> = cherokee_symbols()
        .into_iter()
        .filter(|s| !used.contains(&s.to_string()))
        .collect();

    let max_new = (available.len()).min(max_terms.saturating_sub(existing_key.len()));
    if max_new == 0 {
        return key;
    }

    // Combine tokens and phrases, sort by net benefit (highest first)
    let mut all_terms: Vec<(&str, isize)> = Vec::new();
    for token in &terms.tokens {
        if token.net_benefit > 0 {
            all_terms.push((&token.term, token.net_benefit));
        }
    }
    for phrase in &terms.phrases {
        if phrase.net_benefit > 0 {
            all_terms.push((&phrase.phrase, phrase.net_benefit));
        }
    }
    all_terms.sort_by_key(|b| std::cmp::Reverse(b.1));

    for (i, (term, _)) in all_terms.iter().enumerate() {
        if i >= available.len() || i >= max_new {
            break;
        }
        key.insert(available[i].to_string(), term.to_string());
    }
    key
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Replace whole-word occurrences of `expansion` with `symbol` (case-sensitive,
/// word-bounded) without using regex lookaround (unsupported by `regex` crate).
/// A match is rejected if it sits adjacent to a word character (so terms inside
/// identifiers like `user_database` or `user-database` are never rewritten).
pub fn replace_whole_words(text: &str, expansion: &str, symbol: &str) -> String {
    if expansion.is_empty() {
        return text.to_string();
    }
    let mut result = String::with_capacity(text.len());
    let mut start = 0usize;
    let bytes = text;
    while let Some(rel) = bytes[start..].find(expansion) {
        let abs = start + rel;
        let before_ok = abs == 0
            || text[..abs]
                .chars()
                .next_back()
                .map(|c| !is_word_char(c))
                .unwrap_or(true);
        let after_start = abs + expansion.len();
        let after_ok = after_start >= text.len()
            || text[after_start..]
                .chars()
                .next()
                .map(|c| !is_word_char(c))
                .unwrap_or(true);

        if before_ok && after_ok {
            result.push_str(&text[start..abs]);
            result.push_str(symbol);
            start = after_start;
        } else {
            result.push_str(&text[start..after_start]);
            start = after_start;
        }
    }
    result.push_str(&text[start..]);
    result
}

/// Compress `text` using `key` (symbol -> term). Longest expansions first so
/// overlapping terms resolve correctly.
///
/// Returns a tuple of (compressed_text, used_symbols) where `used_symbols`
/// contains only the symbols that were actually substituted.
pub fn compress_text(text: &str, key: &CompressionKey) -> (String, Vec<String>) {
    let mut sorted: Vec<(String, String)> = key
        .iter()
        .filter(|(_, term)| !term.is_empty())
        .map(|(s, t)| (s.clone(), t.clone()))
        .collect();
    sorted.sort_by_key(|b| std::cmp::Reverse(b.1.len()));

    let mut result = text.to_string();
    let mut used_symbols = Vec::new();
    for (symbol, expansion) in sorted {
        let before = result.len();
        result = replace_whole_words(&result, &expansion, &symbol);
        if result.len() != before {
            used_symbols.push(symbol);
        }
    }
    (result, used_symbols)
}

/// Decompress `text` using `key` (symbol -> term). Symbol order is irrelevant.
pub fn decompress_text(text: &str, key: &CompressionKey) -> String {
    let mut result = text.to_string();
    for (symbol, expansion) in key {
        result = result.replace(symbol, expansion);
    }
    result
}

/// Build the ASCII header prepended to compressed payloads.
/// Only includes symbols that were actually used in compression.
pub fn generate_header(key: &CompressionKey, used_symbols: &[String]) -> String {
    let mut header = String::from(
        "PFC1|PHONETIC FREQ COMPRESSION\n\
         To read: replace each symbol with its expansion from KEY.\n\
         Symbols never appear in English — expand all instances.\n\
         Expansion is substring-safe: Ꮜs=session, Ꮜ_send=session_send\n\n\
         KEY:\n",
    );
    let used_set: std::collections::HashSet<&String> = used_symbols.iter().collect();
    let entries: Vec<(String, String)> = key
        .iter()
        .filter(|(s, _)| used_set.contains(s))
        .map(|(s, t)| (s.clone(), t.clone()))
        .collect();
    for chunk in entries.chunks(1) {
        let line: Vec<String> = chunk.iter().map(|(s, t)| format!("{s}={t}")).collect();
        header.push_str(&line.join("\n"));
        header.push('\n');
    }
    header.push_str("---\n\n");
    header
}

/// Parse a `generate_header` block back into a key. Returns `None` if `text`
/// does not start with a PFC1 header. Also returns the body (post-header text).
///
/// Tolerant of a missing `---` separator: the key block runs until the first
/// non-`sym=term` line, so a truncated header still yields its key.
pub fn parse_header(text: &str) -> Option<(CompressionKey, String)> {
    if !text.starts_with("PFC1|") {
        return None;
    }
    let key_start = text.find("KEY:")?;
    let after_key = &text[key_start + "KEY:".len()..];

    let mut key = CompressionKey::new();
    let mut body_start = 0usize;
    let mut found_separator = false;
    let mut line_idx = 0;

    for (idx, line) in after_key.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed == "---" {
            // Calculate body start: sum lengths of all previous lines + newlines
            body_start = after_key
                .lines()
                .take(idx)
                .map(|l| l.len() + 1) // +1 for the newline character
                .sum();
            found_separator = true;
            line_idx = idx;
            break;
        }
        if trimmed.is_empty() {
            continue;
        }
        if !trimmed.contains('=') {
            // First non-key line: it and everything after is the body.
            body_start = after_key
                .lines()
                .take(idx)
                .map(|l| l.len() + 1)
                .sum();
            line_idx = idx;
            break;
        }
        if let Some((sym, term)) = trimmed.split_once('=')
            && !sym.is_empty()
            && !term.is_empty()
        {
            key.insert(sym.to_string(), term.to_string());
        }
    }

    // Extract body, skipping the "---" separator line if found
    let body = if found_separator {
        // Skip past the separator line
        let separator_offset = after_key
            .lines()
            .take(line_idx + 1)
            .map(|l| l.len() + 1)
            .sum();
        after_key[separator_offset..].trim_start().to_string()
    } else {
        after_key[body_start..].trim_start().to_string()
    };
    Some((key, body))
}

/// Compute stats for an original/compressed pair plus the key used.
pub fn calculate_stats(original: &str, compressed: &str, key: &CompressionKey) -> CompressionStats {
    let original_size = original.len();
    let compressed_size = compressed.len();
    let savings = original_size.saturating_sub(compressed_size);
    let ratio = if original_size > 0 {
        (savings as f64 / original_size as f64) * 100.0
    } else {
        0.0
    };
    let key_size = serde_json::to_string(key).map(|s| s.len()).unwrap_or(0);
    CompressionStats {
        original_size,
        compressed_size,
        savings,
        ratio,
        terms_found: key.len(),
        key_size,
    }
}

/// Convenience: analyze `text`, seed with `seed`, and return a ready key.
pub fn analyze_and_build(text: &str, seed: &CompressionKey) -> CompressionKey {
    let terms = analyze_phonetic_pairs(text, 4, 2, true, CompressionOptions::default());
    generate_compression_key(&terms, seed, 80)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn test_key() -> CompressionKey {
        let mut key = CompressionKey::new();
        key.insert("Ꮜ".to_string(), "session".to_string());
        key.insert("Ꮞ".to_string(), "tools".to_string());
        key.insert("Ꮟ".to_string(), "agent".to_string());
        key
    }

    #[test]
    fn test_cherokee_symbols_count_85() {
        assert_eq!(cherokee_symbols().len(), 85);
    }

    #[test]
    fn test_cherokee_symbols_unique() {
        let syms = cherokee_symbols();
        let unique: std::collections::HashSet<_> = syms.iter().collect();
        assert_eq!(syms.len(), unique.len(), "symbols must be unique");
    }

    #[test]
    fn test_cherokee_symbols_in_range() {
        let lo = std::char::from_u32(0x13A0).unwrap();
        let hi = std::char::from_u32(0x13FF).unwrap();
        for &c in &cherokee_symbols() {
            assert!(lo <= c && c <= hi, "symbol {c} is outside U+13A0..=U+13FF");
        }
    }

    #[test]
    fn test_default_key_has_expected_entries() {
        let key = default_key();
        assert_eq!(key.get("Ꮜ").map(|s| s.as_str()), Some("session"));
        assert_eq!(key.get("Ꮞ").map(|s| s.as_str()), Some("tools"));
        assert_eq!(key.get("Ꮟ").map(|s| s.as_str()), Some("agent"));
        assert!(key.len() >= 30);
    }

    #[test]
    fn test_merge_keys_union() {
        let a = test_key();
        let mut b = CompressionKey::new();
        b.insert("Ꮠ".to_string(), "profile".to_string());
        b.insert("Ꮜ".to_string(), "SESSION_OVERRIDE".to_string());

        let merged = merge_keys(&a, &b);
        assert_eq!(
            merged.get("Ꮜ").map(|s| s.as_str()),
            Some("SESSION_OVERRIDE")
        );
        assert_eq!(merged.get("Ꮞ").map(|s| s.as_str()), Some("tools"));
        assert_eq!(merged.get("Ꮠ").map(|s| s.as_str()), Some("profile"));
        assert_eq!(merged.len(), 4);
    }

    #[test]
    fn test_compress_text_replaces_known_terms() {
        let key = test_key();
        let (compressed, used) = compress_text("the session tools are ready", &key);
        assert!(compressed.contains('Ꮜ'), "session should be compressed");
        assert!(compressed.contains('Ꮞ'), "tools should be compressed");
        assert!(used.contains(&"Ꮜ".to_string()));
        assert!(used.contains(&"Ꮞ".to_string()));
    }

    #[test]
    fn test_compress_text_ignores_unknown_terms() {
        let key = test_key();
        let (compressed, _used) = compress_text("unknown_word_foo", &key);
        assert_eq!(compressed, "unknown_word_foo");
    }

    #[test]
    fn test_decompress_text_expands_symbols() {
        let key = test_key();
        let out = decompress_text("Ꮜ is Ꮞ", &key);
        assert_eq!(out, "session is tools");
    }

    #[test]
    fn test_pfc1_roundtrip_empty_string() {
        let key = test_key();
        let (c, _u) = compress_text("", &key);
        let d = decompress_text(&c, &key);
        assert_eq!(d, "");
    }

    #[test]
    fn test_pfc1_roundtrip_single_word() {
        let key = test_key();
        let (c, _u) = compress_text("session", &key);
        let d = decompress_text(&c, &key);
        assert_eq!(d, "session");
    }

    #[test]
    fn test_pfc1_roundtrip_known_terms_only() {
        let key = test_key();
        let input = "session tools agent profile";
        let (c, _u) = compress_text(input, &key);
        let d = decompress_text(&c, &key);
        assert_eq!(d, input);
    }

    #[test]
    fn test_pfc1_roundtrip_mixed_known_unknown() {
        let key = test_key();
        let input = "session tools unknown_foo agent bar";
        let (c, _u) = compress_text(input, &key);
        let d = decompress_text(&c, &key);
        assert_eq!(d, input);
    }

    #[test]
    fn test_pfc1_roundtrip_unicode() {
        let key = test_key();
        let input = "こんにちは世界 — session tools";
        let (c, _u) = compress_text(input, &key);
        let d = decompress_text(&c, &key);
        assert_eq!(d, input, "PFC1 roundtrip must be lossless for unicode");
    }

    #[test]
    fn test_pfc1_roundtrip_emoji() {
        let key = test_key();
        let input = "🚀 session 🎉 tools";
        let (c, _u) = compress_text(input, &key);
        let d = decompress_text(&c, &key);
        assert_eq!(d, input);
    }

    #[test]
    fn test_pfc1_roundtrip_long_paragraph() {
        let key = default_key();
        let input = "session tools agent profile gateway config ".repeat(40);
        let (c, _u) = compress_text(&input, &key);
        let d = decompress_text(&c, &key);
        assert_eq!(d, input);
    }

    #[test]
    fn test_pfc1_roundtrip_newlines_and_whitespace() {
        let key = test_key();
        let input = "line1\nline2\n\nline3";
        let (c, _u) = compress_text(input, &key);
        let d = decompress_text(&c, &key);
        assert_eq!(d, input);
    }

    #[test]
    fn test_pfc1_roundtrip_special_chars() {
        let key = test_key();
        let input = "session@tools!agent#profile";
        let (c, _u) = compress_text(input, &key);
        let d = decompress_text(&c, &key);
        assert_eq!(d, input);
    }

    #[test]
    fn test_replace_whole_words_basic() {
        let out = replace_whole_words("the session is active", "session", "X");
        assert_eq!(out, "the X is active");
    }

    #[test]
    fn test_replace_whole_words_no_substring_match() {
        // "sessions" should be left alone (whole-word boundary, not prefix)
        let out = replace_whole_words("sessions", "session", "X");
        assert_eq!(out, "sessions");
    }

    #[test]
    fn test_replace_whole_words_no_identifier_match() {
        let out = replace_whole_words("user_session_id", "session", "X");
        assert_eq!(out, "user_session_id");
    }

    #[test]
    fn test_replace_whole_words_empty_expansion() {
        let out = replace_whole_words("session session", "", "X");
        assert_eq!(out, "session session");
    }

    #[test]
    fn test_replace_whole_words_multiple_occurrences() {
        let out = replace_whole_words("session tools session agent session", "session", "X");
        assert_eq!(out, "X tools X agent X");
    }

    #[test]
    fn test_generate_header_contains_used_symbols_only() {
        let mut key = CompressionKey::new();
        key.insert("Ꮜ".to_string(), "session".to_string());
        key.insert("Ꮞ".to_string(), "tools".to_string());
        key.insert("Ꮟ".to_string(), "agent".to_string());
        let used = vec!["Ꮜ".to_string(), "Ꮞ".to_string()];
        let header = generate_header(&key, &used);
        assert!(header.contains("Ꮜ=session"));
        assert!(header.contains("Ꮞ=tools"));
        assert!(!header.contains("agent"), "unused symbol must not appear");
        assert!(header.starts_with("PFC1|"));
        assert!(header.contains("---\n"));
    }

    #[test]
    fn test_parse_header_roundtrip() {
        let mut key = CompressionKey::new();
        key.insert("Ꮜ".to_string(), "session".to_string());
        key.insert("Ꮞ".to_string(), "tools".to_string());
        let used = vec!["Ꮜ".to_string(), "Ꮞ".to_string()];
        let header = generate_header(&key, &used);
        let body = "hello world";
        let full = format!("{header}{body}");
        let (parsed_key, parsed_body) = parse_header(&full).expect("header should parse");
        assert_eq!(parsed_key.get("Ꮜ").map(|s| s.as_str()), Some("session"));
        assert_eq!(parsed_key.get("Ꮞ").map(|s| s.as_str()), Some("tools"));
        assert!(!parsed_key.contains_key("Ꮟ"));
        assert_eq!(parsed_body, body);
    }

    #[test]
    fn test_parse_header_returns_none_for_non_pfc1() {
        assert!(parse_header("just plain text").is_none());
    }

    #[test]
    fn test_parse_header_body_after_separator() {
        let mut key = CompressionKey::new();
        key.insert("Ꮜ".to_string(), "session".to_string());
        let header = generate_header(&key, &["Ꮜ".to_string()]);
        let body = "payload after header";
        let full = format!("{header}{body}");
        let (_k, parsed_body) = parse_header(&full).unwrap();
        assert_eq!(parsed_body, body);
    }

    #[test]
    fn test_calculate_heuristic_benefit_positive() {
        // long frequent term saves bytes
        let b = calculate_heuristic_benefit("config", 5);
        assert!(b.net_benefit > 0, "frequent long term should be beneficial");
    }

    #[test]
    fn test_calculate_heuristic_benefit_negative() {
        // short rare term costs more than it saves
        let b = calculate_heuristic_benefit("cat", 1);
        assert!(
            b.net_benefit < 0,
            "rare short term should not be beneficial"
        );
    }

    #[test]
    fn test_calculate_stats_zero_input() {
        let key = test_key();
        let s = calculate_stats("", "", &key);
        assert_eq!(s.ratio, 0.0);
        assert_eq!(s.savings, 0);
    }

    #[test]
    fn test_calculate_stats_normal() {
        let key = test_key();
        let s = calculate_stats("a".repeat(100).as_str(), "b".repeat(60).as_str(), &key);
        assert_eq!(s.original_size, 100);
        assert_eq!(s.compressed_size, 60);
        assert_eq!(s.savings, 40);
        assert!((s.ratio - 40.0).abs() < 1e-9);
    }

    #[test]
    fn test_analyze_and_build_returns_key() {
        let seed = default_key();
        let text = "config gateway profile agent session tools config gateway config";
        let key = analyze_and_build(text, &seed);
        assert!(key.len() >= seed.len());
    }

    #[test]
    fn test_analyze_phonetic_pairs_min_frequency() {
        // Disable the net-benefit heuristic so only frequency/count filtering applies.
        let opts = CompressionOptions {
            enable_phrases: false,
            ..CompressionOptions::default()
        };
        let terms = analyze_phonetic_pairs(
            "configuration configuration configuration",
            4,
            2,
            false,
            opts,
        );
        // appears 3 times, should pass min_frequency=2
        assert!(terms.tokens.iter().any(|t| t.term == "configuration"));
        let terms2 = analyze_phonetic_pairs("configuration", 4, 2, false, opts);
        // appears once, should be filtered
        assert!(!terms2.tokens.iter().any(|t| t.term == "configuration"));
    }

    #[test]
    fn test_analyze_phonetic_pairs_phrases() {
        let opts = CompressionOptions::default();
        let terms = analyze_phonetic_pairs(
            "run the build run the build run the build",
            4,
            2,
            true,
            opts,
        );
        // the repeated phrase "run the build" should be extracted
        assert!(!terms.phrases.is_empty());
    }

    #[test]
    fn test_compression_options_default_is_lossless() {
        let opts = CompressionOptions::default();
        assert!(!opts.normalize_case);
        assert!(!opts.filter_stopwords);
    }

    #[test]
    fn test_pfc1_roundtrip_large_key_exhaustion() {
        // Build a key with 85 entries (one per symbol) and confirm full roundtrip.
        let syms = cherokee_symbols();
        let mut key = CompressionKey::new();
        for (i, sym) in syms.iter().enumerate() {
            key.insert(sym.to_string(), format!("term{i}"));
        }
        let input: Vec<String> = (0..85).map(|i| format!("term{i}")).collect();
        let input = input.join(" ");
        let (c, _u) = compress_text(&input, &key);
        let d = decompress_text(&c, &key);
        assert_eq!(d, input);
    }
}
