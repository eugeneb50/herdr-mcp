//! `herdr-mcp plugin` — herdr-mcp as a herdr plugin.
//!
//! `plugin link` writes a `herdr-plugin.toml` (generated from [`manifest`],
//! pointing at this binary) and registers it with `herdr plugin link`. herdr
//! then offers the actions below in its action menu and keybindings:
//!
//! - `recipes`: a popup that runs a saved recipe with the invocation context
//!   (herdr's `HERDR_PLUGIN_CONTEXT_JSON`: `workspace_id`, `selected_text`, …,
//!   plus `pane_id` = the focused pane) as `{{herdr.…}}`;
//! - `dashboard`: the TUI dashboard in a split;
//! - `bridge`: the HTTP bridge in its own tab;
//! - `web`: the web playground in a browser.
//!
//! Actions run with no terminal, so each one that needs a screen opens one of
//! the plugin's panes (`plugin open <pane>`), passing herdr's context along.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde::Serialize;
use serde_json::Value;

use herdr_mcp_api::{ApiClient, RunEventKind, StartRunRequest, tool_text};

pub const PLUGIN_ID: &str = "herdr-mcp";
/// Env var carrying the invoking action's context into a plugin pane.
const CONTEXT_ENV: &str = "HERDR_MCP_CONTEXT";
const DEFAULT_PORT: u16 = 7676;

#[derive(Subcommand)]
pub enum PluginCommand {
    /// Write the plugin manifest and register it with herdr.
    Link {
        /// Where to write `herdr-plugin.toml` (default: the herdr-mcp config dir).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Print the manifest instead of writing and linking it.
        #[arg(long)]
        print: bool,
    },
    /// Unregister the plugin from herdr.
    Unlink,
    /// Open one of the plugin's panes with the invoking action's context.
    #[command(hide = true)]
    Open { pane: String },
    /// The recipe picker (the `recipes` pane).
    #[command(hide = true)]
    Recipes,
    /// Open the web playground in a browser.
    #[command(hide = true)]
    Web,
}

#[derive(Serialize)]
struct Manifest {
    id: &'static str,
    name: &'static str,
    version: &'static str,
    min_herdr_version: &'static str,
    description: &'static str,
    platforms: Vec<&'static str>,
    actions: Vec<Action>,
    panes: Vec<Pane>,
}

#[derive(Serialize)]
struct Action {
    id: &'static str,
    title: &'static str,
    contexts: Vec<&'static str>,
    command: Vec<String>,
}

#[derive(Serialize)]
struct Pane {
    id: &'static str,
    title: &'static str,
    placement: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<&'static str>,
    command: Vec<String>,
}

/// The plugin as herdr sees it. `exe` is this binary, `trim` the sibling
/// `herdr-trim` (the dashboard; skipped when absent), and the bridge pane
/// serves `data_dir` on `port` — both resolved from config at link time, so
/// re-run `plugin link` after changing them.
fn manifest(exe: &Path, trim: Option<&Path>, port: u16, data_dir: &Path) -> Manifest {
    let exe = exe.display().to_string();
    let us = |args: &[&str]| {
        std::iter::once(exe.clone())
            .chain(args.iter().map(|a| a.to_string()))
            .collect::<Vec<_>>()
    };
    let everywhere = || vec!["global", "workspace", "tab", "pane", "selection"];
    let mut actions = vec![
        Action {
            id: "recipes",
            title: "herdr-mcp: run a recipe",
            contexts: everywhere(),
            command: us(&["plugin", "open", "recipes"]),
        },
        Action {
            id: "bridge",
            title: "herdr-mcp: start the bridge",
            contexts: vec!["global", "workspace"],
            command: us(&["plugin", "open", "bridge"]),
        },
        Action {
            id: "web",
            title: "herdr-mcp: open the web playground",
            contexts: vec!["global", "workspace"],
            command: us(&["plugin", "web"]),
        },
    ];
    let mut panes = vec![
        Pane {
            id: "recipes",
            title: "Run a recipe",
            placement: "popup",
            width: Some("80%"),
            height: Some("70%"),
            command: us(&["plugin", "recipes"]),
        },
        Pane {
            id: "bridge",
            title: "herdr-mcp bridge",
            placement: "tab",
            width: None,
            height: None,
            command: us(&[
                "serve",
                "--http",
                &port.to_string(),
                "--http-only",
                "--data-dir",
                &data_dir.display().to_string(),
            ]),
        },
    ];
    if let Some(trim) = trim {
        actions.push(Action {
            id: "dashboard",
            title: "herdr-mcp: open the dashboard",
            contexts: vec!["global", "workspace", "pane"],
            command: us(&["plugin", "open", "dashboard"]),
        });
        panes.push(Pane {
            id: "dashboard",
            title: "herdr-mcp dashboard",
            placement: "split",
            width: None,
            height: None,
            command: vec![
                trim.display().to_string(),
                "dashboard".into(),
                "--http-port".into(),
                port.to_string(),
                "--data-dir".into(),
                data_dir.display().to_string(),
            ],
        });
    }
    Manifest {
        id: PLUGIN_ID,
        name: "herdr-mcp",
        version: env!("CARGO_PKG_VERSION"),
        min_herdr_version: "0.9.0",
        description: "Recipes, triggers, dashboard and web playground for herdr",
        platforms: vec!["linux", "macos"],
        actions,
        panes,
    }
}

