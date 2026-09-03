# tdo feature-tour demo tape (https://github.com/charmbracelet/vhs)
#
# Render with:  demo/record.sh   (seeds an isolated board first, never your live one)
# Outputs GIF + MP4 into demo/.
#
# The tour has two halves:
#   1. a read-only pass over the board, keymap, details, and history; then
#   2. a mutating pass done entirely on ONE throwaway card, which is deleted at
#      the end so the final frame matches the first and the GIF loops smoothly.

Output demo/tdo-demo.gif
Output demo/tdo-demo.mp4

Require tdo

Set Shell "bash"
Set FontSize 18
Set Width 1440
Set Height 820
Set Padding 12
Set Theme "Dracula"
Set TypingSpeed 55ms

# Point tdo at the isolated recording board and start from a clean screen.
Hide
Type "export PATH=$HOME/.cargo/bin:$PATH" Enter
Type "export TDO_DATA_DIR=/tmp/tdo-demo-rec TDO_CONFIG=/tmp/tdo-demo-rec.toml" Enter
Type "clear" Enter
Show

# --- Open the board ---------------------------------------------------------
# Hold on the bare prompt, type the command, pause, then launch, so it is easy
# to see how the demo starts.
Sleep 2s
Type "tdo"
Sleep 1.2s
Enter
Sleep 2.5s

# --- Floating keymap (?) ----------------------------------------------------
Type "?"
Sleep 2.5s
Escape
Sleep 700ms

# --- Numbered column jumps 1-9 (ends back on the first column) --------------
Type "2"
Sleep 800ms
Type "3"
Sleep 800ms
Type "4"
Sleep 800ms
Type "5"
Sleep 800ms
Type "1"
Sleep 1s

# --- Task Details on an existing card: tags, due date, checklist, history ---
Type "2"
Sleep 500ms
Type "j"
Sleep 300ms
Type "j"
Sleep 500ms
Enter
Sleep 2.5s
Escape
Sleep 600ms

# --- Git-backed History timeline -------------------------------------------
Type "4"
Sleep 500ms
Type "j"
Sleep 250ms
Type "j"
Sleep 500ms
Enter
Sleep 1.8s
Ctrl+D
Sleep 2s
Escape
Sleep 800ms

# === Everything below happens on ONE throwaway card, deleted at the end =====

# --- Add a task to the BACKLOG column --------------------------------------
Type "1"
Sleep 600ms
Type "a"
Sleep 600ms
Type "Plan the launch"
Sleep 900ms
Enter
Sleep 1.2s

# --- Open it -----------------------------------------------------------------
Enter
Sleep 1s

# --- Add a description ------------------------------------------------------
Type "j"
Sleep 400ms
Enter
Sleep 600ms
Type "Coordinate the v1 announcement across the team"
Sleep 1s
Enter
Sleep 1.2s

# --- Add a label and pick its color -----------------------------------------
# Move to the Tags field and open the tag picker.
Type "j"
Sleep 400ms
Enter
Sleep 900ms
# Skip past the reusable tags to the "New Tag" action (index clamps at the end).
Type "l"
Type "l"
Type "l"
Type "l"
Type "l"
Type "l"
Type "l"
Type "l"
Sleep 700ms
Enter
Sleep 700ms
# Name the new label, move to the color picker, and cycle the color.
Type "priority"
Sleep 800ms
Tab
Sleep 600ms
Type "l"
Sleep 450ms
Type "l"
Sleep 450ms
Type "l"
Sleep 800ms
Enter
Sleep 1.2s
# Close the picker, staying in task details.
Type "q"
Sleep 1s

# --- Add checklist items (narrow dialog floats over the details) ------------
Type "a"
Sleep 500ms
Type "Write the announcement"
Sleep 700ms
Enter
Sleep 600ms
Type "a"
Sleep 400ms
Type "Line up a reviewer"
Sleep 700ms
Enter
Sleep 600ms
Type "a"
Sleep 400ms
Type "Ship it"
Sleep 700ms
Enter
Sleep 1.2s

# --- Complete a couple of checklist items -----------------------------------
# Cursor is on Tags; step down past Due to the first checklist item.
Type "j"
Sleep 150ms
Type "j"
Sleep 400ms
Space
Sleep 900ms
Type "j"
Sleep 300ms
Space
Sleep 1.2s

# --- Return to the board ----------------------------------------------------
Escape
Sleep 1.3s

# --- MOVE mode: carry the card across columns -------------------------------
Type "m"
Sleep 900ms
Type "l"
Sleep 1s
Enter
Sleep 1.3s

# --- Delete it, restoring the board to its starting content -----------------
Type "D"
Sleep 1s
Type "y"
Sleep 1s

# --- Return to the BACKLOG header and hold, so the loop is seamless ---------
# Adding the card left BACKLOG's cursor on a card; step up to the header so the
# final frame matches the very first one.
Type "1"
Sleep 400ms
Type "k"
Sleep 200ms
Type "k"
Sleep 1.3s
