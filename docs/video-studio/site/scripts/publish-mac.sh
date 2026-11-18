#!/bin/zsh
# Packages the Release build of Syncfusion Video Studio as a signed .dmg and uploads it to the
# R2 bucket the docs site serves downloads from (/releases/…). Then the site's download buttons
# point at it: /releases/latest/mac always redirects to the newest upload.
#
# Usage: scripts/publish-mac.sh [--skip-build] [--dry-run]
#   --skip-build  use the app already in apps/macos/build/DD/Build/Products/Release
#   --dry-run     make the .dmg and latest.json, but upload nothing
setopt pipefail errexit nounset
SITE="${0:A:h:h}"
REPO="${SITE:h:h:h}"
MAC="$REPO/apps/macos"
BUCKET=video-studio-releases
APP_NAME="Syncfusion Video Studio"

skip_build=0 dry=0
for a in "$@"; do
  case $a in --skip-build) skip_build=1 ;; --dry-run) dry=1 ;; *) print -u2 "unknown option $a"; exit 2 ;; esac
done

(( skip_build )) || "$MAC/build.sh"
APP="$MAC/build/DD/Build/Products/Release/$APP_NAME.app"
[[ -d $APP ]] || { print -u2 "No Release build at $APP"; exit 1; }
codesign --verify --deep --strict "$APP"

plist="$APP/Contents/Info.plist"
version=$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$plist")
build=$(/usr/libexec/PlistBuddy -c 'Print CFBundleVersion' "$plist")
minos=$(/usr/libexec/PlistBuddy -c 'Print LSMinimumSystemVersion' "$plist" 2>/dev/null || print 26.0)
arch=$(lipo -archs "$APP/Contents/MacOS/$APP_NAME")
# The certificate the app is signed with signs the disk image too.
identity=$(codesign -dvv "$APP" 2>&1 | sed -nE 's/^Authority=(.*)$/\1/p' | head -1)
notarized=false
xcrun stapler validate "$APP" >/dev/null 2>&1 && notarized=true

out="$MAC/build/dist"; mkdir -p "$out"
file="Syncfusion-Video-Studio-$version-$build-mac-$arch.dmg"
dmg="$out/$file"
stage=$(mktemp -d)/stage; mkdir -p "$stage"
ditto "$APP" "$stage/$APP_NAME.app"
ln -s /Applications "$stage/Applications"
rm -f "$dmg"
hdiutil create -quiet -volname "$APP_NAME" -srcfolder "$stage" -fs APFS -format ULFO -ov "$dmg"
rm -rf "${stage:h}"
if [[ -n $identity && $identity != "(unavailable)" ]]; then
  codesign --force --sign "$identity" --timestamp=none "$dmg"
fi
hdiutil verify -quiet "$dmg"

size=$(stat -f %z "$dmg")
sha=$(shasum -a 256 "$dmg" | cut -d' ' -f1)
json="$out/latest.json"
cat > "$json" <<EOF
{
  "platform": "mac",
  "version": "$version",
  "build": "$build",
  "file": "$file",
  "size": $size,
  "sha256": "$sha",
  "minimumSystemVersion": "$minos",
  "architectures": "$arch",
  "notarized": $notarized,
  "published": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF
print "✔ $dmg ($(( size / 1048576 )) MB)"
cat "$json"
(( dry )) && exit 0

cd "$SITE"
npx wrangler r2 object put "$BUCKET/mac/$file" --remote --file "$dmg" \
  --content-type application/x-apple-diskimage --cache-control "public, max-age=31536000, immutable"
npx wrangler r2 object put "$BUCKET/mac/latest.json" --remote --file "$json" \
  --content-type application/json --cache-control "no-cache"
print "✔ Uploaded. Downloads: /releases/latest/mac"
