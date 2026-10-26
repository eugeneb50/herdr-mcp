# Driving herdr with herdr-mcp — a guide for agents

You are an AI agent with herdr-mcp's tools. This guide covers how to
reach panes and agents, how to build and launch recipes, and how to make
one step wait for another pane before the next runs. Every tool's
parameters are in its MCP schema; this is the how and why.

## Connect

herdr-mcp is a stdio MCP server; point your client at the binary:

```bash
claude mcp add herdr-mcp -- /path/to/herdr-mcp          # Claude Code
```

```jsonc
// opencode.jsonc
{ "mcp": { "herdr-mcp": { "type": "local", "command": ["/path/to/herdr-mcp"] } } }
```

Started that way it also serves the HTTP bridge on `127.0.0.1:7676`
unless something (the user's TUI, say) already holds the port; your tools
work either way. Recipes and runs live in herdr-mcp's data dir: they are
shared with the user's TUI and web playground only when both use the same
one (`data_dir` in `~/.config/herdr-mcp/config.toml`).

## Finding things

- herdr nests **workspaces → tabs → panes**. Ids look like `w1`, `w1:t1`,
  `w1:p2`. They are session-local and can renumber when something closes:
  re-read them (`list_panes`) after structural changes.
- Prefer **labels**. `create_workspace`, `create_tab` and `agent_spawn`
  take one, and pane tools accept `label` instead of `pane_id`. The a2a
  tools (`agent_*`) take a `target`: a role, label or pane id.
- `list_panes` shows each pane's agent and its status (`idle`, `working`,
  `blocked`, `done`).

## Waiting: pick the right tool

| You want to continue when… | Use |
|---|---|
| an agent finishes its turn | `agent_wait` (status `idle`) or `agent_prompt` (prompt + wait + reply) |
| new text shows up in a pane | `pane_watch` |
| text is anywhere in the pane, old or new | `wait_output` |
| a shell command exits | `pane_exec` (returns output and exit code) |

`pane_watch` only fires on a match that wasn't there when it started (it
counts matches and fires when the count goes up). That makes it safe to
watch for a word right after sending a prompt. The marker must still not
appear *verbatim* in your own prompt: the prompt can render after the
watch starts and would count as new. Ask for it in a form you didn't type
(“reply with the word 'ready' in uppercase” → watch `READY`), or use
`use_regex` with something only the result prints.

## Recipes

A recipe is a saved list of tool calls run in order; a failing step stops
the run. Steps refer to earlier results and variables with `{{ … }}`:

- `{{ stepId.path }}`: an earlier step's result, followed with dots and
  `[n]` (`{{ ws.result.root_pane.pane_id }}`).
- `{{ name }}`: a recipe variable, a variable passed to the run, or a
  global variable.

A step can also `extract` part of its result into a variable for later
runs. A recipe's optional `gate` decides what a full run does after its last
step: rules are checked in order and the first whose `checks` hold ends the
run, fails it, restarts it from step 1 (a loop until some condition), or
hands off to another saved recipe:

```json
"gate": { "rules": [
  { "checks": [{ "path": "read.text", "op": "missing" }], "then": { "action": "restart", "delay_ms": 2000 } },
  { "checks": [{ "path": "read.text", "op": "contains", "value": "error" }], "then": { "action": "fail", "message": "agent errored" } },
  { "checks": [{ "path": "read.text", "op": "contains", "value": "deploy" }], "then": { "action": "run", "collection": "<recipe id>", "inputs": { "pane": "{{ spawn.pane_id }}" } } },
  { "then": { "action": "end" } }
], "max_steps": 200 }
```

Ops: `eq`, `ne`, `lt`, `gt`, `contains`, `exists`, `missing`. `{{ run.pass }}`
counts passes; `max_steps` caps the requests one run may execute. `run`
starts the other recipe as a child run in the same workspace: it sees its
own variables, the globals and `inputs` (their `{{ }}` filled from this
run), not this run's variables. This run ends as the child ends (a failed
child fails it). Handing back to a recipe already up the chain fails; use
`restart` to loop.

When a run fails, `run_get` with `trail: true` returns its trail as text:
each step's params as sent, unresolved placeholders, results, variables set,
which gate rule fired and the values its checks found, the last lines of
the pane the run touched last, and, indented under it, the trail of the
child run its gate handed off to. A recipe's `trail` sets how long its runs are
kept (`keep_runs`, `keep_days`) and how many pane lines a failure saves
(`snapshot_lines`); unset fields use the global `runs.*` settings. Run state is persisted: a run survives your session, and the user
sees it live in the TUI and web playground.

