#!/bin/sh
# Run the full gate: fmt, clippy, ui, test — in that order.
# Stops at the first failure and names it. Standalone gate: no arguments.
set -eu
cd "$(git rev-parse --show-toplevel)"
echo "== gate: fmt =="
scripts/checks/fmt.sh || { echo "gate failed: fmt" >&2; exit 1; }
echo "== gate: clippy =="
scripts/checks/clippy.sh || { echo "gate failed: clippy" >&2; exit 1; }
echo "== gate: ui =="
scripts/checks/ui.sh || { echo "gate failed: ui" >&2; exit 1; }
echo "== gate: test =="
scripts/checks/test.sh || { echo "gate failed: test" >&2; exit 1; }
echo "== gate: all passed =="
