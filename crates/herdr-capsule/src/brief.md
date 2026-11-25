## Report: capsule (required)
Done or stuck -> one shell cmd. Point at work, not paste it. No chat report; lead reads capsules only.

  {exe} send --to {to} --task {task} --status S --summary "terse line" --artifact file=PATH::what{check}

- S: done (finished+verified) | fail (tried, broken) | block (need decision/access) | wip (checkpoint).
- summary: <={max} chars, terse, no filler.
- --artifact KIND=REF::note, repeat. KIND: file dir log branch commit run pane url. Always a report file {work}/<task>/<id>-report.md (markdown; mkdir -p it; never /tmp): changed, how verified, not verified, questions. done needs >=1.{check_note}
- Cmd exits !=0 -> read msg, fix, rerun. Prints `CAP ...` -> reply that line only, stop.
