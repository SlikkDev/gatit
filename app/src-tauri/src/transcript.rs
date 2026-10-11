//! Turns two per-track Whisper outputs into one "Me"/"Them" transcript.
//!
//! Everything here is pure so it can be unit-tested without audio or a model.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speaker {
    Me,
    Them,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub speaker: Speaker,
    /// Seconds from the start of the recording.
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transcript {
    pub language: String,
    pub model: String,
    pub segments: Vec<Segment>,
}

/// Parses the JSON that `whisper-cli -oj` writes into segments for one speaker.
/// Returns the language Whisper reported (or detected) alongside the segments.
pub fn parse_whisper_json(
    bytes: &[u8],
    speaker: Speaker,
) -> Result<(String, Vec<Segment>), String> {
    // whisper.cpp can split a multi-byte Hebrew character across tokens; be lenient.
    let text = String::from_utf8_lossy(bytes);
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("bad whisper output: {e}"))?;
    let language = value["result"]["language"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let items = value["transcription"]
        .as_array()
        .ok_or("whisper output has no transcription")?;
    let segments = items
        .iter()
        .filter_map(|item| {
            let start = item["offsets"]["from"].as_f64()? / 1000.0;
            let end = item["offsets"]["to"].as_f64()? / 1000.0;
            let text = item["text"].as_str()?.trim().to_string();
            (!text.is_empty() && !is_noise(&text)).then_some(Segment {
                speaker,
                start,
                end,
                text,
            })
        })
        .collect();
    Ok((language, segments))
}

/// Whisper marks non-speech as "[MUSIC]", "(applause)" and similar; drop those lines.
fn is_noise(text: &str) -> bool {
    let t = text.trim();
    (t.starts_with('[') && t.ends_with(']'))
        || (t.starts_with('(') && t.ends_with(')'))
        || t == "..."
}

/// Merges both tracks by time, drops "Me" lines that are just the other side leaking
/// into the microphone (calls on speakers), and joins short consecutive lines from the
/// same speaker so the transcript reads like turns rather than fragments.
pub fn merge(me: Vec<Segment>, them: Vec<Segment>) -> Vec<Segment> {
    let me: Vec<Segment> = me.into_iter().filter(|m| !is_echo(m, &them)).collect();
    let mut all: Vec<Segment> = me.into_iter().chain(them).collect();
    all.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.end.total_cmp(&b.end)));

    let mut merged: Vec<Segment> = Vec::with_capacity(all.len());
    for seg in all {
        if let Some(last) = merged.last_mut() {
            let same_turn = last.speaker == seg.speaker
                && seg.start - last.end < 1.5
                && seg.end - last.start < 45.0;
            if same_turn {
                last.end = last.end.max(seg.end);
                last.text.push(' ');
                last.text.push_str(&seg.text);
                continue;
            }
        }
        merged.push(seg);
    }
    merged
}

fn is_echo(mine: &Segment, theirs: &[Segment]) -> bool {
    theirs.iter().any(|t| {
        let overlap = mine.end.min(t.end + 1.0) - mine.start.max(t.start - 1.0);
        overlap > 0.5 * (mine.end - mine.start).max(0.1) && similarity(&mine.text, &t.text) >= 0.6
    })
}

