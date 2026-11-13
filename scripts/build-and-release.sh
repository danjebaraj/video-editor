#!/usr/bin/env bash
# Build a release cutline into ./bin (see build-bin.sh: binary, assets, Claude manual, schema)
# and make it the `cutline` on your PATH via a symlink:
#   ~/.local/bin/cutline -> <repo>/bin/cutline
# The link points into the repo, so later runs of this script (or build-bin.sh) update it in
# place. Override the link folder with CUTLINE_LINK_DIR.
#
# Usage: scripts/build-and-release.sh
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$(pwd)
LINK_DIR="${CUTLINE_LINK_DIR:-$HOME/.local/bin}"
LINK="$LINK_DIR/cutline"
TARGET="$ROOT/bin/cutline"

# refuse to replace a real file (an installed copy of cutline, or something else entirely)
if [ -e "$LINK" ] && [ ! -L "$LINK" ]; then
  echo "error: $LINK exists and is not a symlink; move it away and run this again" >&2
  exit 1
fi

scripts/build-bin.sh

mkdir -p "$LINK_DIR"
ln -sfn "$TARGET" "$LINK"
echo "✔ $LINK -> $TARGET"

# the link must resolve to the new build and run
"$LINK" --version >/dev/null

case ":$PATH:" in
  *":$LINK_DIR:"*)
    found=$(command -v cutline || true)
    if [ "$found" != "$LINK" ]; then
      echo "warning: \`cutline\` on your PATH is $found, which comes before $LINK_DIR" >&2
    fi
    ;;
  *)
    echo "note: $LINK_DIR is not on your PATH; add it, e.g.:" >&2
    echo "  echo 'export PATH=\"$LINK_DIR:\$PATH\"' >> ~/.zshrc" >&2
    ;;
esac
echo "✔ manual: $ROOT/bin/CUTLINE_MANUAL.md"
