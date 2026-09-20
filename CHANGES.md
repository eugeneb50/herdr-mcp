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
