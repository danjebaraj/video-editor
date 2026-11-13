# SFX

Sound effects are supplied per project (ElevenLabs-generated or licensed). Nothing is bundled.

Either declare them as assets (`whoosh: { path: "sfx/whoosh.wav", role: "sfx" }`) or map the brand aliases in the brand file (paths relative to the brand file):

```json
"sfx": { "whoosh": "../sfx/whoosh.wav", "pop": "../sfx/pop.wav", "ding": "../sfx/ding.wav", "riser": "../sfx/riser.wav" }
```
