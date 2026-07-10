//! Ordered compressor pipeline used by the trim tools and the a2a primitives.
//!
//! A `PipelineResult` runs `text` through an ordered list of [`StageSpec`]s,
//! recording per-stage output and stats. The two compressor families
//! (`caveman` style, `pfc1` phonetic) are non-colliding, so composition is
//! safe: style first, then the private-key dictionary.

use crate::caveman::{self, CavemanLevel};
use crate::code_regions::{DetectMode, detect_all_regions, split_by_regions};
use crate::pfc1::{self, CompressionKey, CompressionStats};

/// A single stage in the trim pipeline.
#[derive(Debug, Clone)]
pub enum StageSpec {
    Caveman(CavemanLevel),
    /// PFC1 phonetic compressor. `emit_header` controls whether the self-
    /// describing key header is embedded. Set `false` for trusted a2a where
    /// both ends share the server's persistent key (keeps short messages small).
    Pfc1 {
        emit_header: bool,
    },
}

/// Parse a stage string (`"caveman"`, `"caveman:lite|full|ultra"`,
/// `"pfc1"`) into a [`StageSpec`]. PFC1 defaults to `emit_header = true`.
pub fn parse_stage_spec(s: &str) -> Result<StageSpec, String> {
    let lower = s.to_ascii_lowercase();
    if lower == "pfc1" {
        return Ok(StageSpec::Pfc1 { emit_header: true });
    }
    if lower == "caveman" || lower == "caveman:full" || lower.is_empty() {
        return Ok(StageSpec::Caveman(CavemanLevel::Full));
    }
    if let Some(level) = CavemanLevel::parse(&lower) {
        if matches!(
            level,
            CavemanLevel::Lite | CavemanLevel::Full | CavemanLevel::Ultra
        ) {
            return Ok(StageSpec::Caveman(level));
        }
        return Err(format!("wenyan levels are not available: {s}"));
    }
    Err(format!(
        "unknown stage '{s}' (expected 'caveman', 'caveman:lite|full|ultra', or 'pfc1')"
    ))
}

/// Parse a list of stage strings, failing the whole list on the first error.
pub fn parse_stage_specs(specs: &[String]) -> Result<Vec<StageSpec>, String> {
    specs.iter().map(|s| parse_stage_spec(s)).collect()
}

/// Per-stage result.
#[derive(Debug, Clone)]
pub struct StageResult {
    pub stage: String,
    pub output: String,
    pub stats: Option<CompressionStats>,
    pub skipped: Option<String>,
    /// PFC1 key used (if any) so the caller can persist it as steady-state memory.
    pub pfc1_key: Option<CompressionKey>,
    /// Header bytes added by this stage (for PFC1).
    pub header_bytes: usize,
}

/// Whole-pipeline result.
#[derive(Debug, Clone)]
pub struct PipelineResult {
    pub input: String,
    pub output: String,
    pub stages: Vec<StageResult>,
    pub total_savings_bytes: usize,
    pub total_ratio: f64,
    /// Total header bytes added by PFC1 stages (for net savings calc).
    pub total_header_bytes: usize,
}

