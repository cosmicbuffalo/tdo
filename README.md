# tdo

`tdo` is a modal, keyboard-first kanban board for the terminal. Its BOARD,
MOVE, DETAILS, INPUT, CONFIRM, and HELP modes each expose a focused set of
actions. Running `tdo` opens the TUI; the same board is also fully accessible
through non-interactive commands designed to be easy for scripts and AI agents
to use.

Every confirmed mutation updates a granular JSON state tree and creates a commit
in a dedicated Git repository. Interactive and CLI changes use the same model,
validation, event ledger, and history-indexing path.

## Highlights

- Vertical kanban swimlanes with up to nine columns, full card titles, colored
  tags, due dates, and mouse selection.
- Modal keyboard navigation using arrow keys or `hjkl`, numbered column jumps,
  reversible MOVE mode, and a global `?` / `Ctrl-/` keymap.
- Task titles, descriptions, timestamped checklist items, reusable colored
  tags, due dates, and immutable creation timestamps.
- A per-task History timeline backed by an append-only Git event ledger and a
  disposable, incrementally maintained SQLite query index.
- A Git-backed JSON state store that commits each confirmed mutation and can
  optionally push every change or on an interval.
- A complete non-interactive CLI with stable IDs and JSON output for scripts and
  AI agents.
- Automatic cross-process refresh when another TUI or CLI changes the same data
  repository, with stale-save protection for conflicting edits.

## Requirements

- Rust and Cargo
- Git
- A terminal with color and mouse-event support for the full TUI experience

## Install and run

```sh
git clone git@github.com:cosmicbuffalo/tdo.git
cd tdo
cargo install --path .
tdo
```

For development, `cargo run` opens the TUI without installing the binary.

On first run, `tdo` creates:

- a config file at `~/.config/tdo/config.toml` (or under
  `$XDG_CONFIG_HOME`); and
- a data repository at `~/.local/share/tdo` (or under `$XDG_DATA_HOME`).

A new board starts with one empty `TODO` column. Its initial granular state is
committed immediately to the data repository.

Use `--config PATH`/`TDO_CONFIG` to select a config file and
`--data-dir PATH`/`TDO_DATA_DIR` to override its data repository.

## TUI

The cursor can land on a column header or an individual task card. A selected
header uses the same `▊` cursor rail in its leftmost interior cell and accents
the connected column outline; its centered label changes to the accent
foreground without applying a background fill. Task cards are borderless in
BOARD mode against a black TUI background, and the selected card is indicated
only by a cursor rail spanning all of its visible rows. Normal navigation
scrolls horizontally when columns are off-screen, and `1`–`9` jumps directly
to a column.

All floating-window titles are centered and every floating window has its own
key-hint row along the bottom. While a window is open, the board hint row keeps
its height but is painted in the board background color, so the active controls
are unambiguous without causing a layout shift. `q` closes the topmost floating
window but quits from BOARD or MOVE mode; `Ctrl-C` always quits immediately.

Mouse controls below are available when `input.mouse = true` (the default).
When disabled, tdo does not request terminal mouse capture, ignores mouse
events, and hides mouse-only controls and hints.

### Board view

| Key | Action |
| --- | --- |
| `?` | Open the floating keymap outside text fields; `Esc` or `q` returns to the prior view |
| `Ctrl-/` | Open the floating keymap anywhere; while editing text it shows only textarea controls |
| Left click | Select a column header, task card, or a column's empty space |
| Double-click a task | Select it and open Task Details |
| Mouse wheel | Scroll the column under the pointer without moving the cursor |
| Arrow keys or `hjkl` | Move between column headers and task cards |
| `1`–`9` | Jump to a column |
| `Enter` | Open details for the selected column or task |
| `a` | Add a task to the current column from anywhere on the board |
| `C` | Add a column (maximum 9) |
| `r` | Rename the selected column header |
| `D` | Delete the selected task or column after confirmation |
| `m` | Begin moving the selected task |
| `q` / `Ctrl-C` | Quit |

`D` opens a confirmation dialog with cursor-selectable Cancel and Delete
buttons. Cancel is selected by default; use left/right, `h`/`l`, or `Tab` and
`Enter` to activate a button. `y` confirms directly, while `n`, `Esc`, or `q`
cancels.

