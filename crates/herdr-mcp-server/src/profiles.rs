//! Agent profiles: bundled defaults (compiled in from `profiles/*.toml`)
//! overlaid by user files in `data/profiles/{id}.toml`. A user file with a
//! bundled id overrides it; deleting the user file restores the default.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use herdr_mcp_api::AgentProfile;

const BUNDLED: &[&str] = &[
    include_str!("../profiles/claude.toml"),
    include_str!("../profiles/codex.toml"),
    include_str!("../profiles/opencode.toml"),
    include_str!("../profiles/gemini.toml"),
    include_str!("../profiles/hermes.toml"),
    include_str!("../profiles/zeroclaw.toml"),
    include_str!("../profiles/aider.toml"),
    include_str!("../profiles/shell.toml"),
];

fn bundled() -> impl Iterator<Item = AgentProfile> {
    BUNDLED.iter().filter_map(|src| match toml::from_str(src) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!("bundled profile does not parse: {e}");
            None
        }
    })
}

fn user_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("profiles")
}

/// Everything wrong with a profile (empty = usable).
pub fn problems(p: &AgentProfile) -> Vec<String> {
    let mut out = p.problems();
    if let Some(re) = &p.ready_regex
        && let Err(e) = regex::Regex::new(re)
    {
        out.push(format!("ready_regex: {e}"));
    }
    out
}

/// Every profile by id: bundled, then user files on top.
pub async fn list(data_dir: &Path) -> Vec<AgentProfile> {
    let mut by_id: BTreeMap<String, AgentProfile> = bundled().map(|p| (p.id.clone(), p)).collect();
    if let Ok(mut entries) = tokio::fs::read_dir(user_dir(data_dir)).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "toml") {
                continue;
            }
            let Ok(src) = tokio::fs::read_to_string(&path).await else {
                continue;
            };
            match toml::from_str::<AgentProfile>(&src) {
                Ok(p) if problems(&p).is_empty() => {
                    by_id.insert(p.id.clone(), p);
                }
                Ok(p) => tracing::warn!("{}: {}", path.display(), problems(&p).join("; ")),
                Err(e) => tracing::warn!("{}: {e}", path.display()),
            }
        }
    }
    by_id.into_values().collect()
}

pub async fn get(data_dir: &Path, id: &str) -> Option<AgentProfile> {
    list(data_dir).await.into_iter().find(|p| p.id == id)
}

/// Validate and write `data/profiles/{id}.toml`.
pub async fn save(data_dir: &Path, profile: &AgentProfile) -> Result<(), SaveError> {
    let problems = problems(profile);
    if !problems.is_empty() {
        return Err(SaveError::Invalid(problems));
    }
    let dir = user_dir(data_dir);
    let src = toml::to_string_pretty(profile).map_err(|e| SaveError::Io(e.into()))?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| SaveError::Io(e.into()))?;
    // `problems` guarantees the id is [a-z0-9_-]+, so this stays in `dir`.
    crate::persistence::write_atomic(dir.join(format!("{}.toml", profile.id)), src)
        .await
        .map_err(|e| SaveError::Io(e.into()))
}

#[derive(Debug)]
pub enum SaveError {
    Invalid(Vec<String>),
    Io(anyhow::Error),
}

/// Remove the user file for `id`. Returns false if there was none (a
/// bundled profile can't be deleted, only overridden).
pub async fn delete(data_dir: &Path, id: &str) -> anyhow::Result<bool> {
    if !id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Ok(false);
    }
    let path = user_dir(data_dir).join(format!("{id}.toml"));
    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Ok(false);
    }
    tokio::fs::remove_file(path).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_profile_parses_and_is_valid() {
        assert_eq!(bundled().count(), BUNDLED.len());
        for p in bundled() {
            assert!(problems(&p).is_empty(), "{}: {:?}", p.id, problems(&p));
        }
    }

    #[tokio::test]
    async fn user_files_override_and_delete_restores() {
        let tmp = tempfile::tempdir().unwrap();
        let bundled_claude = get(tmp.path(), "claude").await.unwrap();
        assert_eq!(bundled_claude.kind.as_deref(), Some("claude"));

        let mut mine = bundled_claude.clone();
        mine.args = vec!["--model".into(), "opus".into()];
        save(tmp.path(), &mine).await.unwrap();
        assert_eq!(get(tmp.path(), "claude").await.unwrap(), mine);
        let count = list(tmp.path()).await.len();

        let mut custom = get(tmp.path(), "shell").await.unwrap();
        custom.id = "python".into();
        custom.command = Some("python3".into());
        save(tmp.path(), &custom).await.unwrap();
        assert_eq!(list(tmp.path()).await.len(), count + 1);

        assert!(delete(tmp.path(), "claude").await.unwrap());
        assert_eq!(get(tmp.path(), "claude").await.unwrap(), bundled_claude);
        assert!(
            !delete(tmp.path(), "claude").await.unwrap(),
            "bundled stays"
        );
        assert!(!delete(tmp.path(), "../x").await.unwrap());
    }

    #[tokio::test]
    async fn invalid_profiles_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let mut p = get(tmp.path(), "shell").await.unwrap();
        p.kind = Some("claude".into()); // both kind and command
        p.ready_regex = Some("(".into());
        p.id = "../evil".into();
        let Err(SaveError::Invalid(errs)) = save(tmp.path(), &p).await else {
            panic!("saved an invalid profile");
        };
        assert_eq!(errs.len(), 3, "{errs:?}");
        assert!(!tmp.path().join("evil.toml").exists());
    }
}
