//! Unified code-region detection engine for herdr-mcp.
//!
//! Provides a pluggable registry of code detectors (fenced blocks, inline code,
//! bracketed JSON/objects) used by both the caveman style compressor and the
//! PFC1 phonetic compressor. Detectors run in priority order and emit
//! non-overlapping regions covering the entire input.
//!
//! Design follows zeroclaw's component registry pattern: each detector implements
//! a trait, the registry runs them in priority order, and new detectors can be
//! added without modifying core logic.

use std::fmt;

/// Type of code region detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegionType {
    /// Fenced block: ```...``` or ~~~...~~~
    Fenced,
    /// Inline code: `...`
    Inline,
    /// Bracket-delimited: {...} or [...] (balanced, JSON-aware)
    Bracketed,
    /// Regular prose (non-code)
    Prose,
}

impl fmt::Display for RegionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegionType::Fenced => write!(f, "fenced"),
            RegionType::Inline => write!(f, "inline"),
            RegionType::Bracketed => write!(f, "bracketed"),
            RegionType::Prose => write!(f, "prose"),
        }
    }
}

/// A detected code region with exact byte offsets for perfect reconstruction.
#[derive(Debug, Clone)]
pub struct CodeRegion {
    /// Byte offset of region start in the original text.
    pub start: usize,
    /// Byte offset of region end (exclusive).
    pub end: usize,
    /// Whether this region is code (true) or prose (false).
    pub is_code: bool,
    /// The specific type of region.
    #[cfg_attr(not(test), allow(dead_code))]
    pub region_type: RegionType,
    /// The region content (for convenience; `text[start..end]` is identical).
    pub content: String,
}

impl CodeRegion {
    pub fn new(start: usize, end: usize, is_code: bool, region_type: RegionType, content: String) -> Self {
        Self { start, end, is_code, region_type, content }
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.end - self.start
    }
}

/// Detection mode — controls which region types to identify.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectMode {
    /// Only fenced blocks (``` / ~~~) — matches caveman's behavior exactly.
    FencedOnly,
    /// Fenced + inline (`...`) + bracketed ({...} / [...]) — for PFC1.
    Full,
}

/// Trait for pluggable code detectors.
///
/// Detectors implement this trait and are registered in the `CodeRegionRegistry`.
/// They run in priority order (higher priority first), and regions from higher-
/// priority detectors take precedence on overlap.
pub trait CodeDetector: Send + Sync {
    /// Priority for overlap resolution (higher = runs first, wins on conflict).
    fn priority(&self) -> i32;

    /// Detect regions in `text`.
    ///
    /// Returns regions with `is_code = true`. The registry will fill in prose
    /// gaps between these regions.
    fn detect(&self, text: &str, mode: DetectMode) -> Vec<CodeRegion>;
}

/// Registry of code detectors, following zeroclaw's component registry pattern.
#[derive(Default)]
pub struct CodeRegionRegistry {
    detectors: Vec<Box<dyn CodeDetector>>,
}

impl CodeRegionRegistry {
    /// Register a new detector. Higher priority detectors run first.
    pub fn register(&mut self, detector: Box<dyn CodeDetector>) {
        self.detectors.push(detector);
        // Sort by priority descending (highest first)
        self.detectors.sort_by_key(|d| -d.priority());
    }

    /// Detect all regions using registered detectors.
    pub fn detect_all(&self, text: &str, mode: DetectMode) -> Vec<CodeRegion> {
        if text.is_empty() {
            return vec![CodeRegion::new(0, 0, false, RegionType::Prose, String::new())];
        }

        let mut all_regions = Vec::new();

        // Run each detector, collecting regions
        for detector in &self.detectors {
            let regions = detector.detect(text, mode);
            for region in regions {
                all_regions.push((detector.priority(), region));
            }
        }

        // Sort by priority (higher first), then by start position
        all_regions.sort_by(|a, b| {
            a.0.cmp(&b.0).reverse().then_with(|| a.1.start.cmp(&b.1.start))
        });

        // Resolve overlaps: higher priority wins
        let mut final_regions = Vec::new();
        for (_, region) in all_regions {
            if final_regions.is_empty() {
                final_regions.push(region);
                continue;
            }

            let mut added = false;
            for existing in &mut final_regions {
                if regions_overlap(existing, &region) {
                    // Overlap detected: higher priority already in final_regions, skip this one
                    added = true;
                    break;
                }
            }
            if !added {
                final_regions.push(region);
            }
        }

        // Sort by start position
        final_regions.sort_by_key(|r| r.start);

        // Fill prose gaps between code regions
        fill_prose_gaps(text, &mut final_regions);

        final_regions
    }
}

