# Test fixture sources

| File | Source | License |
|---|---|---|
| `face_armstrong.jpg` | NASA portrait of Neil Armstrong (S69-31741), via Wikimedia Commons `File:Neil_Armstrong_pose.jpg`, downscaled to 480×600 | Public domain (work of NASA, a US federal agency) |

Used by the reframe tests: the face is panned across a 16:9 canvas with ffmpeg.

| File | Source | License |
|---|---|---|
| `heic/portrait-tiled.heic` | Synthetic test card (four colour quadrants, grid lines, a white marker) drawn by `heic/make_fixtures.swift` and written by macOS ImageIO as an iPhone-style HEIC: 4032×3024 stored as 48 HEVC tiles of 512×512, orientation 6 (shown 3024×4032, rotated 90°) | CC0, made by the maintainer for these tests |
| `heic/single.heif` | The same test card at 800×600, one untiled HEVC image, no rotation (`make_fixtures.swift`) | CC0, made by the maintainer for these tests |

Used by the HEIC/HEIF tests (FEAT-0004). Regenerate with
`swiftc -O make_fixtures.swift -o make && ./make portrait-tiled.heic 4032 3024 6 0.5 && ./make single.heif 800 600 1 0.7`.
