# Swarms — design

Status: **proposal** (2026-10-14). Turns the recipe engine into a flow engine
for deploying observable, interruptible swarms of real CLI agents, with an
n8n-style constructor and a Postman-style runner, served to both the web UI
and the TUI by the one bridge.

## 1. Goals

1. **Agent-agnostic.** Run and talk to any terminal agent — Claude Code,
   Codex, OpenCode, Gemini CLI, Aider, Hermes, ZeroClaw, a plain REPL. What an
   agent can do (MCP or not, how it signals idle, how to interrupt it) is
   *data* (an agent profile), never code.
2. **Real agent-to-agent communication.** Agents message each other, not only
   through an orchestrator scraping panes.
3. **Observable by default.** Every node transition, agent status change and
   a2a message is an event, streamed live and kept per run.
4. **Interruptible at every layer.** Run, node, agent and message.
5. **Construct, then run.** n8n-style graph editor + wizard to build a swarm;
   Postman-style runner (environments, iterations, data rows, checks, reports)
   to run it.

Non-goals: a scripting language (conditions and checks are declarative),
hosting agents outside herdr panes, multi-user/remote deployment.

## 2. What exists today

| Piece | Today | Gap |
|---|---|---|
| Recipe | Linear `steps[]` of tool calls, `{{step.result.path}}` interpolation | No branching, parallelism, loops, gates |
| Execution | Runs inside one HTTP request (`execute_recipe`, duplicated by `run_recipe_by_id_handler`) | No run id until done, no progress, no cancel/pause, history = final blob |
| a2a | `agent_spawn/message/read/wait`: orchestrator types into a pane, scrapes another | Agents never address each other; messages are not recorded or interceptable |
| Live state | herdr event socket → `AgentRegistry` (idle/working/blocked/done) | Not pushed to clients; status only exists for agents herdr recognises |
| Clients | Web + TUI poll `/api/*` | No event stream |

Everything below builds on herdr panes as the substrate: an agent is a
process in a pane; we drive it with `pane send-text` / `send-keys`, read it
with `pane read`, and get status from herdr events where available.

## 3. Concepts

| Concept | Postman / n8n analogue | Source of truth |
|---|---|---|
| **Agent profile** | — | `data/profiles/{id}.toml` (bundled defaults shipped in the binary, user files override by id) |
| **Flow** | Collection / workflow | `data/recipes/{id}.json` (the `Recipe` type, evolved; §6) |
| **Node** | Request / node | inside its Flow |
| **Environment** | Environment | `data/environments/{name}.json` |
| **Run** | Collection run / execution | `data/runs/{run_id}/run.json` + `events.jsonl` |
| **Event** | Console line | `data/runs/{run_id}/events.jsonl` (append-only); live via `/api/events` |
| **Message** | — | an event kind in the run's `events.jsonl`; a message's current state is the fold of its events (§5.3) |
| **Agent instance** | — | the herdr pane + `AgentRegistry` handle (existing) |

## 4. Agent profiles (agent-agnostic core)

A profile describes one CLI agent's *capabilities and conventions*. The
engine only ever talks to agents through a profile.

```toml
# data/profiles/claude.toml
id = "claude"
name = "Claude Code"
command = "claude"
args = []

[status]                 # how we know it's busy vs ready
source = "herdr"         # herdr's built-in detection (known agents)

[prompt]
submit = ["Enter"]       # keys after send-text

[interrupt]
soft = ["Escape"]        # stop the current turn
hard = ["C-c", "C-c"]    # quit

[reply]
mode = "since-prompt"    # pane output after our prompt, once idle

[mcp]                    # how to hand it herdr-mcp (optional)
mode = "flag"            # flag | env | none
flag = "--mcp-config"
format = "claude-json"   # generated file shape
```

```toml
# data/profiles/hermes.toml — MCP is configured in ~/.hermes/config.yaml,
# so we don't inject; the user adds herdr-mcp there once (docs explain how).
id = "hermes"
command = "hermes"
[status]
source = "quiet"         # no herdr detection → output-quiescence
quiet_ms = 2500
ready_regex = '❯\s*$'    # optional prompt marker, sharpens idle detection
[mcp]
mode = "none"
```

```toml
# data/profiles/zeroclaw.toml
id = "zeroclaw"
command = "zeroclaw"
args = ["agent", "-a", "main"]
[status]
source = "quiet"
quiet_ms = 2500
[mcp]
mode = "none"
```

Plus `generic` (any REPL: quiet-based status, Ctrl-C interrupt, no MCP).

- **Status sources:** `herdr` (event-driven, exact) or `quiet` (no output for
  `quiet_ms`, optionally confirmed by `ready_regex` on the last line).
  Exposed uniformly as `idle | working | blocked | done | unknown`.
- **Reply capture modes:** `since-prompt` (scrollback after our prompt),
  `last-lines` (N lines), `marker` (text between `<<<reply>>>` /
  `<<<end>>>` the prompt asks the agent to emit — most robust for full-screen
  TUIs).
