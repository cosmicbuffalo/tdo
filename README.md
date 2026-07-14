# tdo

`tdo` is a modal, keyboard-first kanban board for the terminal. Its BOARD,
MOVE, DETAILS, INPUT, CONFIRM, and HELP modes each expose a focused set of
actions. Running `tdo` opens the TUI; the same board is also fully accessible
through non-interactive commands designed to be easy for scripts and AI agents
to use.

Every confirmed mutation writes `board.json` and creates a commit in a dedicated
Git repository. Interactive and CLI changes use the same model, validation, and
history path.

## Highlights

- Vertical kanban swimlanes with up to nine columns, full card titles, colored
  tags, due dates, and mouse selection.
- Modal keyboard navigation using arrow keys or `hjkl`, numbered column jumps,
  reversible MOVE mode, and a global `?` keymap.
- Task titles, descriptions, checklist items, reusable colored tags, due dates,
  and immutable creation timestamps.
- A Git-backed JSON state store that commits each confirmed mutation and can
  optionally push every change or on an interval.
- A complete non-interactive CLI with stable IDs and JSON output for scripts and
  AI agents.

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

A new board starts with one empty `TODO` column. The first snapshot is committed
immediately to the data repository.

Use `--config PATH`/`TDO_CONFIG` to select a config file and
`--data-dir PATH`/`TDO_DATA_DIR` to override its data repository.

## TUI

The cursor can land on a column header or an individual task card. A selected
header accents the connected column outline; a selected card accents only that
card's text and border. Normal navigation scrolls horizontally when columns are
off-screen, and `1`–`9` jumps directly to a column.

All floating-window titles are centered. `q` closes the topmost floating window
but quits from BOARD or MOVE mode; `Ctrl-C` always quits immediately.

### Board view

| Key | Action |
| --- | --- |
| `?` | Open the floating keymap from anywhere; `Esc` or `q` returns to the prior view |
| Left click | Select a column header, task card, or a column's empty space |
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

Deleting a task removes it and all of its details. Deleting a populated column
appends its tasks, in order, to the prior column before removing it. The first
column cannot be deleted because it has no prior column and every board must
retain at least one column.

### Move mode

Move mode uses arrows or `hjkl` to reorder within a column or move across
columns. `Enter` or `m` confirms the move as one commit; `Esc` restores the
pre-move board without writing history.

### Column details

Press `Enter` on a column header to inspect its name, task count, and stable ID.
Select its name and use `Enter`, `e`, or `r` to rename it. `Esc` or `q` returns
to the board.

### Task details

| Key | Action |
| --- | --- |
| Arrow keys or `hjkl` | Select title, description, checklist items, tags, or due date |
| `Enter` / `Space` | Edit a field, or toggle a checklist item |
| `e` | Edit the selected field/checklist item |
| `a` | Add a checklist item |
| `d` | Remove the selected checklist item |
| `Esc` / `q` | Return to the board |

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

Input dialogs use `Enter` to confirm, `Esc` or `q` to cancel, and `Ctrl-U` to
clear. The CLI continues to accept dates as `YYYY-MM-DD`; tags are managed
through TAG PICKER mode in the TUI.

Input dialogs wrap and grow as text is entered. Task cards also grow vertically
to show their complete wrapped title and every line required for colored tags
and due-date metadata; card hit-testing uses the same dynamic geometry.

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

[theme]
accent = "yellow"
selected_background = "dark_gray"
border = "gray"
text = "white"
muted = "dark_gray"
danger = "red"
```

Theme values accept standard terminal color names or quoted `#RRGGBB` values.
Use `tdo config path` to locate the file and `tdo config show` (optionally with
`--json`) to inspect the effective settings. `~` is expanded in the repository
path.

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
repository. Navigation, opening dialogs, and cancelled edits do not change
`board.json` and therefore do not create commits.

## Storage format

The configured repository contains a human-readable `board.json` and ordinary
Git history. The file stores columns, ordered tasks, descriptions, checklist
state, reusable tag definitions and colors, due dates, creation timestamps, and
stable IDs. On load, externally edited JSON is validated and its ID counters are
repaired before it is accepted. Boards from the earlier string-only tag format
are migrated automatically.

## Development

```sh
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```
