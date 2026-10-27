---
name: herdr-mcp-recipes
description: Run, inspect and author herdr-mcp recipes (saved sequences of herdr tool calls) and their runs through the herdr-mcp HTTP bridge. Use when the user asks to run, list, save, debug, pause or cancel a herdr-mcp recipe, collection or run.
---

# herdr-mcp recipes over the bridge

In local yolo mode (`[mcp] recipes = "bridge"` in herdr-mcp's config) the
recipe tools are off herdr-mcp's MCP tool list to save context. Call them
through the HTTP bridge with curl instead. Every herdr-mcp tool, listed or
not, answers there with the same arguments as its MCP schema.

```bash
B=http://127.0.0.1:7676              # the bridge (http.port in herdr-mcp's config)
curl -sf $B/api/health               # fails: the bridge isn't running, see below
```

## Call a tool

`POST $B/api/tools/<name>` with the tool's arguments as the JSON body. The
answer is an MCP `CallToolResult`: the payload is the JSON text in
`content[0].text`, and `isError: true` means the call failed (the text says
why).

```bash
t() { curl -s "$B/api/tools/$1" -H 'content-type: application/json' -d "${2:-{\}}" \
  | jq -r '.content[0].text'; }
t recipe_list
t recipe_run '{"recipe_id":"<id>","variables":{"dir":"/path"}}'
t run_get '{"run_id":"<id>","trail":true}'
```

| Tool | Does |
|---|---|
| `recipe_list` / `recipe_get` | Find saved recipes / read one (`recipe_id`) |
| `recipe_save` | Create (no `recipe_id`; `name` + `steps`) or update one |
| `recipe_run` | Run a saved recipe (`recipe_id`) or ad-hoc `steps`; `variables` override the recipe's; `wait: true` returns the finished run |
| `run_get` | A run's status and each step's result; `trail: true` explains a failure |
| `run_control` | `run_id` + `action`: `pause`, `resume` or `cancel` |
| `list_templates` / `instantiate_template` | Start from a bundled recipe |

A tool's full argument schema, when you need it:

```bash
curl -s $B/api/tools | jq '.tools[] | select(.name=="recipe_run") | .inputSchema'
```

## Running

- Start without `wait` and poll `run_get` until `status` is `completed` or
  `failed`, so one long call doesn't time out. On `failed`, read
  `run_get` with `trail: true` before retrying: it shows each step's params
  as sent, unresolved placeholders, results and the pane's last lines.
- Steps are `{id, tool, params, extract?}`. Later steps read earlier
  results as `{{ id.path }}` (`{{ ws.result.root_pane.pane_id }}`,
  `{{ exec.output }}`) and variables as `{{ name }}`.
- Gates, loops and child runs: the herdr-mcp://agent-guide MCP resource
  (docs/AGENT-GUIDE.md), section Recipes.

## Rules

- Don't add `triggers` to a recipe or call `schedule_recipe` unless the
  user asked for it. The bridge counts as the user, so herdr-mcp's
  `agent-triggers` guard doesn't stop you here.
- Don't overwrite a recipe you didn't make without asking.

## If the bridge isn't there

- `curl` can't connect: the bridge runs with the herdr-mcp TUI or
  `herdr-mcp --http-only`; an MCP server started with `--mcp-only` has
  none. Ask the user to start one; don't start servers yourself.
- `401`: the bridge wants its token. Add
  `-H "Authorization: Bearer $(cat <data-dir>/bridge.token)"` (ask the user
  for the data dir if it isn't `./data`).
