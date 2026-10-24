# Unreleased

One binary, TUI first, no message trim, and chained playground requests.
The binary, TUI and trim changes are all breaking; the rest are marked
**(breaking)** where they are.

- **Security hardening: `[security]` (breaking).** A new config section,
  read at startup only:
  - `disabled-tools` hides tools and refuses calls to them from every
    caller (MCP, the bridge, recipe steps).
  - `allow-hosts`, `block-private` and `max-response-bytes` limit
    `http_request` and the LLM tools. Every resolved address and every
    redirect is checked. Link-local and cloud-metadata addresses, and the
    bridge's own loopback port, are always refused.
  - `key-env` lists the environment variables `api_key_env` may name.
  - `agent-triggers` (default **off**): MCP clients and recipe runs may no
    longer add triggers or schedules, or change a recipe that has them. The
    web UI and TUI still may.
  - `require-token` puts a bearer token on `/api/*`. It's always required on
    a non-loopback bind, replacing the old warning. The token lives in
    `<data-dir>/bridge.token` and is printed as a `/#token=` link the web UI
    keeps; the TUI and the plugin read the file.
  - `audit-log` writes `audit.jsonl`, one line per tool call, with secrets
    redacted.
  - The data dir is now 0700.
- **Fix: provider keys leaked to look-alike URLs.** `llm_call` and
  `llm_models` sent `OPENROUTER_API_KEY` to any `base_url` that merely
  contained `openrouter.ai`, and `OPENAI_API_KEY` likewise for
  `api.openai.com`. They now send each key only to its exact host.
  `http_request`'s `json` now sends a JSON string as JSON (it used to be
  sent as text).
- **HTTP and LLM requests.** `http_request` sends any HTTP request (text
  or JSON body, headers, query; `check` fails on non-2xx). `llm_call` calls
  an OpenAI-compatible chat API, OpenRouter by default (OpenAI, Ollama, LM
  Studio or any `base_url`), with the key from `api_key` (a `{{variable}}`)
  or an environment variable; `llm_models` lists the models. In the web
  playground, **+ HTTP** opens an endpoint builder and **+ LLM** a provider
  → model → prompt wizard. New settings `timeouts.http-request-ms` and
  `timeouts.llm-call-ms`.
