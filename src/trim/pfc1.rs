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
use serde::{Deserialize, Serialize};

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
pub const CHEROKEE_SYMBOLS_STR: &str =
    "ᎠᎡᎢᎣᎤᎥᎦᎧᎨᎩᎪᎫᎬᎭᎮᎯᎰᎱᎲᎳᎴᎵᎶᎷᎸᎹᎺᎻᎼᎽᎾᎿ\
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
    "were", "not", "but", "all", "can", "has", "had", "its", "out", "our", "into", "than",
    "then", "they", "their", "them", "here", "there", "what", "when", "where", "while",
    "which", "will", "would", "could", "about", "after", "before", "over", "under", "just",
    "only", "also", "more", "most", "some", "many", "very", "each", "other", "such",
];

#[derive(Debug, Clone, Copy)]
pub struct CompressionOptions {
    pub normalize_case: bool,
    pub filter_stopwords: bool,
    pub allow_three_char_terms: bool,
    pub allow_intra_word_substitution: bool,
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
            allow_intra_word_substitution: false,
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

    HeuristicBenefit {
        net_benefit,
        key_cost,
        space_saved_per_occurrence,
        total_space_saved,
    }
}

pub struct HeuristicBenefit {
    pub net_benefit: isize,
    pub key_cost: usize,
    pub space_saved_per_occurrence: usize,
    pub total_space_saved: usize,
}

#[derive(Debug, Clone)]
pub struct PhoneticPair {
    pub term: String,
    pub frequency: usize,
    pub length: usize,
    pub compression_score: usize,
    pub net_benefit: isize,
    pub key_cost: usize,
    pub space_saved_per_occurrence: usize,
}

/// Phrase (n-gram) candidate for compression.
#[derive(Debug, Clone)]
pub struct PhrasePair {
    pub phrase: String,
    pub frequency: usize,
    pub word_count: usize,
    pub length: usize,
    pub net_benefit: isize,
    pub key_cost: usize,
    pub space_saved_per_occurrence: usize,
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
    let total_space_saved = frequency * space_saved_per_occurrence;
    let net_benefit = total_space_saved as isize - key_cost as isize;

    HeuristicBenefit {
        net_benefit,
        key_cost,
        space_saved_per_occurrence,
        total_space_saved,
    }
}

/// Analyze `text` for repeated technical terms and phrases worth keying.
///
/// Extracts tokens `[a-zA-Z0-9_-]{3,}` and phrases (n-grams of 2-4 words with
/// exact punctuation/case preserved), counts frequencies, filters by net-benefit
/// heuristic, and returns both sorted by net benefit (highest first).
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
                frequency,
                length,
                compression_score: length * frequency,
                net_benefit: h.net_benefit,
                key_cost: h.key_cost,
                space_saved_per_occurrence: h.space_saved_per_occurrence,
            }
        })
        .collect();

    if enable_heuristic {
        pairs.retain(|p| p.net_benefit > 0);
    }
    pairs.sort_by(|a, b| b.net_benefit.cmp(&a.net_benefit));

    // Extract phrases if enabled
    let mut phrases: Vec<PhrasePair> = Vec::new();
    if options.enable_phrases {
        let phrase_counts = extract_phrases(
            text,
            options.min_phrase_words,
            options.max_phrase_words,
            options.min_phrase_frequency,
        );

        for (phrase, frequency) in phrase_counts {
            let h = calculate_phrase_benefit(&phrase, frequency);
            let word_count = phrase.split_whitespace().count();
            let phrase_len = phrase.len();
            if h.net_benefit > 0 || !enable_heuristic {
                phrases.push(PhrasePair {
                    phrase,
                    frequency,
                    word_count,
                    length: phrase_len,
                    net_benefit: h.net_benefit,
                    key_cost: h.key_cost,
                    space_saved_per_occurrence: h.space_saved_per_occurrence,
                });
            }
        }

        phrases.sort_by(|a, b| b.net_benefit.cmp(&a.net_benefit));
    }

    AnalyzedTerms { tokens: pairs, phrases }
}

/// Extract word tokens preserving case and punctuation boundaries.
/// Returns (words, original_text) where words are alphanumeric tokens.
fn tokenize_words(text: &str) -> (Vec<String>, String) {
    let word_re = regex::Regex::new(r"[a-zA-Z0-9_-]+").unwrap();
    let words: Vec<String> = word_re.find_iter(text).map(|m| m.as_str().to_string()).collect();
    (words, text.to_string())
}

/// Extract phrases (n-grams) from text with exact punctuation and case preservation.
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
    all_terms.sort_by(|a, b| b.1.cmp(&a.1));

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
    sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

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

/// Compress `text` using `key` without tracking used symbols (legacy API).
pub fn compress_text_simple(text: &str, key: &CompressionKey) -> String {
    compress_text(text, key).0
}

/// Decompress `text` using `key` (symbol -> term). Symbol order is irrelevant.
pub fn decompress_text(text: &str, key: &CompressionKey) -> String {
    let mut result = text.to_string();
    for (symbol, expansion) in key {
        result = result.replace(symbol, expansion);
    }
    result
}

/// Decompress with an explicit shared key (no header required).
pub fn decompress_with_key(text: &str, key: &CompressionKey) -> String {
    decompress_text(text, key)
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
    let mut body_start = after_key.len();
    let mut scanning_keys = true;
    for line in after_key.lines() {
        let trimmed = line.trim();
        if !scanning_keys {
            break;
        }
        if trimmed == "---" {
            body_start = line_as_ptr_offset(after_key, line);
            break;
        }
        if trimmed.is_empty() {
            continue;
        }
        if !trimmed.contains('=') {
            // First non-key line: it and everything after is the body.
            body_start = line_as_ptr_offset(after_key, line);
            break;
        }
        if let Some((sym, term)) = trimmed.split_once('=') {
            if !sym.is_empty() && !term.is_empty() {
                key.insert(sym.to_string(), term.to_string());
            }
        }
    }

    let body = after_key[body_start..]
        .trim_start_matches("---\n")
        .trim_start_matches("---\r\n")
        .trim_start()
        .to_string();
    Some((key, body))
}

/// Byte offset of `line` (a sub-slice of `haystack` from `.lines()`) within `haystack`.
fn line_as_ptr_offset(haystack: &str, line: &str) -> usize {
    line.as_ptr() as usize - haystack.as_ptr() as usize
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

