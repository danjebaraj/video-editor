#!/usr/bin/env bash
# Installs Syncfusion Video Studio on this Mac from this checkout. It does everything:
#   1. Rebuilds cutline (release) into bin/, with its manual, schema and assets, and links it as
#      ~/.local/bin/cutline.
#   2. Installs cutline's required tools (ffmpeg and models) when `cutline doctor` finds some
#      missing.
#   3. Builds the app (release), with the video-studio helper inside it, quits a running copy and
#      installs it as /Applications/Syncfusion Video Studio.app.
#   4. Checks that the installed app uses the cutline just built, from the same sources.
#
# cutline and the app are rebuilt together every time, so they can't fall out of step: run it
# again after pulling changes.
#
# Usage: ./install.sh
set -euo pipefail
cd "$(dirname "$0")"
ROOT=$(pwd)
APP="/Applications/Syncfusion Video Studio.app"

fail() { echo "✘ $*" >&2; exit 1; }

[[ $(uname -s) == Darwin ]] || fail "install.sh installs the macOS app; run it on a Mac."
for tool in cargo xcodebuild xcodegen; do
  command -v "$tool" >/dev/null || case $tool in
    cargo) fail "Rust isn't installed: get it from https://rustup.rs, then run this again." ;;
    xcodebuild) fail "Xcode isn't installed: install it from the App Store, open it once, then run this again." ;;
    xcodegen) fail "XcodeGen isn't installed: run \`brew install xcodegen\`, then run this again." ;;
  esac
done

echo "› 1/4 Rebuilding cutline"
scripts/build-and-release.sh

echo "› 2/4 Checking cutline's tools"
if bin/cutline doctor --json 2>/dev/null | plutil -extract ok raw -o - - 2>/dev/null | grep -qx true; then
  echo "✔ cutline has the tools it needs"
else
  echo "Installing cutline's tools (ffmpeg, models and Chrome; this can take a few minutes)…"
  bin/cutline deps install --all
  bin/cutline doctor --json 2>/dev/null | plutil -extract ok raw -o - - 2>/dev/null | grep -qx true \
    || fail "cutline still reports missing tools: run \`bin/cutline doctor\` to see which."
fi

echo "› 3/4 Building and installing the app"
# cutline was just built in step 1, so the app build doesn't build it again
VS_SKIP_CUTLINE=1 apps/macos/build.sh install

echo "› 4/4 Checking the installed app"
helper="$APP/Contents/Helpers/video-studio"
[[ -x $helper ]] || fail "$helper is missing: the app build didn't bundle its helper."
want=$(realpath "$ROOT/bin/cutline")
info=$("$helper" cutline --json)
got=$(plutil -extract cutline.path raw -o - - <<<"$info" 2>/dev/null || true)
current=$(plutil -extract current raw -o - - <<<"$info" 2>/dev/null || true)
if [[ $got != "$want" ]]; then
  custom=$(plutil -extract settings_path raw -o - - <<<"$info" 2>/dev/null || true)
  if [[ -n $custom ]]; then
    fail "The app would use ${got:-no cutline}, not $want: Settings › cutline is set to $custom. Clear that field."
  fi
  fail "The app would use ${got:-no cutline}, not $want: it comes earlier on your PATH. Remove it, or put ~/.local/bin first."
fi
[[ $current == true ]] || fail "$want isn't the cutline build the app expects (cutline build-info)."
echo "✔ Installed $APP, using $want (built from the same sources)"
