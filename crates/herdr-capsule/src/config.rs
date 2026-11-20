use crate::{Error, Result, home_dir};
use std::{env, path::PathBuf, str::FromStr, time::Duration};
#[derive(Debug, Clone)]
pub struct CapsuleConfig {
    pub summary_max: usize,
    pub artifact_max_bytes: u64,
    pub reply_wait: Duration,
    pub submit_confirm: Duration,
    pub deliver_every: Duration,
    pub work: PathBuf,
    pub max_attempts: u32,
    pub home: PathBuf,
    pub queue_db: PathBuf,
    pub herdr: Option<PathBuf>,
    pub skin: Option<String>,
    pub exe: String,
    pub workspace: Option<String>,
    pub pane: Option<String>,
    pub from: Option<String>,
    pub now: Option<String>,
}
impl Default for CapsuleConfig {
    fn default() -> Self {
        let home = env::var_os("CAPSULE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join("code/herdr-mcp-refs/mailbox"));
        Self {
            summary_max: 120,
            artifact_max_bytes: 10_485_760,
            reply_wait: Duration::from_millis(1_800_000),
            submit_confirm: Duration::from_millis(5_000),
            deliver_every: Duration::from_millis(2_000),
            work: env::var_os("CAPSULE_WORK")
                .map(PathBuf::from)
                .unwrap_or_else(|| home_dir().join("obsidian/herdrmcp/work")),
            max_attempts: 2,
            home,
            queue_db: env::var_os("HERDR_QUEUE_DB")
                .map(PathBuf::from)
                .unwrap_or_else(|| home_dir().join("code/herdr-mcp/data/queue.db")),
            herdr: env::var_os("HERDR_BIN_PATH").map(PathBuf::from),
            skin: env::var("CAPSULE_SKIN").ok(),
            exe: std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "capsule".to_string()),
            workspace: env::var("HERDR_WORKSPACE_ID").ok(),
            pane: env::var("HERDR_PANE_ID").ok(),
            from: env::var("CAPSULE_FROM").ok(),
            now: env::var("CAPSULE_NOW").ok(),
        }
    }
}
fn parsed<T: FromStr>(name: &str) -> Result<Option<T>> {
    match env::var(name) {
        Ok(v) => v
            .parse()
            .map(Some)
            .map_err(|_| Error::Msg(format!("capsule: bad {name}={v}"))),
        Err(_) => Ok(None),
    }
}
impl CapsuleConfig {
    pub fn from_env() -> Result<Self> {
        let mut c = Self::default();
        if let Some(v) = parsed("CAPSULE_SUMMARY_MAX")? {
            c.summary_max = v;
        }
        if let Some(v) = parsed("CAPSULE_ARTIFACT_MAX_BYTES")? {
            c.artifact_max_bytes = v;
        }
        let reply_wait: Option<f64> = parsed("CAPSULE_REPLY_WAIT_S")?;
        if let Some(v) = reply_wait {
            c.reply_wait = Duration::from_millis((v * 1000.0) as u64);
        }
        let submit_confirm: Option<f64> = parsed("CAPSULE_SUBMIT_CONFIRM_S")?;
        if let Some(v) = submit_confirm {
            c.submit_confirm = Duration::from_millis((v * 1000.0) as u64);
        }
        Ok(c)
    }
    pub fn reply_wait_s(&self) -> f64 {
        self.reply_wait.as_secs_f64()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_match_the_contract() {
        let c = CapsuleConfig::default();
        assert_eq!(c.summary_max, 120);
        assert_eq!(c.artifact_max_bytes, 10_485_760);
        assert_eq!(c.reply_wait.as_millis(), 1_800_000);
        assert_eq!(c.submit_confirm.as_millis(), 5_000);
        assert_eq!(c.deliver_every.as_millis(), 2_000);
        assert_eq!(c.max_attempts, 2);
    }
}