pub async fn run(cmd: PluginCommand, config: &herdr_mcp_core::Config) -> Result<()> {
    let port = config.http.port.unwrap_or(DEFAULT_PORT);
    match cmd {
        PluginCommand::Link { dir, print } => link(dir, print, port, &config.data_dir),
        PluginCommand::Unlink => herdr(&["plugin", "unlink", PLUGIN_ID]),
        PluginCommand::Open { pane } => open_pane(&pane),
        PluginCommand::Recipes => {
            let result = recipes(port).await;
            if let Err(e) = &result {
                println!("\n{e:#}");
            }
            pause("\nPress Enter to close.");
            Ok(())
        }
        PluginCommand::Web => web(port).await,
    }
}

fn link(dir: Option<PathBuf>, print: bool, port: u16, data_dir: &Path) -> Result<()> {
    let exe = std::env::current_exe()?.canonicalize()?;
    let trim = exe.with_file_name("herdr-trim");
    let data_dir = std::path::absolute(data_dir)?;
    let text = toml::to_string(&manifest(
        &exe,
        trim.exists().then_some(trim.as_path()),
        port,
        &data_dir,
    ))?;
    if print {
        print!("{text}");
        return Ok(());
    }
    let dir = match dir {
        Some(dir) => dir,
        None => dirs::config_dir()
            .context("no config directory")?
            .join("herdr-mcp/herdr-plugin"),
    };
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("herdr-plugin.toml"), text)?;
    // Re-linking replaces an earlier registration (new binary path, port).
    let _ = herdr_quiet(&["plugin", "unlink", PLUGIN_ID]);
    herdr(&["plugin", "link", &dir.display().to_string()])?;
    println!(
        "\nLinked {PLUGIN_ID} from {}. Bind its actions in herdr's config, e.g.:\n\n\
         [[keys.command]]\nkey = \"prefix+r\"\ntype = \"plugin_action\"\n\
         command = \"{PLUGIN_ID}.recipes\"\ndescription = \"run a recipe\"",
        dir.display()
    );
    Ok(())
}

/// `herdr <args>`, output passed through; fails on a non-zero exit.
fn herdr(args: &[&str]) -> Result<()> {
    let status = std::process::Command::new(herdr_mcp_core::herdr_bin())
        .args(args)
        .status()
        .context("running herdr")?;
    if !status.success() {
        bail!("herdr {} failed ({status})", args.join(" "));
    }
    Ok(())
}

fn herdr_quiet(args: &[&str]) -> Result<std::process::Output> {
    Ok(std::process::Command::new(herdr_mcp_core::herdr_bin())
        .args(args)
        .output()?)
}

/// The invoking action's context: forwarded by [`open_pane`], else herdr's
/// own `HERDR_PLUGIN_CONTEXT_JSON`, else empty.
fn context() -> Value {
    [CONTEXT_ENV, "HERDR_PLUGIN_CONTEXT_JSON"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find_map(|v| serde_json::from_str::<Value>(&v).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Default::default()))
}

fn open_pane(pane: &str) -> Result<()> {
    let env = format!("{CONTEXT_ENV}={}", context());
    herdr(&[
        "plugin",
        "pane",
        "open",
        "--plugin",
        PLUGIN_ID,
        "--entrypoint",
        pane,
        "--env",
        &env,
    ])
}

fn notify(body: &str) {
    let _ = herdr_quiet(&["notification", "show", "herdr-mcp", "--body", body]);
}

fn pause(prompt: &str) {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let _ = std::io::stdin().lock().read_line(&mut String::new());
}

fn read_line(prompt: &str) -> String {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    line.trim().to_string()
}