/// Check if two regions overlap.
fn regions_overlap(a: &CodeRegion, b: &CodeRegion) -> bool {
    a.start < b.end && b.start < a.end
}

/// Fill prose gaps between code regions to cover the entire input.
fn fill_prose_gaps(text: &str, regions: &mut Vec<CodeRegion>) {
    if regions.is_empty() {
        if !text.is_empty() {
            regions.push(CodeRegion::new(0, text.len(), false, RegionType::Prose, text.to_string()));
        }
        return;
    }

    let mut new_regions = Vec::new();
    let mut last_end = 0;

    for region in regions.drain(..) {
        let region_start = region.start;
        let region_len = region.len();
        if region_start > last_end {
            new_regions.push(CodeRegion::new(
                last_end,
                region_start,
                false,
                RegionType::Prose,
                text[last_end..region_start].to_string(),
            ));
        }
        new_regions.push(region);
        last_end = region_start + region_len;
    }

    if last_end < text.len() {
        new_regions.push(CodeRegion::new(
            last_end,
            text.len(),
            false,
            RegionType::Prose,
            text[last_end..].to_string(),
        ));
    }

    *regions = new_regions;
}

/// High-level API: detect all code regions in `text` for the given `mode`.
pub fn detect_all_regions(text: &str, mode: DetectMode) -> Vec<CodeRegion> {
    let registry = build_default_registry();
    registry.detect_all(text, mode)
}

/// Build the default registry with all built-in detectors.
fn build_default_registry() -> CodeRegionRegistry {
    let mut registry = CodeRegionRegistry::default();
    registry.register(Box::new(FencedCodeDetector));
    registry.register(Box::new(InlineCodeDetector));
    registry.register(Box::new(BracketedCodeDetector));
    registry
}

/// Split `text` into (is_code, content) segments using the given regions.
pub fn split_by_regions(_text: &str, regions: &[CodeRegion]) -> Vec<(bool, String)> {
    regions.iter()
        .map(|r| (r.is_code, r.content.clone()))
        .collect()
}

// ============================================================================
// Built-in Detectors
// ============================================================================

/// Detector for fenced code blocks (```...``` or ~~~...~~~).
pub struct FencedCodeDetector;

impl CodeDetector for FencedCodeDetector {
    fn priority(&self) -> i32 {
        100 // Highest: fenced blocks are structural, win on any overlap
    }

    fn detect(&self, text: &str, mode: DetectMode) -> Vec<CodeRegion> {
        // FencedOnly and Full both include fenced blocks
        if mode == DetectMode::FencedOnly || mode == DetectMode::Full {
            detect_fenced_blocks(text)
        } else {
            Vec::new()
        }
    }
}

