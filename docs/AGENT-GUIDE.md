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
- **Your own pane is the default.** When your MCP client runs inside a
  herdr pane, `split_pane` with no pane, and `agent_spawn` with no
  workspace or tab, split *your* pane: right if it is wide, down if it is
  tall. Focus stays where it is (`no_focus` defaults to true). Pass a pane,
  workspace or tab to put work elsewhere.
- Not every tool is listed. Recipe authoring, schedules, templates,
  variables, HTTP, LLM and clipboard tools are off the list unless the user
  adds them under `[mcp] tools`. Recipes and the UIs can still use them.
  With `[mcp] recipes = "bridge"` the recipe tools are off it too: call
  them through the bridge, `POST /api/tools/<name>` with the tool's
  arguments (skills/herdr-mcp-recipes/SKILL.md).

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

A recipe (the web UI calls it a collection) is a saved flow of tool calls.
The simplest is a straight line: save `steps` (`{id, tool, params,
extract?}`) and they run one after another. Nodes refer to earlier results
and variables with `{{ … }}`:

- `{{ nodeId.path }}`: an earlier node's result, followed with dots and
  `[n]` (`{{ ws.result.root_pane.pane_id }}`).
- `{{ name }}`: a recipe variable, a variable passed to the run, or a
  global variable.
- `{{ run.pass }}`: 1, plus one per loop taken.

A tool node can also `extract` part of its result into a variable for later
runs. A node fails when its tool can't be called or its result has
`isError: true` (bad params, herdr refusing a method). A `http_request`
that gets a 4xx/5xx is not a failure unless `check` says so: test its
`status` with a branch.

### Flows

For branches, parallel paths, loops, retries and child runs, save a `flow`
instead of `steps`: nodes, and edges from a node's port to another node.

| Node `type` | Fields | Ports |
|---|---|---|
| `start` | (exactly one) | `out` |
| `tool` | `tool`, `params`, `extract`, `retry: {times, delay_ms}`, `timeout_ms` | `out`, `error` |
| `branch` | `rules: [{checks, any}]` | `1`…`n` (the first rule that holds), `else` |
| `run` | `collection` (a recipe id), `inputs` | `out`, `error` |
| `wait` | `ms` | `out` |
| `fail` | `message` | none |

An edge is `{"from": "a", "port": "error", "to": "b"}`; `port` defaults to
`out`. How a run moves:

- Every edge out of the port a node fires runs its target, so two edges
  from one port run both nodes at once.
- A node with several edges in is a join: it waits until each of them has
  fired or can no longer fire (a branch upstream took another port).
- A port with no edge ends that path; the run completes when no path is
  left. A failure with no `error` edge fails the run.
- An edge back to a node upstream is a loop: taking it runs that part of
  the flow again and adds one to `run.pass`. `max_steps` (default 1000)
  caps the node runs one run may make, retries and loop passes counted.
- `retry` tries a failing tool again; after the last attempt the `error`
  port fires (or the run fails). `timeout_ms` fails an attempt that takes
  longer.
- A `run` node starts another recipe as a child run in the same workspace
  and waits for it. The child sees its own variables, the globals and
  `inputs` (their `{{ }}` filled from this run), not this run's variables.
  Its result is `{run_id, status, error, variables}` (`variables`: what
  the child extracted); `error` fires when the child did not complete. A
  recipe already up the chain can't be started again; loop with an edge.

A check reads `path` like a placeholder and compares it with `value`. Ops:
`eq`, `ne`, `lt`, `gt`, `contains`, `exists`, `missing`. A rule's checks
must all hold (`any: true`: one); a rule with no checks always holds.

Poll a pane until the agent says done, give up after ten passes:

```json
{ "nodes": [
    { "id": "start", "type": "start" },
    { "id": "read", "type": "tool", "tool": "read_pane",
      "params": { "label": "worker", "lines": 20 }, "retry": { "times": 2, "delay_ms": 500 } },
    { "id": "done", "type": "branch", "rules": [
      { "checks": [{ "path": "read.text", "op": "contains", "value": "DONE" }] },
      { "checks": [{ "path": "run.pass", "op": "gt", "value": 10 }] } ] },
    { "id": "nap", "type": "wait", "ms": 5000 },
    { "id": "stuck", "type": "fail", "message": "the worker never said DONE" } ],
  "edges": [
    { "from": "start", "to": "read" },
    { "from": "read", "to": "done" },
    { "from": "done", "port": "2", "to": "stuck" },
    { "from": "done", "port": "else", "to": "nap" },
    { "from": "nap", "to": "read" } ] }
```

Port `1` has no edge, so DONE ends the run `completed`. The web UI draws
and edits the same flow on a canvas and keeps each node's `at` position;
leave `at` out and it lays the nodes out.

When a run fails, `run_get` with `trail: true` returns its trail as text:
each node's params as sent, unresolved placeholders, results, retries,
variables set, the port each branch took and the values its checks found,
loops, the last lines of the pane the run touched last, and, indented under
it, the trails of the child runs its run nodes started. A recipe's `trail`
sets how long its runs are kept (`keep_runs`, `keep_days`) and how many
pane lines a failure saves (`snapshot_lines`); unset fields use the global
`runs.*` settings. Run state is persisted: a run survives your session, and
the user sees it live in the TUI's and the web's Runs tab.

### Tools

| Tool | Does |
|---|---|
| `recipe_list` / `recipe_get` | Find saved recipes / read one |
| `recipe_save` | Create (no `recipe_id`; `name` + `steps` or `flow`) or update one |
| `recipe_run` | Run a saved recipe or an ad-hoc `steps` / `flow`; `variables` override the recipe's; `node_ids` runs only those tool nodes; `wait: true` returns the finished run |
| `run_get` | A run's status and every node's result or error |
| `run_control` | `pause`, `resume` or `cancel` (cancel also stops nodes in flight and child runs) |
| `list_templates` / `instantiate_template` | Start from a bundled recipe |
| `schedule_recipe` | Run a saved recipe on a cron |

Long recipes: start without `wait` and poll `run_get`, so your own tool
call doesn't time out.

## Calling APIs and models

- `http_request` calls any HTTP API: `{"method": "POST", "url": "…",
  "headers": {"Authorization": "Bearer {{token}}"}, "json": {…}}`. A
  non-2xx status comes back as a result (`status`, `ok`); pass
  `check: true` to make it fail the node instead.
- `llm_call` asks a model: `{"model": "openai/gpt-4o-mini", "prompt":
  "Summarise: {{read.output}}"}` goes to OpenRouter with
  `OPENROUTER_API_KEY`; set `base_url` for OpenAI, Ollama
  (`http://localhost:11434/v1`) or any OpenAI-compatible server. The
  reply is `{{node.text}}`. `llm_models` lists model ids.

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

Then `run_get` until `status` is `completed` (or `failed`: the failed node
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
