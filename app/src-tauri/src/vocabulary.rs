//! English terms that Israelis drop into Hebrew speech (CRO, ROI, profit share, LinkedIn...)
//! and how they are kept in English letters in transcripts.
//!
//! Two passes use the vocabulary: a short hint list goes to Whisper as its prompt, and a
//! cleanup pass rewrites Hebrew-letter spellings ("סי אר או") into the term ("CRO"). The
//! terms in effect are the user's own list from Settings followed by the built-in list in
//! `vocabulary/hebrew-english.json`.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use regex::{Captures, Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

/// One user vocabulary entry: the spelling to keep (usually English letters) and the
/// ways Whisper tends to write it in Hebrew letters instead.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VocabTerm {
    pub term: String,
    #[serde(default)]
    pub variants: Vec<String>,
}

#[derive(Deserialize)]
struct BuiltinFile {
    groups: Vec<BuiltinGroup>,
}

#[derive(Deserialize)]
struct BuiltinGroup {
    name: String,
    terms: Vec<BuiltinTerm>,
}

#[derive(Deserialize)]
struct BuiltinTerm {
    term: String,
    #[serde(default)]
    variants: Vec<String>,
    /// Also pass this term to Whisper as a hint.
    #[serde(default)]
    hint: bool,
    /// Match an all-capitals acronym letter by letter ("קיי פי איי" for KPI).
    #[serde(default = "yes")]
    spell: bool,
}

fn yes() -> bool {
    true
}

struct Entry {
    term: String,
    variants: Vec<String>,
    hint: bool,
}

/// The built-in groups, for display in Settings.
#[derive(Clone, Debug, Serialize)]
pub struct BuiltinGroupInfo {
    pub name: String,
    pub terms: Vec<String>,
}

fn builtin_file() -> &'static BuiltinFile {
    static FILE: OnceLock<BuiltinFile> = OnceLock::new();
    FILE.get_or_init(|| {
        serde_json::from_str(include_str!("../vocabulary/hebrew-english.json"))
            .expect("vocabulary/hebrew-english.json is valid")
    })
}

fn builtin_entries() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        builtin_file()
            .groups
            .iter()
            .flat_map(|g| &g.terms)
            .map(|t| {
                let mut variants = t.variants.clone();
                if t.spell && is_acronym(&t.term) {
                    variants.extend(spell(&t.term));
                }
                Entry {
                    term: t.term.clone(),
                    variants,
                    hint: t.hint,
                }
            })
            .collect()
    })
}

pub fn builtin_groups() -> Vec<BuiltinGroupInfo> {
    builtin_file()
        .groups
        .iter()
        .map(|g| BuiltinGroupInfo {
            name: g.name.clone(),
            terms: g.terms.iter().map(|t| t.term.clone()).collect(),
        })
        .collect()
}

/// The four terms v1 shipped as the editable default. Settings saved by v1 still carry
/// them; they are part of the built-in list now.
pub fn is_v1_default(vocabulary: &[VocabTerm]) -> bool {
    let terms: Vec<&str> = vocabulary.iter().map(|v| v.term.as_str()).collect();
    terms == ["CRO", "ROI", "AI", "profit share"]
}

fn is_acronym(term: &str) -> bool {
    term.len() >= 2 && term.chars().all(|c| c.is_ascii_uppercase())
}

/// How each English letter's name tends to be written in Hebrew. A missing geresh and
/// final letter forms are handled by `variant_pattern`, so "ג'י" also covers "גי".
fn letter_names(letter: char) -> &'static [&'static str] {
    match letter {
        'A' | 'E' | 'I' => &["איי", "אי"],
        'B' => &["בי"],
        'C' => &["סי"],
        'D' => &["די"],
        'F' => &["אף"],
        'G' => &["ג'י"],
        'H' => &["אייץ'", "אייטש", "הייץ'"],
        'J' => &["ג'יי"],
        'K' => &["קיי"],
        'L' => &["אל"],
        'M' => &["אם"],
        'N' => &["אן"],
        'O' => &["או"],
        'P' => &["פי"],
        'Q' => &["קיו"],
        'R' => &["אר", "אאר"],
        'S' => &["אס"],
        'T' => &["טי"],
        'U' => &["יו"],
        'V' => &["וי", "ווי"],
        'W' => &["דאבל יו", "דבל יו"],
        'X' => &["אקס"],
        'Y' => &["וואי", "וויי", "ויי"],
        'Z' => &["זי", "זד"],
        _ => &[],
    }
}

