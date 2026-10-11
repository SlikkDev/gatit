//! Turns two per-track Whisper outputs into one "Me"/"Them" transcript.
//!
//! Everything here is pure so it can be unit-tested without audio or a model.

use regex::Regex;
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

/// One user vocabulary entry: the spelling to keep (usually English letters) and the
/// ways Whisper tends to write it in Hebrew letters instead.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VocabTerm {
    pub term: String,
    #[serde(default)]
    pub variants: Vec<String>,
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

/// The initial prompt passed to Whisper. Written in Hebrew so the model stays in Hebrew,
/// while listing the terms that should come out in English letters.
pub fn whisper_prompt(vocabulary: &[VocabTerm]) -> String {
    let terms: Vec<&str> = vocabulary
        .iter()
        .map(|v| v.term.as_str())
        .filter(|t| !t.is_empty())
        .collect();
    if terms.is_empty() {
        return String::new();
    }
    format!("שיחת עבודה בעברית עם מונחים באנגלית: {}.", terms.join(", "))
}

/// Builds the cleanup pass that rewrites Hebrew-letter spellings of known terms
/// (for example "סי אר או") into the term itself ("CRO").
pub struct VocabFixer {
    rules: Vec<(Regex, String)>,
}

/// Hebrew one-letter prefixes (and, the, in, to, from, that, as) that attach to words.
const PREFIXES: &str = "והבלמשכ";

impl VocabFixer {
    pub fn new(vocabulary: &[VocabTerm]) -> Self {
        let mut rules = Vec::new();
        for entry in vocabulary {
            let term = entry.term.trim();
            if term.is_empty() {
                continue;
            }
            let mut variants: Vec<&str> = entry
                .variants
                .iter()
                .map(|v| v.trim())
                .filter(|v| !v.is_empty())
                .collect();
            // Longest first, so "פרופיט שייר" wins over "פרופיט".
            variants.sort_by_key(|v| std::cmp::Reverse(v.chars().count()));
            for variant in variants {
                // Whisper sometimes inserts dots or hyphens between letters: "סי.אר.או".
                let body = variant
                    .split_whitespace()
                    .map(regex::escape)
                    .collect::<Vec<_>>()
                    .join(r"[\s.\-]*");
                let pattern = format!(
                    r"(^|[^\p{{L}}\p{{N}}])([{PREFIXES}]{{0,2}})-?{body}([^\p{{L}}\p{{N}}]|$)"
                );
                if let Ok(re) = Regex::new(&pattern) {
                    rules.push((re, term.to_string()));
                }
            }
        }
        Self { rules }
    }

    pub fn apply(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (re, term) in &self.rules {
            // Each match consumes its trailing boundary, so repeat until stable to catch
            // back-to-back occurrences.
            loop {
                let next = re
                    .replace_all(&out, |caps: &regex::Captures| {
                        let prefix = &caps[2];
                        let joiner = if prefix.is_empty() { "" } else { "-" };
                        format!("{}{}{}{}{}", &caps[1], prefix, joiner, term, &caps[3])
                    })
                    .into_owned();
                if next == out {
                    break;
                }
                out = next;
            }
        }
        out
    }
}

/// Default vocabulary from the plan: terms that come up in Yarin-style business calls.
pub fn default_vocabulary() -> Vec<VocabTerm> {
    let v = |term: &str, variants: &[&str]| VocabTerm {
        term: term.into(),
        variants: variants.iter().map(|s| s.to_string()).collect(),
    };
    vec![
        v("CRO", &["סי אר או", "סיאראו", "סי.אר.או"]),
        v("ROI", &["אר או איי", "ארואיי", "אר.או.איי"]),
        v("AI", &["איי איי", "אי איי", "איי.איי"]),
        v("profit share", &["פרופיט שר", "פרופיט שייר", "פרופיטשר"]),
    ]
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

/// Plain-text transcript, one line per turn, used for the summary prompt and exports.
pub fn to_text(segments: &[Segment]) -> String {
    segments
        .iter()
        .map(|s| {
            let who = match s.speaker {
                Speaker::Me => "Me",
                Speaker::Them => "Them",
            };
            format!("[{}] {}: {}", timestamp(s.start), who, s.text)
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
    fn rewrites_hebrew_spellings_of_known_terms() {
        let fixer = VocabFixer::new(&default_vocabulary());
        assert_eq!(fixer.apply("צריך לשפר את ה סי אר או"), "צריך לשפר את ה CRO");
        assert_eq!(fixer.apply("מה הסי אר או שלנו?"), "מה ה-CRO שלנו?");
        assert_eq!(fixer.apply("סי.אר.או ו אר או איי"), "CRO ו ROI");
        assert_eq!(fixer.apply("נדבר על פרופיט שייר"), "נדבר על profit share");
        assert_eq!(fixer.apply("איי איי, איי איי"), "AI, AI");
        // Not inside other words.
        assert_eq!(fixer.apply("קסי אר אוז"), "קסי אר אוז");
    }

    #[test]
    fn prompt_lists_terms() {
        let prompt = whisper_prompt(&default_vocabulary());
        assert!(prompt.contains("CRO, ROI, AI, profit share"));
        assert_eq!(whisper_prompt(&[]), "");
    }

    #[test]
    fn formats_text_transcript() {
        let text = to_text(&[
            seg(Speaker::Me, 65.0, 70.0, "hi"),
            seg(Speaker::Them, 3700.0, 3701.0, "bye"),
        ]);
        assert_eq!(text, "[01:05] Me: hi\n[1:01:40] Them: bye");
    }
}