Each column keeps its own cursor and vertical scroll position when focus moves
elsewhere, restoring both when focus returns. Cards that only partly fit remain
visible and are clipped at the bottom; moving the cursor onto one scrolls it
fully into view. Muted `↑ (X more)` and `↓ (X more)` indicators count the cards
hidden beyond the corresponding edge and disappear when that edge of the
column is reached.

Deleting a task removes it and all of its details. Deleting a populated column
appends its tasks, in order, to the prior column before removing it. The first
column cannot be deleted because it has no prior column and every board must
retain at least one column.

### Move mode

Move mode uses arrows or `hjkl` to reorder within a column or move across
columns. The cursor rail is replaced by an accent outline around the moving
card. `Enter` or `m` confirms the move as one commit; `Esc` restores the pre-move
board without writing history.

### Column details

Press `Enter` on a column header to inspect its name, task count, and stable ID.
Select its name and use `Enter`, `e`, or `r` to rename it. `Esc` or `q` returns
to the board.

### Task details

The editable metadata at the top is rendered as an aligned two-column table.
Labels share the width of the longest label, a two-space gutter separates the
columns, and wrapped values continue at the value column rather than under the
label.

| Key | Action |
| --- | --- |
| Arrow keys or `hjkl` | Select title, description, checklist items, tags, or due date |
| `Enter` / `Space` | Edit a field, or toggle a checklist item |
| `e` | Edit the selected field/checklist item |
| `a` | Add a checklist item |
| `d` | Remove the selected checklist item |
| `Ctrl-U` / `Ctrl-D` | Scroll up/down by half a page, as in Vim |
| Mouse wheel | Scroll the complete details page |
| `Page Up` / `Page Down` | Additional scrolling bindings, omitted from in-app hints |
| Click a checklist item | Select it and toggle its checked state |
| Click `[×]` | Close Task Details from the top-right border control |
| `Esc` / `q` | Return to the board |

Task Details grows with its contents until it reaches 80% of the terminal
height. Beyond that point the unified metadata, Checklist, and History document
scrolls without changing the modal size. Moving the keyboard selection keeps
the selected field visible; `Ctrl-U`/`Ctrl-D`, the mouse wheel, or the retained
page-key bindings can inspect any other part of the document. When content is
hidden above or below the visible region, centered muted `↑ (more)` and
`↓ (more)` indicators appear on the corresponding edge of the scroll region.
The row immediately above the keymap hints stays blank when there is no content
below and carries the `↓ (more)` indicator when needed.

Checklist items live in their own `Checklist` section below the metadata. Each
item is indented and followed by a muted timing row such as
`Added 2 hours ago`; completed items add `· Completed 1 hour ago` on that same
row. New checklist actions persist those timestamps directly. Items created by
older versions use their semantic history when available and otherwise fall
back to the task creation time.

Selecting a task's Tags field opens TAG PICKER mode. The first row contains the
task's current tags; the second contains reusable tags that can be added plus a
`New Tag` action.

| Key | Action |
| --- | --- |
| Arrow keys or `hjkl` | Move between tag rows and choices |
| `Enter` on a current tag | Remove it from the task |
| `Enter` on an available tag | Add it to the task |
| `Enter` on `New Tag` | Open the new-tag editor |
| `Esc` / `q` | Close the picker |

The new-tag editor starts with a randomly selected palette color. Type the tag
name, use `Tab` or up/down to move to the color picker, select a color with
left/right or `h`/`l`, and press `Enter` to create and attach the tag. Tags are
shown with their selected background color and automatically use black or white
text, whichever has stronger contrast.

Selecting a task's due-date field opens DATE PICKER mode:

| Key | Action |
| --- | --- |
| Left/right or `h`/`l` | Move one day |
| Up/down or `k`/`j` | Move one week |
| `Page Up` / `Page Down` | Move one month |
| `t` | Select today |
| `Enter` | Confirm the selected due date |
| `d` / `Delete` | Clear the due date |
| `Esc` / `q` | Cancel without changing the due date |

Input dialogs use a cursor-aware editing buffer. Arrow keys, Home/End,
Backspace/Delete, and the editor's Emacs-style word and undo/redo bindings work
without moving the cursor out of the textbox. `Enter` confirms and `Esc`
cancels. While typing, `q` and `?` insert those literal characters; use
`Ctrl-/` to open a compact keymap containing only the controls relevant to the
active textarea.