/// Every way to write an acronym said letter by letter: "KPI" -> ["קיי פי איי", ...].
/// Acronyms of three letters or more also get the joined-up forms ("קייפיאיי"); for two
/// letters those are too often real words or names ("יואי").
fn spell(acronym: &str) -> Vec<String> {
    let mut spellings = vec![String::new()];
    for letter in acronym.chars() {
        let names = letter_names(letter);
        spellings = spellings
            .iter()
            .flat_map(|s| {
                names.iter().map(move |n| {
                    if s.is_empty() {
                        n.to_string()
                    } else {
                        format!("{s} {n}")
                    }
                })
            })
            .collect();
    }
    if acronym.chars().count() >= 3 {
        let joined: Vec<String> = spellings.iter().map(|s| s.replace(' ', "")).collect();
        spellings.extend(joined);
    }
    spellings
}

/// The terms in effect for one transcription.
pub struct Vocabulary {
    entries: Vec<Entry>,
}

/// Whisper keeps at most 224 prompt tokens and drops the beginning when there are more,
/// which would lose the Hebrew lead-in. Stay well under that.
const PROMPT_BUDGET: usize = 190;
const PROMPT_LEAD: &str = "שיחת עבודה בעברית עם מונחים באנגלית: ";

/// A deliberately high estimate of Whisper tokens. Measured with Whisper's tokenizer,
/// English runs about 3.3 characters per token and Hebrew about 1.75.
fn estimate_tokens(text: &str) -> usize {
    let ascii = text.chars().filter(char::is_ascii).count();
    let other = text.chars().count() - ascii;
    ascii.div_ceil(3) + (other * 2).div_ceil(3)
}

impl Vocabulary {
    /// The user's own terms come first, so they win over built-in terms and are always
    /// hinted to Whisper.
    pub fn new(user: &[VocabTerm], builtin: bool) -> Self {
        let mut entries: Vec<Entry> = user
            .iter()
            .filter(|v| !v.term.trim().is_empty())
            .map(|v| Entry {
                term: v.term.trim().to_string(),
                variants: v.variants.clone(),
                hint: true,
            })
            .collect();
        if builtin {
            entries.extend(builtin_entries().iter().map(|e| Entry {
                term: e.term.clone(),
                variants: e.variants.clone(),
                hint: e.hint,
            }));
        }
        Self { entries }
    }

    /// The initial prompt passed to Whisper. Written in Hebrew so the model stays in
    /// Hebrew, while listing the terms that should come out in English letters.
    pub fn whisper_prompt(&self) -> String {
        let mut terms: Vec<&str> = Vec::new();
        let mut used = estimate_tokens(PROMPT_LEAD) + 1;
        for entry in self.entries.iter().filter(|e| e.hint) {
            let term = entry.term.as_str();
            if terms.iter().any(|t| t.eq_ignore_ascii_case(term)) {
                continue;
            }
            let cost = estimate_tokens(term) + 1;
            if used + cost > PROMPT_BUDGET {
                break;
            }
            used += cost;
            terms.push(term);
        }
        if terms.is_empty() {
            return String::new();
        }
        format!("{PROMPT_LEAD}{}.", terms.join(", "))
    }

    pub fn fixer(&self) -> VocabFixer {
        VocabFixer::new(
            self.entries
                .iter()
                .flat_map(|e| e.variants.iter().map(|v| (v.as_str(), e.term.as_str()))),
        )
    }
}

/// Hebrew one-letter prefixes (and, the, in, to, from, that, as) that attach to words.
/// Matched lazily, so a term that itself starts with one of these letters ("וואטסאפ")
/// keeps it.
const PREFIXES: &str = "והבלמשכ";

