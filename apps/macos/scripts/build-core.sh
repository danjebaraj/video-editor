#!/bin/bash
# Builds the Rust core for the macOS app: the UniFFI static library, its Swift bindings, and the
# `video-studio` helper binary. Run by Xcode before compiling Swift, and by build.sh.
#
#   CONFIGURATION=Debug|Release   (default Debug)
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
APP_DIR="$(cd "$HERE/.." && pwd)"
REPO="$(cd "$APP_DIR/../.." && pwd)"
# Xcode's build environment has a minimal PATH.
export PATH="$PATH:/opt/homebrew/bin:/usr/local/bin:$HOME/.cargo/bin"

CONFIG="${CONFIGURATION:-Debug}"
if [ "$CONFIG" = "Release" ]; then
  PROFILE=release
  FLAGS=(--release)
else
  PROFILE=debug
  FLAGS=()
fi

cd "$REPO"
# The fakes are only needed for the in-app self test; they are small, so they are always built.
cargo build ${FLAGS[@]+"${FLAGS[@]}"} -p video-studio-ffi -p video-studio-cli

OUT="$REPO/target/$PROFILE"
GEN="$APP_DIR/Generated"
mkdir -p "$GEN"
# Bindings are generated from the library itself, so they always match it.
cargo run -q ${FLAGS[@]+"${FLAGS[@]}"} -p video-studio-ffi --bin uniffi-bindgen -- generate \
  --library "$OUT/libvideo_studio_ffi.dylib" --language swift --out-dir "$GEN.tmp" >/dev/null
mkdir -p "$GEN"
for f in video_studio_ffi.swift video_studio_ffiFFI.h video_studio_ffiFFI.modulemap; do
  # Only replace files that changed, so Xcode doesn't recompile for nothing.
  if ! cmp -s "$GEN.tmp/$f" "$GEN/$f"; then cp "$GEN.tmp/$f" "$GEN/$f"; fi
done
rm -rf "$GEN.tmp"

# Where Xcode links from and copies the helper from.
mkdir -p "$APP_DIR/build/core"
for f in libvideo_studio_ffi.a video-studio vs-fake-claude vs-fake-cutline; do
  if ! cmp -s "$OUT/$f" "$APP_DIR/build/core/$f"; then cp "$OUT/$f" "$APP_DIR/build/core/$f"; fi
done
echo "core: $PROFILE → $APP_DIR/build/core"
