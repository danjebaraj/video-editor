#!/bin/zsh
# Builds Syncfusion Video Studio for macOS: the Rust core, its Swift bindings, then the app.
# Usage:
#   ./build.sh            Release build in build/DD/Build/Products/Release
#   ./build.sh debug      Debug build (includes the in-app self test and the test doubles)
#   ./build.sh selftest   Debug build, then run the in-app self test
#   ./build.sh install    Release build, copied to /Applications
setopt pipefail
cd "${0:A:h}"

APP_NAME="Syncfusion Video Studio.app"
build() {
  # The bindings must exist before XcodeGen lists the sources.
  CONFIGURATION="$1" ./scripts/build-core.sh || return 1
  xcodegen generate -q || return 1
  xcodebuild -project VideoStudio.xcodeproj -scheme VideoStudio -configuration "$1" -derivedDataPath build/DD build 2>&1 \
    | grep -E "error:|warning: unre|\*\* BUILD" | sort -u | head -80
  [[ -d "build/DD/Build/Products/$1/$APP_NAME" ]]
}

case "$1" in
  debug)
    build Debug
    ;;
  selftest)
    build Debug || exit 1
    scratch="$(mktemp -d)/vs-selftest"
    mkdir -p "$scratch"
    VS_SELFTEST="$scratch" "build/DD/Build/Products/Debug/$APP_NAME/Contents/MacOS/Syncfusion Video Studio"
    code=$?
    cat "$scratch/selftest.log" 2>/dev/null
    exit $code
    ;;
  install)
    build Release || exit 1
    src="build/DD/Build/Products/Release/$APP_NAME"
    dest="/Applications/$APP_NAME"
    if pgrep -xq "Syncfusion Video Studio"; then
      echo "Quitting the running Syncfusion Video Studio…"
      osascript -e 'quit app id "com.syncfusion.videostudio"' >/dev/null 2>&1
      for i in {1..50}; do pgrep -xq "Syncfusion Video Studio" || break; sleep 0.2; done
    fi
    rm -rf "$dest" && ditto "$src" "$dest" || { echo "Couldn't copy to $dest" >&2; exit 1; }
    touch "$dest"
    echo "Installed $dest"
    ;;
  *)
    build Release
    ;;
esac