/// Detect fenced code blocks (```...``` or ~~~...~~~).
/// Preserves fence markers in the region for perfect reconstruction.
fn detect_fenced_blocks(text: &str) -> Vec<CodeRegion> {
    let mut regions = Vec::new();
    let mut in_code = false;
    let mut fence_marker = "```";
    let mut fence_start = 0usize;
    let bytes = text.as_bytes();

    let mut i = 0;
    while i < bytes.len() {
        // Check for fence at line start
        let at_line_start = i == 0 || (i > 0 && bytes[i - 1] == b'\n');
        if at_line_start {
            let remaining = &bytes[i..];
            let is_fence = remaining.len() >= 3 &&
                (remaining[..3] == [b'`', b'`', b'`'] || remaining[..3] == [b'~', b'~', b'~']);
            if is_fence {
                let marker_len = 3;
                let marker = &text[i..i + marker_len];

                if in_code {
                    // Check if this is the closing fence (same marker)
                    if remaining.starts_with(fence_marker.as_bytes()) {
                        // Include the closing fence line
                        let line_end = find_line_end(text, i + marker_len);
                        let region_end = line_end;
                        let content = text[fence_start..region_end].to_string();
                        regions.push(CodeRegion::new(
                            fence_start, region_end, true, RegionType::Fenced, content
                        ));
                        in_code = false;
                        i = region_end;
                        continue;
                    }
                } else {
                    // Opening fence
                    fence_marker = marker;
                    fence_start = i;
                    in_code = true;
                }
            }
        }
        if in_code {
            // Just advance to next line
            let line_end = find_line_end(text, i);
            i = line_end;
            continue;
        }
        // Move to next char
        i += bytes[i..].iter().position(|&b| b == b'\n').map(|p| p + 1).unwrap_or(bytes.len() - i);
    }

    // If we ended inside a fence (unclosed), close at EOF
    if in_code {
        let content = text[fence_start..].to_string();
        regions.push(CodeRegion::new(fence_start, text.len(), true, RegionType::Fenced, content));
    }

    regions
}

/// Find the end of the current line (inclusive of newline).
fn find_line_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    if start >= bytes.len() {
        return bytes.len();
    }
    let pos = &bytes[start..].iter().position(|&b| b == b'\n');
    match pos {
        Some(p) => start + p + 1,
        None => bytes.len(),
    }
}

/// Detector for inline code spans (`...`).
pub struct InlineCodeDetector;

impl CodeDetector for InlineCodeDetector {
    fn priority(&self) -> i32 {
        50 // Lower than fenced
    }

    fn detect(&self, text: &str, mode: DetectMode) -> Vec<CodeRegion> {
        if mode == DetectMode::Full {
            detect_inline_code(text)
        } else {
            Vec::new()
        }
    }
}

