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
pre-push gate runs `dev/ci.sh dry-check` (or `cargo test --workspace` +
`cargo clippy --all-targets -- -D warnings`). If it fires, the maintainer
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

- A trim policy cached on `AgentHandle` AND re-derived from herdr pane metadata
  on every read (pick one canonical source).
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

## Run Commands

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
# wire crate + web UI must stay wasm-clean:
cargo clippy -p herdr-mcp-api -p herdr-mcp-ui --target wasm32-unknown-unknown -- -D warnings
```

**Changing an `/api/*` shape:** edit the type in `crates/herdr-mcp-api` (never
redefine it elsewhere), use it in the handler, and extend
`http_contract_roundtrip_with_api_client` in `herdr-mcp-server/src/server.rs`.

Full pre-PR validation (recommended):

```bash
cargo build --release
cargo test --workspace
cargo clippy --all-targets -- -D warnings
```

Web UI (Dioxus, needs `dx` — `cargo install dioxus-cli`):

```bash
cd crates/herdr-mcp-ui && dx serve                          # dev, proxies /api to :7676
cd crates/herdr-mcp-ui && dx bundle --platform web --release  # build served by herdr-mcp
```

## Run Commands

```bash
# Default: run MCP stdio + HTTP on port 7676
herdr-mcp

# Explicit serve with HTTP
herdr-mcp serve --http 7676

# MCP only (no HTTP)
herdr-mcp serve

# HTTP only (no MCP stdio)
herdr-mcp serve --http 7676 --http-only

# Kitchen-sink TUI dashboard (VS Code-style tabs, mouse+keyboard)
herdr-trim dashboard

# Legacy trim-only dashboard
herdr-trim dashboard --legacy

# Message trim CLI
herdr-trim trim "text to compress"
```

## Project Snapshot

herdr-mcp is an MCP (Model Context Protocol) server written in Rust that
exposes [herdr](https://herdr.dev) — a terminal-native agent multiplexer —
as MCP tools. It enables AI clients (Claude Desktop, Cursor, Claude Code,
OpenCode) to control herdr workspaces, tabs, panes, and agents. It also ships
an HTTP bridge (Axum), a Dioxus web playground, a message-trim
(compression) pipeline, a recipe engine for chaining tool calls, an a2a
(agent-to-agent) primitive layer, and a **kitchen-sink TUI dashboard** with
VS Code-style tabs, mouse + keyboard navigation.

Core architecture is **shell-out + trait-light**: the server wraps the local
`herdr` CLI via `tokio::process::Command` with real argv (no shell
injection). The MCP protocol is implemented with `rmcp` 1.7. The project is
organized as a Cargo workspace under `crates/`.

Key subsystems:

- **MCP tool layer** — 55 tool definitions across discovery, lifecycle,
  read, write, synchronize, a2a primitives, message-trim, recipe templates,
  scheduler, and folder-key categories.
- **HTTP bridge** — Axum server mirroring the MCP tools as REST endpoints;
  shares the same `HerdrMcpServer` instance and tool dispatch.
- **Recipe engine** — variable interpolation (`{{ stepId.result.path }}`)
  with dot/bracket navigation; 6 bundled templates.
- **Trim pipeline** — `caveman` (lossy style compressor) + `pfc1` (lossless
  Cherokee-syllabary phonetic compressor), composed via an ordered pipeline.
- **Agent registry & event subscriber** — live pane tracking via herdr Unix
  socket; label resolution by pane_id → role → label.
- **Persistence** — file-based JSON under `data/` (recipes, schedules,
  variables, trim stats, PFC1 memory).
- **TUI dashboard** — `crates/herdr-mcp-tui/` with VS Code-style
  tabbed interface (Overview, Playground, Trim, Variables, Settings), mouse
  + keyboard navigation, herdr sidecar context awareness.

## Stability Tiers

Every workspace crate carries a stability tier.

| Crate | Tier | Notes |
|-------|------|-------|
| `herdr-mcp-api` | Experimental | HTTP bridge wire contract: every `/api/*` type + typed `ApiClient`. Leaf crate, must build for `wasm32-unknown-unknown`. |
| `herdr-mcp-core` | Beta | Config system (`TOML + env + CLI`), error types. Stable schema. |
| `herdr-mcp-trim` | Experimental | Message-trim compressors (caveman, pfc1), pipeline, policy, stats, folder keys. |
| `herdr-mcp-tui` | Experimental | Terminal dashboard (kitchen-sink + legacy). Bridge client only — no server deps. |
| `herdr-mcp-server` | Experimental | MCP server, 57 tools, HTTP bridge, recipe engine, event subscriber, scheduler. |
| `herdr-mcp-cli` | Experimental | Binary entrypoint (`serve`/`trim`/`dashboard`/`folder-key`) + integration tests. |

**Tiers**: Beta = breaking changes permitted in MINOR with changelog notes.
Experimental = no stability guarantee. Tiers are promoted, never demoted,
through deliberate team decision.

## Repository Map

- `crates/herdr-mcp-api/` — single definition of every `/api/*` request/response type (`agents.rs`, `recipe.rs`, `trim.rs`, `variables.rs`) + typed `ApiClient` (`client.rs`). Server/trim/proxy re-export these; web UI and TUI call the bridge through `ApiClient`. No tokio, no `herdr-mcp-*` deps.
- `crates/herdr-mcp-core/` — `config.rs` (full config system), `error.rs` (anyhow-based error context), `lib.rs`
- `crates/herdr-mcp-trim/` — `pfc1.rs`, `caveman.rs`, `code_regions.rs`, `pipeline.rs`, `policy.rs`, `runner.rs`, `stats.rs`, `eval.rs`, `folder_key.rs`
- `crates/herdr-mcp-tui/` — kitchen-sink TUI dashboard (`lib.rs` app loop, `tabs/`, `nav.rs`, `theme.rs`, `http.rs` bridge discovery) + `legacy.rs` trim-only ANSI dashboard; talks to the bridge through `herdr-mcp-api::ApiClient`
- `crates/herdr-mcp-server/` — `server.rs` (tool defs + HTTP handlers + recipe engine), `herdr_client.rs`, `persistence.rs`, `scheduler.rs`, `templates.rs`, `variables.rs`, `lib.rs`
- `crates/herdr-mcp-cli/` — `src/bin/herdr-mcp.rs` (serve), `src/bin/herdr-trim.rs` (trim/dashboard/folder-key), `tests/` (integration)
- `crates/herdr-mcp-ui/` — Dioxus web UI; uses `herdr-mcp-api::ApiClient` (one instance from `window.location.origin`, provided as `Signal` context via `use_api()`); served by the HTTP bridge from `http.web-dir`
- `data/` — runtime persistence (recipes, executions, variables, schedules, sessions, pfc1 memory, folder keys)
- `docs/` — PRD and architecture reference docs
- `herdr-mcp-context.toml` — project's evolving memory for resuming sessions


## Risk Tiers

- **Low risk**: docs/chore/tests-only changes, web UI CSS tweaks
- **Medium risk**: most `crates/*/src/**` behavior changes without boundary/security impact
- **High risk**: `crates/herdr-mcp-server/src/server.rs` (tool definitions, HTTP bridge), `crates/herdr-mcp-trim/src/pfc1.rs` / `caveman.rs` (correctness of compression round-trip), `crates/herdr-mcp-trim/src/folder_key.rs` (non-determinism-sensitive), `.github/workflows/**`

When uncertain, classify as higher risk.

## Workflow

1. **Read before write** — inspect existing module, factory wiring, and adjacent tests before editing.
2. **One concern per PR** — avoid mixed feature+refactor+infra patches.
3. **Implement minimal patch** — no speculative abstractions, no config keys without a concrete use case.
4. **Validate by risk tier** — docs-only: lightweight checks. Code changes: full relevant checks (`cargo test`, `cargo clippy`).
5. **Document impact** — update PR notes for behavior, risk, side effects, and rollback.
6. **Update project memory** — at the end of each session, update `herdr-mcp-context.toml` with new spec facts, gotchas, and test counts.

## Branch/commit/PR rules

- Work from a non-`master` branch. Open a PR to `master`; do not push directly.
- Use conventional commit titles. Prefer small PRs.
- Never commit secrets, personal data, or real identity information.

## Anti-Patterns

- Do not add heavy dependencies for minor convenience.
- Do not silently weaken security policy or access constraints.
- Do not add speculative config/feature flags "just in case".
- Do not mix massive formatting-only changes with functional changes.
- Do not modify unrelated modules "while here".
- Do not bypass failing checks without explicit explanation.
- Do not leave `unwrap()` / `expect()` in production paths; propagate errors or document the invariant that makes panic impossible.
- Do not break the trim round-trip contract: `caveman` is lossy (style), `pfc1` is lossless. The a2a path (`agent_message`→`agent_read`) must stay lossless end-to-end via compact header-less PFC1.
- Do not print to stdout from server code — stdout is exclusively MCP JSON-RPC; all tracing/logging goes to stderr.

## Design Decisions & Trade-offs

- **Shell-out architecture** — every tool shells out via `tokio::process::Command` with real argv (no shell injection). Herdr's wire protocol isn't publicly documented; the CLI is the stable surface. Zero coupling, easy to update. Trade-off: higher latency per call vs. direct socket.
- **Stdout = MCP, Stderr = logs** — prevents log output from corrupting the MCP protocol stream.
- **One web UI** — the Dioxus app (`crates/herdr-mcp-ui`) is the only web frontend. The bridge serves its `dx bundle` output from `http.web-dir` (env `HERDR_MCP_WEB_DIR`) with SPA fallback to `index.html`; no CORS layer since the UI is same-origin (or proxied by `dx serve`).
- **Non-colliding compressors** — `caveman` operates on ASCII/Latin prose (U+0000–U+007F); `pfc1` uses Cherokee syllabary (U+13A0–U+13FF). The two cannot interfere, making composition trivially safe.
- **Adaptive gate (never expand)** — if `candidate.len() >= current.len()`, pass through unchanged. Prevents short messages from drowning in PFC1 headers.
- **Lossy vs. lossless split** — style compression is inherently lossy; dictionary compression must be reversible for a2a comms.
- **File-based persistence** — simple, inspectable, no DB dependency. Adequate for single-server deployment.
- **Workspace-scoped variables** — session variables use herdr workspace ID as scope; they chain across runs in the same workspace but don't leak between workspaces.
- **`start_agent` appends `--`** — prevents herdr from consuming agent-specific flags.
- **Default command is `serve`** — running `herdr-mcp` with no arguments starts MCP stdio + HTTP on port 7676 (bound to `127.0.0.1`), enabling both AI client integration and web playground access out of the box.
- **Kitchen-sink TUI** — the `dashboard` subcommand launches a VS Code-style tabbed terminal interface with mouse + keyboard navigation, consolidating the playground, trim dashboard, variables, and settings into a single herdr sidecar pane.

## Skills

AI coding assistant skills live in `.opencode/skills/` and `~/.config/opencode/skills/`. Use the right one for the job:

- `rust-skills` — comprehensive Rust coding guidelines (265 rules across 26 categories).
- `debloatify` — review code as a furious senior dev rejecting LLM slop (abstraction, defensive theatre, comment noise).
- `opencode-herdr` — run OpenCode in herdr panes for long builds/monitoring.
- `ship-it` — generate professional project documentation (PRD, TRD, UI/UX, Appflow, Schema, Impl Plan).
- `seo-geo-aeo` — web property audit (not relevant to this repo's Rust code).
- `find-skills` — discover and install agent skills.

## Linked References

- `@herdr-mcp-context.toml` — evolving project memory: architecture, spec facts tests rely on, known non-determinism, test layout/counts. Read first when resuming work.
- `@docs/ARCHITECTURE.md` — comprehensive architecture reference (data flow, tool reference, HTTP API, recipe engine, trim system, agent registry, persistence, scheduler, frontend, dependencies, build/run, design decisions, known gotchas).
- `@docs/PRD.md` — product requirements document.
- `@compressorplan.md` — message-trim design plan (caveman + pfc1).
- `@a2a.md` — agent-to-agent primitive design and usage.
- `@CRATES_IMPL_V2.md`, `@CRATES_MIGRATION.md` — 4-crate workspace migration history.
- `@TEST_PLAN.md` — test suite specification (233 tests across 4 crates).

<!-- CODEGRAPH_START -->
## CodeGraph

In repositories indexed by CodeGraph (a `.codegraph/` directory exists at the repo root), reach for it BEFORE grep/find or reading files when you need to understand or locate code:

- **MCP tool** (when available): `codegraph_explore` answers most code questions in one call — the relevant symbols' verbatim source plus the call paths between them, including dynamic-dispatch hops grep can't follow. Name a file or symbol in the query to read its current line-numbered source. If it's listed but deferred, load it by name via tool search.
- **Shell** (always works): `codegraph explore "<symbol names or question>"` prints the same output.

If there is no `.codegraph/` directory, skip CodeGraph entirely — indexing is the user's decision.
<!-- CODEGRAPH_END -->
