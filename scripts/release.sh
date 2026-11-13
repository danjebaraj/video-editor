#!/usr/bin/env bash
# Build release artifacts:
#   macOS: universal (arm64 + x86_64) binary via lipo  → dist/cutline-<ver>-macos-universal.tar.gz
#   Windows (run on Windows / Git Bash):               → dist/cutline-<ver>-windows-x64.zip
#   Linux:                                              → dist/cutline-<ver>-linux-<arch>.tar.gz
# Each archive contains the `cutline` binary and the `assets/` directory next to it.
set -euo pipefail
cd "$(dirname "$0")/.."
VER=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
FEATURES="${CUTLINE_FEATURES:-onnx}"
mkdir -p dist
stage() { # $1 = binary path, $2 = archive name
  local d="dist/$2"
  rm -rf "$d" && mkdir -p "$d"
  cp "$1" "$d/"
  cp -R assets "$d/assets"
  cp README.md docs/CLAUDE_GUIDE.md docs/PROJECT_FORMAT.md "$d/"
  echo "$d"
}
case "$(uname -s)" in
  Darwin)
    rustup target add aarch64-apple-darwin x86_64-apple-darwin
    cargo build --release -p cutline-cli --features "$FEATURES" --target aarch64-apple-darwin
    cargo build --release -p cutline-cli --features "$FEATURES" --target x86_64-apple-darwin
    mkdir -p target/universal
    lipo -create -output target/universal/cutline \
      target/aarch64-apple-darwin/release/cutline target/x86_64-apple-darwin/release/cutline
    d=$(stage target/universal/cutline "cutline-$VER-macos-universal")
    tar -C dist -czf "$d.tar.gz" "$(basename "$d")"
    ;;
  MINGW*|MSYS*|CYGWIN*)
    cargo build --release -p cutline-cli --features "$FEATURES"
    d=$(stage target/release/cutline.exe "cutline-$VER-windows-x64")
    (cd dist && powershell -NoProfile -Command "Compress-Archive -Force -Path '$(basename "$d")' -DestinationPath '$(basename "$d").zip'")
    ;;
  *)
    cargo build --release -p cutline-cli --features "$FEATURES"
    d=$(stage target/release/cutline "cutline-$VER-linux-$(uname -m)")
    tar -C dist -czf "$d.tar.gz" "$(basename "$d")"
    ;;
esac
echo "built: $(ls dist/*.tar.gz dist/*.zip 2>/dev/null)"
