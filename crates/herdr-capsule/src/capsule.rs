use crate::{CapsuleConfig, Error, Result, fail, home_dir};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
pub const KINDS: [&str; 9] = [
    "file", "dir", "log", "branch", "commit", "run", "pane", "url", "capsules",
];
pub const STATUSES: [&str; 5] = ["done", "fail", "block", "wip", "task"];
pub const SKINS: [&str; 3] = ["cap", "raw", "glyph"];
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Capsule {
    pub v: u32,
    pub id: String,
    pub task: String,
    pub from: String,
    pub to: String,
    pub status: String,
    pub summary: String,
    pub artifacts: Vec<Artifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub re: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_check: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_check_files: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollup: Option<Rollup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<Check>,
    pub pane: Option<String>,
    pub at: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Artifact {
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Rollup {
    pub count: usize,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Check {
    pub cmd: String,
    pub exit: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<Vec<String>>,
}
impl Capsule {
    pub fn from_value(value: &serde_json::Value) -> Result<Self> {
        serde_json::from_value(value.clone()).map_err(Error::from)
    }
}
/// Python's `json.dumps(c, indent=1, ensure_ascii=False)` — one space, not two.
pub fn json_pretty(cap: &Capsule) -> Result<String> {
    let mut buf = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
    serde::Serialize::serialize(cap, &mut ser)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}
pub fn normalize_summary(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
pub fn no_control_chars(fields: &[(&str, &str)], specs: &[String]) -> Result<()> {
    for (name, value) in fields {
        let v = if *name == "summary" {
            normalize_summary(value)
        } else {
            (*value).to_string()
        };
        if v.chars()
            .any(|c| (c as u32) < 32 || c == '\u{7f}' || ((c as u32) >= 0x80 && (c as u32) < 0xa0))
        {
            return Err(fail(format!(
                "--{} has a control character",
                name.replace('_', "-")
            )));
        }
    }
    if specs
        .iter()
        .any(|s| s.chars().any(|c| (c as u32) < 32 || c == '\u{7f}'))
    {
        return Err(fail("an --artifact has a control character"));
    }
    Ok(())
}
pub(crate) fn expanduser(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        home_dir().join(rest)
    } else if p == "~" {
        home_dir()
    } else {
        PathBuf::from(p)
    }
}
fn est_tokens(chars: usize) -> usize {
    let (q, r) = (chars / 4, chars % 4);
    let n = match r {
        0 | 1 => q,
        2 if q % 2 == 0 => q,
        2 => q + 1,
        _ => q + 1,
    };
    n.max(1)
}
pub fn sha8(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize())[..8].to_string())
}
pub fn size(path: &Path) -> Result<String> {
    if path.is_dir() {
        let mut count = 0usize;
        let mut stack = vec![path.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let ty = entry.file_type()?;
                if ty.is_dir() {
                    stack.push(entry.path());
                } else if ty.is_file() {
                    count += 1;
                }
            }
        }
        return Ok(format!("{count} files"));
    }
    let raw = fs::read(path)?;
    match String::from_utf8(raw.clone()) {
        Ok(text) => Ok(format!(
            "{}L ~{}t",
            text.matches('\n').count(),
            est_tokens(text.chars().count())
        )),
        Err(_) => Ok(format!("{}B bin", raw.len())),
    }
}
pub fn artifact(spec: &str, cfg: &CapsuleConfig) -> Result<Artifact> {
    let (kind, rest) = match spec.split_once('=') {
        Some((k, r)) if !k.is_empty() && KINDS.contains(&k) && !r.is_empty() => (k, r),
        _ => {
            return Err(fail(format!(
                "--artifact {}: want KIND=REF[::note], KIND: {}",
                crate::py_repr(spec),
                KINDS.join(" ")
            )));
        }
    };
    let (reference, note) = match rest.split_once("::") {
        Some((r, n)) => (r, n),
        None => (rest, ""),
    };
    let mut out = Artifact {
        kind: kind.to_string(),
        reference: reference.to_string(),
        ..Artifact::default()
    };
    if kind == "file" || kind == "dir" || kind == "log" || kind == "capsules" {
        let p = expanduser(reference);
        if !p.exists() {
            return Err(fail(format!(
                "--artifact {kind}={reference}: does not exist"
            )));
        }
        if kind == "file" || kind == "log" {
            if fs::symlink_metadata(&p)?.file_type().is_symlink() {
                return Err(fail(format!(
                    "--artifact {kind}={reference}: symlink; name the file itself"
                )));
            }
            let nbytes = p.metadata()?.len();
            if nbytes > cfg.artifact_max_bytes {
                return Err(fail(format!(
                    "--artifact {kind}={reference}: too large ({nbytes} > {} bytes); link to a smaller file",
                    cfg.artifact_max_bytes
                )));
            }
        }
        let resolved = p.canonicalize()?;
        let home = cfg.home.canonicalize().unwrap_or_else(|_| cfg.home.clone());
        let own_log = resolved
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n == "logs")
            && resolved.starts_with(&home);
        let mut temps: Vec<PathBuf> = ["/tmp", "/var/tmp"].iter().map(PathBuf::from).collect();
        if let Some(t) = std::env::var_os("TMPDIR") {
            temps.push(PathBuf::from(t));
        }
        if temps
            .iter()
            .any(|t| resolved.starts_with(t.canonicalize().unwrap_or_else(|_| t.clone())))
            && !own_log
        {
            return Err(fail(format!(
                "--artifact {kind}={reference}: temp dirs are wiped on reboot; put work under CAPSULE_WORK ({})",
                cfg.work.display()
            )));
        }
        out.reference = resolved.display().to_string();
        out.size = Some(size(&p)?);
        if p.is_file() {
            out.sha = Some(sha8(&p)?);
        }
    }
    if !note.trim().is_empty() {
        out.note = Some(note.split_whitespace().collect::<Vec<_>>().join(" "));
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_whitespace_in_summaries() {
        assert_eq!(normalize_summary("  a\t b\n c "), "a b c");
    }
    #[test]
    fn refuses_control_characters_but_collapses_tabs() {
        let ok = [("summary", "a\tb")];
        assert!(no_control_chars(&ok, &[]).is_ok());
        let bad = [("summary", "a\u{1b}b")];
        assert!(no_control_chars(&bad, &[]).is_err());
        let bad = [("task", "a\u{7}b")];
        assert!(no_control_chars(&bad, &[]).is_err());
    }
}