/// Run `text` through `stages`, seeding PFC1 with `base_key` (typically the
/// default key merged with any persisted workspace memory).
pub fn run(text: &str, stages: &[StageSpec], base_key: &CompressionKey) -> PipelineResult {
    let mut current = text.to_string();
    let mut stage_results = Vec::new();

    for spec in stages {
        match spec {
            StageSpec::Caveman(level) => {
                let r = caveman::compress(&current, *level);
                let stage = level.name().to_string();
                let out = if r.skipped {
                    current.clone()
                } else {
                    r.output.clone()
                };
                stage_results.push(StageResult {
                    stage,
                    output: out.clone(),
                    stats: None,
                    skipped: r.reason.clone(),
                    pfc1_key: None,
                    header_bytes: 0,
                });
                current = out;
            }
            StageSpec::Pfc1 { emit_header } => {
                let key = pfc1::analyze_and_build(&current, base_key);

                // Code-aware compression: detect all code regions (fenced, inline, bracketed)
                // and only compress prose segments. Track which symbols are actually used.
                let regions = detect_all_regions(&current, DetectMode::Full);
                let segments = split_by_regions(&current, &regions);

                let mut compressed_body = String::with_capacity(current.len());
                let mut used_symbols = Vec::new();

                for (is_code, content) in segments {
                    if is_code {
                        compressed_body.push_str(&content);
                    } else {
                        // Track which symbols were used during compression
                        let (compressed, segment_used) = pfc1::compress_text(&content, &key);
                        compressed_body.push_str(&compressed);
                        used_symbols.extend(segment_used);
                    }
                }

                let body = compressed_body;
                let header = if *emit_header {
                    pfc1::generate_header(&key, &used_symbols)
                } else {
                    String::new()
                };
                let candidate = format!("{}{}", header, body);
                let _header_bytes = header.len();
                // Adaptive gate: never expand the wire bytes. If compression
                // isn't beneficial (e.g. a short message drowned by the header),
                // pass the original through untouched.
                if candidate.len() >= current.len() {
                    stage_results.push(StageResult {
                        stage: "pfc1".to_string(),
                        output: current.clone(),
                        stats: Some(pfc1::calculate_stats(&current, &current, &key)),
                        skipped: Some("no net benefit; skipped to avoid expansion".to_string()),
                        pfc1_key: None,
                        header_bytes: 0,
                    });
                } else {
                    let stats = pfc1::calculate_stats(&current, &candidate, &key);
                    let header_bytes = if *emit_header {
                        pfc1::generate_header(&key, &used_symbols).len()
                    } else {
                        0
                    };
                    stage_results.push(StageResult {
                        stage: "pfc1".to_string(),
                        output: candidate.clone(),
                        stats: Some(stats),
                        skipped: None,
                        pfc1_key: Some(key),
                        header_bytes,
                    });
                    current = candidate;
                }
            }
        }
    }

    let total_savings = text.len().saturating_sub(current.len());
    let total_header = stage_results.iter().map(|s| s.header_bytes).sum();
    let total_ratio = if !text.is_empty() {
        (total_savings as f64 / text.len() as f64) * 100.0
    } else {
        0.0
    };

    PipelineResult {
        input: text.to_string(),
        output: current,
        stages: stage_results,
        total_savings_bytes: total_savings,
        total_ratio,
        total_header_bytes: total_header,
    }
}