### Tools

| Tool | Does |
|---|---|
| `recipe_list` / `recipe_get` | Find saved recipes / read one |
| `recipe_save` | Create (no `recipe_id`; `name` + `steps`) or update one |
| `recipe_run` | Run a saved recipe or ad-hoc `steps`; `variables` override the recipe's; `wait: true` returns the finished run |
| `run_get` | A run's status and every step's result or error |
| `run_control` | `pause`, `resume` or `cancel` (cancel also stops a step in flight) |
| `list_templates` / `instantiate_template` | Start from a bundled recipe |
| `schedule_recipe` | Run a saved recipe on a cron |

Long recipes: start without `wait` and poll `run_get`, so your own tool
call doesn't time out.

## Calling APIs and models

- `http_request` calls any HTTP API: `{"method": "POST", "url": "…",
  "headers": {"Authorization": "Bearer {{token}}"}, "json": {…}}`. A
  non-2xx status comes back as a result (`status`, `ok`); pass
  `check: true` to make it fail the step instead.
- `llm_call` asks a model: `{"model": "openai/gpt-4o-mini", "prompt":
  "Summarise: {{read.output}}"}` goes to OpenRouter with
  `OPENROUTER_API_KEY`; set `base_url` for OpenAI, Ollama
  (`http://localhost:11434/v1`) or any OpenAI-compatible server. The
  reply is `{{step.text}}`. `llm_models` lists model ids.

## Worked example: opencode relay

Start opencode in a new workspace on a project, wait for it to finish
the first prompt, then send the second. This is
[`examples/opencode-relay.json`](../examples/opencode-relay.json):

| Step | Tool | Notes |
|---|---|---|
| `ws` | `create_workspace` | `cwd: "{{dir}}"`, `label`, `no_focus: true`; the result holds `result.root_pane.pane_id` |
| `oc` | `agent_spawn` | `agent: "opencode"`, `in_pane: "{{ws.result.root_pane.pane_id}}"` (no leftover shell pane), `args: ["--prompt", "{{first}}"]`, `wait_idle: false` so the watch starts before the reply |
| `first_done` | `pane_watch` | `label: "relay-oc"`, `match_text: "{{first_marker}}"` |
| `send` | `agent_message` | types the second prompt and submits it |
| `second_done` | `pane_watch` | the second marker |

Save it once, then launch it with your own directory and prompts:

```jsonc
// recipe_save: the file's name, description, variables and steps
// recipe_run:
{
  "recipe_id": "<id from recipe_save>",
  "variables": {
    "dir": "/home/me/code/project",
    "first": "Read README.md and summarise it. Then reply with the word 'ready' in uppercase.",
    "second": "Now list three risks you see. End with the word 'second' in uppercase."
  }
}
```

Then `run_get` until `status` is `completed` (or `failed`: the failed step
says why). To steer opencode beyond prompts, `send_keys` takes keys like
`Escape` or `Ctrl+c`, and `read_pane` shows its screen.

## Gotchas

- `agent_spawn` waits for the agent to be idle by default
  (`wait_idle: true`). When a later step watches for the reply, set it
  to `false`, or the reply is already on screen when the watch starts.
- `in_pane` keeps the pane's directory: give the directory to
  `create_workspace` (or `create_tab`), not to `agent_spawn`.
- `pane_exec` needs the pane at a POSIX shell prompt; it can't run inside
  an agent's TUI.
- herdr reports an agent that finished as `done` when nobody watched its
  pane and `idle` when someone did.
- A `pane_watch` without `timeout_ms` waits until matched or the run is
  cancelled. Called directly (not in a recipe), give it a timeout.
- Agents that show their thinking (opencode does) may print the marker
  while reasoning about it, before the reply. Watch for the marker on a
  line of its own: `match_text: "^\\s*READY\\s*$"`, `use_regex: true`.
- The user's `[security]` config may disable tools (they're missing from
  `tools/list`; calling one fails "disabled by [security]"), limit the
  hosts `http_request` reaches, and by default keeps agents from adding
  recipe triggers or schedules (`agent-triggers`). A refusal says which
  setting; ask the user rather than working around it.