/// Detect inline code spans (`...`).
fn detect_inline_code(text: &str) -> Vec<CodeRegion> {
    let mut regions = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'`' {
            let start = i;
            i += 1; // skip opening backtick

            // Find closing backtick
            let code_start = i;
            while i < bytes.len() && bytes[i] != b'`' {
                i += 1;
            }
            let code_end = i;
            if code_end > code_start || i < bytes.len() {
                // Include both backticks in region
                let region_end = if i < bytes.len() { i + 1 } else { i };
                let content = text[start..region_end].to_string();
                regions.push(CodeRegion::new(start, region_end, true, RegionType::Inline, content));
                i = region_end;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    regions
}

/// Detector for balanced bracketed regions ({...} / [...]) — JSON/object literals.
pub struct BracketedCodeDetector;

impl CodeDetector for BracketedCodeDetector {
    fn priority(&self) -> i32 {
        30 // Lower than inline
    }

    fn detect(&self, text: &str, mode: DetectMode) -> Vec<CodeRegion> {
        if mode == DetectMode::Full {
            detect_bracketed(text)
        } else {
            Vec::new()
        }
    }
}

/// Detect balanced bracketed regions ({...} / [...]) with JSON-aware parsing.
/// Handles nested structures and string literals.
fn detect_bracketed(text: &str) -> Vec<CodeRegion> {
    let mut regions = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        // Only start a bracketed region at '{' or '[' that looks like a JSON/object literal
        if c == '{' || c == '[' {
            // Heuristic: preceded by whitespace, punctuation, or start of text
            let prev_ok = i == 0
                || matches!(
                    chars.get(i - 1),
                    Some(p) if p.is_whitespace() || matches!(p, '(' | '[' | '{' | ',' | ':' | '=' | '?' | '|' | '&' | ';')
                );

            if prev_ok {
                let start = char_index_to_byte(text, i);
                let opener = c;
                let closer = if opener == '{' { '}' } else { ']' };
                let mut depth = 0;
                let mut j = i;
                let mut in_string = false;
                let mut escape = false;

                while j < chars.len() {
                    let d = chars[j];
                    if in_string {
                        if escape {
                            escape = false;
                        } else if d == '\\' {
                            escape = true;
                        } else if d == '"' {
                            in_string = false;
                        }
                    } else if d == '"' {
                        in_string = true;
                    } else if d == opener {
                        depth += 1;
                    } else if d == closer {
                        depth -= 1;
                        if depth == 0 {
                            // Balanced region found
                            let end = char_index_to_byte(text, j + 1);
                            let content = text[start..end].to_string();
                            regions.push(CodeRegion::new(
                                start, end, true, RegionType::Bracketed, content
                            ));
                            i = j; // will be incremented by loop
                            break;
                        }
                    }
                    j += 1;
                }
                if depth == 0 && j > i {
                    // Successfully found balanced region
                    i = j;
                    continue;
                }
                // Not balanced or didn't find closer - don't treat as code
            }
        }
        i += 1;
    }

    regions
}

/// Convert character index to byte index in UTF-8 string.
fn char_index_to_byte(text: &str, char_idx: usize) -> usize {
    text.chars().take(char_idx).map(|c| c.len_utf8()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fenced_only_mode() {
        let text = "prose\n```rust\nfn main() {}\n```\nmore prose";
        let regions = detect_all_regions(text, DetectMode::FencedOnly);
        assert_eq!(regions.len(), 3);
        assert!(!regions[0].is_code);
        assert!(regions[1].is_code);
        assert_eq!(regions[1].region_type, RegionType::Fenced);
        assert!(!regions[2].is_code);

        // Verify reconstruction
        let rebuilt: String = regions.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(rebuilt, text);
    }

    #[test]
    fn test_full_mode_inline() {
        let text = "run `cargo build` now";
        let regions = detect_all_regions(text, DetectMode::Full);
        let code = regions.iter().find(|r| r.is_code).unwrap();
        assert_eq!(code.region_type, RegionType::Inline);
        assert_eq!(code.content, "`cargo build`");

        let rebuilt: String = regions.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(rebuilt, text);
    }

    #[test]
    fn test_full_mode_bracketed() {
        let text = "config = { \"key\": \"value\" };";
        let regions = detect_all_regions(text, DetectMode::Full);
        let code = regions.iter().find(|r| r.is_code).unwrap();
        assert_eq!(code.region_type, RegionType::Bracketed);

        let rebuilt: String = regions.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(rebuilt, text);
    }

    #[test]
    fn test_nested_brackets() {
        let text = "obj = { \"nested\": { \"key\": [1, 2] } };";
        let regions = detect_all_regions(text, DetectMode::Full);
        let code = regions.iter().find(|r| r.is_code).unwrap();
        assert_eq!(code.region_type, RegionType::Bracketed);
        assert!(code.content.contains("nested"));

        let rebuilt: String = regions.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(rebuilt, text);
    }

    #[test]
    fn test_fenced_with_language() {
        let text = "```rust\nfn main() {}\n```\n";
        let regions = detect_all_regions(text, DetectMode::FencedOnly);
        assert_eq!(regions.len(), 1);
        assert!(regions[0].is_code);
        assert!(regions[0].content.starts_with("```rust"));
        assert!(regions[0].content.ends_with("```\n"));

        let rebuilt: String = regions.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(rebuilt, text);
    }

    #[test]
    fn test_overlap_resolution() {
        // Fenced should win over inline inside a fence
        let text = "`code`\n```\n`inline in fence`\n```";
        let regions = detect_all_regions(text, DetectMode::Full);
        // The fenced block should be one region, inline inside it should NOT be separate
        let fenced = regions.iter().filter(|r| r.region_type == RegionType::Fenced).count();
        assert_eq!(fenced, 1);
        let inline = regions.iter().filter(|r| r.region_type == RegionType::Inline).count();
        // The inline inside fence should be part of fenced region, not separate
        assert_eq!(inline, 1); // only the first `code`
    }
}