`Ctrl-G` temporarily leaves the TUI and opens the current textbox contents in
the command configured by `$EDITOR` (including commands with arguments, such as
`nvim -f`). Writing and quitting the editor returns the edited contents to the
textbox without submitting them; press `Enter` in tdo when ready. Titles,
column names, checklist items, and tag names are normalized back to one line,
while task descriptions retain multiple lines. The CLI continues to accept
dates as `YYYY-MM-DD`; tags are managed through TAG PICKER mode in the TUI.

Below the editable fields, the `History` section shows the task's Git-backed
timeline in three aligned columns: relative timestamp (`8 minutes ago`), event
type (`added due date`, `added tag`, `removed tag`, `moved`), and event content.
The type column uses Git-diff colors: added events are green, removed events are
red, changed/check-toggle events are cyan, and the initial created event uses
the normal white text color.
Field changes render the prior value in red and the new value in green. Tag
additions/removals retain the tag's configured color; moves, creation, and other
single-value events use muted gray. The newest events appear first. If a very
long history does not fit in the current terminal, the panel reports how many
earlier events are clipped.

Single-value events stay on their event row (`added due date  2026-08-05`).
Checklist status changes use `checked` or `unchecked` as the type and the item
text as the content. Checklist item insertions and removals similarly use the
short types `added` and `removed`, with the item text in the content column.
Only other true before/after changes use indented diff rows. Cross-column moves
show column names without numeric positions (`moved  TODO → DOING`);
same-column movement uses `reordered  within TODO`.

Input dialogs wrap and grow as text is entered, keep the editing cursor visible,
and use the normal modal background rather than a textbox highlight.
Continuation lines align with the text-content column, with one column of right
padding and one blank row between the content and keymap hints. Task cards also
grow vertically to show their complete wrapped title. Colored tags and due dates
appear below the title as hanging-indented bullet items. A task with checklist
items adds `- [ ] X / Y`, where `X` is the completed count; a fully completed
checklist uses `- [x] Y / Y`. Every line required by that metadata remains
visible, and card hit-testing uses the same dynamic geometry.

## CLI and agent use

Objects have stable numeric IDs. Discover them with `tdo list --json`, then use
the IDs in mutations. CLI commands never open the TUI and every successful
mutation uses the same save-and-commit path as an interactive change.

```sh
# Inspect
tdo --json list
tdo --json column list
tdo --json column show 1
tdo --json task list --column 1
tdo --json task show 7
tdo task history 7
tdo --json task history 7

# Columns
tdo column add DOING
tdo column rename 2 IN-PROGRESS
tdo column delete 2

# Tasks and every task field
tdo task add 1 "Ship the first release" \
  --description "Prepare and verify the release" \
  --tag release --tag cli --due 2026-08-01
tdo task edit 7 --title "Ship v0.1" --description "Updated notes"
tdo task edit 7 --tag release --tag urgent
tdo task edit 7 --clear-tags --clear-due
tdo task move 7 --column 2 --position 1
tdo task delete 7

# Checklist items use one-based positions
tdo checklist add 7 "Run tests"
tdo checklist edit 7 1 "Run the full test suite"
tdo checklist toggle 7 1
tdo checklist remove 7 1

# Reusable tag definitions and colors
tdo --json tag list
tdo --json tag show 1
tdo tag create urgent --color '#E06C75'
tdo tag set-color 1 '#FF6B9D'

# Configuration discovery
tdo config path
tdo config show
tdo --json config show
```

Supplying a previously unknown name through `task add --tag` or
`task edit --tag` automatically creates a reusable definition with a palette
color.

`--json` is global, so it works before or after a subcommand. Invalid IDs,
dates, positions, or empty required values return a non-zero exit status with a
specific error. Deleting the first column is rejected by both interfaces. Run
`tdo <command> --help` for the full generated interface.

## Configuration

The generated TOML contains all defaults and can be edited directly:

```toml
[persistence]
repo = "/home/me/.local/share/tdo"
push = "never"                 # never | every_change | interval
push_interval_seconds = 300
remote = "origin"

[input]
mouse = true                    # false disables capture, actions, and mouse UI

[theme]
selected_background = "dark_gray"
border = "gray"
text = "white"
muted = "dark_gray"
danger = "red"
success = "green"
change = "cyan"
```

