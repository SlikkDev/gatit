//! The meeting library. Each meeting is a plain folder of files, so the folders can later be
//! synced between machines (Google Drive, after v1) by copying files:
//!
//! ```text
//! meetings/2026-10-11 14-03-22/
//!   meeting.json     title, time, length, app, language, status
//!   mic.m4a          your side ("Me")
//!   system.m4a       everyone else ("Them")
//!   video.mp4        the call app's window with both sides mixed in, when video was recorded
//!                    (screen.mp4 + screen.json until the two are combined after the call)
//!   transcript.json  merged Me/Them transcript
//!   summary.json     Claude's summary
//!   meeting.md       readable export of all of the above
//! ```
//!
//! The SQLite search index is only a cache: it can always be rebuilt from the folders.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::summary::Summary;
use crate::transcript::{to_text, Transcript};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Recording,
    /// Audio saved, waiting for transcription.
    Recorded,
    Transcribing,
    Transcribed,
    Summarizing,
    Done,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Meeting {
    pub id: String,
    pub title: String,
    pub started_at: DateTime<Local>,
    #[serde(default)]
    pub duration_secs: f64,
    /// "Zoom", "Meet", "WhatsApp", or empty when unknown. Editable by hand.
    #[serde(default)]
    pub source_app: String,
    /// "auto", "he" or "en".
    #[serde(default = "default_language")]
    pub language: String,
    pub status: Status,
    #[serde(default)]
    pub error: Option<String>,
    /// Set when the user edited the title, so a re-summary doesn't overwrite it.
    #[serde(default)]
    pub title_edited: bool,
}

fn default_language() -> String {
    "he".into()
}

pub struct Library {
    root: PathBuf,
    db: Connection,
}

impl Library {
    pub fn open(root: PathBuf, db_path: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let db = Connection::open(db_path).map_err(|e| e.to_string())?;
        Self::with_connection(root, db)
    }

