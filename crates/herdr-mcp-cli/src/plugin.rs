//! `herdr-mcp plugin` — herdr-mcp as a herdr plugin.
//!
//! [`manifest`] is the plugin's `herdr-plugin.toml`, committed at the repo
//! root (`herdr-mcp plugin manifest > herdr-plugin.toml`; a test keeps the
//! two equal) so `herdr plugin install eugeneb50/herdr-mcp` works. Its
//! `[[build]]` puts both binaries in the plugin's `bin/`, and every command
//! runs from the plugin directory, reading port and data dir from config
//! when it runs. herdr offers the actions below in its action menu and
//! keybindings:
//!
//! - `recipes`: a popup that runs a saved recipe with the invocation context
//!   (herdr's `HERDR_PLUGIN_CONTEXT_JSON`: `workspace_id`, `selected_text`, …,
//!   plus `pane_id` = the focused pane) as `{{herdr.…}}`;
//! - `dashboard`: the TUI dashboard in a split;
//! - `bridge`: the HTTP bridge in its own tab (also opened by the startup
//!   and `workspace.created` hooks when no bridge is running);
//! - `web`: the web playground in a browser.
//!
//! Actions run with no terminal, so each one that needs a screen opens one of
//! the plugin's panes (`plugin open <pane>`), passing herdr's context along.

use std::io::{BufRead, Write};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde::Serialize;
use serde_json::Value;

use herdr_mcp_api::{ApiClient, RunEventKind, StartRunRequest, tool_text};
use herdr_mcp_core::PLUGIN_ID;

/// Env var carrying the invoking action's context into a plugin pane.
const CONTEXT_ENV: &str = "HERDR_MCP_CONTEXT";
/// The binaries `[[build]]` installs, relative to the plugin directory.
pub const EXE: &str = "./bin/herdr-mcp";
const TRIM: &str = "./bin/herdr-trim";

#[derive(Subcommand)]
pub enum PluginCommand {
    /// Print the plugin manifest (the repo's `herdr-plugin.toml`).
    Manifest,
    /// Startup / `workspace.created` hook: open the bridge pane unless a
    /// bridge is already running.
    #[command(hide = true)]
    Startup,
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
pub struct Manifest {
    id: &'static str,
    name: &'static str,
    version: &'static str,
    min_herdr_version: &'static str,
    description: &'static str,
    platforms: Vec<&'static str>,
    build: Vec<Hook>,
    pub startup: Vec<Hook>,
    pub events: Vec<Event>,
    pub actions: Vec<Action>,
    pub panes: Vec<Pane>,
}

#[derive(Serialize)]
pub struct Hook {
    pub command: Vec<String>,
}

#[derive(Serialize)]
pub struct Event {
    on: &'static str,
    pub command: Vec<String>,
}

#[derive(Serialize)]
pub struct Action {
    pub id: &'static str,
    title: &'static str,
    contexts: Vec<&'static str>,
    pub command: Vec<String>,
}

#[derive(Serialize)]
pub struct Pane {
    pub id: &'static str,
    title: &'static str,
    placement: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<&'static str>,
    pub command: Vec<String>,
}

/// The plugin as herdr sees it.
pub fn manifest() -> Manifest {
    let argv = |exe: &str, args: &[&str]| {
        std::iter::once(exe)
            .chain(args.iter().copied())
            .map(String::from)
            .collect::<Vec<_>>()
    };
    let us = |args: &[&str]| argv(EXE, args);
    let everywhere = || vec!["global", "workspace", "tab", "pane", "selection"];
    Manifest {
        id: PLUGIN_ID,
        name: "herdr-mcp",
        version: env!("CARGO_PKG_VERSION"),
        min_herdr_version: "0.9.0",
        description: "MCP server for herdr: recipes, triggers, schedules, dashboard and web playground",
        platforms: vec!["linux", "macos"],
        build: vec![Hook {
            command: argv("sh", &["scripts/plugin-build.sh"]),
        }],
        startup: vec![Hook {
            command: us(&["plugin", "startup"]),
        }],
        events: vec![Event {
            on: "workspace.created",
            command: us(&["plugin", "startup"]),
        }],
        actions: vec![
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
            Action {
                id: "dashboard",
                title: "herdr-mcp: open the dashboard",
                contexts: vec!["global", "workspace", "pane"],
                command: us(&["plugin", "open", "dashboard"]),
            },
        ],
        panes: vec![
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
                command: us(&["serve", "--http-only"]),
            },
            Pane {
                id: "dashboard",
                title: "herdr-mcp dashboard",
                placement: "split",
                width: None,
                height: None,
                command: argv(TRIM, &["dashboard"]),
            },
        ],
    }
}

pub async fn run(cmd: PluginCommand, config: &herdr_mcp_core::Config) -> Result<()> {
    let port = || {
        config
            .http
            .port
            .context("http.port is unset, so herdr-mcp's bridge is off")
    };
    match cmd {
        PluginCommand::Manifest => {
            print!("{}", toml::to_string(&manifest())?);
            Ok(())
        }
        PluginCommand::Startup => startup(port()?).await,
        PluginCommand::Open { pane } => open_pane(&pane),
        PluginCommand::Recipes => {
            let result = match port() {
                Ok(port) => recipes(port).await,
                Err(e) => Err(e),
            };
            if let Err(e) = &result {
                println!("\n{e:#}");
            }
            pause("\nPress Enter to close.");
            Ok(())
        }
        PluginCommand::Web => web(port()?).await,
    }
}

/// The startup and `workspace.created` hook: bring the bridge (and with it
/// schedules and triggers) up with herdr, unless one already serves the port.
/// A server with no workspace yet has nowhere to put the tab; the first
/// `workspace.created` opens it then.
async fn startup(port: u16) -> Result<()> {
    if bridge(port).await.is_some() {
        return Ok(());
    }
    let env = format!("{CONTEXT_ENV}={}", context());
    let out = herdr_quiet(&open_args("bridge", &env))?;
    let reply = [out.stdout, out.stderr].concat();
    let reply = String::from_utf8_lossy(&reply);
    if out.status.success() {
        Ok(())
    } else if reply.contains("no_active_workspace") {
        println!("no workspace yet: the bridge opens with the first one");
        Ok(())
    } else {
        bail!("opening the bridge pane: {reply}")
    }
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
    herdr(&open_args(pane, &env))
}

/// `herdr plugin pane open` for one of our panes, passing `env` along.
fn open_args<'a>(pane: &'a str, env: &'a str) -> [&'a str; 9] {
    [
        "plugin",
        "pane",
        "open",
        "--plugin",
        PLUGIN_ID,
        "--entrypoint",
        pane,
        "--env",
        env,
    ]
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
    herdr_mcp_tui::http::live_bridge(port).await
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
    fn committed_manifest_is_current() {
        let committed = include_str!("../../../herdr-plugin.toml");
        assert_eq!(
            committed,
            toml::to_string(&manifest()).unwrap(),
            "regenerate: herdr-mcp plugin manifest > herdr-plugin.toml"
        );
    }

    #[test]
    fn every_open_names_a_pane() {
        let m = manifest();
        for a in &m.actions {
            if a.command[1..3] == ["plugin", "open"] {
                assert!(m.panes.iter().any(|p| p.id == a.command[3]), "{}", a.id);
            }
        }
    }
}
