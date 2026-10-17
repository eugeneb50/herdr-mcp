# herdr-mcp — Master Spec (Final Product)

> Status: **DRAFT for review**. Consolidates all decisions to date:
> Dioxus 0.7 stable thin-client, frozen `/api/*`, big-bang web+TUI replacement,
> `serve`-vs-tools binary split, tiered tool surface, ACP stub.
> Reference: `docs/ARCHITECTURE.md` (current build), `docs/PRD.md` (requirements).

---

## 1. Vision

**herdr-mcp** exposes [herdr](https://herdr.dev) — a terminal-native agent
multiplexer (workspaces → tabs → panes → agents) — to AI clients and humans
through the smallest surface that does the job, scaling up to the full power
surface on demand.

One product, onion layers (small → large):

| Layer | Surface | Consumer |
|---|---|---|
| L0 | Core library (config, trim pipeline, recipes, registry types) | Embedders, tests |
| L1 | herdr CLI passthrough tools (1:1 with `herdr` CLI) | Scripts, debugging |
| L2 | MCP stdio, tiered `core` / `extended` | AI clients (Claude, Cursor, Code, OpenCode) |
| L3 | ACP adapter (sessions ↔ panes), feature-gated stub | Zed-class editors |
| L4 | HTTP REST `/api/*` (frozen, mirrors L2) | Dioxus UI, non-stdio clients |
| L5 | Binaries + Dioxus web UI | Humans |

Non-goals: replacing herdr itself, a hosted cloud service, a new agent model,
a plugin marketplace.

---

## 2. Users & jobs-to-be-done

1. **AI coding client** (via MCP stdio): spawn agents in panes, send work,
   wait for idle, read output, pass results between panes, persist workspace
   memory across runs. Wants a *small* tool list (context tax) with the
   extended surface available on demand.
2. **Human operator** (via Dioxus web UI served from the daemon): watch
   agents/trim savings, run tools ad hoc, build and run multi-step recipes,
   manage variables/policies/settings.
3. **Script/CI caller** (via `herdr-trim` bin + `herdr` CLI): one-shot trim,
   folder-key management, dashboard open, all offline-capable where possible.
4. **Editor user** (via ACP, future): attach any ACP client to any pane agent
   as a session.

---

## 3. Architecture

### 3.1 Crate workspace

```text
herdr-mcp/
├── crates/
│   ├── herdr-mcp-core/     # L0: Config (TOML+env+CLI), errors, keybindings,
│   │                       #       session_context. Tier: Beta.
│   ├── herdr-mcp-trim/     # L0: caveman, pfc1, pipeline, policy, runner,
│   │                       #       stats, eval, folder_key. trim/ subsumes
│   │                       #       legacy src/trim/. Tier: Experimental.
│   ├── herdr-mcp-server/   # L1+L2+L4: HerdrMcpServer, 49 tools, dispatch_tool(),
│   │                       #       http_router(), recipe engine, scheduler,
│   │                       #       persistence, templates, herdr_client
│   │                       #       (AgentRegistry + Unix-socket subscriber).
│   │                       #       Tier: Experimental.
│   ├── herdr-mcp-proxy/    # HTTPS intercept proxy (CA, TLS, per-pane policy).
│   ├── herdr-mcp-acp/      # L3 (NEW, stub): ACP session↔pane adapter behind
│   │                       #       `acp` feature flag. No registry of its own.
│   ├── herdr-mcp-ui/       # L5 (NEW): Dioxus 0.7 stable web frontend (rsx!).
│   │                       #       Thin client over frozen /api/*. No server
│   │                       #       functions. Tier: Experimental.
│   └── herdr-mcp-cli/      # L5: TWO bins (see §6):
│                           #       herdr-mcp (daemon) + herdr-trim (tools).
```

Legacy `src/` monolith (`src/main.rs`, `src/server.rs`, …) is **deleted** in
this spec — the crates are canonical (SSoT; see §9).

### 3.2 Runtime data flow

```text
AI client ──MCP stdio──┐
ACP client ──ACP stdio──┼──▶ dispatch_tool() ──▶ HerdrMcpServer ──▶ herdr CLI (argv, no shell)
Dioxus ──HTTP /api/*───┘         │                         ├─────▶ AgentRegistry (socket-fed, canonical)
herdr-trim ──HTTP/offline──┘     │                         └─────▶ data/ (file JSON persistence)
                                 └──── trim pipeline (in-process) + scheduler + proxy
```

`dispatch_tool()` is transport-free: stdio, ACP, HTTP, and CLI all call the
same function. Adapters are stateless. **Stdout = MCP/ACP JSON-RPC;
all logs → stderr.**

### 3.3 herdr dependence (stable surface we build on)

Shell-out via `tokio::process::Command` with real argv. Commands used:
`status --json`, `workspace list/create`, `tab list/create`, `pane
list/get/split/close/read/send-text/send-keys/run/rename`, `agent
list/get/start/read/send/wait`, `notification show`. Events via Unix socket:
`workspace.created/closed`, `pane.agent_status_changed` → idle-transition
output capture into the registry.

---

## 4. MCP tool surface (L1+L2) — tiered

Total today: **49 tools** (per `docs/ARCHITECTURE.md`). Final shape: two tiers.

### 4.1 `core` tier (always on — the smallest agentic surface)

Discovery (7): `status`, `list_workspaces`, `list_tabs`, `list_panes`,
`list_agents`, `get_pane`, `get_agent`.
Lifecycle (4): `create_workspace`, `create_tab`, `split_pane`, `close_pane`.
Spawn/read/write/wait (9): `start_agent`, `read_pane`, `read_agent`,
`send_text`, `send_keys`, `run_command`, `send_agent`, `wait_output`,
`wait_agent_status` (+ `wait_pane_agent_status`).
a2a + memory (5): `agent_message`, `agent_read`, `agent_wait`, `agent_list`,
`var_get`, `var_set`.

### 4.2 `extended` tier (opt-in via `serve --tools extended` / `... core,recipes`)

Recipes/templates/scheduler (7): `list_templates`, `get_template`,
`instantiate_template`, `schedule_recipe`, `list_schedules`,
`delete_schedule`, `enable_schedule`.
Trim (8): `compress`, `decompress`, `trim_policy_set`, `trim_policy_get`,
`trim_eval`, `trim_bench`, `trim_status`, `trim_diagnose`, `trim_summary`,
`trim_dashboard_open`.
Agents-extra (1): `agent_spawn` (labeled-spawn convenience).
Folder keys (3): `get_folder_key`, `list_folder_keys`,
`decompress_with_folder_key`.
Clipboard (2): `clipboard_set`, `clipboard_get`.
Proxy (4): `proxy_startup`, `proxy_diagnose`, `proxy_policy_set`,
`proxy_policy_get`.

### 4.3 Key contracts (frozen)

- `start_agent` appends `--` before agent args (herdr must not consume them).
- a2a path `agent_message → agent_read` is **lossless end-to-end** via compact
  header-less PFC1. `caveman` is lossy-style; `pfc1` is lossless. Never mix.
- Adaptive gate: if `candidate.len() >= current.len()`, pass through unchanged.
- Session variables are **workspace-scoped** (herdr workspace id); chain within
  a workspace, never leak across.
- `agent_spawn` renames the pane to the label (`pane rename`) for registry identity.

---

## 5. Supporting engines (unchanged behavior)

- **Recipe engine:** `{{ stepId.result.path }}` interpolation with dot/bracket
  navigation; 6 bundled templates; `{{pane_id.output}}` / `{{role.output}}`
  seeding from registry + `SessionVars`.
- **Trim pipeline:** ordered stages `caveman:* → pfc1`, per-pane policies with
  direction (`outbound_with_ack` etc.), aggregate `TrimStatus` (gross/net bytes,
  per-pane map, active policies), diagnose round-trip report, summary
  notification via `notification show`.
- **Scheduler:** cron-based recipe runs persisted under `data/schedules/`.
- **Persistence:** file JSON under `data/` (`recipes/`, `sessions/`,
  `schedules/`, `pfc1_memory.json`, `pfc1_master_key.json`, `folder_keys/`).
  No DB.
- **Proxy:** CA + TLS intercept with per-pane trim policies (outbound/inbound
  + stages); `proxy_diagnose` reports CA status + defaults + active policies.

---

## 6. Binaries (L5) — `serve` vs tools split

`crates/herdr-mcp-cli` ships **two `[[bin]]`** sharing one `common.rs`
(tracing-to-stderr, `Config::load` + `CliOverrides`, `--data-dir` default
`./data`) and calling `serve_core()` from `herdr-mcp-server` (never duplicated).

### 6.1 `herdr-mcp` (daemon)

```text
herdr-mcp serve [--http PORT] [--http-only] [--data-dir DIR]
                [--herdr-socket PATH] [--proxy-port PORT]
                [--tools core|extended|core,recipes,...]
herdr-mcp            # default = serve (stdio + HTTP 7676), per current behavior
```

Owns: MCP stdio server, ACP endpoint (when `acp` feature on), HTTP bridge +
Dioxus bundle at `GET /`, event subscriber, scheduler, trim poller.
Links: `server/proxy/rmcp/axum`. No TUI deps.

### 6.2 `herdr-trim` (local tools)

```text
herdr-trim trim [--stage STAGE...] [--decompress] [--file FILE] [TEXT...]
herdr-trim dashboard [--http-port 7676]      # opens Dioxus UI in browser
herdr-trim folder-key {build|list|show|decompress} ...
```

Offline-capable (`trim` runs `PipelineRunner` locally); `dashboard` is a bridge
client. Links: `trim/reqwest/clap` only. Back-compat: `herdr-mcp trim …`
prints `use herdr-trim` (hard break — accepted alongside the Dioxus break).

---

## 7. HTTP bridge (L4) — frozen contract

Axum 0.8, same `HerdrMcpServer` instance. Routes (unchanged):

```text
GET  /                               # Dioxus bundle (was dist/index.html) + SPA fallback
GET  /api/health  /api/config  /api/tools
POST /api/tools/{name}
GET  /api/agents?workspace_id=   GET /api/workspaces
POST /api/recipe   GET+POST /api/recipes   GET+PUT+DELETE /api/recipes/{id}
GET  /api/templates   POST /api/recipes/{id}/run   GET /api/recipes/{id}/export
GET+POST /api/variables   GET /api/variables/auto?workspace_id=
GET+POST /api/playground  GET+DELETE /api/variables/{key}
GET  /api/executions/{id}
GET  /api/trim/status   POST /api/trim/diagnose  POST /api/trim/summary
POST /api/trim/dashboard/open
```

Static serving keeps the `AppState { server, bundle_bytes }` pattern (read once
at startup, warn + `/api/*`-still-works when missing). CORS: permissive for
`localhost:5173` (dx dev) + bridge port.

---

## 8. Dioxus web UI (L5) — replaces React + ratatui TUI

- **Crate** `herdr-mcp-ui`, `dioxus = "0.7"` (crates.io stable, locked in
  `Cargo.lock`), features `web` (+`desktop` later from the same `rsx!`).
  `dx` CLI 0.7 for `serve`/`bundle`. Built-in Tailwind (drop
  `@tailwindcss/vite`). No server functions — `src/client.rs` is a typed
  `fetch` wrapper over §7 (port of `tui/http.rs:HttpClient`, same URLs/shapes).
- **Routes** (1:1 with React HashRouter): `/` (marketing: Hero, Architecture,
  Tools, Install), `/docs`, `/overview`, `/playground` (Tool Runner with
  JSON-schema `DynamicForm` + `ResponseViewer`; Recipe Builder with
  select/reorder instead of drag-kit; Recipe Library), `/trim` (stat cards,
  per-pane bars, policies, diagnose/summary/open; 5 s poll), `/variables`,
  `/settings` (incl. clipboard backend presets xsel/xclip/wl-copy/pbcopy).
- **State:** `use_signal`/`use_resource` per-render views only; registry/trim
  data resolved on call (SSoT §9). Playground last-used values persist via
  existing `/api/playground`.
- **Deletes:** `src/components/*` (20 files), `src/*.tsx`, `vite.config.ts`,
  `package.json`, `tsconfig.json`, `node_modules/`, `dist/`, `index.html`,
  `crates/herdr-mcp-trim/src/tui/`, `dashboard --legacy`.
  Note: terminal-embedded dashboard UX is **gone**; `dashboard` opens the
  browser UI (accepted trade-off).

---

## 9. ACP adapter (L3) — stub scope

New crate `herdr-mcp-acp` behind cargo feature `acp` (off by default).
Maps: `session/new → agent_spawn`, `prompt → agent send + wait + read
(streamed)`, `session/cancel → pane close`, file/tools bridged to L2-core
dispatch. No registry, no persistence, no trim of its own. Success criterion:
one ACP client drives one workspace end-to-end. Full method coverage is
explicitly **out** of v1.

---

## 10. Cross-cutting rules (from AGENTS.md, binding)

- **SSoT:** no fact lives twice. New struct/config/schema field requires
  stating its source of truth first. Resolver closures + `&Config` params,
  not caches. `AgentRegistry` is canonical for panes/agents/policies.
- **Stdout discipline:** server/daemon code never prints to stdout (MCP/ACP);
  tracing → stderr. (Dioxus dev logging audited for this.)
- **No `unwrap`/`expect` in production paths**; no heavy deps for convenience;
  no speculative flags; one concern per PR; docs/tests-only = low risk,
  `server.rs`/compressor/folder-key changes = high risk.
- **Tiers:** core Beta (breaking only in MINOR + changelog); all else
  Experimental. Tiers promoted, never demoted.

---

## 11. Quality gates & tests

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
cargo build --release
dx bundle --platform web            # from crates/herdr-mcp-ui
```

Test plan: keep the 233-test suite green (update `TEST_PLAN.md` counts);
`assert_cmd` tests parametrized over **both** bins; add server-fn-free
client round-trip tests for `client.rs`; keep trim round-trip +
clipboard-override tests; drop `tsc --noEmit` with the React tree.
Manual: MCP stdio clean, ACP smoke (if enabled), trim `diagnose` green,
`herdr-trim dashboard` opens UI against live daemon.

---

## 12. Migration (big-bang order)

1. Extract `serve_core()` + `common.rs`; add `[[bin]] herdr-trim`; move
   `Trim/Dashboard/FolderKey` handlers verbatim; per-bin dep trim.
2. Scaffold `herdr-mcp-ui` (0.7 hello-world vs live bridge); port `client.rs`.
3. Port shell → docs → overview → trim → variables → settings → playground.
4. Rewire `http_router` static dir to Dioxus bundle; `trim_dashboard_open`
   returns URL.
5. Delete legacy `src/` monolith + React + `tui/` + `--legacy`; update
   `README`, `docs/ARCHITECTURE.md`, `TEST_PLAN.md`, `herdr-mcp-context.toml`.
6. ACP stub crate (feature-gated), validated, docs.

---

## 13. Open decisions for reviewer

1. **Port for `serve --http` default** (8080 legacy vs 7676 TUI-era)? Spec
   assumes **7676**; single default everywhere.
2. **Back-compat shim** for `herdr-mcp trim/dashboard` vs hard break (§6.2
   assumes hard break).
3. **ACP priority:** stub now (§9) or defer entirely until an ACP client user
   appears?
4. **Desktop target:** include `dx bundle --platform desktop` in v1 as the
   TUI's visual successor, or web-only v1?
5. **`extended` default:** should interactive `serve` default to
   `core` (spec) or current everything-on behavior for backward compat?

---

## 14. Acceptance — key user experiences (must-pass)

These are the UX journeys that define "done". Each is written as
Given/When/Then against the frozen contracts in §§4–8, with the exact
endpoint or file that proves it. All journeys assume the daemon runs with a
fixed `--data-dir` (default `./data`).

### UX-1 — Run a herdr command → variables auto-populate

> The core loop: the user runs discovery/lifecycle tools and the workspace's
> variable pool fills itself — no manual copy-paste of pane ids.

- **Given** daemon up with `--http 7676`, workspace `w1` has panes with
  agents and roles.
- **When** the user runs `list_panes` (or `list_agents`) for `w1` from the
  Dioxus playground Tool Runner (or any MCP client).
- **Then:**
  1. `GET /api/variables/auto?workspace_id=w1` returns entries keyed by
     **pane_id** and by **role**, each shaped
     `{pane_id, workspace_id, tab_id, agent, role, status, output}`
     (resolved on demand from `AgentRegistry.seed(ws)` + live snapshot —
     `auto_variables_handler`, never stored in the UI).
  2. The response also contains `panes.snapshot` plus flattened pane entries
     persisted from the `list_panes` run into the canonical
     `SessionVars` store (`data/sessions/w1.json`).
  3. A recipe step param of `"{{agentB.output}}"` (role-keyed) or
     `"{{pane-123.output}}"` (pane-keyed) resolves to that agent's last
     captured output when the recipe runs (`POST /api/recipe`).
- **Prove:** `curl localhost:7676/api/variables/auto?workspace_id=w1 | jq
  '.variables | keys'` shows pane ids + roles after the tool run; run a
  2-step recipe (step 1 `read_agent`, step 2 echoes
  `{{step1.result…}}`) and confirm interpolated output.

### UX-2 — Variables survive restart

- **Given** workspace `w1` with: `var_set` session vars, `panes.snapshot`
  from a prior `list_panes`, and global variables (`POST /api/variables`).
- **When** the daemon is killed and restarted with the same `--data-dir`.
- **Then:** `GET /api/variables/auto?workspace_id=w1` still returns the
  persisted session entries (`data/sessions/w1.json`); `GET /api/variables`
  still lists the globals (`data/variables/`). Live registry entries
  re-seed from the socket subscriber as agents report in and merge over —
  never wipe — the persisted bag.
- **Prove:** diff `data/sessions/w1.json` before/after restart (modulo
  `updated_at`); `curl` both endpoints and compare key sets.

### UX-3 — Recipes survive restart

- **Given** recipes created via `POST /api/recipes` (incl. one built in the
  Dioxus Recipe Builder) and a cron schedule (`schedule_recipe`).
- **When** the daemon restarts with the same `--data-dir`.
- **Then:** `GET /api/recipes` lists every recipe byte-identical
  (`data/recipes/{id}.json`); `list_schedules` still shows the schedule and
  it fires on its cron; `POST /api/recipes/{id}/run` executes green.
- **Prove:** `ls data/recipes/ | wc -l` unchanged; full recipe JSON diff
  clean; run-by-id returns success post-restart.

### UX-4 — Playground picks up where you left it

- **Given** the user selected a tool + filled field values in the playground
  and/or staged builder steps (`builder_recipe_name`, `builder_steps`).
- **When** the browser tab closes (or daemon restarts) and the user returns
  to `/playground` for the same workspace.
- **Then:** `GET /api/playground?workspace_id=w1` returns the last
  `PlaygroundSnapshot` (`data/sessions/w1.playground.json`: `tool_name`,
  `field_values`, `builder_recipe_name`, `builder_steps`) and the UI
  restores tool selection, inputs, and staged steps exactly.
- **Prove:** save snapshot, restart, `curl` the endpoint and compare;
  reload page and confirm inputs repopulated (no `localStorage`-only state —
  server file is canonical).

### UX-5 — Settings survive restart

- **Given** settings changed in `/settings` (clipboard backend preset +
  copy/paste commands, bridge port, config file path shown in UI).
- **When** the daemon restarts.
- **Then:** the config file (TOML, path surfaced in the UI) still holds the
  `[clipboard]` section and other saved sections; `clipboard_set/get`
  round-trips through the configured backend; unrelated config sections are
  preserved byte-for-byte on save (upsert, not rewrite).
- **Prove:** config file diff pre/post restart; `clipboard_set "probe"` →
  `clipboard_get` returns `"probe"`.

### UX-6 — Trim continuity (no silent reset)

- **Given** panes with active trim policies and accumulated savings.
- **When** the daemon restarts.
- **Then:** `GET /api/trim/status` returns the same cumulative
  gross/net/messages-trimmed and per-pane map (file-backed stats), policies
  still listed under `active_policies`; `POST /api/trim/diagnose` still
  round-trips green.
- **Prove:** snapshot `/api/trim/status` JSON before/after (counters
  monotonic, never zeroed).

### Acceptance gate

All six journeys pass via **both** surfaces where applicable (MCP stdio tool
calls **and** Dioxus UI clicks backed by the same `/api/*`), against a
release-mode daemon, on a fresh `--data-dir` plus a pre-populated one. Any
journey that requires manual re-entry of data the system already saw is a
release-blocking bug.
