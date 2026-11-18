#!/bin/zsh
# Retakes the screenshots in the user guide (docs/video-studio/images/) from the real app.
#
#   ./build.sh docshots
#
# It builds the Debug app, makes sample media for a made-up product (Acme Diagrams) in
# /Users/Shared/Demo, runs the app in its screenshot mode (Sources/VideoStudio/App/
# DocShots.swift) and resizes the pictures into docs/video-studio/images/. The app opens and closes
# its windows on screen for about a minute: leave the Mac alone until it finishes.
# Allow Screen Recording for the app you run this from (System Settings › Privacy & Security).
# The mode uses its own library and a stand-in API key; it restores your recent projects.
setopt pipefail
cd "${0:A:h}/.."
APP_DIR=$PWD
REPO="${APP_DIR:h:h}"
DEMO="${VS_DOCSHOTS:-/Users/Shared/Demo}"
IMAGES="$REPO/docs/video-studio/images"

[[ -f $DEMO/media/diagram-demo.mov ]] || ./scripts/demo-media.sh "$DEMO/media" || exit 1
"$APP_DIR/build.sh" debug || exit 1

APP="$APP_DIR/build/DD/Build/Products/Debug/Syncfusion Video Studio.app"
rm -rf "$DEMO/shots" "$DEMO/docshots.log"
# Run as a child of this shell, so the screen-recording permission of the app running the script
# (Terminal, or the Claude desktop app) applies. Without it the pictures fall back to the views'
# own drawing: no sheets, no menus popups, no video, and selected rows come out black.
VS_DOCSHOTS="$DEMO" "$APP/Contents/MacOS/Syncfusion Video Studio" >/dev/null 2>&1
tail -60 "$DEMO/docshots.log" 2>/dev/null
[[ -d $DEMO/shots ]] || { print -u2 "No screenshots were taken. See $DEMO/docshots.log"; exit 1; }

# Nothing personal may be in a picture that is published: read them all (OCR) before they are copied.
swiftc -O -o "$DEMO/scan-shots" scripts/scan-shots.swift 2>/dev/null || { print -u2 "Couldn't build scripts/scan-shots.swift"; exit 1; }
"$DEMO/scan-shots" "$DEMO"/shots/*.png || { print -u2 "Not copied: a picture shows personal details (see above)."; exit 1; }

mkdir -p "$IMAGES"
for f in "$DEMO"/shots/*.png; do
  name=${f:t}
  # 2× pictures are 3000 px wide or more; 1800 px is sharp on a Retina display and light on the web.
  sips -Z 1800 "$f" --out "$IMAGES/$name" >/dev/null
done
print "✔ $(ls "$IMAGES" | wc -l | tr -d ' ') pictures in $IMAGES"
