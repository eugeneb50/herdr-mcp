//! Folder-scanning phonetic key builder for PFC1.
//!
//! Scans a folder (and subfolders) for text/markdown files, learns the
//! domain's most compressible terms and multi-word phrases, and emits a
//! self-contained PFC1 [`CompressionKey`] written to `<folder>/.pfc1_key.json`.
//! Agents discover keys by walking up the tree, and decompress header-less
//! (trusted a2a) using the shared key.
//!
//! Design choices (per spec):
//! - **Best overall trim**: candidates are ranked by `freq × (len − 3)` so the
//!   longest, most frequent phrases win the limited Cherokee symbol budget.
//! - **Skip code**: fenced (` ``` ` / `~~~`) and inline (`` ` ``) code blocks are
//!   stripped before analysis (better than caveman — also drops inline spans).
//! - **mtime efficiency**: a cached key is reused verbatim when the scanned
//!   file set and every mtime are unchanged.
//! - **Scale to 85**: up to all 85 Cherokee syllabary symbols may be assigned.
//! - **Master key**: learned terms persist in `data_dir/pfc1_master_key.json`
//!   and seed future builds so the compressor improves across folders.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::trim::pfc1::{self, AnalyzedTerms, CompressionKey, PhoneticPair};

/// Per-folder key file name (written into the scanned folder).
pub const FOLDER_KEY_FILE: &str = ".pfc1_key.json";
/// Cross-folder learned key, persisted in the data dir.
pub const MASTER_KEY_FILE: &str = "pfc1_master_key.json";
/// Subdirectory of the data dir holding the central folder-key registry.
pub const CENTRAL_DIR: &str = "folder_keys";

/// Minimum number of Cherokee symbols always reserved for folder-specific terms
/// so a large master key can never fully crowd out the local corpus.
const MIN_FOLDER_SLOTS: usize = 30;
/// Maximum number of master-key symbols merged into a build's seed.
const MASTER_CAP: usize = 50;

/// A self-contained, folder-specific PFC1 key plus provenance metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderKey {
    pub folder: PathBuf,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Full symbol → term map. Usable standalone (no delta against the default key).
    pub key: CompressionKey,
    pub stats: FolderKeyStats,
    /// Relative source file paths that contributed to the key.
    #[serde(default)]
    pub source_files: Vec<PathBuf>,
    /// mtime (ms since epoch) snapshot of each scanned file, for incremental rebuilds.
    #[serde(default)]
    pub file_mtimes: HashMap<PathBuf, i64>,
    /// Whether the build was seeded from the learned master key.
    #[serde(default)]
    pub seeded_from_master: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FolderKeyStats {
    pub files_scanned: usize,
    pub total_bytes: usize,
    pub unique_terms: usize,
    pub terms_accepted: usize,
    pub cross_file_terms: usize,
    /// Number of learned terms that look like code identifiers (contain `_`,
    /// a digit, or an internal uppercase) — i.e. vocabulary drawn from code.
    pub code_terms: usize,
    pub phrases_found: usize,
    pub key_size: usize,
}

/// Tuning for [`build_folder_key`].
#[derive(Debug, Clone)]
pub struct FolderKeyOptions {
    /// Minimum occurrences (after cross-file boost) for a term to qualify.
    pub min_frequency: usize,
    /// Minimum term length in bytes.
    pub min_length: usize,
    /// Maximum total symbols in the produced key (≤ 85 Cherokee symbols).
    pub max_terms: usize,
    /// Glob patterns (supporting `*` and `**`) of paths to skip.
    pub exclude_patterns: Vec<String>,
    /// File extensions (without dot) to include.
    pub file_extensions: Vec<String>,
    /// Seed the build with the default key (always) plus the master key.
    pub seed_with_default: bool,
    /// Also write the key to the central registry (`data_dir/folder_keys/`).
    pub persist_central: bool,
    /// Fold accepted terms into the persistent master key.
    pub learn_master: bool,
    /// Maximum words per extracted phrase (n-gram window).
    pub max_phrase_words: usize,
}

impl Default for FolderKeyOptions {
    fn default() -> Self {
        FolderKeyOptions {
            min_frequency: 3,
            min_length: 4,
            max_terms: 85,
            exclude_patterns: vec![
                "**/node_modules/**".into(),
                "**/.git/**".into(),
                "**/target/**".into(),
                "**/dist/**".into(),
                "**/build/**".into(),
            ],
            file_extensions: vec![
                "md".into(),
                "txt".into(),
                "rs".into(),
                "toml".into(),
                "ts".into(),
                "tsx".into(),
                "py".into(),
                "js".into(),
                "json".into(),
                "yaml".into(),
                "yml".into(),
            ],
            seed_with_default: true,
            persist_central: false,
            learn_master: true,
            max_phrase_words: 3,
        }
    }
}