- **Agents can launch and follow recipes.** New MCP tools `recipe_list`,
  `recipe_get`, `recipe_save`, `recipe_run` (optionally waiting for the
  finished run), `run_get` and `run_control` expose what `/api/recipes`
  and `/api/runs` already did. `pane_watch` waits for *new* matching text
  in a pane (text already there, like the marker in a prompt just sent,
  doesn't count), so a recipe step can wait for an agent's reply before the
  next sends more. `agent_spawn` takes `in_pane` to start the agent in an
  existing pane, such as a new workspace's first one. The recipe step and
  trigger types now carry JSON schemas, and `docs/AGENT-GUIDE.md` (with
  `examples/opencode-relay.json`) shows agents how to use all of it.
- **Settings: one place for every knob (breaking).** Keybindings, timeouts,
  limits and intervals live in one registry (`herdr_mcp_api::settings`),
  stored as `[settings.<section>]` tables in the config file and edited
  on the Settings page (web: Options card; TUI: Options panel), applied
  live. `/api/settings` lists, sets and resets them. The TUI's tab, refresh
  and quit keys are remappable; a chord must use Ctrl or Alt (or an
  F-key), must not be one a tab uses, and can't be bound twice. Invalid
  values are refused with the reason; a bad entry in the file falls back
  to its default with a startup warning, and Reset (one or all) restores
  the defaults. Server defaults that were hard-coded (pane_exec timeout
  and scrollback, herdr_api, agent_spawn / agent_prompt / agent start
  timeouts, watcher interval and error limit) are now these settings.
  `[herdr] reconnect-attempts` / `reconnect-backoff-ms` moved to
  `[settings.limits] reconnect-attempts` / `[settings.intervals]
  reconnect-backoff-ms`; the old keys now fail to load.
- **Fixes (settings, collections).** A key rebinding the API accepted
  (say `alt+right` moved from next-tab to the Send tab) no longer drops
  back to its default on restart because of the order the keys sort in
  the file. Two settings or clipboard saves at once no longer fail with
  "No such file or directory" or undo each other live. A chord with
  Shift on a non-letter (`alt+shift+]`) is refused: terminals send the
  shifted character, so it could never fire. The web Collections view
  reloads after Send, so saving the collection right after a request no
  longer wipes the variables that request extracted.
- **TUI Overview shows the picked pane's text.** Like a web deck card:
  the pane picked in the table (arrows or a click) is shown below it,
  re-read every `intervals.tui-pane-ms` (1s), screen or scrollback (`r`).
  Drag with the mouse to select (reading pauses while a selection is up),
  then Ctrl+C or right-click Copy; with nothing selected, Copy takes the
  whole text. Tab moves focus between the table and the text (arrows,
  PgUp/PgDn, Home/End scroll it; End follows new output). Clicking a
  table row now picks that row: it picked the one below it (the header
  wasn't counted) and ignored the table's scroll.
- **One binary.** `herdr-trim` is gone; `herdr-mcp` is the only binary, and
  the release archives, `scripts/install.sh` and the plugin ship just it.
  Remove a stale `herdr-trim` from an earlier install by hand.
- **TUI by default.** `herdr-mcp` in a terminal opens the TUI dashboard over
  its own HTTP bridge; launched by an MCP client (stdin not a terminal) it
  serves MCP stdio + HTTP as before, so MCP client configs keep working.
  `--no-tui` forces MCP stdio + HTTP, `--http-only` the bridge alone. The
  `serve` subcommand is gone: its flags (`--http`, `--http-only`,
  `--data-dir`, `--herdr-socket`) are top-level. In TUI mode logs go to
  `<data_dir>/herdr-mcp.log` instead of the terminal.
- **Message trim removed.** The `herdr-mcp-trim` crate (caveman, PFC1,
  folder keys), the Trim and Folder Key tools (`compress`, `decompress`,
  `trim_*`, `*_folder_key`), `/api/trim/*`, trim policies on agents, savings
  badges and the Trim tabs in the web UI and TUI. `agent_message` and
  `agent_read` send and store plain text (their `compress` / `decompress`
  parameters are gone). Leftover `trim_stats`, `pfc1_*` and
  `folder_keys` files in a data dir are ignored.
- **Plugin.** The bridge tab runs `herdr-mcp --http-only`, the dashboard
  split `herdr-mcp`. `scripts/plugin-build.sh` replaces `bin/herdr-mcp` by
  rename, so rebuilding while the bridge runs no longer fails with "Text
  file busy".
- **Cross-site requests refused.** Any website could make a body-less
  `POST /api/tools/{name}` from your browser (no CORS preflight), e.g.
  `create_workspace`, opening focused workspaces on every page load. The
  bridge now refuses a browser `Origin` that isn't a loopback page or the
  bind name; MCP clients, the TUI and curl send none and are unaffected.
- **Bind warning.** A non-loopback `http.bind-addr` logs a warning at
  startup: the bridge has no auth.
- **TUI tab keys.** Alt+→ / Alt+← go to the next / previous tab (wrapping)
  in any terminal, Ctrl+Tab / Ctrl+Shift+Tab where the terminal speaks the
  kitty keyboard protocol, and Alt+1-5 / Ctrl+1-5 jump directly. None of
  them type text or edit a field. VTE terminals send Ctrl+Tab as Tab and
  Ctrl+1 / Ctrl+3 as `1` / Esc, so use the Alt keys there.
- **Duplicate agent tools removed (breaking).** `send_agent`, `read_agent`,
  `get_agent`, `wait_agent_status` and `wait_pane_agent_status` repeated the
  a2a tools: use `agent_message`, `agent_read`, `agent_list` and
  `agent_wait`, whose `target` (role, label or pane id) now also takes a
  pane the registry hasn't seen. The Dev Watch template's notify step uses `agent_message`; saved
  recipes with the old tool names need the same edit. 36 tools remain.
- **TUI stays live during requests.** Playground tool calls and recipe
  runs run in the background: the screen keeps drawing ("running …"),
  tabs switch, and the result lands when it finishes. Recipe runs are
  polled to the end, so one longer than 30s is no longer reported as
  failed. A held key or steady typing no longer freezes the redraw.
- **TUI number fields.** Integer parameters (`timeout_ms`, `lines`) were
  sent as `6000.0` and always rejected; integers now go as integers.
- **Terminal restored on a TUI panic.** Release builds abort on panic; a
  panic hook now leaves the alternate screen, raw mode and mouse capture
  first, so the shell isn't left unusable.
- **Atomic saves.** Data-dir files (recipes, variables, session vars,
  playground, runs, schedules, profiles) and the `[clipboard]` config are
  written to a temp file and renamed into place: a crash or two racing
  saves can no longer leave a truncated or torn file.
- **`agent_spawn` reports failed waits.** A `needs` target that is unknown
  or never goes idle now fails the call before anything spawns (it was
  skipped, spawning anyway). A `wait_idle` that times out leaves `status`
  `working` and sets `wait_error`, instead of claiming `idle`.
- **Chaining across requests.** A recipe step can carry `extract` rules
  (`{path, var, scope}`) that save part of its result as a global or a
  collection (recipe) variable; later steps and later runs resolve it.
- **One resolver (breaking).** Runs resolve `{{ }}` in one scope: live
  agents < globals < the recipe's variables < the request's < step results.
  Registry handles no longer override recipe variables.
  `POST /api/variables/resolve` previews a value in that scope.
- **Send one request.** `POST /api/runs` takes `step_ids` to run only those
  steps of a recipe.
- **Playground = collections (web).** Postman-style: collections (recipes,
  in folders) of requests (any tool or herdr method), Send / Run collection,
  click a result path to extract it, collection variables and globals. The
  Tool Runner, herdr API and Recipe Builder tabs are folded into it; the
  browser no longer substitutes `{{ }}` itself.

# 0.2.0

Installable as a herdr plugin, one Home page for sending to panes, sessions,
and a built-in OS clipboard. Breaking changes are marked **(breaking)**.

- **One-line install.** `curl -fsSL
  https://raw.githubusercontent.com/eugeneb50/herdr-mcp/main/scripts/install.sh | sh`
  installs `herdr-mcp` and `herdr-trim` from the latest GitHub release (macOS
  and Linux, x86_64 and arm64; Linux builds are static musl, so any distro).
  A `v*` tag builds and attaches those archives
  (`.github/workflows/release.yml`).
- **Rust 1.96.1** is pinned in `rust-toolchain.toml`: on 1.99 the web UI
  bundle fails (wasm-bindgen 0.2.126, macOS `rust-objcopy`).
- **One-file deployment.** The web UI is built into `herdr-mcp`: its build
  compiles the `dx bundle` output in, so the binary alone serves the
  playground. Bundle before `cargo build`. `http.web-dir` /
  `HERDR_MCP_WEB_DIR` now only override it; the compile-time path into the
  checkout is gone.
- **herdr plugin.** `herdr plugin install eugeneb50/herdr-mcp` (or
  `sh scripts/plugin-build.sh && herdr plugin link .`). `herdr-plugin.toml`
  is generated by `herdr-mcp plugin manifest` and kept equal by a test; its
  commands read port and data dir from config at run time. As the plugin,
  `data_dir` defaults to `HERDR_PLUGIN_STATE_DIR`. `plugin link`/`unlink`
  are gone **(breaking)**.
- **One daemon per data dir.** Every process sharing a data dir used to run
  the scheduler, triggers and trim poller, so schedules and triggers fired
  twice. `<data_dir>/daemon.lock` now elects one; the others take over when
  it exits. The daemon fires against its own herdr session, so give each herdr
  session that uses triggers or schedules its own data dir.
- **Sessions + one typed pane list.** New `list_sessions` tool and
  `GET /api/herdr/sessions`; Discovery/Read tools take `session`.
  `list_panes` takes filters and returns `{"panes": [...]}`
  **(breaking:** recipes use `{{ step.panes }}`**)**. The Overview is a pane
  browser; the TUI shows plain shells and cycles sessions with `s`.
- **Home page.** `/` sends text to a pane addressed by `pane_id` or `label`,
  with dropdowns from the auto variables and a clickable pane table
  (`/home` and `/docs` are gone). The text stays after a send; Clear
  empties it; "highlight code" colors it. Every pane field in the web UI gets
  the same dropdowns. The TUI opens on a Send tab (tabs are now Ctrl+1-6).
- **Built-in clipboard.** `clipboard_set` / `clipboard_get` use the OS
  clipboard (macOS, Windows, X11, Wayland) unless both `[clipboard]`
  commands are set; Settings gets a "built-in" choice. The commands live only
  in the config file: `HERDR_MCP_CLIPBOARD_COPY` / `_PASTE` are gone
  **(breaking)**, as they overrode a saved Settings choice on every restart.
- **One settings file** **(breaking)**. `herdmcp.toml` / `[persistent]` are
  gone: settings come from the config file, env and flags. Bare `herdr-mcp`
  no longer forces port 7676 over the config, and bridge discovery only
  tries the configured port. `Cargo.lock` is committed; the version lives
  once in `[workspace.package]`.
- **Fixes.** `trim_dashboard_open` splits a real pane (herdr 0.9.5 rejected
  every call) and runs the sibling `herdr-trim`; a clipboard copy command
  that exits non-zero is an error.

# Dioxus Consolidation (one web UI, one wire contract)

- **Web UI is Dioxus.** The React/Vite site and the legacy `src/` monolith are
  gone; all code lives in `crates/`. Build the UI with
  `cd crates/herdr-mcp-ui && dx bundle --platform web --release`; the bridge
  serves it from `http.web-dir` / `HERDR_MCP_WEB_DIR` (default: the in-repo
  bundle output) with SPA fallback. No Node toolchain.
- **`herdr-mcp-api` crate.** Every `/api/*` request/response type plus a typed
  `ApiClient` (native + wasm) is defined once and shared by the server, the
  web UI and (next) the TUI. Covered by
  `http_contract_roundtrip_with_api_client`.
- **Binaries split.** `herdr-mcp` serves; `herdr-trim` runs `trim`,
  `dashboard` and `folder-key`.
- **Security.** The HTTP bridge now honors `http.bind-addr` (default
  `127.0.0.1`; it previously always bound `0.0.0.0`), and the permissive CORS
  layer is removed (the UI is same-origin).
- **Web UI fixes.** API calls used relative URLs (rejected by reqwest), empty
  `201`/`204` responses were reported as failures, and playground "last used"
  values never saved — all fixed by the shared client.

# Variable Auto-Storage & Recipe Hardening

## Variable Auto-Storage

When running tools in the TUI playground, results are now automatically extracted
and stored as session variables for use in recipes.

### How It Works

1. **Run a tool** (e.g., `list_panes`, `list_agents`, `get_pane`, `status`)
2. **Results are parsed** and stored as variables:
   - Full result: `_{tool_name}_result`
   - Individual items: `_{tool_name}_result[0]`, `_{tool_name}_result[1]`, etc.
   - Pane-specific fields: `pane_0_id`, `pane_0_label`, `pane_0_status`, etc.

### Supported Tools

- `list_workspaces` - Store workspace IDs
- `list_tabs` - Store tab IDs
- `list_panes` - Store pane IDs, labels, statuses
- `list_agents` - Store agent information
- `get_pane` - Store specific pane details
- `get_agent` - Store specific agent details
- `status` - Store server status
- `agent_spawn` - Store spawned agent info
- `agent_read` - Store agent output

### Using Stored Variables in Recipes

In the recipe builder, reference variables using the `{{ variable_name }}` syntax:

```
{{ pane_0_id }}           # First pane's ID
{{ pane_0_label }}        # First pane's label
{{ _list_panes_result[0] }} # First pane from list_panes result
```

### Auto-Stored vs User Variables

- **Auto-stored variables** (marked with ⟐ in the Variables tab):
  - Generated automatically from tool results
  - Cannot be edited or deleted (regenerated on each run)
  - Start with `_` or `pane_` or contain `[`

- **User variables** (marked with ▸):
  - Created manually with Ctrl+N
  - Can be edited and deleted
  - Persist until explicitly removed

## Recipe Hardening

### Validation

Before saving or running a recipe, the builder now validates:

1. **Empty step IDs** - Each step must have a unique ID
2. **Empty tool names** - Each step must specify a tool
3. **Duplicate IDs** - All step IDs must be unique
4. **Malformed variable references** - Unclosed `{{` or `}}` patterns
5. **Invalid step references** - References to non-existent steps

### Keyboard Shortcuts

- **Ctrl+V** in the builder - Validate the current recipe
- **Ctrl+R** in the builder - Run (only if validation passes)

### Visual Indicators

- ✓ "recipe validates cleanly" - No issues found
- ✗ "X validation issue(s)" - Problems need fixing

### Error Messages

Validation errors are specific and actionable:
- "step 2 (send_agent): duplicate step ID 'step1'"
- "step 3 (read_pane): references non-existent step 'nonexistent'"
- "step 1: unclosed variable reference in param 'pane_id'"
