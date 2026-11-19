#!/bin/zsh
# Builds Syncfusion Video Studio for macOS: the Rust core, its Swift bindings, then the app.
# Usage:
#   ./build.sh            Release build in build/DD/Build/Products/Release
#   ./build.sh debug      Debug build (includes the in-app self test and the test doubles)
#   ./build.sh selftest   Debug build, then run the in-app self test
#   ./build.sh install    Release build, copied to /Applications
#   ./build.sh docshots   Retake the user guide's screenshots (docs/video-studio/images/)
#   ./build.sh signing    Fill in Signing.local.xcconfig from this Mac's Apple certificates
#   ./build.sh distribute Release build signed with Developer ID, hardened runtime, notarized and
#                         stapled → build/dist/Syncfusion-Video-Studio-<version>.zip
#
# Every build except distribute, docshots and signing first builds the latest cutline into
# <repo>/bin and links it as ~/.local/bin/cutline (scripts/build-and-release.sh), then checks
# that the app will use that cutline, and stops with the fix if something else would win
# (Settings › cutline, or another cutline earlier on PATH). VS_SKIP_CUTLINE=1 skips both.
setopt pipefail
cd "${0:A:h}"

APP_NAME="Syncfusion Video Studio.app"
DD=build/DD
build() { # $1 = configuration, then extra build settings (NAME=value) for xcodebuild
  local config="$1"; shift
  # The bindings must exist before XcodeGen lists the sources.
  CONFIGURATION="$config" ./scripts/build-core.sh || return 1
  xcodegen generate -q || return 1
  rm -rf "$DD/Build/Products/$config/$APP_NAME"
  xcodebuild -project VideoStudio.xcodeproj -scheme VideoStudio -configuration "$config" -derivedDataPath "$DD" "$@" build 2>&1 \
    | grep -E "error:|warning: unre|\*\* BUILD" | sort -u | head -80
  [[ -d "$DD/Build/Products/$config/$APP_NAME" ]]
}
REPO="${0:A:h:h:h}"
build_cutline() { # the latest cutline in <repo>/bin, linked as ~/.local/bin/cutline
  [[ -n $VS_SKIP_CUTLINE ]] && { echo "Skipping the cutline build (VS_SKIP_CUTLINE)"; return 0; }
  echo "› Building the latest cutline"
  "$REPO/scripts/build-and-release.sh" || { echo "✘ the cutline build failed" >&2; return 1; }
}
check_cutline() { # $1 = configuration. The app must pick the cutline just built.
  [[ -n $VS_SKIP_CUTLINE ]] && return 0
  local profile=debug; [[ $1 == Release ]] && profile=release
  local vs="$REPO/target/$profile/video-studio" want got
  want="${$(realpath "$REPO/bin/cutline"):-$REPO/bin/cutline}"
  got=$("$vs" cutline --json | plutil -extract cutline.path raw -o - - 2>/dev/null)
  if [[ $got == "$want" ]]; then
    echo "✔ the app uses $want"
    return 0
  fi
  echo "✘ the app would use ${got:-no cutline} instead of the one just built ($want)." >&2
  local custom=$("$vs" cutline --json | plutil -extract settings_path raw -o - - 2>/dev/null)
  if [[ -n $custom ]]; then
    echo "  Settings › cutline is set to $custom: clear that field (or set it to $want)." >&2
  elif [[ -n $got ]]; then
    echo "  $got comes first on your PATH: remove it, or put ~/.local/bin before it." >&2
  fi
  return 1
}
setting() { # value of NAME in Signing.local.xcconfig
  [[ -f Signing.local.xcconfig ]] && sed -nE "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*(.*[^[:space:]])[[:space:]]*$/\1/p" Signing.local.xcconfig | tail -1
}

