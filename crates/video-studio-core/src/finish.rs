//! The finish of a video: how shots join, how the music starts, sits under the voice and ends,
//! and how the edit follows the music's beat. One set lives in Settings as the app-wide default;
//! a project either follows it or keeps its own.
//!
//! Nothing here renders anything. `directions` turns the choices into the instructions a run's
//! prompt carries, in cutline's own terms, so every run starts from the same finished look
//! instead of whatever the spec happened to mention.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How one shot hands over to the next.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Transitions {
    /// A short soft move between scenes, plain cuts inside a scene.
    #[default]
    Smooth,
    /// Plain cuts everywhere; fades only for the title and the end card.
    Cuts,
    /// Quick moving transitions (whips, pushes) between scenes, for social cuts.
    Lively,
}

impl Transitions {
    pub fn label(self) -> &'static str {
        match self {
            Transitions::Smooth => "Smooth",
            Transitions::Cuts => "Plain cuts",
            Transitions::Lively => "Lively",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(default)]
pub struct Finish {
    pub transitions: Transitions,
    /// Length of a transition between scenes, in seconds.
    pub transition_s: f64,
    /// Land cuts on the music's beat and ask for music with a beat that can be followed.
    pub cut_on_beat: bool,
    /// Seconds of picture and music before the first spoken word.
    pub lead_in_s: f64,
    /// Seconds the last picture holds after the last spoken word.
    pub tail_s: f64,
    /// Seconds the music takes to come in.
    pub music_fade_in_s: f64,
    /// Seconds the music takes to go out when it has no ending of its own to land.
    pub music_fade_out_s: f64,
    /// Ease on-screen text in and out instead of popping it on.
    pub ease_text: bool,
    /// Ease into close-ups of the part of the screen being talked about.
    pub guide_the_eye: bool,
}

impl Default for Finish {
    fn default() -> Self {
        Finish {
            transitions: Transitions::Smooth,
            transition_s: 0.4,
            cut_on_beat: true,
            lead_in_s: 1.2,
            tail_s: 2.0,
            music_fade_in_s: 1.0,
            music_fade_out_s: 2.0,
            ease_text: true,
            guide_the_eye: true,
        }
    }
}

fn secs(v: f64) -> String {
    let s = format!("{:.2}", v.max(0.0));
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

impl Finish {
    /// Values a hand-edited file could carry are pulled back into the range the app offers.
    pub fn clamped(&self) -> Finish {
        Finish {
            transition_s: self.transition_s.clamp(0.15, 1.5),
            lead_in_s: self.lead_in_s.clamp(0.0, 5.0),
            tail_s: self.tail_s.clamp(0.0, 8.0),
            music_fade_in_s: self.music_fade_in_s.clamp(0.0, 5.0),
            music_fade_out_s: self.music_fade_out_s.clamp(0.0, 8.0),
            ..self.clone()
        }
    }

    /// One line for run.json, the logs and the project summary.
    pub fn summary(&self) -> String {
        let f = self.clamped();
        format!(
            "{} transitions ({} s), {}, {} s lead-in, {} s tail, music in over {} s and out over {} s",
            f.transitions.label().to_lowercase(),
            secs(f.transition_s),
            if f.cut_on_beat { "cuts on the beat" } else { "cuts not tied to the beat" },
            secs(f.lead_in_s),
            secs(f.tail_s),
            secs(f.music_fade_in_s),
            secs(f.music_fade_out_s)
        )
    }

    /// The "Finish" section of a run's prompt. `has_music` is false when the run can neither
    /// generate music nor has any attached, so the music lines would only confuse.
    pub fn directions(&self) -> String {
        let f = self.clamped();
        let t = secs(f.transition_s);
        let mut s = String::from(
            "These are the user's standing choices for how every video is finished. They apply unless spec.md or the brand rules say otherwise, and where they differ from the general advice in the cutline guide below, these win.\n",
        );
        match f.transitions {
            Transitions::Smooth => s.push_str(&format!(
                "- **Shots join smoothly.** A scene is one heading of the spec. Between two scenes put a transition item of {t} s: `{{transition: {{type: \"smoothleft\", duration: {t}}}}}` when the next scene moves on to a new page, screen or topic, and `{{transition: {{type: \"fade\", duration: {t}}}}}` into and out of full-frame cards (title, section, statistic, end card) and between camera footage. Use at most two transition types in one video. Inside a scene, between two views of the same page or app, use a plain cut. A `fade` between two different screen recordings shows both screens' text at once (`W_SCREEN_DISSOLVES`): use the moving transition there.\n\
                 - **No shot starts or ends mid-motion.** Cut while the picture is at rest: start a clip just before a scroll or an action begins and end it after it has settled, never halfway through a scroll. When two parts of one recording are joined, cut from rest to rest.\n"
            )),
            Transitions::Cuts => s.push_str(&format!(
                "- **Shots join with plain cuts.** No transition items between scenes; keep a `fade` of {t} s out of the title and into the end card only. Cut while the picture is at rest, never halfway through a scroll or an action.\n"
            )),
            Transitions::Lively => s.push_str(&format!(
                "- **Shots join with energy.** Between scenes use a moving transition of {t} s (`whip_left`, `slideleft`, `zoom_punch`), the same one or two types throughout; plain cuts inside a scene; a `fade` into the end card. A quiet `whoosh` on scene changes is welcome (`auto.sfx_on_transitions`) when sound effects can be generated.\n"
            )),
        }
        s.push_str(&format!(
            "- **Open and close with room to breathe.** The picture and the music start first and the first spoken word comes {lead} s later: give the voice-over layer `start: {lead}` (after a title card, start it as the card hands over). Open on a title card or the strongest still picture, not on a cold cut into motion. The very first frame is that picture, never black: feeds and players show it before anything plays, and QA flags a black one (`first_frame`). Ease the title's text in over the card instead of fading the whole picture up from black. After the last spoken word the end card holds for another {tail} s, so the video never stops on the final syllable. Plan the length with both in mind.\n",
            lead = secs(f.lead_in_s),
            tail = secs(f.tail_s),
        ));
        s.push_str(&format!(
            "- **Music comes in and goes out gently.** Give the music layer `fade_in: {fi}`. End it on the track's own ending with `align: \"end\"` when the track has one; otherwise, or when the track is longer than the video, give it `fade_out: {fo}` so it never stops dead. Keep it under the voice with `audio.music_level` rather than `gain_db`, and leave `audio.duck` at cutline's defaults: the music then stays down through the short pauses between sentences instead of swelling and dropping in each one. Let the music lift only where the voice stops for a second and a half or more: the open, a section change, the end card.\n",
            fi = secs(f.music_fade_in_s),
            fo = secs(f.music_fade_out_s),
        ));
        if f.cut_on_beat {
            s.push_str(
                "- **The edit follows the beat.** Add `auto: {snap_to_beats: {}}` so each cut timed to the narration moves onto the nearest beat, and run `cutline beats project.json5` to see the bars: put scene changes, the title and the end card on the first beat of a bar where one is close. A transition is centred on its cut, so the beat falls in the middle of it. `cutline expand` lists what moved; `W_SNAP_NO_BEATS` means the track has no beat that can be followed, so choose or generate another track.\n\
                 - **Choose music that can be cut to.** When you generate music, ask for it in the brief, whatever mood the spec gives: an instrumental with a clear, steady beat in 4/4 at one constant tempo (name the BPM: about 100 to 120 for a calm business video, faster for social cuts), drums or a pulse from the first bar, no tempo changes, no long beatless intro or breakdown, and a clean ending on a final chord. Make it about as long as the video plus 4 seconds. When the spec attaches music, use it, and say in the summary if its beat could not be followed.\n",
            );
        }
        if f.ease_text {
            s.push_str(
                "- **On-screen text eases in and out.** Give text layers `enter: \"fade\"` and `exit: \"fade\"` (or a short `slide_up` in) with the default duration, bring each one in a few tenths of a second after its scene's cut rather than on it, and take it out before the next transition starts. No `pop`.\n",
            );
        }
        if f.guide_the_eye {
            s.push_str(
                "- **Guide the eye.** On screen recordings, when the narration talks about one part of the screen for two seconds or more, ease into it with a `zoom_to` (about 0.6 s, at most about 1.6× so text stays sharp) and ease back out before the cut. One zoom per shot at most, none on shots under three seconds.\n",
            );
        }
        s.push_str("- **Check the joins.** In the review pack, look at every cut pair and at the audio picture: no cut mid-scroll, no text popping on a cut, no music swell between two sentences, no silence under the end card. Fix what you find before rendering.\n");
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_ask_for_a_smooth_finish() {
        let d = Finish::default().directions();
        for needle in [
            "smoothleft",
            "duration: 0.4",
            "snap_to_beats",
            "start: 1.2",
            "never black",
            "another 2 s",
            "fade_in: 1",
            "fade_out: 2",
            "constant tempo",
            "enter: \"fade\"",
            "zoom_to",
        ] {
            assert!(d.contains(needle), "missing {needle:?} in:\n{d}");
        }
    }

    #[test]
    fn choices_change_the_directions() {
        let f = Finish {
            transitions: Transitions::Cuts,
            cut_on_beat: false,
            ease_text: false,
            guide_the_eye: false,
            ..Finish::default()
        };
        let d = f.directions();
        assert!(d.contains("plain cuts") && !d.contains("smoothleft"));
        assert!(!d.contains("snap_to_beats") && !d.contains("zoom_to") && !d.contains("enter:"));
        assert!(Finish { transitions: Transitions::Lively, ..Finish::default() }.directions().contains("whip_left"));
    }

    #[test]
    fn stored_values_are_tolerant_and_clamped() {
        // an older settings file has no finish at all; a newer one may have more keys
        let f: Finish = serde_json::from_str("{\"transitions\": \"cuts\", \"future\": 1}").unwrap();
        assert_eq!((f.transitions, f.cut_on_beat), (Transitions::Cuts, true));
        let wild = Finish { transition_s: 30.0, lead_in_s: -4.0, ..Finish::default() }.clamped();
        assert_eq!((wild.transition_s, wild.lead_in_s), (1.5, 0.0));
        assert!(Finish::default().summary().starts_with("smooth transitions (0.4 s), cuts on the beat"));
    }
}