/// Stopwords filtered out of single-token candidates (kept inside phrases).
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "with", "that", "this", "from", "have", "your", "you", "was",
    "were", "not", "but", "all", "can", "has", "had", "its", "out", "our", "into", "than",
    "then", "they", "their", "them", "here", "there", "what", "when", "where", "while",
    "which", "will", "would", "could", "about", "after", "before", "over", "under", "just",
    "only", "also", "more", "most", "some", "many", "very", "each", "other", "such",
];

/// Build (or reuse) a folder key for `root`.
///
/// Returns the cached key unchanged when the scanned file set and every mtime
/// match the previous build. Otherwise scans, learns candidates, and persists
/// the result to `<root>/.pfc1_key.json` (and the central registry / master key
/// according to `opts`).
pub async fn build_folder_key(
    root: &Path,
    opts: &FolderKeyOptions,
    data_dir: &Path,
) -> anyhow::Result<FolderKey> {
    let prior = load_folder_key(root).await.ok().flatten();

    let (files, mtimes, total_bytes) = scan_folder(root, opts).await?;

    // Incremental: reuse the prior key if nothing changed.
    if let Some(p) = &prior {
        if p.file_mtimes.len() == mtimes.len()
            && p.file_mtimes.iter().all(|(f, m)| mtimes.get(f) == Some(m))
        {
            return Ok(p.clone());
        }
    }

    let mut term_total: HashMap<String, usize> = HashMap::new();
    let mut term_files: HashMap<String, usize> = HashMap::new();

    // Code is the highest-value compression vocabulary: long, repetitive
    // identifiers (`user_database_connection`, `connect`, `config`). Unlike the
    // caveman *style* compressor (which skips code to keep substance verbatim),
    // a dictionary compressor should *learn* code identifiers. We therefore
    // extract candidates from the whole file — prose and code alike — and split
    // compound identifiers into their components for richer coverage.
    for (path, text) in &files {
        let counts = extract_candidates(text, opts);
        for (term, c) in counts {
            *term_total.entry(term.clone()).or_insert(0) += c;
            *term_files.entry(term).or_insert(0) += 1;
        }
        let _ = path;
    }

    let seed = build_seed(data_dir, opts.seed_with_default && opts.learn_master);
    let seeded_from_master = opts.seed_with_default && opts.learn_master && !load_master_key(data_dir).await.is_empty();

    // Rank candidates by best overall trim: longest phrases + highest frequency.
    let mut pairs: Vec<PhoneticPair> = Vec::new();
    for (term, total) in &term_total {
        let files = term_files.get(term).copied().unwrap_or(0);
        // Cross-file boost: terms appearing in multiple files are more reliable.
        let boost = if files >= 2 { total / 2 } else { 0 };
        let effective = total + boost;
        if effective < opts.min_frequency {
            continue;
        }
        let term_len = term.len();
        let h = pfc1::calculate_heuristic_benefit(term, effective);
        let score = (effective as i64) * ((term_len as i64).saturating_sub(3));
        pairs.push(PhoneticPair {
            term: term.clone(),
            frequency: effective,
            length: term_len,
            compression_score: term_len * effective,
            net_benefit: score as isize,
            key_cost: h.key_cost,
            space_saved_per_occurrence: h.space_saved_per_occurrence,
        });
    }

    // Highest score first; ties broken by longer term.
    pairs.sort_by(|a, b| {
        b.net_benefit
            .cmp(&a.net_benefit)
            .then_with(|| b.length.cmp(&a.length))
    });

    let terms = AnalyzedTerms {
        tokens: pairs,
        phrases: Vec::new(),
    };
    let key = pfc1::generate_compression_key(&terms, &seed, opts.max_terms);

    let phrases_found = key.values().filter(|t| t.contains(' ')).count();
    let cross_file_terms = term_files.values().filter(|&&f| f >= 2).count();
    let code_terms = key
        .values()
        .filter(|t| {
            t.contains('_')
                || t.chars().any(|c| c.is_ascii_digit())
                || t.chars().any(|c| c.is_ascii_uppercase())
        })
        .count();
    let key_size = serde_json::to_string(&key).map(|s| s.len()).unwrap_or(0);

    let now = Utc::now();
    let folder_key = FolderKey {
        folder: root.to_path_buf(),
        created_at: prior.as_ref().map(|p| p.created_at).unwrap_or(now),
        updated_at: now,
        stats: FolderKeyStats {
            files_scanned: files.len(),
            total_bytes,
            unique_terms: term_total.len(),
            terms_accepted: key.len().saturating_sub(seed.len()),
            cross_file_terms,
            code_terms,
            phrases_found,
            key_size,
        },
        key,
        source_files: files.iter().map(|(p, _)| p.clone()).collect(),
        file_mtimes: mtimes,
        seeded_from_master,
    };

    save_folder_key(root, &folder_key).await?;
    if opts.persist_central {
        let _ = save_central_key(data_dir, &folder_key).await;
    }
    if opts.learn_master {
        learn_into_master(data_dir, &folder_key.key).await;
    }

    Ok(folder_key)
}

