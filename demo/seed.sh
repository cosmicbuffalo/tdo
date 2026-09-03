#!/usr/bin/env bash
# Seed a self-documenting demo board for tdo.
#
# Every card's title (and often its checklist/tags/due date) describes the very
# feature it is demonstrating, so the board doubles as a live feature tour —
# handy for the demo recording and for poking around a populated board.
#
# Usage:
#   TDO_DATA_DIR=/tmp/tdo-demo demo/seed.sh   # seed an isolated demo board
#
# The target data directory is WIPED and rebuilt so the result is deterministic.
# Because it wipes, seed.sh never guesses a path: it REQUIRES an explicit
# TDO_DATA_DIR and refuses to run against your live board. Usually you just run
# demo/record.sh, which sets up the isolation for you.

set -euo pipefail

DATA_DIR="${TDO_DATA_DIR:-}"
if [ -z "$DATA_DIR" ]; then
  echo "error: TDO_DATA_DIR is not set; seed.sh wipes its target, so it will not" >&2
  echo "       guess a path. Point it at a throwaway directory, e.g.:" >&2
  echo "         TDO_DATA_DIR=/tmp/tdo-demo demo/seed.sh" >&2
  echo "       or run demo/record.sh, which sets up isolation for you." >&2
  exit 1
fi

# Never wipe the live board. Compare canonical paths (realpath -m does not
# require the path to exist); override with ALLOW_LIVE=1 only if you mean it.
canon() { realpath -m -- "$1" 2>/dev/null || printf '%s' "$1"; }
LIVE_DEFAULT="${XDG_DATA_HOME:-$HOME/.local/share}/tdo"
if [ "${ALLOW_LIVE:-0}" != "1" ] && [ "$(canon "$DATA_DIR")" = "$(canon "$LIVE_DEFAULT")" ]; then
  echo "refusing to seed the live board at $(canon "$LIVE_DEFAULT")" >&2
  echo "(choose a different TDO_DATA_DIR, or set ALLOW_LIVE=1 to override)" >&2
  exit 1
fi

export TDO_DATA_DIR="$DATA_DIR"
# Isolate config too, so seeding never reads or writes your live tdo config.
export TDO_CONFIG="${TDO_CONFIG:-${DATA_DIR%/}.config.toml}"

echo "Seeding demo board at: $DATA_DIR"
rm -rf "$DATA_DIR"

# Helpers -------------------------------------------------------------------
# Run a tdo mutation and return the created/affected object id from JSON.
id() { tdo --json "$@" | jq -r '.id'; }
run() { tdo "$@" >/dev/null; }

# First run auto-creates the board with a single first column (id 1).
tdo --json list >/dev/null

# Reusable tags with an intentional, readable palette ----------------------
run tag create feature --color '#98C379'   # green
run tag create design  --color '#61AFEF'   # blue
run tag create urgent  --color '#E06C75'   # red
run tag create docs    --color '#C678DD'   # purple
run tag create bug     --color '#E5C07B'   # yellow
run tag create ai      --color '#56B6C2'   # cyan

# Columns: a classic kanban flow (first column is renamed, not re-created) --
run column rename 1 BACKLOG
run column add TODO      # id 2
run column add DOING     # id 3
run column add REVIEW    # id 4
run column add DONE      # id 5

# BACKLOG -------------------------------------------------------------------
id task add 1 "Full card titles wrap on the board, so even a long and deliberately verbose descriptive title like this one is shown in its entirety" \
  --tag design >/dev/null

id task add 1 "Numbered column jumps: press 1-9 to leap straight to any column" \
  --description "Navigate with the arrow keys or hjkl, and jump directly to a column by its number." \
  --tag docs >/dev/null

# TODO ----------------------------------------------------------------------
T_TAGS=$(id task add 2 "Colored tags and due dates render as bullet items beneath the card title" \
  --description "Tags use their configured color with automatic black/white text for contrast." \
  --tag design --tag urgent --due 2026-09-05)

T_CHECK=$(id task add 2 "Checklist progress is summarized on the card as - [ ] X / Y" \
  --description "Open the card to check items off; the board card tracks the running count." \
  --tag feature)
run checklist add "$T_CHECK" "Draft the feature spec"
run checklist add "$T_CHECK" "Review it with the team"
run checklist add "$T_CHECK" "Implement and test"
run checklist add "$T_CHECK" "Announce the release"
run checklist toggle "$T_CHECK" 1
run checklist toggle "$T_CHECK" 2

# DOING ---------------------------------------------------------------------
id task add 3 "Press m to enter MOVE mode, then reorder within a column or move across columns" \
  --description "MOVE mode outlines the card in orange; Enter or m commits the move as one commit, Esc cancels." \
  --tag feature >/dev/null

T_DETAILS=$(id task add 3 "Press Enter to open Task Details: description, checklist, tags, due date, and a Git-backed History timeline" \
  --description "Task Details is the full editor. Every field here is also settable from the CLI, so AI agents and humans edit the same model." \
  --tag docs --tag feature --due 2026-09-10)
run checklist add "$T_DETAILS" "Explore each editable field"
run checklist add "$T_DETAILS" "Scroll the History timeline"
run checklist toggle "$T_DETAILS" 1

# REVIEW --------------------------------------------------------------------
id task add 4 "Overdue due dates make slipping work easy to spot" \
  --description "This card's due date is in the past." \
  --tag urgent --tag bug --due 2026-08-20 >/dev/null

# History demo: build up a real timeline through several confirmed mutations.
T_HIST=$(id task add 4 "Every confirmed change is a Git commit; the History tab shows the full timeline" \
  --tag feature)
run task edit "$T_HIST" --description "Draft: history is backed by an append-only event ledger."
run task edit "$T_HIST" --due 2026-09-08
run task edit "$T_HIST" --tag feature --tag docs
run task edit "$T_HIST" --title "Every confirmed change is a Git commit; open History to see this card's timeline"
run task edit "$T_HIST" --description "History is backed by an append-only Git event ledger and a disposable SQLite index."

# DONE ----------------------------------------------------------------------
T_DONE=$(id task add 5 "Fully-checked checklists render as - [x] Y / Y" --tag feature)
run checklist add "$T_DONE" "Everything here is finished"
run checklist add "$T_DONE" "And so is this"
run checklist toggle "$T_DONE" 1
run checklist toggle "$T_DONE" 2

id task add 5 "A complete non-interactive CLI lets AI agents manage the board with stable IDs and --json output" \
  --description "This entire demo board was built by the tdo CLI. See: tdo --json list, tdo task add/edit/move, tdo checklist, tdo tag." \
  --tag ai --tag docs >/dev/null

echo
echo "Demo board seeded. Summary:"
tdo list