/// Dice coefficient over character bigrams, ignoring punctuation and case.
pub fn similarity(a: &str, b: &str) -> f64 {
    fn bigrams(s: &str) -> Vec<(char, char)> {
        let chars: Vec<char> = s
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect();
        chars.windows(2).map(|w| (w[0], w[1])).collect()
    }
    let (a, mut b) = (bigrams(a), bigrams(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let total = a.len() + b.len();
    let mut shared = 0;
    for pair in &a {
        if let Some(i) = b.iter().position(|p| p == pair) {
            b.swap_remove(i);
            shared += 1;
        }
    }
    2.0 * shared as f64 / total as f64
}

/// "01:02:03" or "02:03" for a time in seconds.
pub fn timestamp(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// What each track is called in transcripts: the user's own name instead of "Me" once they
/// set it, and the other person's name instead of "Them" when exactly one other
/// participant is known. On a group call the other side stays "Them", because everyone
/// else shares one audio track and nothing tells their voices apart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Names {
    pub me: String,
    pub them: String,
}

impl Default for Names {
    fn default() -> Self {
        Self::new("", &[])
    }
}

impl Names {
    pub fn new(your_name: &str, participants: &[String]) -> Self {
        let me = your_name.trim();
        let others: Vec<&str> = participants
            .iter()
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .collect();
        Self {
            me: if me.is_empty() { "Me" } else { me }.to_string(),
            them: match others.as_slice() {
                [only] => only.to_string(),
                _ => "Them".to_string(),
            },
        }
    }

    pub fn label(&self, speaker: Speaker) -> &str {
        match speaker {
            Speaker::Me => &self.me,
            Speaker::Them => &self.them,
        }
    }
}

/// Plain-text transcript, one line per turn, used for the summary prompt and exports.
pub fn to_text(segments: &[Segment], names: &Names) -> String {
    segments
        .iter()
        .map(|s| {
            format!(
                "[{}] {}: {}",
                timestamp(s.start),
                names.label(s.speaker),
                s.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(speaker: Speaker, start: f64, end: f64, text: &str) -> Segment {
        Segment {
            speaker,
            start,
            end,
            text: text.into(),
        }
    }

    #[test]
    fn parses_whisper_cli_json() {
        let json = r#"{"result":{"language":"he"},"transcription":[
            {"offsets":{"from":0,"to":2500},"text":" שלום, מה שלומך?"},
            {"offsets":{"from":2500,"to":4000},"text":" [MUSIC]"},
            {"offsets":{"from":4000,"to":6000},"text":"  "}]}"#;
        let (lang, segs) = parse_whisper_json(json.as_bytes(), Speaker::Them).unwrap();
        assert_eq!(lang, "he");
        assert_eq!(segs, vec![seg(Speaker::Them, 0.0, 2.5, "שלום, מה שלומך?")]);
    }

    #[test]
    fn merges_by_time_and_labels_speakers() {
        let me = vec![
            seg(Speaker::Me, 0.0, 2.0, "היי"),
            seg(Speaker::Me, 10.0, 12.0, "מעולה"),
        ];
        let them = vec![seg(Speaker::Them, 3.0, 8.0, "אהלן, מה נשמע?")];
        let merged = merge(me, them);
        let order: Vec<_> = merged
            .iter()
            .map(|s| (s.speaker, s.text.as_str()))
            .collect();
        assert_eq!(
            order,
            vec![
                (Speaker::Me, "היי"),
                (Speaker::Them, "אהלן, מה נשמע?"),
                (Speaker::Me, "מעולה")
            ]
        );
    }

    #[test]
    fn joins_consecutive_lines_from_same_speaker() {
        let them = vec![
            seg(Speaker::Them, 0.0, 3.0, "first"),
            seg(Speaker::Them, 3.5, 5.0, "second"),
        ];
        let merged = merge(vec![], them);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "first second");
        assert_eq!(merged[0].end, 5.0);
    }

    #[test]
    fn drops_their_voice_leaking_into_my_mic() {
        let them = vec![seg(Speaker::Them, 5.0, 9.0, "אז מה לגבי ה-ROI של הקמפיין?")];
        let me = vec![
            seg(Speaker::Me, 5.2, 9.1, "אז מה לגבי הROI של הקמפיין"),
            seg(Speaker::Me, 10.0, 12.0, "נראה לי סביב 20 אחוז"),
        ];
        let merged = merge(me, them);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].speaker, Speaker::Them);
        assert_eq!(merged[1].text, "נראה לי סביב 20 אחוז");
    }

    #[test]
    fn keeps_my_line_when_text_differs_even_if_overlapping() {
        let them = vec![seg(
            Speaker::Them,
            5.0,
            9.0,
            "what do you think about pricing",
        )];
        let me = vec![seg(Speaker::Me, 6.0, 8.0, "yes exactly")];
        assert_eq!(merge(me, them).len(), 2);
    }

    #[test]
    fn formats_text_transcript() {
        let text = to_text(
            &[
                seg(Speaker::Me, 65.0, 70.0, "hi"),
                seg(Speaker::Them, 3700.0, 3701.0, "bye"),
            ],
            &Names::default(),
        );
        assert_eq!(text, "[01:05] Me: hi\n[1:01:40] Them: bye");
    }

    #[test]
    fn names_the_tracks_when_people_are_known() {
        let lines = [
            seg(Speaker::Me, 0.0, 1.0, "hi"),
            seg(Speaker::Them, 2.0, 3.0, "hey"),
        ];
        let one = Names::new("Noa", &["Dana Cohen".into()]);
        assert_eq!(
            to_text(&lines, &one),
            "[00:00] Noa: hi\n[00:02] Dana Cohen: hey"
        );

        // Several people share the other track, so it stays "Them".
        let group = Names::new(" Noa ", &["Dana".into(), "Avi".into()]);
        assert_eq!((group.me.as_str(), group.them.as_str()), ("Noa", "Them"));

        // Blank entries don't count as people.
        let blank = Names::new("", &["  ".into(), "Dana".into()]);
        assert_eq!((blank.me.as_str(), blank.them.as_str()), ("Me", "Dana"));
    }
}