/// Rewrites Hebrew-letter spellings of known terms (for example "סי אר או") into the term
/// itself ("CRO"), in one pass over the text.
pub struct VocabFixer {
    re: Option<Regex>,
    /// Normalized spelling (see `spelling_key`) -> term.
    terms: HashMap<String, String>,
}

impl VocabFixer {
    /// Takes (spelling, term) pairs. When two terms share a spelling, the first one wins.
    fn new<'a>(pairs: impl Iterator<Item = (&'a str, &'a str)>) -> Self {
        let mut terms = HashMap::new();
        let mut spellings: Vec<(usize, String)> = Vec::new();
        let mut seen = HashSet::new();
        for (variant, term) in pairs {
            let (variant, term) = (variant.trim(), term.trim());
            let key = spelling_key(variant);
            if key.is_empty() || term.is_empty() {
                continue;
            }
            let pattern = variant_pattern(variant);
            if seen.insert(pattern.clone()) {
                spellings.push((key.chars().count(), pattern));
            }
            terms.entry(key).or_insert_with(|| term.to_string());
        }
        // Longest first: the regex takes the first alternative that matches, so
        // "גוגל דרייב" must come before "גוגל".
        spellings.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.len().cmp(&a.1.len())));
        let re = (!spellings.is_empty()).then(|| {
            let alternatives: Vec<&str> = spellings.iter().map(|(_, p)| p.as_str()).collect();
            let pattern = format!(
                r"(^|[^\p{{L}}\p{{N}}])([{PREFIXES}]{{0,2}}?)-?({})([^\p{{L}}\p{{N}}]|$)",
                alternatives.join("|")
            );
            RegexBuilder::new(&pattern)
                .case_insensitive(true)
                .size_limit(64 << 20)
                .build()
        });
        Self {
            re: re.and_then(Result::ok),
            terms,
        }
    }

    pub fn apply(&self, text: &str) -> String {
        let Some(re) = &self.re else {
            return text.to_string();
        };
        let mut out = text.to_string();
        // Each match consumes its trailing boundary, so repeat to catch back-to-back
        // occurrences ("AI, AI").
        for _ in 0..8 {
            let next = re
                .replace_all(&out, |caps: &Captures| {
                    let Some(term) = self.terms.get(&spelling_key(&caps[3])) else {
                        return caps[0].to_string();
                    };
                    let prefix = &caps[2];
                    let joiner = if prefix.is_empty() { "" } else { "-" };
                    format!("{}{prefix}{joiner}{term}{}", &caps[1], &caps[4])
                })
                .into_owned();
            if next == out {
                break;
            }
            out = next;
        }
        out
    }
}

fn is_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '.' | '-')
}

fn is_geresh(c: char) -> bool {
    matches!(c, '\'' | '׳' | '’')
}

fn final_to_regular(c: char) -> char {
    match c {
        'ך' => 'כ',
        'ם' => 'מ',
        'ן' => 'נ',
        'ף' => 'פ',
        'ץ' => 'צ',
        c => c,
    }
}

/// A spelling with everything `variant_pattern` is loose about taken out, so a matched
/// text and the variant it matched map to the same key.
fn spelling_key(text: &str) -> String {
    let mut key = String::new();
    for c in text.chars() {
        if is_separator(c) || is_geresh(c) {
            continue;
        }
        key.extend(final_to_regular(c).to_lowercase());
    }
    key
}

