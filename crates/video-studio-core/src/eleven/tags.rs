//! Delivery tags in voice-over text: ElevenLabs' `[audio tags]` (Eleven v3 and later) and
//! Cartesia's SSML-like `<tags/>`. The spec and the run's text files may carry either; before a
//! request goes out, the tags the chosen voice can't perform are removed so they are never read
//! aloud, and captions and checks use the spoken words only.

/// The ElevenLabs model used when neither the brand nor the command names one. It performs
/// audio tags.
pub const DEFAULT_MODEL: &str = "eleven_v4";

/// Cartesia's inline tags (`<break time="1s"/>`, `<spell>…</spell>`, `<speed ratio="0.9"/>`,
/// `<volume ratio="1.2"/>`, `<emotion value="calm"/>`).
const CARTESIA_TAGS: [&str; 5] = ["break", "spell", "speed", "volume", "emotion"];

/// True when an ElevenLabs model performs `[audio tags]` rather than reading them aloud:
/// Eleven v3 and every later generation (`eleven_v3`, `eleven_v4`, `eleven_v4_turbo`, …).
pub fn eleven_performs_tags(model: &str) -> bool {
    let Some(rest) = model.strip_prefix("eleven_v") else { return false };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u32>().is_ok_and(|n| n >= 3)
}