/// Reverse a PFC1-compressed payload. Tries the self-describing header first;
/// if absent and `shared_key` is provided (trusted a2a), decompresses with it.
/// Returns the input unchanged if neither applies.
pub fn decompress_pfc1(text: &str, shared_key: Option<&CompressionKey>) -> String {
    if let Some((key, body)) = pfc1::parse_header(text) {
        return pfc1::decompress_text(&body, &key);
    }
    if let Some(key) = shared_key {
        return pfc1::decompress_text(text, key);
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caveman::CavemanLevel;
    use crate::pfc1::{self, default_key};
    use pretty_assertions::assert_eq;

    #[test]
    fn test_parse_pfc1() {
        let s = parse_stage_spec("pfc1").unwrap();
        assert!(matches!(s, StageSpec::Pfc1 { emit_header: true }));
    }

    #[test]
    fn test_parse_caveman_defaults_full() {
        let s = parse_stage_spec("caveman").unwrap();
        assert!(matches!(s, StageSpec::Caveman(CavemanLevel::Full)));
    }

    #[test]
    fn test_parse_caveman_full() {
        let s = parse_stage_spec("caveman:full").unwrap();
        assert!(matches!(s, StageSpec::Caveman(CavemanLevel::Full)));
    }

    #[test]
    fn test_parse_caveman_lite() {
        let s = parse_stage_spec("caveman:lite").unwrap();
        assert!(matches!(s, StageSpec::Caveman(CavemanLevel::Lite)));
    }

    #[test]
    fn test_parse_caveman_ultra() {
        let s = parse_stage_spec("caveman:ultra").unwrap();
        assert!(matches!(s, StageSpec::Caveman(CavemanLevel::Ultra)));
    }

    #[test]
    fn test_parse_empty_defaults_full() {
        let s = parse_stage_spec("").unwrap();
        assert!(matches!(s, StageSpec::Caveman(CavemanLevel::Full)));
    }

    #[test]
    fn test_parse_unknown_errors() {
        assert!(parse_stage_spec("bogus").is_err());
        assert!(parse_stage_spec("pfc1:extra").is_err());
    }

    #[test]
    fn test_parse_wenyan_errors() {
        assert!(parse_stage_spec("caveman:wenyan-lite").is_err());
        assert!(parse_stage_spec("wenyan-full").is_err());
    }

    #[test]
    fn test_parse_stage_specs_valid() {
        let specs = vec!["caveman:full".to_string(), "pfc1".to_string()];
        let parsed = parse_stage_specs(&specs).unwrap();
        assert_eq!(parsed.len(), 2);
        assert!(matches!(parsed[0], StageSpec::Caveman(CavemanLevel::Full)));
        assert!(matches!(parsed[1], StageSpec::Pfc1 { emit_header: true }));
    }

    #[test]
    fn test_parse_stage_specs_invalid_fails_fast() {
        let specs = vec![
            "caveman:full".to_string(),
            "bogus".to_string(),
            "pfc1".to_string(),
        ];
        assert!(parse_stage_specs(&specs).is_err());
    }

    #[test]
    fn test_run_caveman_compresses_prose() {
        let key = default_key();
        let input = "the quick brown fox jumps over the lazy dog";
        let result = run(input, &[StageSpec::Caveman(CavemanLevel::Full)], &key);
        assert_eq!(result.stages.len(), 1);
        assert!(result.output.len() < input.len());
    }

    #[test]
    fn test_run_pfc1_on_repetitive_text_compresses() {
        let key = default_key();
        let input =
            "configuration gateway profile session database connection retry timeout ".repeat(10);
        let result = run(&input, &[StageSpec::Pfc1 { emit_header: true }], &key);
        assert_eq!(result.stages.len(), 1);
        let stage = &result.stages[0];
        assert!(
            stage.skipped.is_none(),
            "expected compression, got: {:?}",
            stage.skipped
        );
        assert!(result.output.len() < input.len());
    }

    #[test]
    fn test_run_pfc1_adaptive_gate_skips_short() {
        let key = default_key();
        let input = "hi";
        let result = run(input, &[StageSpec::Pfc1 { emit_header: true }], &key);
        assert_eq!(result.stages.len(), 1);
        assert!(result.stages[0].skipped.is_some());
        // Adaptive gate never expands: short message passes through untouched.
        assert_eq!(result.output, input);
    }

    #[test]
    fn test_run_caveman_then_pfc1_compose() {
        let key = default_key();
        let input = "the configuration gateway profile session database connection is ready";
        let stages = vec![
            StageSpec::Caveman(CavemanLevel::Full),
            StageSpec::Pfc1 { emit_header: true },
        ];
        let result = run(input, &stages, &key);
        assert_eq!(result.stages.len(), 2);
        // The pipeline never expands the wire bytes overall.
        assert!(result.output.len() <= input.len());
    }

    #[test]
    fn test_decompress_pfc1_roundtrip_with_header() {
        let key = default_key();
        let input =
            "configuration gateway profile session database connection retry timeout ".repeat(10);
        let result = run(&input, &[StageSpec::Pfc1 { emit_header: true }], &key);
        assert!(result.stages[0].skipped.is_none());
        let recovered = decompress_pfc1(&result.output, None);
        assert_eq!(recovered, input);
    }

    #[test]
    fn test_decompress_pfc1_no_header_uses_shared_key() {
        let key = default_key();
        let input = "session configuration gateway profile";
        // Headerless PFC1 (trusted a2a): compress with the shared key directly.
        let (compressed, _used) = pfc1::compress_text(input, &key);
        let recovered = decompress_pfc1(&compressed, Some(&key));
        assert_eq!(recovered, input);
    }

    #[test]
    fn test_run_records_stage_count_and_savings() {
        let key = default_key();
        let input = "the quick brown fox jumps over the lazy dog and the cat sat still";
        let result = run(input, &[StageSpec::Caveman(CavemanLevel::Full)], &key);
        assert_eq!(result.stages.len(), 1);
        // savings is non-negative; ratio is a percentage in [0, 100].
        assert!(result.total_savings_bytes <= input.len());
        assert!(result.total_ratio >= 0.0 && result.total_ratio <= 100.0);
    }
}
