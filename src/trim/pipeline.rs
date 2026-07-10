//! Ordered compressor pipeline used by the trim tools and the a2a primitives.
//!
//! A `PipelineResult` runs `text` through an ordered list of [`StageSpec`]s,
//! recording per-stage output and stats. The two compressor families
//! (`caveman` style, `pfc1` phonetic) are non-colliding, so composition is
//! safe: style first, then the private-key dictionary.

use crate::trim::caveman::{self, CavemanLevel};
use crate::trim::pfc1::{self, CompressionKey, CompressionStats};

/// A single stage in the trim pipeline.
#[derive(Debug, Clone)]
pub enum StageSpec {
    Caveman(CavemanLevel),
    /// PFC1 phonetic compressor. `emit_header` controls whether the self-
    /// describing key header is embedded. Set `false` for trusted a2a where
    /// both ends share the server's persistent key (keeps short messages small).
    Pfc1 { emit_header: bool },
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
                let body = pfc1::compress_text(&current, &key);
                let header = if *emit_header {
                    pfc1::generate_header(&key)
                } else {
                    String::new()
                };
                let candidate = format!("{}{}", header, body);
                let header_bytes = header.len();
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
                        pfc1::generate_header(&key).len()
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
    let total_ratio = if text.len() > 0 {
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