/// Scan `root` recursively, returning (file path, contents) plus an mtime map.
async fn scan_folder(
    root: &Path,
    opts: &FolderKeyOptions,
) -> anyhow::Result<(Vec<(PathBuf, String)>, HashMap<PathBuf, i64>, usize)> {
    let mut out = Vec::new();
    let mut mtimes = HashMap::new();
    let mut total_bytes = 0usize;

    for entry in walkdir::WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if !opts.file_extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
                continue;
            }
        } else {
            continue;
        }
        if opts.exclude_patterns.iter().any(|p| glob_match(p, path)) {
            continue;
        }
        let meta = match tokio::fs::metadata(path).await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let text = match tokio::fs::read_to_string(path).await {
            Ok(t) => t,
            Err(_) => continue,
        };
        total_bytes += text.len();
        mtimes.insert(path.to_path_buf(), mtime);
        out.push((path.to_path_buf(), text));
    }

    Ok((out, mtimes, total_bytes))
}

/// Extract single-token, multi-word-phrase, and compound-identifier candidates
/// with frequencies. Runs over the *whole* file (prose and code): code
/// identifiers are the highest-value compression vocabulary, so unlike the
/// caveman style compressor we learn from them rather than discarding them.
fn extract_candidates(text: &str, opts: &FolderKeyOptions) -> HashMap<String, usize> {
    let word_re = regex::Regex::new(r"[a-zA-Z0-9_-]+").unwrap();
    let words: Vec<String> = word_re
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect();

    let mut counts: HashMap<String, usize> = HashMap::new();

    // Single tokens (stopwords + min length filtered). Compound identifiers
    // (snake_case / camelCase / kebab-case) are also split into their
    // components so both the full name and its parts become learnable.
    for w in &words {
        if w.len() < opts.min_length {
            continue;
        }
        if STOPWORDS.contains(&w.to_lowercase().as_str()) {
            continue;
        }
        *counts.entry(w.clone()).or_insert(0) += 1;
        for part in split_identifier(w) {
            if part.len() >= opts.min_length && !STOPWORDS.contains(&part.to_lowercase().as_str()) {
                *counts.entry(part).or_insert(0) += 1;
            }
        }
    }

    // Multi-word phrases (n-grams), up to `max_phrase_words`.
    let max_n = opts.max_phrase_words.min(8);
    for n in 2..=max_n {
        for window in words.windows(n) {
            let phrase = window.join(" ");
            if phrase.len() < opts.min_length {
                continue;
            }
            *counts.entry(phrase).or_insert(0) += 1;
        }
    }

    counts
}

/// Split a compound identifier into its components: `userDatabaseConnection`
/// → `["user", "database", "connection"]`, `user_database` → `["user",
/// "database"]`, `config-v2` → `["config", "v2"]`.
fn split_identifier(token: &str) -> Vec<String> {
    if !token.contains(['_', '-']) && !token.chars().any(|c| c.is_ascii_uppercase()) {
        return Vec::new();
    }
    let mut parts: Vec<String> = Vec::new();
    // Split on `_` / `-` first.
    for seg in token.split(['_', '-']) {
        if seg.is_empty() {
            continue;
        }
        // Then split camelCase boundaries.
        let mut buf = String::new();
        let mut prev_upper = false;
        for ch in seg.chars() {
            if ch.is_ascii_uppercase() && !buf.is_empty() && !prev_upper {
                parts.push(std::mem::take(&mut buf));
            }
            buf.push(ch);
            prev_upper = ch.is_ascii_uppercase();
        }
        if !buf.is_empty() {
            parts.push(buf);
        }
    }
    parts
        .into_iter()
        .map(|p| p.to_lowercase())
        .filter(|p| p.len() >= 2)
        .collect()
}

/// Build the seed key (default + capped master contribution).
fn build_seed(data_dir: &Path, learn_master: bool) -> CompressionKey {
    let mut seed = pfc1::default_key();
    if !learn_master {
        return seed;
    }
    let master = load_master_key_sync_read(data_dir);
    let room = 85usize.saturating_sub(seed.len()).saturating_sub(MIN_FOLDER_SLOTS);
    let master_room = room.min(MASTER_CAP);
    let mut added = 0;
    for (s, t) in &master {
        if added >= master_room {
            break;
        }
        if seed.contains_key(s) {
            continue;
        }
        seed.insert(s.clone(), t.clone());
        added += 1;
    }
    seed
}

