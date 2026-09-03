#!/usr/bin/env bash
# Repeatably record the tdo feature-tour demo.
#
# Seeds an ISOLATED, ephemeral board and renders demo/demo.tape to
# demo/tdo-demo.gif and demo/tdo-demo.mp4 with VHS.
#
# Your live tdo data is never touched. The recording overrides BOTH the data
# directory (TDO_DATA_DIR) and the config file (TDO_CONFIG) to throwaway paths,
# which are wiped before and after the run — so you can re-record a fresh demo
# at any time, even after you have started using tdo with your own real data.
#
# Usage:  demo/record.sh
#
# Requires: vhs, ttyd, ffmpeg (VHS deps) and the tdo binary on PATH.

set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Isolated, ephemeral recording sandbox. These paths must match the exports in
# demo/demo.tape (which is what actually points the recorded tdo at them).
REC_DIR="/tmp/tdo-demo-rec"
REC_CONFIG="/tmp/tdo-demo-rec.toml"

# vhs installed via `go install` lives in the Go bin dir; make sure it's found.
export PATH="$HOME/.cargo/bin:$HOME/go/bin:$PATH"

for bin in vhs ttyd ffmpeg tdo; do
  command -v "$bin" >/dev/null || { echo "error: '$bin' not found on PATH" >&2; exit 1; }
done

echo "==> Seeding isolated recording board at $REC_DIR (your live board is untouched)"
rm -rf "$REC_DIR" "$REC_CONFIG"
TDO_DATA_DIR="$REC_DIR" TDO_CONFIG="$REC_CONFIG" "$REPO_DIR/demo/seed.sh" >/dev/null

echo "==> Recording demo (this drives a real TUI; do not touch the keyboard)"
cd "$REPO_DIR"
vhs demo/demo.tape

echo "==> Cleaning up recording sandbox"
rm -rf "$REC_DIR" "$REC_CONFIG"

echo "==> Done. Wrote:"
ls -lh "$REPO_DIR"/demo/tdo-demo.gif "$REPO_DIR"/demo/tdo-demo.mp4 2>/dev/null || true
