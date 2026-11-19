#!/usr/bin/env bash
# Build the release Syncfusion Video Studio macOS app and install it to
#   /Applications/Syncfusion Video Studio.app
# quitting a running copy first. This is `apps/macos/build.sh install`; see that script for
# the other builds (debug, selftest, distribute).
#
# Usage: scripts/install-video-studio-app.sh
set -euo pipefail
cd "$(dirname "$0")/.."
exec apps/macos/build.sh install