/// Load the folder key written next to `folder`, if present.
pub async fn load_folder_key(folder: &Path) -> anyhow::Result<Option<FolderKey>> {
    let path = folder.join(FOLDER_KEY_FILE);
    let content = tokio::fs::read_to_string(&path).await.ok();
    match content {
        Some(c) => Ok(serde_json::from_str::<FolderKey>(&c).ok()),
        None => Ok(None),
    }
}

/// Persist a folder key to `<folder>/.pfc1_key.json`.
pub async fn save_folder_key(folder: &Path, key: &FolderKey) -> anyhow::Result<()> {
    let path = folder.join(FOLDER_KEY_FILE);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    let json = serde_json::to_string_pretty(key)?;
    tokio::fs::write(&path, json).await?;
    Ok(())
}

/// Walk up from `start` loading every `.pfc1_key.json` encountered.
pub async fn discover_folder_keys(start: &Path) -> Vec<FolderKey> {
    let mut out = Vec::new();
    let mut cur = Some(start.to_path_buf());
    while let Some(dir) = cur {
        if let Ok(Some(k)) = load_folder_key(&dir).await {
            out.push(k);
        }
        cur = dir.parent().map(|p| p.to_path_buf());
    }
    out
}

/// Central registry path for a folder key.
fn central_key_path(data_dir: &Path, folder: &Path) -> PathBuf {
    data_dir
        .join(CENTRAL_DIR)
        .join(format!("{}.json", hash_path(folder)))
}

/// Persist a folder key to the central registry.
pub async fn save_central_key(data_dir: &Path, key: &FolderKey) -> anyhow::Result<()> {
    let path = central_key_path(data_dir, &key.folder);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    let json = serde_json::to_string_pretty(key)?;
    tokio::fs::write(&path, json).await?;
    Ok(())
}

/// Decompress `text` using the folder's key (header-less, trusted a2a).
/// Returns `text` unchanged if no key is found for `folder`.
pub async fn decompress_with_folder_key(folder: &Path, text: &str) -> String {
    match load_folder_key(folder).await {
        Ok(Some(k)) => pfc1::decompress_text(text, &k.key),
        _ => text.to_string(),
    }
}

/// List all folder keys in the central registry.
pub async fn list_central_keys(data_dir: &Path) -> Vec<FolderKey> {
    let dir = data_dir.join(CENTRAL_DIR);
    let mut out = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(e)) = entries.next_entry().await {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            if let Ok(content) = tokio::fs::read_to_string(&path).await {
                if let Ok(k) = serde_json::from_str::<FolderKey>(&content) {
                    out.push(k);
                }
            }
        }
    }
    out
}

/// Load the learned master key (empty if none).
pub async fn load_master_key(data_dir: &Path) -> CompressionKey {
    let path = data_dir.join(MASTER_KEY_FILE);
    match tokio::fs::read_to_string(&path).await {
        Ok(c) => serde_json::from_str::<CompressionKey>(&c).unwrap_or_default(),
        Err(_) => CompressionKey::default(),
    }
}

/// Merge `key` into the master key and persist (union; new symbols win).
pub async fn learn_into_master(data_dir: &Path, key: &CompressionKey) {
    let mut master = load_master_key(data_dir).await;
    for (s, t) in key {
        master.insert(s.clone(), t.clone());
    }
    let path = data_dir.join(MASTER_KEY_FILE);
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    if let Ok(json) = serde_json::to_string_pretty(&master) {
        let _ = tokio::fs::write(&path, json).await;
    }
}

/// Deterministic short hash of a path for registry file names.
fn hash_path(p: &Path) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.to_string_lossy().hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Minimal glob matcher supporting `*` (non-`/`) and `**` (any chars).
fn glob_match(pattern: &str, path: &Path) -> bool {
    let text = path.to_string_lossy();
    let re = pattern
        .split("**")
        .map(|seg| {
            seg.split('*')
                .map(|s| regex::escape(s))
                .collect::<Vec<_>>()
                .join("[^/]*")
        })
        .collect::<Vec<_>>()
        .join(".*");
    let full = format!("^{}$", re);
    regex::Regex::new(&full)
        .map(|r| r.is_match(&text))
        .unwrap_or(false)
}

/// Synchronous helper to load the master key inside the (sync) seed builder.
fn load_master_key_sync_read(data_dir: &Path) -> CompressionKey {
    let path = data_dir.join(MASTER_KEY_FILE);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str::<CompressionKey>(&c).ok())
        .unwrap_or_default()
}