/// The text without `[bracketed]` tags.
fn without_brackets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// The text without Cartesia's tags; what a `<spell>` tag wraps stays.
fn without_cartesia_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(end) = tail.find('>') else {
            rest = tail;
            break;
        };
        let inner = tail[1..end].trim().trim_start_matches('/');
        let name: String = inner.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        if CARTESIA_TAGS.contains(&name.to_ascii_lowercase().as_str()) {
            rest = &tail[end + 1..];
        } else {
            out.push('<');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// Spaces left where a tag was removed are folded, line by line.
fn tidy(text: &str) -> String {
    text.lines()
        .map(|l| {
            let words: Vec<&str> = l.split_whitespace().collect();
            let mut s = words.join(" ");
            for p in [" ,", " .", " !", " ?", " ;", " :"] {
                s = s.replace(p, &p[1..]);
            }
            s
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// The words that are spoken: no tags of either kind.
pub fn spoken(text: &str) -> String {
    tidy(&without_cartesia_tags(&without_brackets(text)))
}

/// The text as ElevenLabs should get it: `[audio tags]` kept when `model` performs them,
/// Cartesia's tags always removed.
pub fn for_eleven(text: &str, model: &str) -> String {
    let t = without_cartesia_tags(text);
    if eleven_performs_tags(model) {
        if t == text {
            return text.to_string();
        }
        tidy(&t)
    } else {
        tidy(&without_brackets(&t))
    }
}

/// The text as Cartesia should get it: its own tags kept, ElevenLabs' `[audio tags]` removed.
pub fn for_cartesia(text: &str) -> String {
    let t = without_brackets(text);
    if t == text {
        return text.to_string();
    }
    tidy(&t)
}

/// The house set of ElevenLabs audio tags for business-to-business video, and how to place them.
/// Shared by the spec assistant and the production run, so both write the same way.
pub const ELEVEN_TAG_GUIDE: &str = "Delivery tags: Eleven v3 and later models perform short directions in square brackets, placed just before the words they shape, and the tone carries on until the next tag. They are not a fixed list; this is the house set for business-to-business video:\n\
  - `[warmly]` welcomes, thanks, lines about the customer and their team.\n\
  - `[confident]` capabilities, results, proof points, the claim a scene makes.\n\
  - `[reassuring]` security, reliability, support, migration, a pain point answered.\n\
  - `[enthusiastic]` the reveal of the key feature or launch; once or twice a video.\n\
  - `[clear and measured]` steps, numbers, technical detail, settings and code.\n\
  - `[explaining patiently]` a how-to walkthrough.\n\
  - `[matter-of-fact]` specifications, compatibility, platforms, pricing facts.\n\
  - `[curious]` a question that sets up the problem.\n\
  - `[thoughtful]` a line about the customer's challenge.\n\
  - `[proud]` milestones, customer counts, awards.\n\
  - `[inviting]` the call to action.\n\
  - `[upbeat]` transitions and light moments.\n\
  - `[emphasizing]` just before the one phrase a viewer must remember.\n\
  - `[short pause]` before a reveal or after a key number; prefer punctuation (commas, dashes, ellipses) for ordinary pauses.\n\
  Two qualities may be combined in one tag, such as `[warm, confident]` or `[confident, inviting]`.\n\
  How to place them: open every passage with one tag, because each passage is voiced on its own and the tone doesn't carry from one to the next. Inside a passage add a tag only where the tone really changes, at most one every two or three sentences; tagging every sentence makes the read jumpy. Put tags between sentences or clauses, never inside a product name or a number. Keep the words themselves exactly as written. Don't use reactions (laughs, sighs, gasps, clears throat), whispering or shouting, accents, character voices, sound-effect tags, or negative emotions unless the spec asks for them: they sound out of place in a business video. Use square brackets only; parentheses and braces are read aloud.\n";

/// How a production run uses Cartesia's tags.
pub const CARTESIA_TAG_GUIDE: &str = "Cartesia reads ElevenLabs' `[audio tags]` aloud, so the helper removes them before the text goes to Cartesia: text files may keep the spec's tags. Cartesia has inline tags of its own (sonic-3 and later), which ElevenLabs never receives:\n\
  - `<spell>WPF</spell>` spells out a code, ID, version or acronym letter by letter where the voice would otherwise try to say it as a word. Put no punctuation inside, and don't put it next to a `<break/>`.\n\
  - `<break time=\"500ms\"/>` (or `\"1s\"`) is an explicit silence. Use punctuation first; use a break only where a pause must be exact, never several close together, since each one splits the reading and can make it sound less natural.\n\
  - `<speed ratio=\"0.8\"/>` (0.6 to 1.5) slows a dense passage of numbers or steps and `<volume ratio=\"1.2\"/>` (0.5 to 2) lifts a line; both are guidance, not exact, so make changes large enough to hear and reset them with ratio 1 afterwards. Use them rarely: the same `--settings` for every passage keeps the voice steady.\n\
  - `<emotion value=\"content\"/>` is experimental and works reliably only with voices tagged Emotive. Prefer one `emotion` in `--settings` for the whole video; translate the spec's `[warmly]`, `[confident]` and similar into that choice rather than into many inline emotions.\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_models_perform_tags() {
        for m in ["eleven_v3", "eleven_v4", "eleven_v4_turbo", "eleven_v12"] {
            assert!(eleven_performs_tags(m), "{m}");
        }
        for m in ["eleven_multilingual_v2", "eleven_flash_v2_5", "eleven_turbo_v2", "eleven_v2", "sonic-3.6", ""] {
            assert!(!eleven_performs_tags(m), "{m}");
        }
    }

    #[test]
    fn spoken_text_has_no_tags() {
        assert_eq!(
            spoken(
                "[warmly] Meet <spell>WPF</spell> charts. <break time=\"500ms\"/>[confident, inviting] Try it , today!"
            ),
            "Meet WPF charts. Try it, today!"
        );
        assert_eq!(spoken("If a < b and b > c"), "If a < b and b > c", "a bare angle bracket is text");
        assert_eq!(spoken("Line one.\n[upbeat] Line two."), "Line one.\nLine two.");
    }

    #[test]
    fn each_provider_gets_only_the_tags_it_performs() {
        let text = "[warmly] Welcome to <spell>WPF</spell>.";
        assert_eq!(for_eleven(text, "eleven_v4"), "[warmly] Welcome to WPF.");
        assert_eq!(for_eleven(text, "eleven_multilingual_v2"), "Welcome to WPF.");
        assert_eq!(for_cartesia(text), "Welcome to <spell>WPF</spell>.");
        // untouched text keeps its exact form (and the request cache its key)
        assert_eq!(for_eleven("[warmly]  Hi.", "eleven_v3"), "[warmly]  Hi.");
        assert_eq!(for_cartesia("Hi.  <break time=\"1s\"/>"), "Hi.  <break time=\"1s\"/>");
    }
}
