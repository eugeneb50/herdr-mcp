# herdr-mcp — Architecture Reference

Comprehensive architecture reference for the herdr-mcp project: data flow,
tool reference, HTTP API, recipe engine, trim system, agent registry,
persistence, scheduler, frontend, dependencies, build/run, design decisions,
and known gotchas.

---

## 1. Project Overview

**herdr-mcp** is an MCP (Model Context Protocol) server written in Rust that
exposes [herdr](https://herdr.dev) — a terminal-native agent multiplexer —
as MCP tools. It enables AI clients (Claude Desktop, Cursor, Claude Code,
OpenCode) to control herdr workspaces, tabs, panes, and agents. It also
includes an HTTP bridge, a Dioxus (Rust → wasm) web playground, a
sophisticated message-trim (compression) pipeline, and a recipe engine for
chaining tool calls with variable interpolation.

**License:** AGPL v3
**Current test suite:** run `cargo test --workspace` (361 passing at the
Dioxus consolidation)
**Architecture:** Cargo workspace under `crates/`; all code lives there.

---

## 2. Architecture Overview

### 2.1 High-Level Data Flow

```
┌─────────────────────────────────────────────────────────────┐
│                     AI Clients                               │
│  Claude Desktop / Cursor / Claude Code / OpenCode            │
└───────────────┬─────────────────────────────────────────────┘
                │ MCP stdio (JSON-RPC over stdin/stdout)
                ▼
┌─────────────────────────────────────────────────────────────┐
│  ┌──────────────────────┐  ┌──────────────────────────────┐ │
│  │  herdr-mcp Binary    │  │  Axum HTTP Bridge (opt.)     │ │
│  │  (main.rs)           │  │  server.rs:start_http()      │ │
│  │                      │  │  Port configurable            │ │
│  │  rmcp ServerHandler  │  │  Same-origin (no CORS)        │ │
│  │  tool_router macro   │  │  REST: /api/tools/:name       │ │
│  └──────────┬───────────┘  └──────────┬───────────────────┘ │
│             │                         │                      │
│             └──────────┬──────────────┘                      │
│                        ▼                                     │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │              HerdrMcpServer (server.rs)                  │ │
│  │  - 58 tool definitions (discovery, lifecycle, read, write, synchronize, a2a, trim, recipes, scheduler, folder-key) │ │
│  │  - Recipe engine with {{ variable }} interpolation      │ │
│  │  - Outbound trim: apply_outbound_trim()                 │ │
│  │  - Badge push: push_badge_for_workspace()               │ │
│  │  - Scheduler for cron-based recipe runs                 │ │
│  └──────────┬──────────────────────┬───────────────────────┘ │
│             │                      │                         │
│     ┌───────▼────────┐   ┌────────▼────────┐                │
│     │ herdr CLI      │   │ Trim Pipeline   │                │
│     │ (tokio::process│   │ (in-process)    │                │
│     │  ::Command)    │   │ caveman → pfc1  │                │
│     └───────┬────────┘   └─────────────────┘                │
│             │                                                │
│     ┌───────▼────────────┐  ┌──────────────────────────┐    │
│     │ herdr daemon       │  │ Persistence (file-based)  │    │
│     │ (workspaces,       │  │ data/recipes/*.json       │    │
│     │  tabs, panes,      │  │ data/sessions/*.json      │    │
│     │  agents)           │  │ data/schedules/*.json     │    │
│     └────────────────────┘  │ data/pfc1_memory.json     │    │
│                              │ data/pfc1_master_key.json │    │
│                              │ data/folder_keys/         │    │
│                              └──────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│  Event Subscriber (herdr_client.rs)                         │
│  Unix socket → herdr daemon                                 │
│  Subscribes to: workspace.created, workspace.closed,        │
│                 pane.agent_status_changed                   │
│  Populates AgentRegistry (in-memory RwLock<HashMap>)        │
│  Captures work product on idle transitions                  │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 Dual Transport Design

The server supports **two transport modes simultaneously**:

1. **MCP stdio** (JSON-RPC over stdin/stdout) — for AI client integration
2. **HTTP bridge** (Axum, optional `--http <port>`) — for browser-based web playground

Both share the same `HerdrMcpServer` instance, tool dispatch, and recipe
engine. The HTTP bridge exposes REST endpoints that mirror the MCP tools.

### 2.3 CLI Architecture

The binary has 4 subcommands:

| Subcommand | Purpose |
|---|---|
| `serve` | Run the MCP server (stdio + optional HTTP) |
| `trim` | One-shot message-trim runner (CLI) |
| `dashboard` | Live ANSI TUI of trim savings |
| `folder-key` | Manage per-folder PFC1 phonetic keys (build/list/show/decompress) |

---

## 3. Workspace & Crate Structure

All code lives in a Cargo workspace under `crates/`. The legacy `src/`
monolith and the React site were removed when the Dioxus UI became the only
web frontend.

```
herdr-mcp/
├── Cargo.toml              # Workspace root
├── crates/
│   ├── herdr-mcp-api/      # HTTP wire contract (leaf crate, native + wasm32)
│   │   └── src/
│   │       ├── lib.rs      # Re-exports every wire type + ApiClient
│   │       ├── agents.rs   # AgentHandle, TrimPolicy/TrimDirection, ProxyPolicy
│   │       ├── recipe.rs   # Recipe, RecipeStep, ExecutionResult, templates, recipe requests
│   │       ├── trim.rs     # TrimStatusResponse, PaneStatus, DiagnoseReport
│   │       ├── variables.rs # VariableStore, PlaygroundSnapshot, variable/tool envelopes
│   │       └── client.rs   # Typed ApiClient + ApiError
│   ├── herdr-mcp-core/     # Config, error types
│   │   └── src/
│   │       ├── lib.rs      # Re-exports Config, CliOverrides, Error
│   │       ├── config.rs   # Full config system: TOML + env + CLI
│   │       └── error.rs    # anyhow-based error context
│   ├── herdr-mcp-trim/     # Message-trim pipeline + TUI dashboard
│   │   └── src/
│   │       ├── lib.rs      # Re-exports all public types
│   │       ├── pfc1.rs     # PFC1 phonetic compressor
│   │       ├── caveman.rs  # Caveman style compressor
│   │       ├── code_regions.rs  # Code detection engine
│   │       ├── pipeline.rs # Composable stage pipeline
│   │       ├── policy.rs   # TrimPolicyExt (stage parsing); policy data re-exported from api
│   │       ├── runner.rs   # PipelineRunner with persistence
│   │       ├── stats.rs    # TrimStats per workspace
│   │       ├── eval.rs     # trim_eval + trim_bench
│   │       ├── dashboard.rs # Legacy trim-only TUI
│   │       ├── folder_key.rs # Per-folder PFC1 keys
│   │       └── tui/        # Kitchen-sink TUI dashboard (crossterm + ratatui)
│   │           ├── mod.rs      # App state, event loop, mouse handling
│   │           ├── http.rs     # HTTP client for bridge communication
│   │           ├── nav.rs      # NavigationFrame + FrameFocus
│   │           ├── theme.rs    # Color palette and styles
│   │           └── tabs/       # Tabbed panels (overview, playground, trim, variables, settings, proxy)
│   ├── herdr-mcp-server/   # MCP server, tools, HTTP bridge
│   │   └── src/
│   │       ├── lib.rs      # Re-exports HerdrMcpServer, Persistence, etc.
│   │       ├── server.rs   # Tool definitions + HTTP handlers + recipe engine
│   │       ├── herdr_client.rs # Event subscriber + AgentRegistry
│   │       ├── persistence.rs  # File-based storage
│   │       ├── scheduler.rs    # Cron-based recipe scheduling
│   │       ├── templates.rs    # Bundled recipe templates
│   │       └── variables.rs    # ScheduledRecipe + variable extraction (wire types from api)
│   ├── herdr-mcp-proxy/    # HTTPS intercepting proxy (CA, TLS, per-pane policy)
│   ├── herdr-mcp-ui/       # Dioxus 0.7 web UI (see §11)
│   └── herdr-mcp-cli/      # Binaries
│       ├── src/bin/herdr-mcp.rs  # Daemon: serve (MCP stdio + HTTP)
│       ├── src/bin/herdr-trim.rs # Local tools: trim / dashboard / folder-key
│       ├── src/common.rs         # Shared tracing, config, serve_core
│       └── tests/                # Integration tests
```

---

## 4. MCP Tool Reference (58 tools total)

### 4.1 Discovery Tools (7)

| Tool | Parameters | Description |
|---|---|---|
| `status` | — | Overall herdr server status |
| `list_workspaces` | — | List all workspaces in session |
| `list_tabs` | `workspace_id?` | List tabs, optionally by workspace |
| `list_panes` | `workspace_id?` | List panes, optionally by workspace |
| `list_agents` | — | List all detected agents |
| `get_pane` | `pane_id?`, `label?` | Get pane details by ID or label |
| `get_agent` | `target` | Get agent details by terminal/name/pane |

### 4.2 Lifecycle Tools (5)

| Tool | Parameters | Description |
|---|---|---|
| `create_workspace` | `cwd?`, `label?`, `no_focus?` | Create a new workspace |
| `create_tab` | `workspace_id?`, `label?`, `cwd?` | Create a new tab |
| `split_pane` | `pane_id?`, `label?`, `direction?`, `cwd?`, `no_focus?` | Split a pane right or down |
| `close_pane` | `pane_id?`, `label?` | Close a pane |
| `herdr_api` | `method`, `params?`, `timeout_ms?` | Any herdr socket API method; returns herdr's response |

### 4.3 Read Tools (2)

| Tool | Parameters | Description |
|---|---|---|
| `read_pane` | `pane_id?`, `label?`, `source?`, `lines?` | Read pane output (visible/recent/recent-unwrapped) |
| `read_agent` | `target`, `source?`, `lines?` | Read agent output |

### 4.4 Write Tools (4)

| Tool | Parameters | Description |
|---|---|---|
| `send_text` | `pane_id?`, `label?`, `text` | Send text to pane (no Enter) |
| `send_keys` | `pane_id?`, `label?`, `keys` | Send key presses |
| `run_command` | `pane_id?`, `label?`, `command` | Atomic text+Enter |
| `send_agent` | `target`, `text` | Send text to agent stream |

### 4.5 Synchronize Tools (3)

| Tool | Parameters | Description |
|---|---|---|
| `wait_output` | `pane_id?`, `label?`, `match_text`, `timeout_ms?`, `source?`, `use_regex?` | Wait for text in pane |
| `wait_pane_agent_status` | `pane_id?`, `label?`, `status`, `timeout_ms?` | Wait for agent status |
| `wait_agent_status` | `target`, `status`, `timeout_ms?` | Wait for agent status by target |

### 4.6 A2A Primitives (5)

| Tool | Parameters | Description |
|---|---|---|
| `agent_spawn` | `role`, `agent`, `args`, `cwd?`, `workspace_id?`, `tab_id?`, `split?`, `needs`, `wait_idle`, `label?` | Spawn agent, register role→pane, wait for deps, capture output |
| `agent_message` | `target`, `text`, `compress?` | Send message with optional trim; uses per-pane policy if no explicit stages |
| `agent_read` | `target`, `source?`, `lines?`, `decompress?` | Read + store as work product; auto-decompress PFC1 |
| `agent_wait` | `target`, `status?`, `timeout_ms?` | Wait for agent by role/pane |
| `agent_list` | `workspace_id?` | List registry agents |

### 4.7 Session Variables (2)

| Tool | Parameters | Description |
|---|---|---|
| `var_get` | `session_id`, `key` | Get session variable |
| `var_set` | `session_id`, `key`, `value` | Set session variable |

### 4.8 Message-Trim Tools (10)

| Tool | Parameters | Description |
|---|---|---|
| `compress` | `text`, `stages`, `workspace_id?` | Compress text via pipeline; embeds PFC1 header |
| `decompress` | `text` | Decompress PFC1 payload |
| `trim_policy_set` | `target`, `policy?` | Attach/clear trim policy on agent |
| `trim_policy_get` | `target` | Read trim policy |
| `trim_eval` | `text`, `stages` | Offline savings evaluation |
| `trim_bench` | `corpus`, `level` | Corpus sweep savings distribution |
| `trim_status` | `workspace_id?` | Aggregate savings metrics |
| `trim_diagnose` | `workspace_id?` | End-to-end readiness check |
| `trim_summary` | `workspace_id?` | Fire herdr notification with savings |
| `trim_dashboard_open` | `workspace_id?` | Open live TUI dashboard in split pane |

### 4.9 Recipe Template Tools (3)

| Tool | Parameters | Description |
|---|---|---|
| `list_templates` | — | List bundled templates |
| `get_template` | `template_id` | Get template by ID |
| `instantiate_template` | `template_id`, `variables`, `name?` | Instantiate template → recipe, auto-save |

### 4.10 Scheduler Tools (4)

| Tool | Parameters | Description |
|---|---|---|
| `schedule_recipe` | `recipe_id`, `cron_schedule`, `enabled?` | Schedule recipe on cron |
| `list_schedules` | — | List active schedules |
| `delete_schedule` | `id` | Delete schedule |
| `enable_schedule` | `id`, `enabled` | Enable/disable schedule |

### 4.11 Folder-Key Tools (4)

| Tool | Parameters | Description |
|---|---|---|
| `build_folder_key` | `folder_path`, `min_frequency?`, `min_length?`, `max_terms?`, `persist_central?`, `learn_master?` | Scan folder, build PFC1 key |
| `get_folder_key` | `folder_path` | Load folder key (walks up tree) |
| `list_folder_keys` | — | List central registry keys |
| `decompress_with_folder_key` | `folder_path`, `text` | Decompress with folder's key |

### 4.12 Clipboard Tools (2)

| Tool | Parameters | Description |
|---|---|---|
| `clipboard_set` | `text` | Write text to system clipboard |
| `clipboard_get` | — | Read text from system clipboard |

### 4.13 Proxy Tools (4)

| Tool | Parameters | Description |
|---|---|---|
| `proxy_startup` | `port?`, `bind_addr?` | Start the HTTPS intercepting proxy listener |
| `proxy_diagnose` | — | CA status, TLS config, default and per-pane policies |
| `proxy_policy_set` | `target`, `trim_outbound?`, `trim_inbound?`, `stages?` | Set a pane's proxy interception policy |
| `proxy_policy_get` | `target` | Get a pane's proxy interception policy |

---

## 5. HTTP Bridge API

### 5.1 Core Routes

| Method | Path | Handler | Description |
|---|---|---|---|---|
| `GET` | `/*` | `ServeDir` fallback | Serves the Dioxus web UI from `http.web-dir` (see §5.5) |
| `GET` | `/api/health` | `health_handler` | Returns `"ok"` |
| `GET` | `/api/tools` | `list_tools_handler` | Lists all tools with JSON schemas |
| `POST` | `/api/tools/{name}` | `call_tool_handler` | Dispatch tool by name |
| `GET` | `/api/agents?workspace_id=` | `agents_handler` | Live pane list from AgentRegistry |
| `GET` | `/api/workspaces` | `workspaces_handler` | Workspace list via herdr CLI |
| `POST` | `/api/runs` | `start_run_handler` | Start a run; `?wait=true` waits for the final record |

### 5.2 Recipe CRUD Routes

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/recipes` | List all recipes |
| `POST` | `/api/recipes` | Create recipe |
| `GET` | `/api/recipes/{id}` | Get recipe by ID |
| `PUT` | `/api/recipes/{id}` | Update recipe |
| `DELETE` | `/api/recipes/{id}` | Delete recipe |

### 5.3 Variable & Execution Routes

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/variables` | List all variables |
| `POST` | `/api/variables` | Save variable |
| `GET` | `/api/variables/{key}` | Get variable |
| `DELETE` | `/api/variables/{key}` | Delete variable |

### 5.4 Trim Routes

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/trim/status?workspace_id=` | Aggregate trim savings |
| `POST` | `/api/trim/diagnose` | End-to-end readiness check |
| `POST` | `/api/trim/summary` | Fire savings notification |
| `POST` | `/api/trim/dashboard/open` | Open live dashboard pane |

### 5.5 Frontend Serving

The bridge serves the Dioxus bundle with `tower_http::services::ServeDir`:

- Directory: `http.web-dir` (TOML) / `HERDR_MCP_WEB_DIR` (env); unset →
  `HttpConfig::resolved_web_dir()` = `<workspace>/target/dx/herdr-mcp-ui/release/web/public`
  (the `dx bundle --platform web --release` output)
- Any path that isn't an `/api/*` route or a file falls back to `index.html`,
  so Dioxus router deep links (`/playground`, `/trim`, …) load the app
- A missing `index.html` logs a `tracing::warn!` with the build command;
  `/api/*` keeps working
- No CORS layer: the UI is same-origin with the API (and `dx serve` proxies
  `/api` server-side via `Dioxus.toml [web.proxy]`)
- Binds `http.bind-addr` (default `127.0.0.1`)

### 5.6 Wire Contract

Every request/response type on these routes is defined once in
`crates/herdr-mcp-api` and re-exported by the server; nothing redefines it.
Rust clients (web UI, TUI) use `herdr_mcp_api::ApiClient`, which requires an
absolute base URL, treats `201`/`204` empty bodies as `Ok(())`, and returns
`ApiError::Status { status, body }` for non-2xx responses.
`server::tests::http_contract_roundtrip_with_api_client` runs that client
against the real router, so a server/client drift fails `cargo test`.

To change a shape: edit the type in `herdr-mcp-api`, use it in the handler,
and extend the contract test.

---

## 6. Recipe Engine

### 6.1 Variable Interpolation

Variables use `{{ path }}` syntax with dot/bracket navigation:

```
{{ step1.result.content[0].text }}
{{ pane_id.output }}
{{ role.output }}
{{ template.variable_name }}
```

**Resolution order:**
1. Agent registry seed: `{{pane_id.output}}` and `{{role.output}}` from live agent registry
2. Accumulated step results: `{{stepId.result.path}}` from prior steps
3. Template variables: `{{template.var}}` from template instantiation

**Key spec facts:**
- `extract_variables` always prefixes keys with `result.` (e.g., `{"result":"hello"}` → `"result.result"`)
- `extract_variables` ignores arrays
- `resolve_json_path` requires a dotted path (`step.field`); bare single keys return `None`
- Unknown `{{ path }}` placeholders are left unchanged

### 6.2 Recipe Lifecycle

```
1. HTTP POST /api/runs (or the scheduler)
2. Seed accumulated from agent registry (for session_id workspace)
3. For each step:
   a. resolve_variables() in params from accumulated
   b. dispatch_tool() — calls the actual tool
   c. extract_variables() from result → accumulated
   d. Insert step result under both step.id AND pane_id/role
4. Persist ExecutionResult
5. Return RecipeResponse with results + status
```

### 6.3 Bundled Templates

| ID | Name | Category | Description |
|---|---|---|---|
| `dev-watch` | Dev Watch | development | Watch files, notify agent |
| `git-status` | Git Status | git | Run git status + read output |
| `restart-agent` | Restart Agent | agents | Close + start agent |
| `build-and-test` | Build & Test | development | Build → wait → test |
| `health-check` | Health Check | monitoring | Status + workspaces + panes + agents |

---

## 7. Message-Trim System

### 7.1 Architecture

The trim system compresses messages crossing the wire between agents, using
two **non-colliding, composable** compressors:

```
Input Text
    │
    ▼
┌──────────────┐     ┌──────────────┐
│   Caveman    │────▶│     PFC1     │────▶ Compressed Output
│ (style lossy)│     │(phonetic     │      (with optional header)
│              │     │ lossless)    │
└──────────────┘     └──────────────┘
  ASCII/Latin           Cherokee
  prose only            syllabary
  U+0000-U+007F         U+13A0-U+13FF
```

### 7.2 Caveman Compressor (`caveman.rs`)

**Lossy style compressor** for ASCII/Latin prose. Never touches code or
technical identifiers.

**Levels:**
| Level | Behavior |
|---|---|
| `lite` | Drop pleasantries, fillers, multi-word phrases |
| `full` | + Drop articles (a/an/the), synonym replacements |
| `ultra` | + Drop hedging, causal arrows (→), prose abbreviations (database→DB) |
| `wenyan-*` | Reserved (requires zh corpus, currently skips) |

**Safety:**
- `is_technical()` detects CamelCase, digits, separators → never rewritten
- `DESTRUCTIVE` list: security/danger keywords → auto-clarity guard (skips compression)
- Code regions (fenced blocks) detected via `code_regions.rs` → passed through untouched
- `replace_whole_words()` ensures `user_database` is never corrupted when replacing `database`

### 7.3 PFC1 Compressor (`pfc1.rs`)

**Lossless phonetic/frequency key-dictionary compressor.** Replaces frequent
technical terms with **Cherokee syllabary symbols** (U+13A0–U+13FF, 3 UTF-8
bytes each).

**Alphabet:** 85 Cherokee symbols — a private code point space that cannot
appear in English/model output.

**Algorithm:**
1. `analyze_phonetic_pairs()` — tokenizes text, counts frequencies, extracts phrases (n-grams)
2. `calculate_heuristic_benefit()` — net benefit = `freq × (len − 3) − key_cost`
3. `generate_compression_key()` — assigns Cherokee symbols to highest-benefit terms (max 80-85)
4. `compress_text()` — whole-word replacement, longest-first for overlap safety
5. `generate_header()` — ASCII header with key mapping (self-describing mode)

**Code-aware compression:**
- `code_regions.rs` provides a pluggable detection engine with 3 built-in detectors:
  - `FencedCodeDetector` (priority 100): ``` / ~~~ blocks
  - `InlineCodeDetector` (priority 50): `...` spans
  - `BracketedCodeDetector` (priority 30): `{...}` / `[...]` balanced regions
- Only prose segments are compressed; code passes through untouched

**Two modes:**
| Mode | Header | Use Case |
|---|---|---|
| Self-describing | Embedded ASCII header with full key | Standalone `compress` tool, cross-system |
| Compact (header-less) | No header | Trusted a2a (both ends share server key) |

**Adaptive gate:** Never expands wire bytes. If `candidate.len() >= current.len()`,
the original passes through unchanged.

### 7.4 Pipeline (`pipeline.rs`)

Composable ordered pipeline of `StageSpec`s:
```rust
pub enum StageSpec {
    Caveman(CavemanLevel),
    Pfc1 { emit_header: bool },
}
```

`pipeline::run()` chains stages sequentially, recording per-stage output,
stats, and header bytes. Returns `PipelineResult` with `total_savings_bytes`,
`total_ratio`, and `total_header_bytes`.

### 7.5 Policy System (`policy.rs`)

Per-pane trim policies stored on `AgentHandle`:

```rust
pub struct TrimPolicy {
    pub stages: Vec<String>,     // e.g. ["caveman:full", "pfc1"]
    pub direction: TrimDirection, // None | Outbound | OutboundWithAck
}
```

**Direction semantics:**
- `None` — no transform (default)
- `Outbound` — compress before sending
- `OutboundWithAck` — compress outbound; auto-detect + decompress on `agent_read`

**Trim application flow (`apply_outbound_trim`):**
1. Explicit per-call `compress` stages win
2. Fall back to target's per-pane policy
3. Run pipeline with adaptive gate
4. Record stats + refresh badge

### 7.6 Statistics & Badges (`stats.rs`)

**Two metrics:**
- `workspace_net_pct()` = `net_saved / gross_saved × 100` (efficiency: what survived header tax)
- `savings_pct()` = `gross_saved / total_input × 100` (savings: what's shown on badge)

Stats persisted per-workspace to `data/sessions/{ws}.trim_stats.json`.

**Badge push:** After each trim, `push_badge_for_workspace()` sends
`herdr pane report-metadata --custom-status "-12%" --ttl-ms 25000` to all
panes with active policies.

### 7.7 PFC1 Memory Persistence (`runner.rs`)

- `PipelineRunner` owns the persistent base key
- After each run, the latest PFC1 key is merged into `data/pfc1_memory.json`
- Union merge: new symbols win, existing preserved
- Shared across CLI tools and MCP calls

### 7.8 Folder Keys (`folder_key.rs`)

Per-folder PFC1 keys for domain-specific compression:
- Scans folder recursively for text/markdown/code files
- Learns most compressible terms + phrases
- Writes `<folder>/.pfc1_key.json` (self-contained key)
- `MIN_FOLDER_SLOTS = 30` symbols always reserved for local terms
- Master key persistence: `data/pfc1_master_key.json` seeds future builds
- Central registry: `data/folder_keys/` for cross-folder visibility
- mtime-based caching: unchanged file set reuses the cached key

### 7.9 Legacy Dashboard (`dashboard.rs`)

Live ANSI TUI using crossterm:
- 2-second refresh cycle
- Reads `data/sessions/*.trim_stats.json`
- Shows per-workspace savings %, per-pane bar charts
- Quit with `q` or `Ctrl+C`
- RAII terminal restoration guard

### 7.10 Kitchen-Sink TUI Dashboard (`tui/`)

Full-featured terminal dashboard with VS Code-style tabs, mouse + keyboard
navigation, running as a herdr sidecar pane:

**5 tabs:**
| Tab | Content |
|---|---|
| Overview | Welcome, workspace stats, pane table with agent/status/role |
| Playground | Tool runner + recipe builder (same as web playground) |
| Trim | Trim savings dashboard + per-pane policy editor |
| Variables | Workspace-scoped variable editor |
| Settings | Runtime info, clipboard config, keyboard shortcuts |

**Architecture:**
- Runs **in-process on top of the `serve` stack** (A1 approach): the HTTP
  bridge + AgentRegistry + event subscriber come up headless, then the TUI
  takes over the terminal. MCP stdio is auto-disabled (`http_only=true`) so
  the TUI owns stdout.
- Reads pane data from the **live AgentRegistry** via `GET /api/agents`
  (canonical), falls back to `herdr pane list` CLI (diagnostic) if the bridge
  is unavailable.
- **Trim policy editor** — select a pane, add/remove/reorder pipeline stages
  (`caveman:lite/full/ultra`, `pfc1`), set direction, Get/Apply via the
  bridge's tool endpoints.
- **Mouse support**: Tab strip clicks, scroll wheel on lists, right-click
  context menu with copy actions.
- **Clipboard integration**: Copy pane text, tool results, or JSON via
  right-click menu; `clipboard_set`/`clipboard_get` tool integration in the
  Settings panel.
- Stats persisted per-workspace to
  `data/sessions/{ws}.trim_stats.json`.
- **Badge push:** After each trim, `push_badge_for_workspace()` sends
  `herdr pane report-metadata --custom-status "-12%" --ttl-ms 25000` to all
  panes with active policies.

---

## 8. Agent Registry & Event System

### 8.1 AgentRegistry (`herdr_client.rs`)

Thread-safe in-memory registry keyed by herdr pane ID:

```rust
pub struct AgentRegistry {
    inner: Arc<RwLock<HashMap<String, AgentHandle>>>,
    persistence: Arc<Persistence>,
}
```

**AgentHandle fields:** `pane_id`, `workspace_id`, `tab_id`, `agent`,
`role`, `label`, `status`, `output`, `trim_policy`, `updated_at`

**Resolution order:** pane_id (exact match) → role → label (optionally
scoped to workspace)

**Registry operations:** `upsert`, `set_status`, `set_output`, `get`,
`set_trim_policy`, `set_label`, `resolve`, `list_for_ws`, `inner_snapshot`,
`seed`

### 8.2 Event Subscriber (`HerdrClient`)

Connects to herdr's Unix socket (`~/.config/herdr/herdr.sock`) for live
events:

**Subscriptions:**
- `workspace.created` → re-subscribe all panes, seed default trim policy
- `workspace.closed` → send trim summary notification
- `pane.agent_status_changed` → update registry, capture work product on idle

**Transport:** Newline-terminated JSON requests/responses. Dedicated
long-lived socket with background writer task via `mpsc` channel.

### 8.3 Label Resolution

`registry.resolve(ws, target)` matches by:
1. **pane_id** (exact key match)
2. **role** (filtering by workspace if provided)
3. **label** (filtering by workspace if provided)

Used by `agent_spawn`, `agent_message`, `agent_read`, `agent_wait`,
`trim_policy_set/get`.

---

## 9. Persistence Layer

File-based storage under `data/`:

```
data/
├── recipes/{uuid}.json          # Saved recipes
├── executions/{uuid}.json       # Recipe execution results
├── variables/{key}.json         # Variable store
├── schedules/{uuid}.json        # Cron schedules
├── sessions/
│   ├── {session_id}.json        # Session variables
│   ├── {ws}.agents.json         # Agent registry blob
│   └── {ws}.trim_stats.json     # Trim statistics per workspace
├── pfc1_memory.json             # Persistent PFC1 key (learned terms)
├── pfc1_master_key.json         # Cross-folder master key
└── folder_keys/                 # Central folder key registry
```

**Key types:**
- `VariableStore`: per-key variables with session/execution scoping
- `SessionVars`: bag of variables shared across recipe runs within a session (herdr workspace ID)
- `ExecutionResult`: full execution record with results, variables, status, error

---

## 10. Scheduler

Cron-based recipe scheduling using the `cron` crate:

- `Scheduler::schedule_one()` spawns a background task per schedule
- Each task sleeps until next cron fire, then invokes the executor
- Executor function injected at runtime (currently not wired in `main.rs`)
- `DashMap` for concurrent schedule access
- Persistence: `data/schedules/{uuid}.json`
- Enable/disable without losing schedule state

---

## 11. Web UI (Dioxus 0.7)

### 11.1 Build System

- Crate `crates/herdr-mcp-ui`, `dioxus = "0.7"` (features `web`, `router`),
  compiled to `wasm32-unknown-unknown` by the `dx` CLI
- `dx serve` — dev server with hot reload; proxies `/api` to
  `http://localhost:7676/` (`Dioxus.toml [web.proxy]`)
- `dx bundle --platform web --release` — production build served by the bridge (§5.5)
- Styles: `assets/app.css` (plain CSS, no Node toolchain)

### 11.2 Routes

| Route | Component | Description |
|---|---|---|
| `/` | `Home` | Landing page |
| `/docs` | `Docs` | Documentation |
| `/overview` | `Overview` | Live agents from `GET /api/agents` |
| `/playground` | `Playground` | Tool runner (schema-driven form) + recipe builder/library |
| `/trim` | `Trim` | Trim savings, per-pane bars, active policies, diagnose/summary |
| `/variables` | `Variables` | User variables (CRUD/import) + auto variables |
| `/settings` | `Settings` | Runtime config + clipboard round-trip test |

### 11.3 Talking to the Bridge

- `App` builds one `ApiClient` from `window.location.origin` and provides it
  as `Signal<ApiClient>` context; components call `use_api()` and clone it
  into async tasks
- No server functions and no cached server state: every view resolves from
  `/api/*` on demand
- `src/vars.rs` holds the client-side `{{var}}` preview substitution (the
  server re-resolves on run)

---

## 12. Dependencies

### 12.1 Rust (Workspace)

| Crate | Version | Purpose |
|---|---|---|
| `rmcp` | 1.7 (server, transport-io, macros) | MCP protocol implementation |
| `tokio` | 1 (full) | Async runtime |
| `axum` | 0.8 | HTTP framework |
| `tower-http` | 0.6 (fs) | Static file serving for the web UI |
| `clap` | 4 (derive) | CLI argument parsing |
| `serde` / `serde_json` | 1 | Serialization |
| `schemars` | 1 | JSON Schema generation for MCP tools |
| `anyhow` | 1 | Error handling |
| `tracing` / `tracing-subscriber` | 0.1 / 0.3 | Structured logging |
| `regex` | 1 | Pattern matching |
| `uuid` | 1 (v4, serde) | Unique identifiers |
| `chrono` | 0.4 (serde) | Date/time |
| `cron` | 0.12 | Cron expression parsing |
| `dashmap` | 6 | Concurrent hashmap |
| `lazy_static` | 1 | Static regex compilation |
| `crossterm` | 0.28 | Terminal TUI for dashboard |
| `dirs` | 5 | XDG directory resolution |
| `toml` | 0.8 | Config file parsing |
| `walkdir` | 2 | Directory traversal |
| `tempfile` | 3 | Temp file management (dev) |

### 12.2 Rust (Dev/Test)

| Crate | Version | Purpose |
|---|---|---|
| `pretty_assertions` | 1 | Enhanced assertion diffs |
| `rstest` | 0.23 | Parameterized tests |
| `assert_cmd` | 2 | CLI integration testing |
| `predicates` | 3 | Assertion predicates |

### 12.3 Web UI

| Crate | Version | Purpose |
|---|---|---|
| `dioxus` | 0.7 (web, router) | UI framework (Rust → wasm) |
| `web-sys` | 0.3 (Window, Location) | Page origin for the API base URL |
| `reqwest` | 0.12 (json) | HTTP client inside `herdr-mcp-api` (fetch on wasm) |

---

## 13. Build & Run

### 13.1 Build Commands

```bash
# Rust binaries
cargo build --release                    # → target/release/{herdr-mcp,herdr-trim}

# Web UI
cd crates/herdr-mcp-ui && dx bundle --platform web --release
                                         # → target/dx/herdr-mcp-ui/release/web/public

# Development (Rust bridge + Dioxus hot reload)
cargo run --release --bin herdr-mcp -- serve --http-only   # Terminal 1 (port 7676)
cd crates/herdr-mcp-ui && dx serve                          # Terminal 2 (proxies /api)

# Tests
cargo test --workspace
cargo test -p herdr-mcp-server http_contract   # API wire-contract round-trip

# Lint + format
cargo clippy --all-targets -- -D warnings
cargo clippy -p herdr-mcp-api -p herdr-mcp-ui --target wasm32-unknown-unknown -- -D warnings
cargo fmt --all -- --check
```

### 13.2 Binary Subcommands

```bash
herdr-mcp                         # MCP stdio + HTTP bridge on port 7676
herdr-mcp serve                   # Explicit MCP + HTTP
herdr-mcp dashboard               # Kitchen-sink TUI (no legacy flag)
herdr-mcp dashboard --legacy      # Legacy trim-only dashboard
herdr-mcp trim --stage caveman:full --stage pfc1 "text to compress"
herdr-mcp trim --decompress "compressed text"
herdr-mcp trim --file input.txt
herdr-mcp folder-key build ./my-folder --data-dir ./data --max-terms 85
herdr-mcp folder-key list --data-dir ./data
herdr-mcp folder-key show ./my-folder
herdr-mcp folder-key decompress ./my-folder "compressed text"
```

### 13.3 Environment Variables

| Variable | Default | Description |
|---|---|---|
| `HERDR_BIN` | `"herdr"` | herdr CLI binary path |
| `HERDR_SOCKET_PATH` | `~/.config/herdr/herdr.sock` | herdr socket |
| `RUST_LOG` | `herdr_mcp=info` | Tracing filter |
| `HERDR_MCP_DATA_DIR` | `./data` | Data directory |
| `HERDR_MCP_HTTP_PORT` | — | HTTP port override |
| `HERDR_MCP_HTTP_ONLY` | — | HTTP-only mode |
| `HERDR_MCP_HTTP_BIND` | `127.0.0.1` | HTTP bind address |
| `HERDR_MCP_WEB_DIR` | in-repo `dx bundle` output | Built web UI directory |
| `HERDR_MCP_CONFIG` | — | Config file path |
| `HERDR_MCP_TOOL_TIMEOUT` | — | Tool call timeout (seconds) |
| `HERDR_MCP_MAX_CONCURRENT` | — | Max concurrent tool calls |
| `HERDR_MCP_HERDR_TIMEOUT` | — | herdr CLI timeout (seconds) |
| `HERDR_MCP_CLIPBOARD_COPY` | — | Clipboard copy command override |
| `HERDR_MCP_CLIPBOARD_PASTE` | — | Clipboard paste command override |
| `HERDR_MCP_LOG_LEVEL` | — | Log level override |
| `HERDR_MCP_LOG_FORMAT` | — | Log format (full, compact, json) |
| `HERDR_MCP_PFC1_MAX_SYMBOLS` | 85 | Max PFC1 Cherokee symbols |
| `HERDR_MCP_PFC1_ENABLE_PHRASES` | — | Enable multi-word PFC1 phrases |

---

## 14. Design Decisions & Trade-offs

### 14.1 Shell-out Architecture

**Decision:** Every tool shells out via `tokio::process::Command` with real
argv (no shell injection).  
**Rationale:** Herdr's wire protocol isn't publicly documented and may
change; the CLI is the stable, documented surface. Zero coupling, easy to
update.  
**Trade-off:** Higher latency per call vs. direct socket communication.

### 14.2 Stdout = MCP, Stderr = Logs

**Decision:** `stdout` is exclusively for MCP JSON-RPC; all tracing/logging
goes to `stderr`.  
**Rationale:** Prevents log output from corrupting the MCP protocol stream.

### 14.3 One Web UI, One Wire Contract

**Decision:** The Dioxus app is the only web frontend, served by the bridge
from a static directory; every `/api/*` type lives in `herdr-mcp-api`, shared
by the server, the web UI and the TUI.  
**Rationale:** One language and toolchain (no Node), and the
single-source-of-truth rule applied to the HTTP contract — a shape change is
one edit, checked by the contract test.  
**Trade-off:** Building the UI needs the `dx` CLI and the wasm32 target.

### 14.4 Non-Colliding Compressors

**Decision:** Caveman operates on ASCII/Latin prose (U+0000–U+007F); PFC1
uses Cherokee syllabary (U+13A0–U+13FF).  
**Rationale:** The two compressors cannot interfere with each other's output,
making composition trivially safe.

### 14.5 Adaptive Gate (Never Expand)

**Decision:** If `candidate.len() >= current.len()`, pass through unchanged.  
**Rationale:** Short messages drowned by PFC1 headers would expand rather
than compress. The gate prevents this.

### 14.6 Lossy vs. Lossless Split

**Decision:** `caveman` is lossy (drops articles/fillers, never technical
identifiers); `pfc1` is lossless.  
**Rationale:** Style compression is inherently lossy; dictionary compression
must be reversible for a2a communication. The a2a path
(`agent_message`→`agent_read`) uses compact header-less PFC1 and is
**lossless end-to-end**.

### 14.7 File-Based Persistence

**Decision:** All state (recipes, schedules, variables, stats, agent
registry) persisted as JSON files under `data/`.  
**Rationale:** Simple, inspectable, no database dependency. Adequate for
single-server deployment.

### 14.8 Workspace-Scoped Variables

**Decision:** Session variables use herdr workspace ID as scope.  
**Rationale:** Recipes run within a workspace context; variables should chain
across runs in the same workspace but not leak between workspaces.

### 14.10 Clipboard Detection & Override

**Decision:** Clipboard commands are auto-detected from the environment
(pbcopy/pbpaste on macOS, wl-copy/wl-paste on Wayland, xclip/xsel on X11)
with env-var overrides (`HERDR_MCP_CLIPBOARD_COPY`, `HERDR_MCP_CLIPBOARD_PASTE`)
and config-file overrides (`clipboard.copy_command`, `clipboard.paste_command`).  
**Rationale:** The clipboard is needed for the TUI's copy/paste operations; no
single command works across all platforms and display servers. Env/config
overrides let users pin a specific tool.  
**Detection order:** environment override → config override → platform auto-detect.

---

## 15. Key Architectural Patterns

1. **Tool-as-function pattern:** Each MCP tool is an `async fn` on
   `HerdrMcpServer` with `#[tool]` macro attributes. The `#[tool_router]`
   macro auto-generates dispatch.

2. **Dual dispatch:** The same tool functions serve both MCP (via
   `tool_router`) and HTTP (via `dispatch_tool` match statement). Manual HTTP
   handlers exist for routes that need custom response shaping.

3. **Registry-driven a2a:** The live `AgentRegistry` (fed by herdr events)
   enables `agent_spawn`→`agent_message`→`agent_read` chains where one
   agent's output becomes another's input.

4. **Pipeline composition:** Trim stages compose as an ordered list. Each
   stage is independent and the pipeline tracks per-stage stats, making it
   easy to add new compressor types.

5. **RAII terminal management:** The dashboard uses `RestoreTerm` guard
   struct to ensure terminal state is restored on any exit path.

6. **Pluggable code detection:** The `CodeDetector` trait + `CodeRegionRegistry`
   pattern allows adding new language-specific detectors without modifying
   core compression logic.

---

## 16. Known Gotchas

1. **Folder key non-determinism:** `build_folder_key` writes its output INTO
   the scanned root folder, so a second scan re-ingests the JSON file →
   produced key is non-deterministic across separate builds (HashMap
   iteration order). A single persisted-on-disk key IS stable.

2. **Config merge semantics:** `Config::merge` takes `other` wholesale for
   every field (no per-field default-preservation). If `other.data_dir` is
   empty, it overwrites the base.

3. **IDs may compact:** Session-local IDs (workspace, pane, tab) may change
   when items are closed. Always re-read from list commands after structural
   changes.

4. **Event subscriber only on Unix:** The herdr event subscriber uses Unix
   sockets. On non-Unix platforms, the registry is populated only by explicit
   tool calls.

5. **`pane_id` vs `id` field name:** The `herdr pane list` CLI output may
   return the pane identifier as either `pane_id` or `id`. The `resolve_pane_id`
   function in the server and the TUI fallback both handle this via
   `.or_else(|| p.get("id"))`.
