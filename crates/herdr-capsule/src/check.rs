use crate::{Result, Store, fail};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, process::Command};
const DECISIVE: &str = "error,fail,panic,warning,assert,expected,exception,denied,not found";
fn shlex_split(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut open = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' => {
                open = true;
                let quote = c;
                while let Some(&d) = chars.peek() {
                    chars.next();
                    if d == quote {
                        break;
                    }
                    if quote == '"'
                        && d == '\\'
                        && let Some(&e) = chars.peek()
                    {
                        chars.next();
                        current.push(e);
                    } else {
                        current.push(d);
                    }
                }
            }
            '\\' => {
                open = true;
                if let Some(e) = chars.next() {
                    current.push(e);
                }
            }
            c if c.is_whitespace() => {
                if open {
                    words.push(std::mem::take(&mut current));
                    open = false;
                }
            }
            c => {
                open = true;
                current.push(c);
            }
        }
    }
    if open || !current.is_empty() {
        words.push(current);
    }
    words
}
pub fn check_files(cmd: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for word in shlex_split(cmd) {
        let path = super::capsule::expanduser(&word);
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            return Err(fail(format!(
                "check file {word} is a symlink; name the file itself"
            )));
        }
        if path.is_file() {
            let bytes = fs::read(&path)?;
            out.insert(
                path.canonicalize()?.display().to_string(),
                format!("{:x}", Sha256::digest(&bytes)),
            );
        }
    }
    Ok(out)
}
pub fn decisive(lines: &[String]) -> Vec<String> {
    let lower: Vec<String> = lines.iter().map(|l| l.to_lowercase()).collect();
    let hits: Vec<String> = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| DECISIVE.split(',').any(|p| lower[*i].contains(p)))
        .map(|(_, l)| l.clone())
        .collect();
    let from = if hits.is_empty() { lines } else { &hits };
    from.iter().rev().take(12).rev().cloned().collect()
}
pub fn run_check(cmd: &str) -> Result<(i32, String)> {
    let out = Command::new("sh").arg("-c").arg(cmd).output()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok((out.status.code().unwrap_or(-1), text))
}
pub fn reply_check_of(
    store: &mut Store,
    re_id: &str,
    given: Option<&str>,
) -> Result<Option<String>> {
    let task = store.find_capsule(re_id)?.filter(|t| t.status == "task");
    let Some(task) = task else {
        return Ok(given.map(str::to_string));
    };
    let Some(fixed) = task.reply_check.as_deref() else {
        return Ok(given.map(str::to_string));
    };
    if let Some(g) = given
        && g != fixed
    {
        return Err(fail(format!(
            "task {re_id} fixes its check; drop --check (it runs: {fixed})"
        )));
    }
    let mut changed = Vec::new();
    for (path, hash) in task.reply_check_files.unwrap_or_default() {
        let now = check_files(&path)?;
        if now.get(&path).map(String::as_str) != Some(hash.as_str()) {
            changed.push(path);
        }
    }
    if !changed.is_empty() {
        return Err(fail(format!(
            "the check of task {re_id} changed since it was sent: {}; send --status block and say why",
            changed.join(", ")
        )));
    }
    Ok(Some(fixed.to_string()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decisive_keeps_the_last_error_lines() {
        let lines: Vec<String> = ["ok", "first error", "tail"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(decisive(&lines), ["first error"]);
    }
    #[test]
    fn shlex_split_handles_quotes() {
        assert_eq!(shlex_split("sh '/x/y z'"), ["sh", "/x/y z"]);
    }
}