The TUI background is always true-color `#000000`, independent of the
terminal's configurable ANSI black palette. Cursor rails, active outlines, and
selection accents are always true-color orange (`#FF8700`). Remaining theme
values accept standard terminal color names or quoted `#RRGGBB` values.
Use `tdo config path` to locate the file and `tdo config show` (optionally with
`--json`) to inspect the effective settings. `~` is expanded in the repository
path. Existing config files that omit `[input]` retain mouse support by default;
add the section above and set `mouse = false` for a keyboard-only TUI.

Push behavior:

- `never` keeps history local (the default).
- `every_change` pushes after each newly created commit.
- `interval` checks on each TUI event-loop tick and once per CLI invocation,
  pushing after the interval has elapsed. A CLI-only workflow therefore checks
  the interval the next time `tdo` is invoked.

For either push mode, configure the named remote in the data repository first.
A failed push never discards the local commit; the error is surfaced in the TUI
or CLI.

For example:

```sh
git -C ~/.local/share/tdo remote add origin git@github.com:OWNER/BOARD-DATA.git
```

The persistence repository is intentionally separate from this source-code
repository. Navigation, opening dialogs, and cancelled edits do not change the
persisted state and therefore do not create commits.

Open TUI instances monitor the shared manifest and reload a newly committed
board within the normal 250 ms event-loop interval. Selection follows stable
column/task IDs, so a selected task remains selected if another process moves
it. Reloading pauses while an input, picker, confirmation, help window, or MOVE
mode is active so partially entered work is never silently discarded, then
resumes when the TUI returns to a stable board/details view. Every save also
checks its original Git revision; if another process committed first, the stale
save is rejected and the newer committed board is loaded instead of being
overwritten. An OS-level repository-local lock serializes the short write and
commit phase across TUI/CLI processes and is released automatically on process
exit.

## Storage format

The configured repository uses persistence format v2:

```text
manifest.json                 schema, stable counters, column/tag ordering
columns/<column-id>.json      column name and ordered task IDs
tasks/<task-id>.json          one task and all of its editable metadata
tags/<tag-id>.json            one reusable tag definition and color
events/<segment>.jsonl        append-only semantic event batches
```

Normal saves calculate a small state delta and rewrite only affected task,
column, or tag files plus the manifest. Editing one task does not serialize or
stage every other task. Event batches are placed into ordered segments capped at
1,000 batches, avoiding both one-file-per-commit checkout overhead and an
unbounded monolithic log. Segments are atomically replaced and Git delta
compression handles their append-heavy contents efficiently.

Git remains the source of truth. A disposable SQLite projection lives at
`.git/tdo/history-v1.sqlite3`; it is never committed or pushed. The first history
query indexes the current event segments, records the represented Git `HEAD`,
and subsequent queries reindex only event segments changed since that commit.
The TUI requests a bounded newest page of history lazily when task details are
opened, retains only the currently viewed task's page, and reports the count of
older events. Board startup and UI memory therefore do not grow with every task
timeline. `tdo task history` intentionally returns the full requested timeline
for agent/export workflows. Deleting the SQLite file is safe—it rebuilds from
the ledger without walking historical board snapshots.

A compact save journal under `.git/tdo/` makes multi-file updates recoverable
after interruption. It contains only the changed state records, event batch,
and next manifest, then is removed after the Git commit succeeds.

Repositories containing the former monolithic `board.json` migrate
automatically on first v2 run. That one-time migration walks legacy commits,
derives their semantic events, writes the granular current state and event
ledger, removes `board.json`, and commits the migration. Normal v2 operation
never repeats the legacy history scan. Older string-only tags are migrated as
part of the same validation path.

On load, externally edited state files are validated before being accepted and
committed. For interactive latency, `push = "interval"` is preferable to
synchronous `every_change` pushing when the remote is slow.

For a repository with hundreds of thousands of commits, normal board startup is
independent of commit count. A fresh history index performs one linear pass over
the compact current event ledger—not one `git show` per commit—and later updates
are bounded by changed segments. Git maintenance and clone/pull cost still scale
with repository history in the usual way, but history queries do not use Git as
a database.

## Development

```sh
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```
