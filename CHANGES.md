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
