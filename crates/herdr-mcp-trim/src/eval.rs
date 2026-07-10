//! Offline evaluation helpers — let the model verify its own trim savings.

use serde_json::json;

use crate::pfc1::CompressionKey;
use crate::pipeline::{parse_stage_specs, run};

/// Run the pipeline over `text` and return a JSON report with byte + token
/// estimates and per-stage stats. `stages` is a list of stage strings.
pub fn trim_eval(text: &str, stages: &[String], base_key: &CompressionKey) -> serde_json::Value {
    let parsed = match parse_stage_specs(stages) {
        Ok(p) => p,
        Err(e) => {
            return json!({ "error": e });
        }
    };
    let result = run(text, &parsed, base_key);

    let stage_reports: Vec<serde_json::Value> = result
        .stages
        .iter()
        .map(|s| {
            json!({
                "stage": s.stage,
                "output_len": s.output.len(),
                "skipped": s.skipped,
                "stats": s.stats,
            })
        })
        .collect();

    json!({
        "input_bytes": result.input.len(),
        "output_bytes": result.output.len(),
        "input_tokens_est": result.input.split_whitespace().count(),
        "output_tokens_est": result.output.split_whitespace().count(),
        "total_savings_bytes": result.total_savings_bytes,
        "total_ratio_pct": result.total_ratio,
        "stages": stage_reports,
        "output": result.output,
    })
}

/// Sweep a corpus file, running `level` (a stage string, or `caveman:full,pfc1`)
/// over each line and reporting a savings distribution.
pub fn trim_bench(corpus_path: &str, level: &str, base_key: &CompressionKey) -> serde_json::Value {
    let content = match std::fs::read_to_string(corpus_path) {
        Ok(c) => c,
        Err(e) => return json!({ "error": format!("cannot read {corpus_path}: {e}") }),
    };

    let stage_strs: Vec<String> = level
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let parsed = match parse_stage_specs(&stage_strs) {
        Ok(p) => p,
        Err(e) => return json!({ "error": e }),
    };

    let mut ratios: Vec<f64> = Vec::new();
    let mut total_in = 0usize;
    let mut total_out = 0usize;
    let mut skipped = 0usize;
    let mut lines = 0usize;

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        lines += 1;
        let r = run(line, &parsed, base_key);
        if r.input.is_empty() {
            continue;
        }
        total_in += r.input.len();
        total_out += r.output.len();
        let ratio =
            (r.input.len().saturating_sub(r.output.len())) as f64 / r.input.len() as f64 * 100.0;
        ratios.push(ratio);
        if r.stages.iter().any(|s| s.skipped.is_some()) {
            skipped += 1;
        }
    }

    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mean = if !ratios.is_empty() {
        ratios.iter().sum::<f64>() / ratios.len() as f64
    } else {
        0.0
    };
    let median = if ratios.is_empty() {
        0.0
    } else {
        ratios[ratios.len() / 2]
    };
    let min = ratios.first().copied().unwrap_or(0.0);
    let max = ratios.last().copied().unwrap_or(0.0);

    json!({
        "corpus": corpus_path,
        "level": level,
        "lines": lines,
        "skipped_lines": skipped,
        "total_input_bytes": total_in,
        "total_output_bytes": total_out,
        "aggregate_ratio_pct": if total_in > 0 {
            (total_in.saturating_sub(total_out)) as f64 / total_in as f64 * 100.0
        } else { 0.0 },
        "mean_ratio_pct": mean,
        "median_ratio_pct": median,
        "min_ratio_pct": min,
        "max_ratio_pct": max,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pfc1::default_key;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_trim_eval_returns_expected_fields() {
        let key = default_key();
        let report = trim_eval(
            "the quick brown fox jumps over the lazy dog",
            &["caveman:full".to_string(), "pfc1".to_string()],
            &key,
        );
        assert!(report.get("input_bytes").is_some());
        assert!(report.get("output_bytes").is_some());
        assert!(report.get("total_savings_bytes").is_some());
        assert!(report.get("stages").unwrap().is_array());
        assert!(report.get("output").is_some());
    }

    #[test]
    fn test_trim_eval_invalid_stage_returns_error() {
        let key = default_key();
        let report = trim_eval("hello world", &["bogus".to_string()], &key);
        assert!(report.get("error").is_some());
    }

    #[test]
    fn test_trim_eval_output_not_larger_than_input() {
        let key = default_key();
        let text = "the quick brown fox jumps over the lazy dog because it is slow";
        let report = trim_eval(
            text,
            &["caveman:full".to_string(), "pfc1".to_string()],
            &key,
        );
        let in_b = report["input_bytes"].as_u64().unwrap();
        let out_b = report["output_bytes"].as_u64().unwrap();
        assert!(out_b <= in_b);
    }

    #[test]
    fn test_trim_bench_reads_corpus() {
        let tmp = tempfile::tempdir().unwrap();
        let corpus = tmp.path().join("corpus.txt");
        std::fs::write(
            &corpus,
            "the quick brown fox\nlazy dog sleeps\nconfiguration session gateway profile\n",
        )
        .unwrap();
        let key = default_key();
        let report = trim_bench(corpus.to_str().unwrap(), "caveman:full,pfc1", &key);
        assert_eq!(report["lines"].as_u64().unwrap(), 3);
        assert!(report.get("aggregate_ratio_pct").is_some());
        assert!(report.get("mean_ratio_pct").is_some());
    }

    #[test]
    fn test_trim_bench_missing_file_returns_error() {
        let key = default_key();
        let report = trim_bench("/nonexistent/path/corpus.txt", "pfc1", &key);
        assert!(report.get("error").is_some());
    }
}