- **MCP modes:** `flag` (we write a per-agent config file and pass its path),
  `env` (env var pointing at it), `none`. Agents with `mcp = none` still join
  swarms through lane B (§5).
- Bundled profiles for third-party CLIs are best-effort defaults; flags change
  upstream, so every field is user-overridable and the UI has a
  "test profile" action (spawn, prompt "reply with OK", check status + reply,
  interrupt, close).

## 5. Agent-to-agent communication

### 5.1 Identity

Every spawned agent gets an **agent id** (`{run_id}:{role}` or a user label),
mapped to its pane in the `AgentRegistry` (existing; pane_id ↔ role ↔ label).

### 5.2 Two lanes, one mailbox

**Lane A — MCP-capable agents talk for themselves.** The bridge serves MCP
over streamable HTTP at `/mcp` (rmcp `transport-streamable-http-server`,
mounted on the existing axum router). The generated per-agent MCP config
points at `/mcp?agent={agent_id}`; for stdio-only agents it runs
`herdr-mcp agent-shim --agent {agent_id}`, a stdio↔HTTP relay to the same
endpoint. Tools exposed to agents:

- `a2a_send(to, text)` — `to` is a role, label or agent id; `*` broadcasts
  within the run.
- `a2a_inbox(wait_ms?)` — pending messages for the caller (long-poll).
- `a2a_peers()` — who else is in the swarm, with status.

A delivered message also gets a one-line **nudge** typed into the
recipient's pane (`[swarm] 1 message from planner — call a2a_inbox`) so an
idle agent notices it.