async fn bridge(port: u16) -> Option<ApiClient> {
    let port = herdr_mcp_tui::http::discover_bridge(port).await?;
    herdr_mcp_tui::http::client_for(port).ok()
}

async fn web(port: u16) -> Result<()> {
    if bridge(port).await.is_none() {
        notify("The herdr-mcp bridge is not running: run \"herdr-mcp: start the bridge\" first.");
        return Ok(());
    }
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(format!("http://localhost:{port}"))
        .status()
        .with_context(|| format!("running {opener}"))?;
    Ok(())
}

/// The `recipes` popup: pick a saved recipe, run it with the context as
/// `herdr`, and follow the run to the end.
async fn recipes(port: u16) -> Result<()> {
    let mut ctx = context();
    // Name the focused pane like trigger events do, so one recipe serves
    // both a keybinding and a trigger (`{{herdr.pane_id}}`).
    if let Some(pane) = ctx.get("focused_pane_id").cloned()
        && let Some(map) = ctx.as_object_mut()
    {
        map.entry("pane_id").or_insert(pane);
    }
    let api = bridge(port).await.with_context(|| {
        format!("The herdr-mcp bridge is not running on port {port}: run \"herdr-mcp: start the bridge\".")
    })?;

    let mut saved = api.list_recipes().await?;
    saved.sort_by(|a, b| a.name.cmp(&b.name));
    if saved.is_empty() {
        bail!("No saved recipes yet: build one in the web playground (http://localhost:{port}).");
    }
    println!("Run a recipe\n");
    for (i, r) in saved.iter().enumerate() {
        let desc = r.description.as_deref().unwrap_or("");
        println!("  {:>2}  {}  \x1b[2m{desc}\x1b[0m", i + 1, r.name);
    }
    let choice = read_line(&format!("\nRecipe [1-{}], empty to cancel: ", saved.len()));
    let Some(recipe) = choice
        .parse::<usize>()
        .ok()
        .and_then(|n| saved.get(n.checked_sub(1)?))
    else {
        return Ok(());
    };

    let req = StartRunRequest {
        recipe_id: Some(recipe.id),
        session_id: ctx
            .get("workspace_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        variables: [("herdr".to_string(), ctx)].into(),
        ..Default::default()
    };
    let run = api.start_run(&req).await?;
    println!("\n{} — run {}\n", recipe.name, run.id);
    follow(&api, run.id).await
}

/// Print a run's steps as its events arrive, until it finishes.
async fn follow(api: &ApiClient, id: herdr_mcp_api::Uuid) -> Result<()> {
    let mut since = 0;
    loop {
        for event in api.run_events(id, since).await? {
            since = event.seq + 1;
            match event.kind {
                RunEventKind::StepStarted { step } => println!("▶ {step}"),
                RunEventKind::StepFinished { step, result } => {
                    println!("✔ {step}");
                    for line in tool_text(&result).lines().take(12) {
                        println!("    \x1b[2m{line}\x1b[0m");
                    }
                }
                RunEventKind::StepFailed { step, error, .. } => println!("✘ {step}: {error}"),
                RunEventKind::Status { status } if status.is_finished() => {
                    println!("\n{status:?}");
                    return Ok(());
                }
                _ => {}
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_points_every_command_at_this_install() {
        let m = manifest(
            Path::new("/opt/bin/herdr-mcp"),
            Some(Path::new("/opt/bin/herdr-trim")),
            7700,
            Path::new("/srv/data"),
        );
        let text = toml::to_string(&m).unwrap();
        let back: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(back["id"].as_str(), Some(PLUGIN_ID));
        let actions = back["actions"].as_array().unwrap();
        let panes = back["panes"].as_array().unwrap();
        // Every `plugin open <pane>` action names a declared pane.
        for a in actions {
            let cmd: Vec<&str> = a["command"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            assert!(cmd[0].starts_with("/opt/bin/"));
            if cmd[1..3] == ["plugin", "open"] {
                assert!(panes.iter().any(|p| p["id"].as_str() == Some(cmd[3])));
            }
        }
        assert!(text.contains("\"7700\""), "bridge port: {text}");
        assert!(text.contains("/srv/data"));
        assert!(text.contains("placement = \"popup\""));

        let no_trim = manifest(Path::new("/x"), None, 1, Path::new("/d"));
        assert!(!no_trim.panes.iter().any(|p| p.id == "dashboard"));
        assert!(!no_trim.actions.iter().any(|a| a.id == "dashboard"));
    }
}
