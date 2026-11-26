## Capsules
Fields in order: `<sigil> <id> <task> <status> <from> [re:<id>] [ok|chk:N] <summary> <kind>:...`
This guide is the schema, so the line carries data, not labels. The sigil and the
status/kind markers change with the skin; the order never does:
- cap:   `CAP ... "summary"`  self-describing (sigil CAP, status word, quoted summary).
- raw:   `> ... summary`      lean, for a cloud agent that already has this guide.
- glyph: `Ꮙ ... summary`      local board; markers are Cherokee, meaning in glyphs.json -> `{exe} legend`.
status: done fail block wip. ok = sender's check passed; chk:N = it failed (exit N).
Read cheapest first: the line -> `{exe} card <id>` (artifact sizes ~tokens, notes, why a check failed) -> open only artifacts you need. Trust artifacts over summary. Answer a task delivered to you with the one command its delivery gives: `{exe} reply <id>@<delivery> done|fail|block "summary"`.
Checks time out after 600 seconds by default. Set `CAPSULE_CHECK_TIMEOUT_S` to a positive number of seconds. A timeout refuses a done reply, and a fail/block reply records the timeout.
Compact a finished task: `{exe} rollup --task T --send` archives its READ terminal capsules into one index and sends a closing capsule whose card carries the lessons; `{exe} verify <closing-id>` audits the chain.
Store: a mailbox IS the herdr-queue queue `mail-<to>`, a capsule IS an item.