case "$1" in
  debug)
    build_cutline || exit 1
    build Debug || exit 1
    check_cutline Debug
    ;;
  selftest)
    build_cutline || exit 1
    build Debug || exit 1
    check_cutline Debug || exit 1
    scratch="$(mktemp -d)/vs-selftest"
    mkdir -p "$scratch"
    VS_SELFTEST="$scratch" "build/DD/Build/Products/Debug/$APP_NAME/Contents/MacOS/Syncfusion Video Studio"
    code=$?
    cat "$scratch/selftest.log" 2>/dev/null
    exit $code
    ;;
  install)
    build_cutline || exit 1
    build Release || exit 1
    check_cutline Release || exit 1
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
  docshots)
    ./scripts/docshots.sh
    ;;
  signing)
    ./scripts/setup-signing.sh
    ;;
  distribute)
    ident=$(setting VS_DISTRIBUTION_IDENTITY)
    profile=$(setting VS_NOTARY_PROFILE)
    if [[ -z $ident ]]; then
      echo "No Developer ID Application certificate configured. Add it in Xcode (Settings → Accounts →" >&2
      echo "Manage Certificates → + → Developer ID Application), then run ./build.sh signing." >&2
      exit 1
    fi
    if ! security find-identity -v -p codesigning | grep -qF -- "$ident"; then
      echo "The Developer ID certificate \"$ident\" is not in this Mac's keychain (or has no private key)." >&2
      exit 1
    fi
    DD=build/DD-dist
    build Release CODE_SIGN_IDENTITY="$ident" ENABLE_HARDENED_RUNTIME=YES OTHER_CODE_SIGN_FLAGS=--timestamp \
      CODE_SIGN_INJECT_BASE_ENTITLEMENTS=NO || exit 1
    app="$DD/Build/Products/Release/$APP_NAME"
    codesign --verify --deep --strict "$app" || { echo "signature check failed" >&2; exit 1; }
    for f in "$app" "$app/Contents/Helpers/"*; do
      codesign -dv "$f" 2>&1 | grep -q "flags=.*runtime" || { echo "not hardened: $f" >&2; exit 1; }
    done
    if codesign -d --entitlements - --xml "$app" 2>/dev/null | grep -q get-task-allow; then
      echo "the app still has the get-task-allow entitlement; notarization would reject it" >&2; exit 1
    fi
    echo "✔ signed: $(codesign -dv "$app" 2>&1 | sed -n 's/^Authority=//p' | head -1)"
    ver=$(/usr/libexec/PlistBuddy -c "Print CFBundleShortVersionString" "$app/Contents/Info.plist")
    mkdir -p build/dist
    zip="build/dist/Syncfusion-Video-Studio-$ver.zip"
    rm -f "$zip"
    ditto -c -k --keepParent "$app" "$zip"
    if [[ -z $profile ]] || ! xcrun notarytool history --keychain-profile "$profile" >/dev/null 2>&1; then
      echo "⚠ signed but NOT notarized: no working notarytool profile \"$profile\". Create it once with" >&2
      echo "  xcrun notarytool store-credentials ${profile:-videostudio-notary} --apple-id <Apple ID> --team-id <Team ID>" >&2
      echo "then run ./build.sh distribute again. Unnotarized zip: $PWD/$zip" >&2
      exit 1
    fi
    echo "Submitting to Apple's notary service (usually a few minutes)…"
    out=$(xcrun notarytool submit "$zip" --keychain-profile "$profile" --wait --output-format json) || true
    sub_status=$(print -r -- "$out" | plutil -extract status raw -o - - 2>/dev/null)
    sub_id=$(print -r -- "$out" | plutil -extract id raw -o - - 2>/dev/null)
    if [[ $sub_status != Accepted ]]; then
      echo "✘ notarization ${sub_status:-failed} ($sub_id)" >&2
      [[ -n $sub_id ]] && xcrun notarytool log "$sub_id" --keychain-profile "$profile" >&2
      exit 1
    fi
    xcrun stapler staple -q "$app" && xcrun stapler validate -q "$app" || { echo "stapling failed" >&2; exit 1; }
    spctl --assess --type execute "$app" || { echo "Gatekeeper rejected the app" >&2; exit 1; }
    rm -f "$zip" && ditto -c -k --keepParent "$app" "$zip"
    echo "✔ notarized and stapled: $PWD/$zip"
    ;;
  *)
    build_cutline || exit 1
    build Release || exit 1
    check_cutline Release
    ;;
esac