/// The regex for one spelling, loose in the ways Hebrew transcriptions vary: any mix of
/// spaces, dots and hyphens between words ("סי.אר.או", "סי-אר-או"), final or regular
/// letter forms, and a dropped geresh ("צרן"). Joined-up forms ("פולואפ") and doubled
/// letters ("ווטסאפ") are separate variants: allowing them everywhere turned common words
/// into terms ("איטי" into IT, "מנדי" into monday.com).
fn variant_pattern(variant: &str) -> String {
    let mut out = String::new();
    let mut chars = variant.trim().chars().peekable();
    while let Some(c) = chars.next() {
        if is_separator(c) {
            while chars.next_if(|&n| is_separator(n)).is_some() {}
            out.push_str(r"[\s.\-]+");
        } else if is_geresh(c) {
            out.push_str("['׳’]?");
        } else {
            let regular = final_to_regular(c);
            let alternate = match regular {
                'כ' => Some('ך'),
                'מ' => Some('ם'),
                'נ' => Some('ן'),
                'פ' => Some('ף'),
                'צ' => Some('ץ'),
                _ => None,
            };
            match alternate {
                Some(f) => out.push_str(&format!("[{regular}{f}]")),
                None => out.push_str(&regex::escape(&c.to_string())),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin() -> VocabFixer {
        Vocabulary::new(&[], true).fixer()
    }

    #[test]
    fn rewrites_hebrew_spellings_of_known_terms() {
        let fixer = builtin();
        assert_eq!(fixer.apply("צריך לשפר את ה סי אר או"), "צריך לשפר את ה CRO");
        assert_eq!(fixer.apply("מה הסי אר או שלנו?"), "מה ה-CRO שלנו?");
        assert_eq!(fixer.apply("סי.אר.או ו אר או איי"), "CRO ו ROI");
        assert_eq!(fixer.apply("נדבר על פרופיט שייר"), "נדבר על profit share");
        assert_eq!(fixer.apply("איי איי, איי איי"), "AI, AI");
        // Not inside other words.
        assert_eq!(fixer.apply("קסי אר אוז"), "קסי אר אוז");
    }

    #[test]
    fn spells_acronyms_letter_by_letter() {
        let fixer = builtin();
        assert_eq!(fixer.apply("מה הקיי פי איי שלכם"), "מה ה-KPI שלכם");
        assert_eq!(fixer.apply("קייפיאיי"), "KPI");
        assert_eq!(fixer.apply("ה-סי-טי-אר ירד"), "ה-CTR ירד");
        assert_eq!(fixer.apply("דיברתי עם הסי אי או"), "דיברתי עם ה-CEO");
        assert_eq!(fixer.apply("אייץ' אר ופי אר"), "HR ו-PR");
        assert_eq!(fixer.apply("שלחו לי אן די איי"), "שלחו לי NDA");
    }

    #[test]
    fn tolerates_spelling_differences() {
        let fixer = builtin();
        // Spelling variants, a dropped geresh, joined words.
        assert_eq!(fixer.apply("תשלח בואטסאפ"), "תשלח ב-WhatsApp");
        assert_eq!(fixer.apply("בפייסבוק ובלינקדאין"), "ב-Facebook וב-LinkedIn");
        assert_eq!(fixer.apply("אל טי ווי"), "LTV");
        assert_eq!(fixer.apply("הצרן עלה"), "ה-churn עלה");
        assert_eq!(fixer.apply("פולואפ מחר"), "follow up מחר");
    }

    #[test]
    fn prefers_the_longest_term() {
        let fixer = builtin();
        assert_eq!(fixer.apply("שמתי בגוגל דרייב"), "שמתי ב-Google Drive");
        assert_eq!(fixer.apply("חיפשתי בגוגל"), "חיפשתי ב-Google");
        assert_eq!(
            fixer.apply("הקונברז'ן רייט של הלנדינג פייג'"),
            "ה-conversion rate של ה-landing page"
        );
        assert_eq!(fixer.apply("שאלתי את צ'אט ג'י פי טי"), "שאלתי את ChatGPT");
    }

    #[test]
    fn leaves_ordinary_hebrew_alone() {
        let fixer = builtin();
        for text in [
            "אני יושב ליד המחשב ושומע את הקול שלך",
            "שלחתי לך מייל עם הפידבק על הקמפיין, הדדליין מחר",
            "אוקיי סבבה, נדבר אחרי הפגישה",
            "אי אפשר לסגור את זה היום",
            "יש לי סלק בגינה",
            // Words the looser matching once caught (slow, Mandy, drawer, to spread).
            "הוא איטי, מנדי שם את זה במגירה כדי להפיץ",
            "יואי אמר לי ללכת לרבנו",
            "לידיים שלי",
        ] {
            assert_eq!(fixer.apply(text), text);
        }
    }

    #[test]
    fn user_terms_come_first() {
        let user = [VocabTerm {
            term: "Gatit".into(),
            variants: vec!["גטית".into()],
        }];
        let vocab = Vocabulary::new(&user, true);
        assert!(vocab
            .whisper_prompt()
            .starts_with("שיחת עבודה בעברית עם מונחים באנגלית: Gatit, "));
        assert_eq!(vocab.fixer().apply("הקלטתי בגטית"), "הקלטתי ב-Gatit");
        // Turning the built-in list off keeps only the user's terms.
        let only_user = Vocabulary::new(&user, false);
        assert_eq!(only_user.fixer().apply("סי אר או"), "סי אר או");
        assert_eq!(
            only_user.whisper_prompt(),
            "שיחת עבודה בעברית עם מונחים באנגלית: Gatit."
        );
        assert_eq!(Vocabulary::new(&[], false).whisper_prompt(), "");
    }

    #[test]
    fn prompt_fits_whisper() {
        let prompt = Vocabulary::new(&[], true).whisper_prompt();
        assert!(prompt.contains("ROI, CRO, KPI"), "{prompt}");
        assert!(prompt.contains("profit share"), "{prompt}");
        assert!(estimate_tokens(&prompt) <= PROMPT_BUDGET);
        // A long user list is cut to the budget instead of overflowing.
        let many: Vec<VocabTerm> = (0..200)
            .map(|i| VocabTerm {
                term: format!("מונח ארוך מספר {i}"),
                variants: vec![],
            })
            .collect();
        let prompt = Vocabulary::new(&many, true).whisper_prompt();
        assert!(estimate_tokens(&prompt) <= PROMPT_BUDGET);
        assert!(prompt.starts_with(PROMPT_LEAD));
    }

    #[test]
    fn builtin_spellings_round_trip_without_clashes() {
        let mut owner: HashMap<String, &str> = HashMap::new();
        for entry in builtin_entries() {
            assert!(
                !entry.variants.is_empty(),
                "{} has no spellings",
                entry.term
            );
            for variant in &entry.variants {
                let key = spelling_key(variant);
                if let Some(other) = owner.insert(key, &entry.term) {
                    assert_eq!(other, entry.term, "{variant} is used by two terms");
                }
            }
        }
        let fixer = builtin();
        for entry in builtin_entries() {
            for variant in &entry.variants {
                assert_eq!(
                    fixer.apply(&format!("אמרנו {variant} היום")),
                    format!("אמרנו {} היום", entry.term),
                    "{variant}"
                );
            }
        }
    }

    #[test]
    fn recognizes_v1_default_vocabulary() {
        let v1: Vec<VocabTerm> = ["CRO", "ROI", "AI", "profit share"]
            .iter()
            .map(|t| VocabTerm {
                term: t.to_string(),
                variants: vec!["x".into()],
            })
            .collect();
        assert!(is_v1_default(&v1));
        assert!(!is_v1_default(&v1[..3]));
    }

    /// Lists the common Hebrew words the built-in list would rewrite. Run with a word
    /// frequency list (one "word count" per line), for example FrequencyWords' he_50k.txt:
    /// GATIT_WORDLIST=he_50k.txt cargo test vocabulary_collisions -- --ignored --nocapture
    #[test]
    #[ignore]
    fn vocabulary_collisions() {
        let path = std::env::var("GATIT_WORDLIST").expect("set GATIT_WORDLIST");
        let words = std::fs::read_to_string(path).unwrap();
        let fixer = builtin();
        for line in words.lines() {
            let word = line.split_whitespace().next().unwrap_or("");
            let fixed = fixer.apply(word);
            if fixed != word {
                println!("{line}\t-> {fixed}");
            }
        }
    }
}
