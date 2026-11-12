//! Audio building blocks: role chains, speech activity, ducking envelopes, loudness, WAV I/O
//! and the `audio.png` visualizer.

pub mod chains;
pub mod envelope;
pub mod loudness;
pub mod vad;
pub mod viz;
pub mod wav;

pub const SR: u32 = 48_000;
