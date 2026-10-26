# herdr-mcp — Architecture Reference

Comprehensive architecture reference for the herdr-mcp project: data flow,
tool reference, HTTP API, recipe engine, agent registry,
persistence, scheduler, frontend, dependencies, build/run, design decisions,
and known gotchas.

---

## 1. Project Overview

**herdr-mcp** is an MCP (Model Context Protocol) server written in Rust that
exposes [herdr](https://herdr.dev) — a terminal-native agent multiplexer —
as MCP tools. It enables AI clients (Claude Desktop, Cursor, Claude Code,
OpenCode) to control herdr workspaces, tabs, panes, and agents. It also
includes an HTTP bridge, a Dioxus (Rust → wasm) web playground, a TUI
dashboard, and a recipe engine for chaining tool calls with variable
interpolation.

**License:** AGPL v3
**Current test suite:** run `cargo nextest run --workspace` (226 passing)
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
│  │  - 46 tool definitions (discovery, lifecycle, read,     │ │
│  │    write, synchronize, a2a, variables, templates,       │ │
│  │    scheduler, clipboard)                                │ │
│  │  - Recipe engine with {{ variable }} interpolation      │ │
│  │  - Scheduler for cron-based recipe runs                 │ │
│  └──────────┬──────────────────────────────────────────────┘ │
│             │                                                │
│     ┌───────▼────────┐                                       │
│     │ herdr CLI      │                                       │
│     │ (tokio::process│                                       │
│     │  ::Command)    │                                       │
│     └───────┬────────┘                                       │
│             │                                                │
│     ┌───────▼────────────┐  ┌──────────────────────────┐    │
│     │ herdr daemon       │  │ Persistence (file-based)  │    │
│     │ (workspaces,       │  │ data/recipes/*.json       │    │
│     │  tabs, panes,      │  │ data/sessions/*.json      │    │
│     │  agents)           │  │ data/schedules/*.json     │    │
│     └────────────────────┘  └──────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│  Event Subscriber (herdr_client.rs)                         │
│  Unix socket → herdr daemon                                 │
│  Subscribes to: herdr's global events, plus                 │
│                 pane.agent_status_changed per pane          │
│  Populates AgentRegistry (in-memory RwLock<HashMap>)        │
│  Captures work product on idle transitions                  │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 Front Ends on One Server

One process serves the HTTP bridge (Axum, `--http <port>`; none with
`--mcp-only`) plus one of:

1. **TUI dashboard** — in a terminal (stdin is a TTY), the default
2. **MCP stdio** (JSON-RPC over stdin/stdout) — when an MCP client launches
   it (stdin is a pipe), or with `--no-tui`; `--mcp-only` drops the bridge
3. nothing more — `--http-only` (`http.http_only`)

All share the same `HerdrMcpServer` instance, tool dispatch, and recipe
engine. The HTTP bridge exposes REST endpoints that mirror the MCP tools.

### 2.3 CLI Architecture

One binary, `herdr-mcp`. With no subcommand it serves (mode as above); its
flags are `--no-tui`, `--http`, `--http-only`, `--mcp-only`, `--data-dir`,
`--herdr-socket`.
The only subcommand is `plugin` (manifest + the entry points herdr runs).
While the TUI owns the terminal, tracing goes to `<data_dir>/herdr-mcp.log`.

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
│   │       ├── agents.rs   # AgentHandle, AgentsResponse
│   │       ├── recipe.rs   # Recipe, RecipeStep, ExecutionResult, templates, recipe requests
│   │       ├── variables.rs # VariableStore, resolve, variable/tool envelopes
│   │       ├── playground.rs # Collections model shared by web + TUI: tool/method forms, extract paths, request saves
│   │       └── client.rs   # Typed ApiClient + ApiError
│   ├── herdr-mcp-core/     # Config
│   │   └── src/
│   │       ├── lib.rs      # Re-exports Config, CliOverrides
│   │       ├── config.rs   # Full config system: TOML + env + CLI
│   │       └── session_context.rs # inside-herdr / pane id from herdr's env
│   ├── herdr-mcp-tui/      # TUI dashboard (crossterm + ratatui); bridge client only
│   │   └── src/
│   │       ├── lib.rs      # App state, event loop, mouse handling
│   │       ├── http.rs     # Bridge liveness probe (configured port only); every call goes through ApiClient
│   │       ├── theme.rs    # Color palette and styles
│   │       └── tabs/       # send, overview, playground, variables, settings
│   ├── herdr-mcp-server/   # MCP server, tools, HTTP bridge
│   │   └── src/
│   │       ├── lib.rs      # Re-exports HerdrMcpServer, Persistence, etc.
│   │       ├── server.rs   # Tool definitions + HTTP handlers + recipe engine
│   │       ├── herdr_client.rs # Socket client (events, raw API calls) + AgentRegistry
│   │       ├── runs.rs     # Background run engine (pause/resume/cancel, event log)
│   │       ├── profiles.rs # Agent profiles (drive any CLI agent)
│   │       ├── agents.rs   # Agent helpers shared by the a2a tools
│   │       ├── persistence.rs  # File-based storage
│   │       ├── scheduler.rs    # Cron-based recipe scheduling
│   │       ├── templates.rs    # Bundled recipe templates
│   │       └── variables.rs    # ScheduledRecipe + variable extraction (wire types from api)
│   ├── herdr-mcp-ui/       # Dioxus 0.7 web UI (see §11)
│   └── herdr-mcp-cli/      # The herdr-mcp binary
│       ├── src/main.rs           # Mode choice: TUI / MCP stdio / HTTP only; `plugin`
│       ├── src/common.rs         # Tracing, serve_core, run_serve, daemon lock
│       ├── src/plugin.rs         # herdr plugin manifest + entry points
│       └── tests/                # Integration tests
```

---

## 4. MCP Tool Reference (46 tools total)

### 4.1 Discovery Tools (7)

`session?` (a name from `list_sessions`) points a Discovery or Read tool at
another herdr session; the label in `get_pane`/`read_pane` is looked up
there too. Unset: the server's own session. Every other tool, and the run
engine, always acts on the server's own session.

| Tool | Parameters | Description |
|---|---|---|
| `status` | — | Overall herdr server status |
| `list_sessions` | — | Every named herdr session; `current` marks the server's own |
| `list_workspaces` | `session?` | List all workspaces in a session |
| `list_tabs` | `workspace_id?`, `session?` | List tabs, optionally by workspace |
| `list_panes` | `session?`, `workspace_id?`, `tab_id?`, `agent_status?`, `focused?`, `label?` | Typed `{panes: [HerdrPane]}`; every filter narrows (`label`: case-insensitive substring of label or agent) |
| `list_agents` | `session?` | List all detected agents |
| `get_pane` | `pane_id?`, `label?`, `session?` | Get pane details by ID or label |

### 4.2 Lifecycle Tools (4)

| Tool | Parameters | Description |
|---|---|---|
| `create_workspace` | `cwd?`, `label?`, `no_focus?` | Create a new workspace |
| `create_tab` | `workspace_id?`, `label?`, `cwd?` | Create a new tab |
| `split_pane` | `pane_id?`, `label?`, `direction?`, `cwd?`, `no_focus?` | Split a pane right or down |
| `close_pane` | `pane_id?`, `label?` | Close a pane |

### 4.3 Read Tools (1)

| Tool | Parameters | Description |
|---|---|---|
| `read_pane` | `pane_id?`, `label?`, `source?`, `lines?`, `session?` | Read pane output (visible/recent/recent-unwrapped) |

### 4.4 Write Tools (5)

| Tool | Parameters | Description |
|---|---|---|
| `send_text` | `pane_id?`, `label?`, `text` | Send text to pane (no Enter) |
| `send_keys` | `pane_id?`, `label?`, `keys` | Send key presses |
| `run_command` | `pane_id?`, `label?`, `command` | Atomic text+Enter |
| `pane_exec` | `pane_id?`, `label?`, `command`, `timeout_ms?`, `lines?`, `check?` | Run in the pane's shell, wait, return `{exit_code, output}` |
| `herdr_api` | `method`, `params?`, `timeout_ms?` | Any herdr socket API method; returns herdr's response |

### 4.5 Synchronize Tools (2)

| Tool | Parameters | Description |
|---|---|---|
| `wait_output` | `pane_id?`, `label?`, `match_text`, `timeout_ms?`, `source?`, `use_regex?` | Wait for text in pane (herdr's `pane wait-output`: text already there matches) |
| `pane_watch` | `pane_id?`, `label?`, `match_text`, `use_regex?`, `source?`, `timeout_ms?` | Poll the pane (500 ms, last 500 lines) and fire when the number of matching occurrences rises above what was there at the start; a drop (scrolled out, redrawn) lowers the bar. No timeout: until matched or the run is cancelled |

### 4.6 A2A Primitives (7)

| Tool | Parameters | Description |
|---|---|---|
| `agent_spawn` | `role`, `agent`, `args`, `cwd?`, `workspace_id?`, `tab_id?`, `split?`, `needs`, `wait_idle`, `label?`, `in_pane?` | Spawn agent (in a new split, or in the existing `in_pane`), register role→pane, wait for deps, capture output |
| `agent_prompt` | `target`, `text`, `timeout_ms?` | Prompt an agent and wait for its turn to finish; returns its reply |
| `agent_interrupt` | `target`, `hard` | Stop the agent's turn (profile `interrupt` keys) or quit it (`hard`) |
| `agent_message` | `target`, `text` | Send a message to an agent and submit it |
| `agent_read` | `target`, `source?`, `lines?` | Read + store as work product |
| `agent_wait` | `target`, `status?`, `timeout_ms?` | Wait for an agent status; `target` is a role, label or pane id |
| `agent_list` | `workspace_id?` | List registry agents |

### 4.7 Session Variables (2)

| Tool | Parameters | Description |
|---|---|---|
| `var_get` | `session_id`, `key` | Get session variable |
| `var_set` | `session_id`, `key`, `value` | Set session variable |

### 4.8 Recipe Template Tools (3)

| Tool | Parameters | Description |
|---|---|---|
| `list_templates` | — | List bundled templates |
| `get_template` | `template_id` | Get template by ID |
| `instantiate_template` | `template_id`, `variables`, `name?` | Instantiate template → recipe, auto-save |

### 4.9 HTTP & LLM Tools (3)

Outbound calls from the server (`outbound.rs`, reqwest + rustls). Timeouts
default to the `timeouts.http-request-ms` / `timeouts.llm-call-ms` settings.

| Tool | Parameters | Description |
|---|---|---|
| `http_request` | `method?`, `url`, `headers`, `query`, `body?` (text) or `json?`, `check?`, `timeout_ms?` | `{status, ok, headers, body}`; the response body parsed as JSON when it is. A non-2xx is a result unless `check` |
| `llm_call` | `model`, `prompt?`, `system?`, `messages`, `base_url?`, `api_key?`, `api_key_env?`, `temperature?`, `max_tokens?`, `extra?`, `timeout_ms?` | `POST {base_url}/chat/completions` (default OpenRouter); returns `{text, model, finish_reason, usage}`. Key: `api_key`, else env `api_key_env`, else `OPENROUTER_API_KEY` / `OPENAI_API_KEY` by host, else none |
| `llm_models` | `base_url?`, `api_key?`, `api_key_env?`, `filter?` | `GET {base_url}/models` as `{base_url, models: [{id, name, context_length}]}` |

The playground's request editor shows an endpoint builder and an LLM wizard
for these two tools instead of the schema form: on the web `HttpBuilder` /
`LlmWizard` (`playground/builders.rs`), in the TUI the `HttpRow` / `LlmRow`
form rows. Both edit the same raw values, and "Plain fields" switches back.
What they share (body and key modes, provider presets, key/value rows,
`load_models`) is in `herdr-mcp-api::playground`.

### 4.10 Recipe & Run Tools (6)

The MCP face of `/api/recipes` and `/api/runs`: the same server methods
(`create_recipe` / `update_recipe`, `runs::start_run` / `load` / `Runs::control`).

| Tool | Parameters | Description |
|---|---|---|
| `recipe_list` | — | Saved recipes: id, name, description, category, step ids, trigger count |
| `recipe_get` | `recipe_id` | One recipe |
| `recipe_save` | `recipe_id?`, `name?`, `description?`, `steps?`, `variables?`, `triggers?`, `category?` | Create (no id: `name` + `steps`) or update the fields given |
| `recipe_run` | `recipe_id?` or `steps?`, `name?`, `session_id?`, `variables`, `step_ids`, `wait` | Start a run; returns its first record, or the final one with `wait` |
| `run_get` | `run_id` | A run's record |
| `run_control` | `run_id`, `action` | Pause, resume or cancel (an error if it isn't executing) |

### 4.11 Scheduler Tools (4)

| Tool | Parameters | Description |
|---|---|---|
| `schedule_recipe` | `recipe_id`, `cron_schedule`, `enabled?` | Schedule recipe on cron |
| `list_schedules` | — | List active schedules |
| `delete_schedule` | `id` | Delete schedule |
| `enable_schedule` | `id`, `enabled` | Enable/disable schedule |

### 4.12 Clipboard Tools (2)

| Tool | Parameters | Description |
|---|---|---|
| `clipboard_set` | `text` | Write text to system clipboard |
| `clipboard_get` | — | Read text from system clipboard |

Both go through `clipboard.rs` (see §14.6).

---

## 5. HTTP Bridge API

### 5.1 Core Routes

| Method | Path | Handler | Description |
|---|---|---|---|---|
| `GET` | `/*` | `web::serve` / `ServeDir` fallback | Serves the Dioxus web UI built into the binary, or from `http.web-dir` (see §5.4) |
| `GET` | `/api/health` | `health_handler` | Returns `"ok"` |
| `GET` | `/api/settings` | `settings_handler` | `SettingsResponse`: every spec in `herdr_mcp_api::settings::SPECS` with default, bounds, value in effect, plus the config file's invalid entries (`warnings`) |
| `PUT` / `DELETE` | `/api/settings/{key}` | `save_setting_handler` / `reset_setting_handler` | Validate (range; chords: Ctrl/Alt or F-key, not reserved by a tab, not bound twice), write `[settings.<section>]` atomically, apply live; 400 with the reason, nothing written |
| `DELETE` | `/api/settings` | `reset_settings_handler` | Remove every override: all defaults |
| `GET` | `/api/tools` | `list_tools_handler` | Lists all tools with JSON schemas |
| `POST` | `/api/tools/{name}` | `call_tool_handler` | Dispatch tool by name |
| `GET` | `/api/agents?workspace_id=` | `agents_handler` | Live pane list from AgentRegistry |
| `GET` | `/api/workspaces` | `workspaces_handler` | Workspace list via herdr CLI |
| `GET` | `/api/herdr/sessions` | `herdr_sessions_handler` | `SessionsResponse`: every named herdr session, `current` = the bridge's |
| `GET` | `/api/herdr/panes?session=&workspace_id=&tab_id=&agent_status=&focused=&label=` | `herdr_panes_handler` | `PanesResponse`, filtered like `list_panes`; 502 for an unknown or stopped session |
| `POST` | `/api/runs` | `start_run_handler` | Start a run; `?wait=true` waits for the final record; `variables` add to the run's scope |
| `GET` | `/api/runs?limit=` | `list_runs_handler` | The newest `limit` runs (all without it), newest first; sorted on the headers, so only those runs' logs are read |
| `GET` | `/api/runs/{id}/trail` | `run_trail_handler` | The run's trail as text, the trail of the child run its gate handed off to indented under it |
| `GET` | `/api/herdr/events` | `herdr_events_handler` | SSE: herdr's live event stream (`HerdrEvent` per message) |

### 5.2 Recipe CRUD Routes

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/recipes` | List all recipes |
| `POST` | `/api/recipes` | Create recipe |
| `GET` | `/api/recipes/{id}` | Get recipe by ID |
| `PUT` | `/api/recipes/{id}` | Update recipe (incl. `triggers`) |
| `DELETE` | `/api/recipes/{id}` | Delete recipe |

### 5.3 Variable & Execution Routes

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/variables` | List all variables |
| `POST` | `/api/variables` | Save variable |
| `GET` | `/api/variables/{key}` | Get variable |
| `DELETE` | `/api/variables/{key}` | Delete variable |
| `POST` | `/api/variables/resolve` | Resolve `{{ }}` placeholders in a value, in the scope a run would start with |

### 5.4 Frontend Serving

The bridge serves the Dioxus bundle built into the binary:

- `herdr-mcp-server/build.rs` turns every file of the `dx bundle --platform web
  --release` output (`<target>/dx/herdr-mcp-ui/release/web/public`) into an
  `include_bytes!` entry; `web.rs` serves them. Build order: `dx bundle`, then
  `cargo build`. No bundle at build time → no entries.
- `http.web-dir` (TOML) / `HERDR_MCP_WEB_DIR` (env) serves a bundle directory
  from disk with `tower_http::services::ServeDir` instead (UI development)
- Any path that isn't an `/api/*` route or a file falls back to `index.html`,
  so Dioxus router deep links (`/playground`, `/variables`, …) load the app
- A missing `index.html` logs a `tracing::warn!` with the build command;
  `/api/*` keeps working
- No CORS layer: the UI is same-origin with the API (and `dx serve` proxies
  `/api` server-side via `Dioxus.toml [web.proxy]`)
- Binds `http.bind-addr` (default `127.0.0.1`). With `[security]
  require-token`, or on any non-loopback bind, `/api/*` (not `/api/health`)
  needs the bridge token (`<data-dir>/bridge.token`, 0600) as
  `Authorization: Bearer`, or `?token=` for `EventSource`; startup prints a
  `/#token=` link the web UI keeps the token from
- Refuses a foreign `Host` (DNS rebinding) and a foreign browser `Origin`
  (cross-site simple requests); requests without `Origin` pass

### 5.4.1 Security (`[security]`)

Every tool call, whoever makes it, goes through `HerdrMcpServer::call_tool`
(`server.rs`), which checks `security.rs`'s policy before routing and
appends to the audit log after:

- **Origin:** the in-process session (`dispatch_tool`) tags each call
  `api` (`POST /api/tools/{name}`) or `run` (a recipe step) in the request
  `_meta`; only that session's tag is read, so any other caller is `mcp`.
- **Policy:** `disabled-tools` are refused (and hidden from both tool
  lists). `[mcp] tools` only narrows what external MCP clients list
  (`CORE_TOOLS` by default; `lists` matches core, all, a category or a
  name). The in-process session and `/api/tools` see every enabled tool.
  `get_info` sends `INSTRUCTIONS` (herdr's skill rules, said in these
  tools' terms) and enables resources, so `herdr-mcp://agent-guide` serves
  AGENT-GUIDE.md. `caller_pane` is the client's `HERDR_PANE_ID`, inherited
  by this stdio process, and `mcp` calls only: `split_pane` and
  `agent_spawn` split it by default (`agents::split_direction`: right if
  width ≥ 2 × height, else down) with `--no-focus`. Unless `agent-triggers` is on, only `api` calls may add triggers,
  change a recipe that has triggers or a schedule, or call
  `schedule_recipe` / `enable_schedule`.
- **Outbound** (`outbound.rs`, `http_request` and the LLM tools): a
  `Guard` checks `allow-hosts`, then every address (a custom DNS resolver
  drops refused ones, IP literals are checked directly) and every redirect
  hop, which herdr-mcp follows itself, dropping credentials across origins.
  Link-local/metadata addresses and the bridge's own loopback port are
  always refused; `block-private` adds loopback and private ranges.
  Provider keys go only to their exact hosts; `api_key_env` must be in
  `key-env`.

### 5.5 Wire Contract

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
{{ greeting }}                       a variable in scope
{{ step1.panes[0].pane_id }}         a step's result (or the JSON its tool returned)
{{ step1 }} / {{ step1.text }}       the text a step's tool returned
{{ pane_id.output }} {{ role.output }}
```

**Scope** (`runs::scope`, lowest priority first): live agent handles by pane
id and role (every workspace's, then the run's), `template.session_id` /
`template.workspace_id` (the run's workspace), globals (`/api/variables`),
the recipe's `variables`, the run request's `variables`. Step results and
extracted variables go on top as the run proceeds. `POST
/api/variables/resolve` resolves in the same scope, so a preview never
disagrees with a run.

**Extract rules** chain requests: a step's `extract: [{path, var, scope}]`
saves `path` of its result (`""` = the text it returned) as `var` once the
step succeeds — `scope: "global"` into `/api/variables`, `"collection"` into
the recipe's `variables` (needs a saved recipe). Later steps of the same run
see it at once (`variable_set` run event). A path that isn't in the result
fails the step.

**Key spec facts:**
- `extract_variables` always prefixes keys with `result.` (e.g., `{"result":"hello"}` → `"result.result"`) and ignores arrays
- an exact scope key wins over `step.path` navigation
- Unknown `{{ path }}` placeholders are left unchanged

### 6.2 Recipe Lifecycle

```
1. POST /api/runs (or the scheduler / a trigger / the plugin picker):
   a recipe (optionally only `step_ids`) or ad-hoc `steps`
2. Build the scope (6.1); persist the run header
3. For each step:
   a. resolve_variables() in its params
   b. dispatch_tool() — the tool through rmcp's router (in-process MCP session)
   c. record the result (step_finished) under the step id, flattened, and
      under pane_id/role for agent handles
   d. apply its extract rules (variable_set), or fail the step
4. After the last step, the recipe's gate (if any; skipped when only
   `step_ids` run) picks its first matching rule (gate_decided):
   end → completed, fail → `error` (its message, or the rule that failed)
   + failed, restart → `pass` + 1 and back to 3 after `delay_ms`, run →
   hand off (below). No match completes the run.
5. The event log is the run; RunRecord is its fold
```

**Trail.** The event log is also the run's audit trail. `step_started`
carries the params as sent (placeholders filled, secret-named fields
redacted as in the security audit log), the `{{ }}` nothing resolved, and
the pane the step targets; the header records who started the run
(`origin`: api, agent + its pane, run + the run whose step called
`recipe_run`, gate + the run that handed off, schedule, trigger + event;
`RunOrigin::parent` gives the parent run);
`gate_decided` lists every check evaluated with what its path resolved to.
When a run fails (a step, a gate `fail`, `max_steps`), `pane_snapshot` saves
the last `snapshot_lines` lines of the pane it touched last (a step's
`pane_id`, or an agent a step returned). `herdr_mcp_api::trail_text`
renders it all as text (`run_get` with `trail: true`).

**Retention.** After each run, and for every group at daemon start,
finished runs of the same collection (ad-hoc runs and deleted collections'
runs form one group) beyond the newest `keep_runs`, or older than
`keep_days`, are deleted. A collection's `trail` fields win over the global
`runs.keep-runs` / `runs.keep-days` / `runs.snapshot-lines` settings; 0 means
no limit, and the defaults keep every run (snapshot: 40 lines).

**Gate** (`herdr_mcp_api::gate`, shared by the server and both UIs): an
ordered list of rules; each has `checks` (all must hold, or any with
`any: true`; none = always) and a `then` action. A check resolves `path`
like a `{{ path }}` placeholder in the run's scope and compares it with
`value`: `eq` / `ne` (a string and a number compare by text), `lt` / `gt`
(numbers), `contains` (substring, array element, object key), `exists` /
`missing` (null and `""` count as missing). `{{ run.pass }}` is the pass
number from 1. A gated run executes at most `max_steps` requests (default
1000, every pass counted), then fails. Restart loops run in the one
executor task: no recursion, and cancel interrupts a restart delay.

**Hand-off** (`Executor::hand_off`): a `run` action starts the other saved
collection as a child run (`start_with`, origin `gate` + this run's id) in
this run's workspace, its scope built like any run's (its own variables,
globals) plus `inputs` with their `{{ }}` filled from this run. The parent
records `child_started`, forwards pause / resume / cancel to the child
while it waits (reading as paused itself while it forwards a pause), and ends with the child's status (a failed child's error,
prefixed with its name, becomes the parent's). Before starting it walks
the `gate` origins up (`ancestors`): a collection already in the chain
fails the run, so hand-offs can't nest forever (`restart` is the loop).
`start_child` boxes the start as a `Send` future so the recursion has a
type to stop at. `runs::trail` (`run_get` trail, `GET /api/runs/{id}/trail`)
prints a run's trail with each child's indented under it.
`gate::rule_text` / `parse_rule` give a rule a one-line form
(`code gt 399 or read.text contains "error" → fail msg`, `always → end`):
the web's gate editor previews it under each rule, the TUI edits rules as
that text, and the trail prints decisions with it. A `run` names its
collection by name (`playground::collections` gives the UIs the list) or
id, then `key=value` inputs (`inputs_text` / `parse_inputs`).

**Runs view.** Both playgrounds have a Runs view (web: the Runs tab,
`playground/runs.rs`; TUI: the tree's Runs row, `tabs/playground/runs.rs`):
the run list, newest first, filtered by collection or failed only, and the
picked run's trail (`/api/runs/{id}/trail`, so a hand-off's child trail is
included). The list is the newest `playground::RUNS_SHOWN` runs. The web
re-reads the list, and a live run's trail, when `GET /api/events` (SSE)
streams a run event, at most every 300 ms. From there: copy the
trail, focus the pane it last used in herdr (`herdr_api` `pane.focus`), run
it again (`RunRecord::rerun`), cancel it, or open its parent run or the
runs it started (TUI: `p` / `d`). The collection editor has the
gate (`playground/flow.rs` on the web) and the run history fields; both
flag a `run` whose collection no longer exists (`missing_collection`).

**Tracing.** Each run's task runs in a `run` span (`run_id`, `collection`),
and each step's tool call in a `step` span (`step`, `tool`). So every log
line the run causes, down to herdr calls, names its run. A hand-off's child
span nests under its parent's; other runs are root spans. The executor logs
the start (with origin), the finish (status, passes) and failed steps.

### 6.3 Bundled Templates

| ID | Name | Category | Description |
|---|---|---|---|
| `dev-watch` | Dev Watch | development | Watch files, notify agent |
| `git-status` | Git Status | git | Run git status + read output |
| `restart-agent` | Restart Agent | agents | Close + start agent |
| `build-and-test` | Build & Test | development | Build → wait → test |
| `health-check` | Health Check | monitoring | Status + workspaces + panes + agents |

---

## 7. TUI Dashboard (`crates/herdr-mcp-tui`)

What `herdr-mcp` opens when run in a terminal: VS Code-style tabs, mouse +
keyboard navigation, sized for a herdr sidecar pane.

**5 tabs**, opening on Send. The global keys come from the `keys.*`
settings (Settings → Options, applied live); the defaults: next / previous
`Alt+→` / `Alt+←` (any terminal) or `Ctrl+Tab` / `Ctrl+Shift+Tab`; directly
`Alt+1`–`Alt+5` or `Ctrl+1`–`Ctrl+5`; `Ctrl+r` refresh, `Ctrl+q` quit.
Without the kitty keyboard protocol (VTE terminals, for one) Ctrl+Tab
arrives as a plain Tab and Ctrl+1 / Ctrl+3 as `1` / Esc, so use the Alt
keys there. A binding must use Ctrl or Alt (or an F-key), so none of them
type text or edit a field:
| Tab | Content |
|---|---|
| Send | Pick a pane (labeled first), type, Enter sends — `send_text`, or `run_command` with "run the command" (`Ctrl+t`); addressed by pane id |
| Overview | Stats, pane table (tab/label/agent/status/cwd) for the shown session (`s` cycles sessions), and the picked pane's text below it: re-read every `intervals.tui-pane-ms` (screen, or scrollback with `r`), scrollable, mouse drag selects (reading pauses while a selection is up), Ctrl+C or right-click Copy copies the selection or the whole text, Esc clears; Tab moves focus between table and text |
| Playground | Collections, as on the web: a tree of collections (saved recipes) and their requests; edit a request's tool or herdr method, params (resolved preview) and extract rules, Send it (Ctrl+G) or run the collection, pick a result path to extract it; collection variables and triggers; new from a template. HTTP and LLM requests get the endpoint builder and LLM wizard (Alt+Enter for a new line in a body or prompt). Ctrl+E shows herdr's live event stream |
| Variables | Workspace-scoped variable editor |
| Settings | Options (every `/api/settings` knob: Enter edit, Del default, Shift+Del all defaults), runtime info, clipboard config |

**Architecture:**
- Runs **in-process on top of the serve stack** (`serve_core`): the HTTP
  bridge + AgentRegistry + event subscriber come up headless, then the TUI
  takes over the terminal. No MCP stdio in this mode, and tracing goes to
  `<data_dir>/herdr-mcp.log` so nothing writes over the TUI. If another
  herdr-mcp already serves the port, the bind fails (logged) and the TUI
  talks to that bridge.
- Panes come from herdr via `GET /api/herdr/panes` (the shown session),
  joined by pane id with the registry (`GET /api/agents`, own session only)
  for role and captured output. Writes (Send, paste-to-pane) refuse while
  another session is shown.
- **Mouse support**: Tab strip clicks, scroll wheel on lists, right-click
  context menu with copy actions.
- **Clipboard integration**: Copy pane text, tool results, or JSON via
  right-click menu; `clipboard_set`/`clipboard_get` tool integration in the
  Settings panel.

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
`role`, `label`, `status`, `output`, `updated_at`

**Resolution order:** pane_id (exact match) → role → label (optionally
scoped to workspace)

**Registry operations:** `upsert`, `set_status`, `set_output`, `get`,
`set_label`, `resolve`, `list_for_ws`, `inner_snapshot`,
`seed`

### 8.2 Event Subscriber (`HerdrClient`)

One `events.subscribe` connection to herdr's socket. Subscriptions: every
parameterless type in herdr's schema (`herdr api schema --json` →
`Subscription.oneOf`, never hand-listed) plus `pane.agent_status_changed`
per live pane. herdr reads nothing more on a streaming connection, so a
`pane.created` / `pane.closed` reconnects with the new pane set.

herdr names global events `pane_created` on the wire; `HerdrEvent::parse`
(`herdr-mcp-api`) maps them to the subscription name (`pane.created`).
Every event is broadcast (`HerdrClient::events()`) to recipe triggers and
`GET /api/herdr/events`, and handled locally:
`pane.agent_status_changed` updates the registry and captures the work
product on idle.

### 8.3 Recipe Triggers (`triggers.rs`)

`Recipe.triggers: [{ on, when }]`: an event named `on` whose `data` equals
every `when` entry (dotted paths, e.g. `agent_status = "done"`) starts a run
in the event's workspace with `herdr` = the event data + `event`
(`{{herdr.pane_id}}`). Triggers are read from the saved recipes on every
event. A recipe whose triggered run is still going is skipped, so it cannot
re-trigger itself.

### 8.4 Label Resolution

`registry.resolve(ws, target)` matches by:
1. **pane_id** (exact key match)
2. **role** (filtering by workspace if provided)
3. **label** (filtering by workspace if provided)

Used by `agent_spawn`, `agent_message`, `agent_read`, `agent_wait`.

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
│   └── {ws}.agents.json         # Agent registry blob
├── daemon.lock                  # Held by the data dir's daemon (schedules, triggers)
└── herdr-mcp.log                # Tracing while the TUI owns the terminal
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
- Executor: the run engine (wired in `HerdrMcpServer::with_config`)
- Persisted schedules start in `serve_core`, which builds the one
  `HerdrMcpServer` shared by HTTP, stdio, the TUI and triggers
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
- `dx bundle --platform web --release` — production build served by the bridge (§5.4)
- Styles: `assets/app.css` (plain CSS, no Node toolchain)

### 11.2 Routes

| Route | Component | Description |
|---|---|---|
| `/` | `Home` | Landing page: send to a pane by `pane_id` and/or `label` (typed, or picked from dropdowns of the panes and labels in `GET /api/variables/auto`; `ApiClient::send_to_pane` → `send_text` / `run_command`, `pane_id` wins), remembers the last target in `localStorage`; collapsed getting-started notes; pane table at the bottom from the same auto variables (click a row to target it) |
| `/overview` | `Overview` | Pane browser: `GET /api/herdr/panes` with session/status/label filters, grouped by workspace and tab |
| `/playground` | `Playground` | Collections, Postman style (`playground/collections.rs`): a collection is a saved recipe (folders = `category`), its requests are the steps. A request is any tool or a `herdr_api` method (picked from herdr's schema); its params preview through `POST /api/variables/resolve`; Send saves it and runs just that step (`step_ids`); clicking a path in the response saves its value as a collection variable and adds an extract rule (scope `collection`) so later sends update it. The collection view edits its variables, request order and triggers and runs it in order. Globals reuse the Variables page table. Plus the herdr Events tab. Pane fields offer the auto-variable pane dropdowns (`components/pane_options.rs`) |
| `/variables` | `Variables` | Globals and each collection's own variables (CRUD/rename/import, `playground::save_global` / `save_collection_var`) + auto variables (read-only). The TUI's Variables tab lists the same three groups |
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
| `crossterm` | 0.28 | Terminal TUI for dashboard |
| `dirs` | 5 | XDG directory resolution |
| `toml` | 0.8 | Config file parsing |
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
cargo build --release                    # → target/release/herdr-mcp

# Web UI
cd crates/herdr-mcp-ui && dx bundle --platform web --release
                                         # → target/dx/herdr-mcp-ui/release/web/public

# Development (Rust bridge + Dioxus hot reload)
cargo run --release --bin herdr-mcp -- --http-only   # Terminal 1 (port 7676)
cd crates/herdr-mcp-ui && dx serve                          # Terminal 2 (proxies /api)

# Tests
cargo nextest run --workspace
cargo nextest run -p herdr-mcp-server http_contract   # API wire-contract round-trip

# Lint + format
cargo clippy --all-targets -- -D warnings
cargo clippy -p herdr-mcp-api -p herdr-mcp-ui --target wasm32-unknown-unknown -- -D warnings
cargo fmt --all -- --check
```

### 13.2 Command Line

```bash
herdr-mcp                         # terminal: TUI + HTTP bridge; piped stdin: MCP stdio + HTTP
herdr-mcp --no-tui                # MCP stdio + HTTP bridge
herdr-mcp --http-only             # HTTP bridge only
herdr-mcp --mcp-only              # MCP stdio only, no bridge
herdr-mcp plugin manifest         # print the herdr plugin manifest (see §13.3)
```

### 13.3 herdr Plugin

The repo-root `herdr-plugin.toml` is generated from
`crates/herdr-mcp-cli/src/plugin.rs::manifest` (`herdr-mcp plugin manifest >
herdr-plugin.toml`; a test fails when they differ), so `herdr plugin install
eugeneb50/herdr-mcp` works and the repo can carry the `herdr-plugin`
marketplace topic. `[[build]]` runs `scripts/plugin-build.sh` (release
`herdr-mcp` into `bin/`, web UI when `dx` exists); every command
runs `./bin/…` from the plugin directory and resolves port and data dir from
config at run time. With no `data_dir` configured, a process herdr runs as this
plugin (`HERDR_PLUGIN_ID=herdr-mcp`) defaults to `HERDR_PLUGIN_STATE_DIR`, not
the checkout (replaced on update). Project-local config
(`herdr-mcp.toml` in the working directory) is skipped as the
plugin: that directory is the checkout. `[[startup]]` and the
`workspace.created` event hook run `plugin startup`: open the `bridge` pane
unless a bridge already answers on `http.port` (a fresh server with no
workspace yet waits for the first one). The recipe picker, web action and TUI
dashboard only talk to the bridge on the configured port (no fallback ports:
another bridge may serve another data dir).

| Action (`herdr-mcp.<id>`) | Opens |
|---|---|
| `recipes` | popup: pick a saved recipe, run it with the invocation context as `{{herdr.…}}` (`workspace_id`, `pane_id` = focused pane, `selected_text`, …), follow its steps |
| `bridge` | tab: `herdr-mcp --http-only` |
| `dashboard` | split: `herdr-mcp` (the TUI) |
| `web` | the web playground in a browser |

Bind one in herdr's config: `[[keys.command]] key = "prefix+r"`,
`type = "plugin_action"`, `command = "herdr-mcp.recipes"`.

### 13.4 Environment Variables

| Variable | Default | Description |
|---|---|---|
| `HERDR_BIN` | `"herdr"` | herdr CLI binary path |
| `HERDR_SOCKET_PATH` | `~/.config/herdr/herdr.sock` | herdr socket |
| `RUST_LOG` | `herdr_mcp=info` | Tracing filter |
| `HERDR_MCP_DATA_DIR` | `./data` | Data directory |
| `HERDR_MCP_HTTP_PORT` | — | HTTP port override |
| `HERDR_MCP_HTTP_ONLY` | — | HTTP bridge only (no TUI, no MCP stdio) |
| `HERDR_MCP_HTTP_BIND` | `127.0.0.1` | HTTP bind address |
| `HERDR_MCP_WEB_DIR` | the web UI built into the binary | Serve a `dx bundle` directory from disk instead |
| `HERDR_MCP_CONFIG` | — | Config file path |

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

### 14.4 File-Based Persistence

**Decision:** All state (recipes, schedules, variables, stats, agent
registry) persisted as JSON files under `data/`.  
**Rationale:** Simple, inspectable, no database dependency. Adequate for
single-server deployment.

### 14.5 Workspace-Scoped Variables

**Decision:** Session variables use herdr workspace ID as scope.  
**Rationale:** Recipes run within a workspace context; variables should chain
across runs in the same workspace but not leak between workspaces.

### 14.6 Built-in Clipboard & Command Override

**Decision:** `clipboard_set` / `clipboard_get` use the OS clipboard through
[arboard](https://github.com/1Password/arboard) (macOS, Windows, X11; Wayland
via the data-control protocol, falling back to XWayland). When both
`clipboard.copy-command` and `clipboard.paste-command` are set in the config
file (Settings writes them; no env override, so a saved choice sticks) the
server runs those commands instead (argv split on whitespace, no shell).
Saving both empty from Settings removes them.  
**Rationale:** no tool to install on the common desktops; the commands remain
for compositors without data-control or a preferred tool.  
**Handle lifetime:** one arboard handle per process (`clipboard::NATIVE`),
used on a blocking thread. On X11/Wayland the copying process serves the text
while the handle lives, so a copy outlives the call but not the process unless
a clipboard manager takes it. Pure-Rust X11/Wayland backends: no system
libraries linked.

---

## 15. Key Architectural Patterns

1. **Tool-as-function pattern:** Each MCP tool is an `async fn` on
   `HerdrMcpServer` with `#[tool]` macro attributes. The `#[tool_router]`
   macro auto-generates dispatch.

2. **One dispatch:** MCP clients, `POST /api/tools/{name}` and recipe runs
   all reach tools through rmcp's `tool_router`: the bridge and run engine
   hold an in-process MCP client session to the server (`tool_session`,
   opened on first use). Each tool's `_meta.category` groups it in both
   playgrounds. Manual HTTP handlers exist only for custom response shapes.

3. **Registry-driven a2a:** The live `AgentRegistry` (fed by herdr events)
   enables `agent_spawn`→`agent_message`→`agent_read` chains where one
   agent's output becomes another's input.

4. **RAII terminal management:** The dashboard uses `RestoreTerm` guard
   struct to ensure terminal state is restored on any exit path.

---

## 16. Known Gotchas

1. **Config merge semantics:** `Config::merge` takes `other` wholesale for
   every field (no per-field default-preservation). If `other.data_dir` is
   empty, it overwrites the base.

2. **IDs may compact:** Session-local IDs (workspace, pane, tab) may change
   when items are closed. Always re-read from list commands after structural
   changes.

3. **Event subscriber only on Unix:** The herdr event subscriber uses Unix
   sockets. On non-Unix platforms, the registry is populated only by explicit
   tool calls.

4. **One pane list:** every caller that needs herdr's panes (label lookup,
   agent placement, auto variables, the event
   subscriber, `list_panes`, `/api/herdr/panes`) goes through
   `panes::panes(&PaneFilter)` in `herdr-mcp-server/src/panes.rs`, typed as
   `herdr_mcp_api::HerdrPane`. herdr 0.9 always names the id `pane_id`.
   Another session is reached by running `herdr` with that session's
   `HERDR_SOCKET_PATH` (from `herdr session list --json`).