    fn with_connection(root: PathBuf, db: Connection) -> Result<Self, String> {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS meetings (id TEXT PRIMARY KEY, json TEXT NOT NULL);
             -- Trigram tokenizer = substring search. Hebrew attaches prefixes (ה, ו, ב, ש...)
             -- to words, so word-based search would miss \"המחיר\" when looking for \"מחיר\".
             CREATE VIRTUAL TABLE IF NOT EXISTS docs USING fts5(meeting_id UNINDEXED, kind UNINDEXED, body, tokenize = 'trigram');",
        )
        .map_err(|e| e.to_string())?;
        let library = Self { root, db };
        library.rebuild_index()?;
        Ok(library)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn folder(&self, id: &str) -> Result<PathBuf, String> {
        // Ids are folder names we created; refuse anything that could escape the library.
        if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
            return Err("invalid meeting id".into());
        }
        Ok(self.root.join(id))
    }

    pub fn create(
        &mut self,
        started_at: DateTime<Local>,
        source_app: String,
        language: String,
    ) -> Result<Meeting, String> {
        let id = started_at.format("%Y-%m-%d %H-%M-%S").to_string();
        std::fs::create_dir_all(self.folder(&id)?).map_err(|e| e.to_string())?;
        let meeting = Meeting {
            title: default_title(&started_at, &source_app),
            id,
            started_at,
            duration_secs: 0.0,
            source_app,
            language,
            status: Status::Recording,
            error: None,
            title_edited: false,
        };
        self.save(&meeting)?;
        Ok(meeting)
    }

    pub fn get(&self, id: &str) -> Result<Meeting, String> {
        let path = self.folder(id)?.join("meeting.json");
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }

    pub fn save(&mut self, meeting: &Meeting) -> Result<(), String> {
        let json = serde_json::to_string_pretty(meeting).map_err(|e| e.to_string())?;
        write_atomic(
            &self.folder(&meeting.id)?.join("meeting.json"),
            json.as_bytes(),
        )?;
        self.db
            .execute(
                "INSERT OR REPLACE INTO meetings (id, json) VALUES (?1, ?2)",
                params![meeting.id, json],
            )
            .map_err(|e| e.to_string())?;
        self.index_text(&meeting.id, "title", &meeting.title)
    }

    pub fn update(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut Meeting),
    ) -> Result<Meeting, String> {
        let mut meeting = self.get(id)?;
        change(&mut meeting);
        self.save(&meeting)?;
        Ok(meeting)
    }

    pub fn list(&self) -> Result<Vec<Meeting>, String> {
        let mut stmt = self
            .db
            .prepare("SELECT json FROM meetings")
            .map_err(|e| e.to_string())?;
        let mut meetings: Vec<Meeting> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .filter_map(|json| serde_json::from_str(&json.ok()?).ok())
            .collect();
        meetings.sort_by_key(|m| std::cmp::Reverse(m.started_at));
        Ok(meetings)
    }

    pub fn transcript(&self, id: &str) -> Option<Transcript> {
        read_json(&self.folder(id).ok()?.join("transcript.json"))
    }

    pub fn summary(&self, id: &str) -> Option<Summary> {
        read_json(&self.folder(id).ok()?.join("summary.json"))
    }

    pub fn save_transcript(&mut self, id: &str, transcript: &Transcript) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(transcript).map_err(|e| e.to_string())?;
        write_atomic(&self.folder(id)?.join("transcript.json"), &json)?;
        self.index_text(id, "transcript", &to_text(&transcript.segments))?;
        self.write_markdown(id)
    }

    pub fn save_summary(&mut self, id: &str, summary: &Summary) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(summary).map_err(|e| e.to_string())?;
        write_atomic(&self.folder(id)?.join("summary.json"), &json)?;
        self.index_text(id, "summary", &summary.to_markdown())?;
        self.write_markdown(id)
    }

    /// Writes meeting.md, a readable copy of the summary and transcript for export.
    pub fn write_markdown(&self, id: &str) -> Result<(), String> {
        let meeting = self.get(id)?;
        let mut md = format!(
            "# {}\n\n{} · {}{}\n\n",
            meeting.title,
            meeting.started_at.format("%A %-d %B %Y, %H:%M"),
            format_duration(meeting.duration_secs),
            if meeting.source_app.is_empty() {
                String::new()
            } else {
                format!(" · {}", meeting.source_app)
            }
        );
        if let Some(summary) = self.summary(id) {
            md.push_str(&summary.to_markdown());
            md.push('\n');
        }
        if let Some(transcript) = self.transcript(id) {
            md.push_str("## Transcript\n\n");
            for line in to_text(&transcript.segments).lines() {
                md.push_str(line);
                md.push_str("  \n");
            }
        }
        write_atomic(&self.folder(id)?.join("meeting.md"), md.as_bytes())
    }

    pub fn delete(&mut self, id: &str) -> Result<(), String> {
        let folder = self.folder(id)?;
        if folder.exists() {
            // To the system Trash, so a mistaken delete can be undone in Finder.
            trash::delete(&folder).map_err(|e| e.to_string())?;
        }
        self.db
            .execute("DELETE FROM meetings WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        self.db
            .execute("DELETE FROM docs WHERE meeting_id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Returns matching meeting ids, newest first, each with a short snippet around the hit.
    pub fn search(&self, query: &str) -> Result<Vec<(String, String)>, String> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(vec![]);
        }
        // Trigram FTS needs at least 3 characters; shorter queries fall back to LIKE.
        let sql = if query.chars().count() >= 3 {
            "SELECT meeting_id, body FROM docs WHERE body MATCH ?1"
        } else {
            "SELECT meeting_id, body FROM docs WHERE body LIKE ?1"
        };
        let arg = if query.chars().count() >= 3 {
            format!("\"{}\"", query.replace('"', "\"\""))
        } else {
            format!("%{}%", query.replace(['%', '_'], ""))
        };
        let mut stmt = self.db.prepare(sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![arg], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        let mut hits: Vec<(String, String)> = Vec::new();
        for row in rows.flatten() {
            if !hits.iter().any(|(id, _)| id == &row.0) {
                hits.push((row.0, snippet(&row.1, query)));
            }
        }
        hits.sort_by(|a, b| b.0.cmp(&a.0));
        Ok(hits)
    }

    fn index_text(&mut self, id: &str, kind: &str, body: &str) -> Result<(), String> {
        let tx = self.db.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM docs WHERE meeting_id = ?1 AND kind = ?2",
            params![id, kind],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO docs (meeting_id, kind, body) VALUES (?1, ?2, ?3)",
            params![id, kind, body],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }

    /// Re-reads every meeting folder into the index. Runs at startup, so folders copied in
    /// from elsewhere (or edited by hand) show up.
    pub fn rebuild_index(&self) -> Result<(), String> {
        self.db
            .execute_batch("DELETE FROM meetings; DELETE FROM docs;")
            .map_err(|e| e.to_string())?;
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().to_string();
            let Ok(meeting) = self.get(&id) else { continue };
            let json = serde_json::to_string(&meeting).map_err(|e| e.to_string())?;
            let insert = |kind: &str, body: &str| {
                self.db.execute(
                    "INSERT INTO docs (meeting_id, kind, body) VALUES (?1, ?2, ?3)",
                    params![id, kind, body],
                )
            };
            self.db
                .execute(
                    "INSERT OR REPLACE INTO meetings (id, json) VALUES (?1, ?2)",
                    params![id, json],
                )
                .map_err(|e| e.to_string())?;
            insert("title", &meeting.title).map_err(|e| e.to_string())?;
            if let Some(t) = self.transcript(&id) {
                insert("transcript", &to_text(&t.segments)).map_err(|e| e.to_string())?;
            }
            if let Some(s) = self.summary(&id) {
                insert("summary", &s.to_markdown()).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}

pub fn default_title(started_at: &DateTime<Local>, source_app: &str) -> String {
    let app = if source_app.is_empty() {
        "Call"
    } else {
        source_app
    };
    format!("{} at {}", app, started_at.format("%H:%M"))
}

pub fn format_duration(secs: f64) -> String {
    let mins = (secs / 60.0).round() as u64;
    if mins >= 60 {
        format!("{} h {} min", mins / 60, mins % 60)
    } else {
        format!("{} min", mins.max(if secs > 0.0 { 1 } else { 0 }))
    }
}

fn snippet(body: &str, query: &str) -> String {
    let lower_body = body.to_lowercase();
    let lower_query = query.to_lowercase();
    let chars: Vec<char> = body.chars().collect();
    let Some(byte_pos) = lower_body.find(&lower_query) else {
        return chars.iter().take(120).collect();
    };
    // Lowercasing can change byte lengths for some scripts; work in chars from here.
    let char_pos = lower_body[..byte_pos].chars().count().min(chars.len());
    let start = char_pos.saturating_sub(50);
    let end = (char_pos + query.chars().count() + 70).min(chars.len());
    let mut s: String = chars[start..end]
        .iter()
        .collect::<String>()
        .replace('\n', " ");
    if start > 0 {
        s.insert(0, '…');
    }
    if end < chars.len() {
        s.push('…');
    }
    s
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Writes via a temp file and rename, so a crash never leaves a half-written file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{Segment, Speaker};

    fn temp_library(name: &str) -> Library {
        let root = std::env::temp_dir().join(format!("gatit-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Library::with_connection(root, Connection::open_in_memory().unwrap()).unwrap()
    }

    fn transcript(lines: &[&str]) -> Transcript {
        Transcript {
            language: "he".into(),
            model: "test".into(),
            segments: lines
                .iter()
                .enumerate()
                .map(|(i, t)| Segment {
                    speaker: Speaker::Them,
                    start: i as f64,
                    end: i as f64 + 1.0,
                    text: t.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn hebrew_search_finds_words_with_attached_prefixes() {
        let mut lib = temp_library("search");
        let m = lib
            .create(Local::now(), "Zoom".into(), "he".into())
            .unwrap();
        lib.save_transcript(
            &m.id,
            &transcript(&["דיברנו על המחיר החדש", "והלקוח ביקש הנחה"]),
        )
        .unwrap();

        assert_eq!(lib.search("מחיר").unwrap().len(), 1, "finds המחיר");
        assert_eq!(lib.search("לקוח").unwrap().len(), 1, "finds והלקוח");
        assert!(lib.search("מחיר").unwrap()[0].1.contains("המחיר"));
        assert_eq!(lib.search("שולחן").unwrap().len(), 0);
        // Two-letter queries fall back to LIKE.
        assert_eq!(lib.search("הנ").unwrap().len(), 1);
    }

    #[test]
    fn index_rebuilds_from_folders() {
        let mut lib = temp_library("rebuild");
        let m = lib
            .create(Local::now(), String::new(), "he".into())
            .unwrap();
        lib.save_transcript(&m.id, &transcript(&["pricing for the CRO project"]))
            .unwrap();
        let root = lib.root().to_path_buf();
        drop(lib);

        let lib = Library::with_connection(root, Connection::open_in_memory().unwrap()).unwrap();
        assert_eq!(lib.list().unwrap().len(), 1);
        assert_eq!(lib.search("cro project").unwrap().len(), 1);
    }

    #[test]
    fn rejects_ids_that_escape_the_library() {
        let lib = temp_library("ids");
        assert!(lib.folder("../etc").is_err());
        assert!(lib.folder("a/b").is_err());
        assert!(lib.folder("2026-10-11 10-00-00").is_ok());
    }

    #[test]
    fn writes_markdown_export() {
        let mut lib = temp_library("md");
        let m = lib
            .create(Local::now(), "Meet".into(), "he".into())
            .unwrap();
        lib.save_transcript(&m.id, &transcript(&["שלום"])).unwrap();
        let md = std::fs::read_to_string(lib.folder(&m.id).unwrap().join("meeting.md")).unwrap();
        assert!(md.contains("## Transcript"));
        assert!(md.contains("Them: שלום"));
    }

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(0.0), "0 min");
        assert_eq!(format_duration(20.0), "1 min");
        assert_eq!(format_duration(3720.0), "1 h 2 min");
    }
}