**Lane B — any CLI agent.** The engine delivers by typing a framed message
into the recipient pane (`[from planner] …`, then the profile's submit keys)
and, if a reply is expected, captures it per the profile's reply mode once
the recipient is idle. That reply becomes a message back to the sender.

Both lanes go through the **mailbox**, so every message is recorded and
interceptable regardless of lane.

### 5.3 Message lifecycle

`queued → (held) → delivered → (replied) | dropped`

Each transition is an event in the run's `events.jsonl`; a message's state is
the fold of its events (no separate message table). While **held** (operator
pause on a link, an agent, or the whole run) a message can be edited, dropped
or released. Operators can **inject** messages as a pseudo-agent `operator`.

## 6. Flows (recipes, evolved)

`Recipe` gains a graph; existing linear recipes and templates load as a
straight chain (migration on read: `steps[i] → node`, edges `i → i+1`), so
there is one type and one executor.

```rust
pub struct Recipe {            // herdr-mcp-api
    // …existing id/name/description/variables/category/timestamps…
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    pub label: Option<String>,
    pub position: Option<(f32, f32)>,   // canvas layout
    pub timeout_ms: Option<u64>,
    pub retry: Option<u32>,
    pub checks: Vec<Check>,             // Postman "tests", declarative
}
pub struct Edge { pub from: String, pub port: Option<String>, pub to: String }
```

Node kinds:

| Kind | Does |
|---|---|
| `trigger.manual` / `trigger.cron` | Entry points (cron reuses the scheduler) |
| `tool` | Any MCP tool with params (today's step) |
| `agent.spawn` | Start an agent from a profile with role, cwd, initial prompt; joins the run's swarm |
| `agent.prompt` | Send a prompt to an agent, wait idle, capture reply |
| `a2a.send` | Mailbox send between agents (or from the flow) |
| `wait` | Until agent status / output regex / timeout |
| `branch` | Ports chosen by a condition (`eq`, `contains`, `regex`, `exists`) |
| `join` | Wait for all / any incoming |
| `foreach` | Run its body per item of a list |
| `approval` | Human gate: pauses the run until approved/rejected in a UI |

Interpolation stays `{{ node.result.path }}` / `{{ role.output }}`.
`Check`s are `{ path, op, value }` (eq, contains, regex, exists,
status_is); results appear in the run report.

## 7. Run engine

- `POST /api/runs {flow_id | flow, env?, inputs?, iterations?, data?}` →
  `{run_id}` immediately; the run proceeds in a background task.
- Scheduler: ready-set over the graph (a node is ready when its incoming
  edges are satisfied per `join` semantics); parallel branches run
  concurrently; each node has timeout + retry.
- Control: `pause | resume | cancel` per run (`tokio_util::CancellationToken`
  + a pause gate checked between nodes and inside waits), `skip | retry` per
  node.
- Record: `data/runs/{run_id}/run.json` holds the flow snapshot it ran, the
  resolved environment, and the final summary; `events.jsonl` holds
  everything that happened. Node state during the run is the fold of
  events. `ExecutionResult` is superseded by this (old executions stay
  readable).
- Runs survive client disconnects; on server restart, runs that were
  `running` are marked `interrupted` (agents keep running in their panes and
  can be re-adopted).

## 8. Events & monitoring

`RunEvent` (in herdr-mcp-api): `run.status`, `node.started|finished|failed|
skipped`, `check.result`, `agent.spawned|status|output` (output = throttled
tail), `msg.queued|held|delivered|replied|dropped`, `log`.

- Server: one `tokio::sync::broadcast` bus; writer appends to
  `events.jsonl`; herdr subscriber status changes are republished as
  `agent.status` for agents in a run.
- `GET /api/events?run={id}&since={seq}` — SSE; `since` replays from the
  file, so late joiners and the TUI after reconnect see the full history.

## 9. Interrupts

| Target | Actions |
|---|---|
| Run | pause, resume, cancel |
| Node | skip, retry, approve/reject (`approval`) |
| Agent | interrupt (profile `soft` keys), stop (`hard`), steer (inject operator message), kill pane, hold its inbox |
| Message | hold, edit, release, drop; inject new |

All are `POST /api/runs/{id}/…` calls and all emit events, so every client
sees every intervention.

## 10. Runner (Postman)

- **Environments** (`data/environments/*.json`): named variable sets
  (workspace id, cwd, model flags, …). Resolution order at run start:
  run inputs > data row > environment > flow variables > global variables.
  The resolved map is written into `run.json` as the record of what ran.
- **Iterations & data:** run N times or once per row of a CSV/JSON data set
  (row fields become variables); delay between iterations; stop-on-fail.
- **Report:** per iteration × node: status, duration, checks passed/failed;
  run history list with filters; export as JSON.

## 11. Clients

**Web (Dioxus):**
- *Flows* sidebar (collections: folders = category), *Environments*, *Profiles*.
- *Canvas* editor: SVG nodes and edges, drag, connect ports, pan/zoom;
  inspector panel renders node params from the tool's JSON schema (reuses
  the playground forms).
- *Swarm wizard*: topology (pipeline, supervisor/workers, debate,
  map-reduce) → roles → profile per role → prompts → generated flow, opened
  in the canvas.
- *Runner* tab and *Run monitor*: live graph colouring, agent cards (status,
  output tail, interrupt buttons), message timeline (sequence-diagram style,
  hold/edit/drop inline), console, report.

**TUI:** *Runs* tab — run list, live node states, agents with interrupt keys,
message timeline with hold/release, approvals, report. Flow editing stays the
list-based builder (graph editing is web-only).

Both are clients of the same `/api/*` + `/api/events`; no logic in either.

## 12. API additions (all types in `herdr-mcp-api`)

```
GET/PUT/DELETE  /api/profiles[/{id}]        POST /api/profiles/{id}/test
GET/PUT/DELETE  /api/environments[/{name}]
POST /api/runs                               GET /api/runs[?flow=]
GET  /api/runs/{id}                          GET /api/events?run=&since=
POST /api/runs/{id}/{pause|resume|cancel}
POST /api/runs/{id}/nodes/{node}/{skip|retry|approve|reject}
POST /api/runs/{id}/agents/{agent}/{interrupt|stop|steer|kill}
POST /api/runs/{id}/messages            (inject)
POST /api/runs/{id}/messages/{msg}/{hold|release|drop|edit}
/mcp                                         (streamable HTTP, agent tools)
```

## 13. Roadmap (one PR each)

0. Land the open stack (#8–#13).
1. **Run engine (linear):** background runs, run ids, cancel/pause,
   `data/runs/`, `/api/runs*`; unify the two executors. Existing recipe
   endpoints become thin wrappers.
2. **Event bus + SSE:** `RunEvent`, `events.jsonl`, `/api/events`; web +
   TUI live run view (minimal).
3. **Agent profiles:** schema, bundled profiles, quiet-based status,
   reply modes, `agent.spawn`/`agent.prompt` via profiles, profile test.
4. **Mailbox + lane B:** message events, delivery by typing, reply capture,
   hold/edit/drop/inject.
5. **Lane A:** `/mcp` streamable HTTP, `agent-shim`, `a2a_*` tools, MCP
   config injection per profile.
6. **Graph:** nodes/edges, migration, scheduler (parallel, branch, join,
   foreach, approval), checks.
7. **Runner:** environments, iterations, data rows, report, history.
8. **Web canvas + wizard.**
9. **Monitors & interrupts:** full web run monitor + TUI Runs tab.

Old plan items fold in: SSE is PR 2; deduping `dispatch_tool` against the
`#[tool]` router and splitting `server.rs` happen inside PR 1/5 where those
paths are rewritten anyway.

## 14. Risks / open questions

- **Status for unknown agents** is heuristic (quiescence). `ready_regex`
  and the `marker` reply mode mitigate; the profile test surfaces bad
  profiles early.
- **Full-screen TUIs** (alternate screen) make scrollback capture lossy;
  `marker` mode + `pane read --source screen` is the fallback.
- **Agent tool access:** an agent with herdr-mcp can drive other panes.
  Per-profile tool allowlist on `/mcp` (default: `a2a_*` + read-only tools).
- **Third-party CLI flags drift** (Hermes, ZeroClaw, …): profiles are data,
  user-overridable, versioned with a `tested_with` note.
- `/mcp` per-connection identity relies on rmcp exposing request parts to
  the handler; verify on rmcp 1.7 before PR 5.
