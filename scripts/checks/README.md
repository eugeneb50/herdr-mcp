# Gate scripts

This directory holds the single source of truth for the herdr-mcp build
gates. Each script runs one gate (`fmt`, `clippy`, `ui`, `test`, `release`)
or the full chain (`gate`); `gate.sh` runs `fmt`, `clippy`, `ui` and `test`
in that order, stopping at the first failure and naming it.

Conventions:

- POSIX `sh`, `#!/bin/sh`, `set -eu`.
- Each script takes **no arguments**. A queue payload can only name the
  script (e.g. `scripts/checks/clippy.sh`); it never passes
  shell text, so there is nothing to interpolate or inject.
- Each script `cd`s to the git toplevel first, so it works from any working
  directory.
- Each script echoes the command it runs before running it.

These scripts are used by humans, agents and the build queue. Edit them here
and nowhere else.
