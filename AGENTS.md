# herdr-mcp — AGENTS.md

Cross-tool agent instructions for any AI coding assistant working on this repository.

## ABSOLUTE RULE — SINGLE SOURCE OF TRUTH (NO DRY VIOLATIONS)

**No piece of state lives in two places. Ever. Anywhere in this codebase.**

This is not a guideline. It is not a preference. It is not deferrable to a
follow-up PR. If a fact already lives somewhere in this codebase, you do NOT
copy it into a new field, struct, config block, schema entry, runtime cache,
or anywhere else. You reference it. You resolve it from its source on demand.

### Forcing mechanism — what happens when you violate

Adding a duplicate state field is an automatic-revert-on-detect change. The
pre-push [gates](#gates) (`cargo nextest run --workspace` + `cargo clippy
--all-targets -- -D warnings`) and review catch it. When they do, the maintainer
will `git reset --hard` your branch back to the prior good state, and the
time you spent is wasted. Save yourself the burn: do not write the duplicate
in the first place.

### Pre-edit ritual — before any new struct field, channel/handle field, schema field, config entry

State, in your response text, the source of truth for the new data BEFORE you
write the field. Two valid answers:

1. **"This is the source of truth — created here."** OK to write the
   field. State what it represents.
2. **"Source of truth is `<path/to/canonical>` — this would be a
   duplicate."** Do NOT write the field. Resolve from the canonical
   location at use-time (closure, helper, `&Config` parameter, getter
   trait, whatever fits — never a cache).

### Patterns that ARE duplicate state (forbidden)

- The data dir copied into a `HerdrMcpServer` field AND held by its
  `Persistence` (read `persistence.data_dir()`; pick one canonical source).
- A recipe's step results cloned into both `ExecutionResult` and a separate
  `HashMap` that the caller could already reach through the execution record.
- Re-emitting the herdr socket path into a runtime struct field when the
  runtime already has it from `Config` / env.

### Patterns that are NOT duplicate state (allowed)

- Resolver closures (`Arc<dyn Fn() -> T + Send + Sync>`) that close over
  shared config and resolve on call.
- `&Config` / `&HerdrMcpServer` parameters threaded through call sites.
- Materialized views built ON-DEMAND from canonical state (cached per-call,
  not stored).
- Derive macros that emit multiple surfaces from one input table.

## Gates

Run before every push (all four must pass):

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo nextest run --workspace   # cargo install cargo-nextest --locked
# wire crate + web UI must stay wasm-clean:
cargo clippy -p herdr-mcp-api -p herdr-mcp-ui --target wasm32-unknown-unknown -- -D warnings
```

The workspace has no doctests; if you add one, also run `cargo test --doc`.

**Changing an `/api/*` shape:** edit the type in `crates/herdr-mcp-api` (never
redefine it elsewhere), use it in the handler, and extend
`http_contract_roundtrip_with_api_client` in `herdr-mcp-server/src/server.rs`.

Web UI (Dioxus, needs `dx` — `cargo install dioxus-cli`):

```bash
cd crates/herdr-mcp-ui && dx serve                          # dev, proxies /api to :7676
cd crates/herdr-mcp-ui && dx bundle --platform web --release  # build served by herdr-mcp
```

## Run Commands

```bash
herdr-mcp                                   # terminal: TUI + HTTP on 127.0.0.1:7676; piped stdin: MCP stdio + HTTP
herdr-mcp --no-tui                          # MCP stdio + HTTP (flags: --http, --data-dir, --herdr-socket, …)
herdr-mcp --http 7676 --http-only           # HTTP bridge only
herdr-mcp plugin manifest                   # print herdr-plugin.toml (regenerate after changing plugin.rs)
sh scripts/plugin-build.sh && herdr plugin link .   # the herdr plugin from this checkout
```

**Smoke testing against herdr:** never create panes or link plugins in the
user's live herdr session. Use an isolated session (`HERDR_SESSION=hmcp-smoke`),
an explicit `--data-dir` under a temp dir, and, for `herdr plugin link`, a short
`XDG_CONFIG_HOME` under `/tmp` (herdr's plugin registry is global per user).

## Project Snapshot

herdr-mcp is an MCP (Model Context Protocol) server written in Rust that
exposes [herdr](https://herdr.dev) — a terminal-native agent multiplexer —
as MCP tools, and extends herdr itself. It enables AI clients (Claude Desktop,
Cursor, Claude Code, OpenCode) to control herdr workspaces, tabs, panes, and
agents. It also ships an HTTP bridge (Axum), a Dioxus web playground, a TUI
dashboard, a recipe run engine with herdr-event triggers and a cron scheduler,
a herdr plugin and an a2a (agent-to-agent) primitive layer.

Core architecture is **shell-out + trait-light**: the server wraps the local
`herdr` CLI via `tokio::process::Command` with real argv (no shell
injection), and uses herdr's Unix socket for the event subscription and
`herdr_api`. The MCP protocol is implemented with `rmcp` 1.7. The project is a
Cargo workspace under `crates/`.

Key subsystems:

- **MCP tool layer** — 41 tools, grouped by `#[tool(meta = category("…"))]`:
  Discovery, Lifecycle, Read, Write, Synchronize, A2A, Variables,
  Templates, Scheduler, Clipboard.
- **HTTP bridge** — Axum server mirroring the tools as REST endpoints plus
  runs, recipes, variables, profiles, herdr schema and SSE event streams;
  shares the one `HerdrMcpServer` and its tool router.
- **Run engine** (`runs.rs`) — the one executor for `/api/runs`, the scheduler
  and triggers; variable interpolation (`{{ stepId.result.path }}`) with
  dot/bracket navigation; 6 bundled templates.
- **herdr events** (`herdr_client.rs`) — socket subscriber feeding the agent
  registry, a `broadcast` of `HerdrEvent`s (relayed at `/api/herdr/events`) and
  recipe triggers (`triggers.rs`).
- **herdr plugin** (`herdr-mcp-cli/src/plugin.rs`) — generated
  `herdr-plugin.toml`: recipe picker popup, bridge tab, dashboard split, web.
- **Agent registry** — live pane tracking; label resolution by pane_id → role → label.
- **Persistence** — file-based JSON under `data/` (recipes, runs, schedules,
  variables, sessions, profiles).

## Stability Tiers

Every workspace crate carries a stability tier.

| Crate | Tier | Notes |
|-------|------|-------|
| `herdr-mcp-api` | Experimental | HTTP bridge wire contract: every `/api/*` type + typed `ApiClient`. Leaf crate, must build for `wasm32-unknown-unknown`. |
| `herdr-mcp-core` | Beta | Config system (`TOML + env + CLI`). Stable schema. |
| `herdr-mcp-tui` | Experimental | Terminal dashboard. Bridge client only — no server deps. |
| `herdr-mcp-server` | Experimental | MCP server, 41 tools, HTTP bridge, run engine, triggers, event subscriber, scheduler. |
| `herdr-mcp-ui` | Experimental | Dioxus web UI. Bridge client only, must build for `wasm32-unknown-unknown`. |
| `herdr-mcp-cli` | Experimental | The one binary, `herdr-mcp` (TUI / MCP stdio / HTTP-only modes, `plugin`) + integration tests. |

**Tiers**: Beta = breaking changes permitted in MINOR with changelog notes.
Experimental = no stability guarantee. Tiers are promoted, never demoted,
through deliberate team decision.

## Repository Map

- `crates/herdr-mcp-api/` — single definition of every `/api/*` request/response type (`agents.rs`, `config.rs`, `herdr.rs` (herdr events, recipe triggers, panes, sessions), `profile.rs`, `recipe.rs`, `run.rs`, `variables.rs`) + typed `ApiClient` (`client.rs`). The server re-exports these; web UI and TUI call the bridge through `ApiClient`. No tokio, no `herdr-mcp-*` deps.
- `crates/herdr-mcp-core/` — `config.rs` (full config system: `[http]`, `[herdr]`, `[clipboard]`), `session_context.rs` (inside-herdr / pane id from herdr's env)
- `crates/herdr-mcp-tui/` — TUI dashboard (`lib.rs` app loop, `tabs/`, `theme.rs`, `http.rs` bridge discovery); talks to the bridge through `herdr-mcp-api::ApiClient`
- `crates/herdr-mcp-server/` — `server.rs` (tool defs + HTTP handlers), `runs.rs` (run engine), `triggers.rs`, `scheduler.rs`, `herdr_client.rs` (socket client, event subscriber, agent registry), `panes.rs` (herdr panes + sessions, the one pane fetch), `agents.rs` / `profiles.rs` (agent profiles), `persistence.rs`, `templates.rs`, `variables.rs`
- `crates/herdr-mcp-cli/` — `src/main.rs` (mode choice: TUI / MCP stdio / HTTP only; `plugin`), `src/common.rs` (`serve_core`: the one server instance + background tasks), `src/plugin.rs` (herdr plugin), `tests/cli.rs`
- `crates/herdr-mcp-ui/` — Dioxus web UI; uses `herdr-mcp-api::ApiClient` (one instance from `window.location.origin`, provided as `Signal` context via `use_api()`); `components/playground/` holds the tool runner, herdr API, recipe builder and herdr events tabs; built into `herdr-mcp` and served by the HTTP bridge
- `data/` — runtime persistence (recipes, runs, variables, schedules, sessions, profiles)
- `docs/` — `ARCHITECTURE.md` (reference), `SWARMS.md` (flow-engine proposal)

## Risk Tiers

- **Low risk**: docs/chore/tests-only changes, web UI CSS tweaks
- **Medium risk**: most `crates/*/src/**` behavior changes without boundary/security impact
- **High risk**: `crates/herdr-mcp-server/src/server.rs` (tool definitions, HTTP bridge), `crates/herdr-mcp-server/src/triggers.rs` (runs recipes unattended), `.github/workflows/**`

When uncertain, classify as higher risk.

## Workflow

1. **Read before write** — inspect existing module, factory wiring, and adjacent tests before editing.
2. **One concern per PR** — avoid mixed feature+refactor+infra patches.
3. **Implement minimal patch** — no speculative abstractions, no config keys without a concrete use case. Fix verified bugs; mention hypothetical ones instead of coding guards for them.
4. **Validate by risk tier** — docs-only: lightweight checks. Code changes: all [gates](#gates).
5. **Document impact** — update PR notes for behavior, risk, side effects, and rollback.

## Branch/commit/PR rules

- Work from a non-`main` branch. Open a PR to `main`; do not push directly.
- Use conventional commit titles. Prefer small PRs.
- Never commit secrets, personal data, or real identity information.
- Stage files by name; never sweep in untracked local files (`data/`, drafts, `.codegraph/`).

## Anti-Patterns

- Do not add heavy dependencies for minor convenience.
- Do not silently weaken security policy or access constraints.
- Do not add speculative config/feature flags "just in case".
- Do not mix massive formatting-only changes with functional changes.
- Do not modify unrelated modules "while here".
- Do not bypass failing checks without explicit explanation.
- Do not leave `unwrap()` / `expect()` in production paths; propagate errors or document the invariant that makes panic impossible (`expect("…")` naming it). `clippy::unwrap_used` is denied workspace-wide; tests may unwrap (`clippy.toml`).
- Write data and config files through `persistence::write_atomic` (temp file + rename), never `fs::write` in place.
- Do not print to stdout from server code — stdout is exclusively MCP JSON-RPC; all tracing/logging goes to stderr.

## Design Decisions & Trade-offs

- **Shell-out architecture** — every tool shells out via `tokio::process::Command` with real argv (no shell injection). The CLI is herdr's stable surface. Trade-off: higher latency per call vs. direct socket. The socket is used only for the event subscription and `herdr_api`.
- **Stdout = MCP, Stderr = logs** — prevents log output from corrupting the MCP protocol stream.
- **One server instance** — `serve_core` builds one `HerdrMcpServer` and starts the background tasks once; stdio reuses it.
- **One daemon per data dir** — many herdr-mcp processes can share a data dir (MCP clients, the plugin's bridge, the dashboard). Only the holder of `<data_dir>/daemon.lock` fires schedules and triggers; the others retry every 15 s and take over when it exits. The daemon fires them against its own herdr session only, so each herdr session that runs triggers or schedules needs its own data dir. `Scheduler::schedule_one` only saves; the daemon's `Scheduler::run` picks saved schedules up within 5 s.
- **One web UI** — the Dioxus app (`crates/herdr-mcp-ui`) is the only web frontend. `herdr-mcp-server/build.rs` compiles its `dx bundle` output into the binary (bundle first, then `cargo build`), so one `herdr-mcp` file is the deployment; `http.web-dir` (env `HERDR_MCP_WEB_DIR`) serves a directory from disk instead. SPA fallback to `index.html`; no CORS layer since the UI is same-origin (or proxied by `dx serve`).
- **File-based persistence** — simple, inspectable, no DB dependency. Adequate for single-server deployment.
- **Workspace-scoped variables** — session variables use herdr workspace ID as scope; they chain across runs in the same workspace but don't leak between workspaces.
- **herdr from inside or outside a pane** — socket: `--herdr-socket` > `HERDR_SOCKET_PATH` (herdr sets it in panes) > config file > `HERDR_SESSION` > default; binary: `HERDR_BIN` > `HERDR_BIN_PATH` > `herdr`. The resolved socket is exported to child `herdr` processes so CLI calls and the socket client hit the same server.
- **One pane list, any session** — herdr owns panes and sessions; `panes::panes(&PaneFilter)` (`herdr-mcp-server/src/panes.rs`) is the only fetch, typed as `herdr_mcp_api::HerdrPane`. Discovery/Read tools and `/api/herdr/panes` take `session`, reached by running `herdr` with that session's socket; writes, runs and triggers stay on the server's own session.
- **Every herdr method** — `herdr_api` + `GET /api/herdr/schema`; the playground's herdr API tab and the trigger editor's event list are generated from the installed herdr's schema, never hand-listed.
- **herdr events** — herdr reads nothing more on a connection after `events.subscribe`, so the subscriber reconnects (with a fresh pane list) on `pane.created` / `pane.closed` instead of re-subscribing. Global events arrive as `pane_created`; `HerdrEvent::parse` normalizes them to the subscription name `pane.created`.
- **Triggers live on recipes** — `Recipe.triggers` is the only copy, read from disk on every event; a recipe never has two triggered runs in flight (this also stops self-retriggering).
- **One tool dispatch** — the bridge and run engine call tools through rmcp's router over an in-process MCP session, never a hand-written match. Tool groups come from each tool's `#[tool(meta = category("…"))]`.
- **Auto variables are a view** — `GET /api/variables/auto` resolves live herdr panes with their registry handles laid over them (by pane id and role, `AgentRegistry::seed`) (+ a workspace's `var_set` vars) on request — the pane dropdowns and the Home pane table read it; tool calls never write variables behind the user's back.
- **Host allowlist** — the bridge answers only `localhost`, `*.localhost`, IP literals and its bind name (DNS-rebinding guard; it can type into panes).
- **One binary, mode by terminal** — `herdr-mcp` with no subcommand runs the TUI over its own HTTP bridge (port 7676, bound to `127.0.0.1`) when stdin is a terminal, else MCP stdio + HTTP (what an MCP client launching it gets); `--no-tui` forces the latter, `--http-only` serves the bridge alone. In TUI mode tracing goes to `<data_dir>/herdr-mcp.log`.

## Skills

AI coding assistant skills live in `~/.config/opencode/skills/`. Useful here:

- `rust-skills` — comprehensive Rust coding guidelines.
- `debloatify` / `deslop` — review code for LLM slop (abstraction, defensive theatre, comment noise).
- `herdr` — driving herdr panes and agents.
- `bug-reaper` — bug hunting.
- `ship-it` — generate project documentation (PRD, TRD, UI/UX, Appflow, Schema, Impl Plan).

## Linked References

- `@docs/ARCHITECTURE.md` — comprehensive architecture reference (data flow, tool reference, HTTP API, recipe engine, triggers, agent registry, persistence, scheduler, web UI, plugin, build/run, design decisions, known gotchas).
- `@docs/SWARMS.md` — swarm / flow engine design (proposal).
- `@CHANGES.md` — notable changes.

<!-- CODEGRAPH_START -->
## CodeGraph

In repositories indexed by CodeGraph (a `.codegraph/` directory exists at the repo root), reach for it BEFORE grep/find or reading files when you need to understand or locate code:

- **MCP tool** (when available): `codegraph_explore` answers most code questions in one call — the relevant symbols' verbatim source plus the call paths between them, including dynamic-dispatch hops grep can't follow. Name a file or symbol in the query to read its current line-numbered source. If it's listed but deferred, load it by name via tool search.
- **Shell** (always works): `codegraph explore "<symbol names or question>"` prints the same output.

If there is no `.codegraph/` directory, skip CodeGraph entirely — indexing is the user's decision.
<!-- CODEGRAPH_END -->
