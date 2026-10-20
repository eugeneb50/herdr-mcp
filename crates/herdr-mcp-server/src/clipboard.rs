//! System clipboard for the `clipboard_set` / `clipboard_get` tools: the
//! configured `[clipboard]` commands when both are set, else the OS clipboard
//! through arboard (macOS, Windows, X11, and Wayland compositors with the
//! data-control protocol, falling back to XWayland).

use std::sync::{Mutex, PoisonError};

use anyhow::{Context, Result, bail};
use herdr_mcp_core::ClipboardConfig;

/// This process's OS clipboard handle, opened on first use. It has to outlive
/// each call: on X11 and Wayland this process serves the copied text for as
/// long as the handle lives. Statics are never dropped, so [`release`] drops
/// it at exit (arboard then hands the text to a clipboard manager).
static NATIVE: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);

/// Drop the OS clipboard handle so copied text outlives the process. Blocks
/// while arboard hands it over; call once, on the way out.
pub fn release() {
    drop(NATIVE.lock().unwrap_or_else(PoisonError::into_inner).take());
}

/// Write `text` to the clipboard.
pub async fn set(cfg: &ClipboardConfig, text: String) -> Result<()> {
    match cfg.commands() {
        Some((copy, _)) => run_copy(copy, &text).await,
        None => tokio::task::spawn_blocking(move || native(|cb| cb.set_text(text))).await?,
    }
}

/// The clipboard's text; empty when it holds no text.
pub async fn get(cfg: &ClipboardConfig) -> Result<String> {
    match cfg.commands() {
        Some((_, paste)) => run_paste(paste).await,
        None => {
            tokio::task::spawn_blocking(|| {
                native(|cb| match cb.get_text() {
                    Err(arboard::Error::ContentNotAvailable) => Ok(String::new()),
                    other => other,
                })
            })
            .await?
        }
    }
}

/// Run `f` on the process's clipboard handle (blocking: call off the runtime).
fn native<T>(f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>) -> Result<T> {
    let mut slot = NATIVE.lock().unwrap_or_else(PoisonError::into_inner);
    let cb = match slot.take() {
        Some(cb) => cb,
        None => arboard::Clipboard::new()
            .context("no OS clipboard (no display?); set [clipboard] commands instead")?,
    };
    Ok(f(slot.insert(cb))?)
}

/// `cmd` split on whitespace into argv (no shell).
fn command(cmd: &str) -> Result<tokio::process::Command> {
    let mut argv = cmd.split_whitespace();
    let bin = argv.next().context("empty clipboard command")?;
    let mut c = tokio::process::Command::new(bin);
    c.args(argv);
    Ok(c)
}

/// Pipe `text` into the copy command. Its stdout/stderr are not captured:
/// `xclip` leaves a child holding them open to serve the selection, so
/// reading them would wait until something else is copied.
async fn run_copy(cmd: &str, text: &str) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut child = command(cmd)?
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("running `{cmd}`"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).await?;
    }
    let status = child.wait().await?;
    if !status.success() {
        bail!("`{cmd}` failed ({status})");
    }
    Ok(())
}

/// The paste command's stdout.
async fn run_paste(cmd: &str) -> Result<String> {
    let out = command(cmd)?
        .output()
        .await
        .with_context(|| format!("running `{cmd}`"))?;
    if !out.status.success() {
        bail!(
            "`{cmd}` failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(copy: &str, paste: &str) -> ClipboardConfig {
        ClipboardConfig {
            copy_command: Some(copy.into()),
            paste_command: Some(paste.into()),
        }
    }

    #[tokio::test]
    async fn configured_commands_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let clip = tmp.path().join("clip.txt");
        let cfg = commands(
            &format!("tee {}", clip.display()),
            &format!("cat {}", clip.display()),
        );
        set(&cfg, "hello clipboard".into()).await.unwrap();
        assert_eq!(std::fs::read_to_string(&clip).unwrap(), "hello clipboard");
        assert_eq!(get(&cfg).await.unwrap(), "hello clipboard");
    }

    #[tokio::test]
    async fn failing_copy_command_is_an_error() {
        let err = set(&commands("false", "true"), "x".into())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("`false` failed"), "{err}");
        assert!(
            set(&commands("no-such-clipboard-tool", "true"), "x".into())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn failing_paste_command_is_an_error() {
        let err = get(&commands("true", "false")).await.unwrap_err();
        assert!(err.to_string().contains("`false` failed"), "{err}");
    }
}
