#!/bin/bash
# Optional live test (ui/spec.md §20): one real end-to-end run with Claude Haiku and a
# 10-second Shorts draft. It uses your signed-in Claude Code and the real cutline, and costs a
# few cents. Nothing is written outside the scratch folder it prints.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
SCRATCH="${1:-$(mktemp -d)/video-studio-live}"
mkdir -p "$SCRATCH/footage"
cargo build -q -p video-studio-cli --manifest-path "$REPO/Cargo.toml"
VS="$REPO/target/debug/video-studio"
FFMPEG="$(cutline deps path ffmpeg)"
"$FFMPEG" -y -loglevel error -f lavfi -i "testsrc2=size=1920x1080:rate=30:duration=12" \
  -f lavfi -i "sine=frequency=330:duration=12" -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest "$SCRATCH/footage/demo-clip.mp4"
export VIDEO_STUDIO_HOME="$SCRATCH/library"
cd "$SCRATCH"
"$VS" new live.cutproj --intent social_clip --name "Live test" --media footage
python3 - "$SCRATCH" <<'PY'
import json, sys
scratch = sys.argv[1]
p = json.load(open("live.cutproj"))
p["spec"] = """# Live test clip

A 10-second vertical clip made from one piece of footage. No narration and no music: keep the clip's own audio.

## Scene 1: The clip
- **Duration:** 10 s
- **Media:** ![demo clip](media:m-demo-clip)
- **On-screen text:** Hello, Video Studio

Show the footage for the full 10 seconds with the on-screen text as a title for the first 3 seconds.

## Call to action
None: this is a technical test.
"""
p["intent"]["target_length_s"] = 10
p["output_configs"] = [{"id": "out_shorts", "preset": "youtube_shorts", "overrides": {}}]
p["run_configs"][0].update({"name": "Shorts", "outputs": ["out_shorts"], "production_model": "haiku", "quality": "draft",
                            "output_folder": scratch + "/out", "dry_run_first": True})
json.dump(p, open("live.cutproj", "w"), indent=2)
PY
"$VS" run live.cutproj --config Shorts --yes --log-level debug
echo "Run folder(s): $SCRATCH/out"
