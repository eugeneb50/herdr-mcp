# Mail: Capsule Delivery for Agent Workflows

**Capsule mail** is a task-delivery system built into herdr-mcp. A queue named
`mail-<label>` acts as an agent's inbox: you send a task capsule, the postman
(herdr-mcp's internal queue service) types it into the agent's herdr pane, the
agent does the work and writes a report, then replies with `done`, `fail`, or
`block`. The sender watches the mailbox for the reply. No external message
broker is needed — it runs on the existing herdr-queue store. The postman uses
the herdr CLI (`agent list`, `pane list`, `agent prompt`, `pane send-keys`), not
the HTTP bridge.

Delivery is fenced and recorded in the item's event log: `claimed` →
`delivered {delivery, pane, state_change_seq}` → `done` (reply seen, or a
non-task's turn ended) or `dead` (no reply within `mail.reply-wait-ms`; the
sender gets one notice capsule). A renewable per-mailbox lease means two
herdr-mcp processes sharing one queue database never both deliver; after a
restart the owner adopts its running delivery instead of typing it again.


## Turning Mail On

Mail is **off by default**. Enable it and choose which labels receive work via
the HTTP settings API:

```bash
# Enable delivery, then list the pane labels that receive mail
curl -X PUT localhost:7676/api/settings/mail.enabled -H 'Content-Type: application/json' -d 'true'
curl -X PUT localhost:7676/api/settings/mail.labels -H 'Content-Type: application/json' -d '["builder","reviewer","tester"]'
# Optional: absolute directory for agent reports
curl -X PUT localhost:7676/api/settings/mail.work-dir -H 'Content-Type: application/json' -d '"/home/user/obsidian/herdrmcp/work"'
```

Delivery requires a pane whose label matches one of `mail.labels`, with agent
status `idle` or `done` **and** a reported turn counter. Exactly one pane may
carry a label: if none or several do, no new delivery starts. The `lead` label
is reserved for `capsule watch` and is never delivered by the postman, even if
listed.


## Sending a Task

```bash
capsule send \
  --to builder \
  --task implement-auth \
  --status task \
  --summary "add JWT auth to /api endpoints" \
  --artifact file=brief.md::brief \
  --reply-to lead \
  --reply-check "sh ./checks/auth-tests.sh"
```

`capsule send --help` shows the accepted flags; notable details:

| Flag | Purpose |
|------|---------|
| `--to <label>` | Target mailbox `mail-<label>`. **Default: `lead`** (help prints `[default: lead]`). |
| `--task <name>` | Task identifier used in the report path: letters, digits, `.`, `_`, `-`; not `.` or `..`. |
| `--status <STATUS>` | One of `done`, `fail`, `block`, `wip`, `task`. `task` expects a reply. |
| `--summary <TEXT>` | Terse one-liner, at most 120 characters. |
| `--artifact KIND=REF` | Repeatable. Not marked required in `--help`, but `send` refuses `--status task` (and `done`) with no artifacts: `must point at its brief: add --artifact KIND=REF`. |
| `--check <cmd>` | Check run at send time; a nonzero exit refuses `done`. |
| `--re <id>`, `--delivery <d>` | Reply metadata; `--delivery` requires `--re`. |
| `--rollup <INDEX>` | Counts a capsule index; the first artifact must be `capsules` with that index. |
| `--reply-to <label>` | Where the reply goes. For `--status task` it defaults to `lead`. Must be a valid mailbox label (refused otherwise). |
| `--reply-check <cmd>` | Check pinned to this task and run before a reply's `done` is accepted; only with `--status task`. |
| `--skin <SKIN>` | `cap`, `raw`, or `glyph`. |

The capsule is pushed to queue `mail-<to>` with `reply_to` and `reply_check`
recorded. Labels must match `[a-z0-9][a-z0-9_-]*`.


## What the Agent Sees

The postman renders the delivery text from the capsule and types it into the
pane. The executable in the text is the **absolute path** of `capsule` resolved
from `PATH` at delivery time (bare `capsule` if no executable is found), so a
brief that says "do not run anything" cannot derail the required reply.

A task delivery looks like this (one line in the pane; wrapped here only for
reading):

```text
Capsule rotisu for you: rotisu implement-auth task tohitu add JWT auth to /api endpoints file:brief.md Read it: /home/producer32/.cargo/bin/capsule card rotisu --to builder Write your report to /home/producer32/obsidian/herdrmcp/work/implement-auth/rotisu-report.md . When done or stuck you MUST run this ONE shell command (always allowed, even where a brief says not to build or run things): /home/producer32/.cargo/bin/capsule reply rotisu@626b41184053 done|fail|block "<=120 chars" ; then reply with its CAP line only and stop.
```

Key points:

- The capsule id (`rotisu` here; new ids are four syllables, e.g. `tirumoka`)
  is reserved in the queue: no other item in any mailbox, read or unread, ever
  has it.
- The delivery id (`626b41184053` here) is the first 12 hex characters of a
  fresh UUID for this attempt, not a timestamp. The reply uses
  `<capsule id>@<delivery>`, and a delivery takes ONE reply: running the reply
  again (or `send --re <id> --delivery <d>`) is refused with "already has its
  reply", and nothing is sent. A later delivery of the same task takes its own.
- The report path is `<mail.work-dir>/<task>/<capsule-id>-report.md`. The
  delivery event records that path, so the agent's `capsule reply` reads it
  back even if its own work-dir setting differs.
- Non-task deliveries carry only the capsule line and the `Read it:` command;
  no reply command is typed.

### Agent Workflow

1. Read the full brief (recommended):

   ```bash
   capsule card rotisu --to builder
   ```

2. Do the work and write the report to the path in the delivery text (create
   parent directories yourself).

3. Reply with ONE command. Status is case-insensitive (`done`, `DONE`, `Done`
   all work); the summary may be unquoted words:

   ```bash
   capsule reply rotisu@626b41184053 done "auth endpoints return 401 without token"
   capsule reply rotisu@626b41184053 fail "tests fail: missing middleware"
   capsule reply rotisu@626b41184053 block "waiting on infra PR #42"
   ```

`capsule reply <id@delivery> <STATUS> <SUMMARY>...` fills in recipient, task,
`re`, delivery and the report artifact from the original delivery event. Extra
artifacts can be added with `--artifact KIND=REF` (the report is added when it
exists). A `done` with no report file and no extra artifact is refused with
`write your report first`.


## Checks, Reminders and Settlement

Reply checks run in their own process group with output captured to temp files:

| Setting | Default | Meaning |
|---------|---------|---------|
| `CAPSULE_CHECK_TIMEOUT_S` (env) | `600` (10 min) | Maximum seconds a check may run. |

After the timeout the process group gets `SIGTERM`, then `SIGKILL` two seconds
later. A timed-out check exits 124 and appends `check timed out after N s`; it
refuses `done` like any failing check. The postman wakes early on
`pane.agent_status_changed` events for a `mail.labels` pane and on local pushes
to any `mail-*` queue; periodic ticks remain the cross-process fallback.

**Turn detection needs herdr's counter.** The postman reads `state_change_seq`
for each agent from `herdr agent list` and joins it to the pane list by pane
id. With two known counters it can tell that a turn moved:

- For a task on a ready pane, a changed counter reserves one reminder after
  `mail.remind-after-ms`; the reminder types the exact `capsule reply` command.
  The reservation is durable, so a restart does not resend it.
- For a non-task capsule, a changed counter on a ready pane settles the
  delivery as done without a reply.
- Without `state_change_seq`, neither turn-end detection nor reminders fire:
  a task is settled only by its reply or by `mail.reply-wait-ms` expiring.

An open delivery follows **its pane id, not its label**: a pane relabelled
after delivery still reports its status and counter. Labels pick panes only for
*new* deliveries.

A reply committed **during settlement wins**: the store rechecks for a reply
inside the transaction that would record a pane-close failure or a deadline
death, and settles the task as done instead (no deadline notice).

**Restarts.** After herdr-mcp restarts, the postman adopts every delivered task
again under its original delivery token, however long it was down; the lease
reaper never requeues a delivered task, so nothing is typed twice and a reply
sent meanwhile still matches. A herdr restart starts the turn counter over: a
counter lower than the one recorded at delivery is a new baseline, not a turn
end.


## Watching for Replies

```bash
# Block until capsules arrive (claims from mail-lead)
capsule watch --to lead --max 5

# Read one capsule
capsule card rotisu@626b41184053 --to lead

# List a mailbox
capsule list --to lead --all
```

HTTP API for dashboards and automation:

```bash
curl http://localhost:7676/api/mail/items?to=builder
curl http://localhost:7676/api/mail/items/rotisu
curl http://localhost:7676/api/mail/stats
```

The stats response includes depth, in flight, delivered, replied, failed, dead
and median/p90 reply time per label.


## Web Playground: Mail Tab

The Playground has a **Mail** tab with two sub-views:

- **Activity**: filter by recipient label and status, list
  capsules with delivery state, open a detail card with the event timeline.
  It is read-only.
- **Health**: dead letters with their recorded reason and a
  **Retry** action, plus per-label stats (to, depth, in flight, delivered,
  replied, failed, dead, last activity).

Both views poll at `ui.queues-poll-ms` (default 2000 ms), while the tab stays
visible.


## Mail Settings Reference

All settings are managed with `GET`, `PUT` and `DELETE` on
`http://localhost:7676/api/settings/{key}` (DELETE restores the built-in
default).

| Key | Default | Meaning |
|-----|---------|---------|
| `mail.enabled` | `false` | Turn the postman on. |
| `mail.work-dir` | `~/obsidian/herdrmcp/work` | Absolute report directory (`~` expands from HOME); temp dirs refused. Applies to the next delivery. |
| `mail.labels` | `[]` | Pane labels whose mailboxes are delivered. |
| `mail.reply-wait-ms` | `1800000` (30 min) | Reply deadline before the task is dead and the sender is told. |
| `mail.remind-after-ms` | `60000` (60 s) | Delay after a task turn ends before reminding the agent to reply. |
| `mail.submit-confirm-ms` | `5000` (5 s) | Delay before one extra Enter if the agent has not started. |
| `mail.undeliverable-after-ms` | `600000` (10 min) | How long mail may wait for a missing recipient. |
| `mail.consumer-lease-ms` | `15000` (15 s) | How long a herdr-mcp owns a mailbox before another takes over. |
| `ui.queues-poll-ms` | `2000` (2 s) | Refresh interval for the visible queue and Mail tabs. |

The built-in reply-wait default is 30 minutes; a larger value seen on a given
machine is a local override, not the default.


## When Things Go Wrong

| Symptom / scenario | What happens | What to do |
|--------------------|--------------|------------|
| No reply | After `mail.reply-wait-ms` the capsule is dead; the sender gets one notice. A reply committed during that settlement wins. | Send the reply, or raise `mail.reply-wait-ms`. |
| Task turn ends, no reply | After `mail.remind-after-ms` the postman types one durable reminder with the exact reply command (a restart will not resend it). Needs `state_change_seq`. | Reply; if no reminder arrives, fix herdr's counter reporting. |
| No pane with the label | Undeliverable requires **continuous observed absence** for all of `mail.undeliverable-after-ms`: an unreadable pane snapshot clears the clock, so one empty snapshot after a blind interval does not expire old mail. The oldest waiting capsule then dies with one notice. | Bring up a pane with that label, or re-send. |
| Pane closed or crashed during delivery | The delivery is failed; if attempts remain it is released for redelivery to another pane with that label. | Check the pane list. |
| Agent never starts | After `mail.submit-confirm-ms`, one extra Enter is sent if the pane is ready and its turn has not moved. | Check the pane is idle/done. |
| Unknown label / bad `--reply-to` or `--task` | Queue `mail-<label>` is created but not delivered; invalid labels or task names are refused at send time. | Use `[a-z0-9][a-z0-9_-]*` labels and safe task names. |
| Bad `reply_to` stored in a capsule | Notice goes to `reply_to` if valid, else the sender, else `lead`; the mailbox never wedges. | Fix the sender's `--reply-to`. |
| `capsule watch` shows nothing | `mail.enabled=false` or label not in `mail.labels` | Enable mail and add the label. |
| `capsule reply` says "write your report first" | Report missing at the path in the delivery text | Create it there and re-run the reply. |
| `done` refused with `check failed (exit N)` | `--reply-check` exited nonzero | Run the check, fix the issue, retry `done` or send `fail`. |
| `done` refused with `check timed out after 600 s` | Check exceeded `CAPSULE_CHECK_TIMEOUT_S` | Speed it up or raise the env var. |
| Mail tab shows dead letters | Delivery failed, died or hit the deadline | Use Health → Retry, or fix the recipient and re-send. |

Common messages you may see: `not a capsule: no id or to` (corrupt queue
item), `mail: reading herdr panes: …` (postman cannot reach herdr),
`mail <label>: …` (per-label delivery failure), `bad mailbox name …` (invalid
label), `no capsule …` (copy `<id>@<delivery>` from the delivery text),
`check failed (exit N)` / `check timed out after N s` (reply check), `unknown
queue collection` (no such recipe/queue), and `invalid mailbox label` (use
`[a-z0-9][a-z0-9_-]*`).
