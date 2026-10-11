//! Guesses about a call from the windows open when recording starts: which app the call is
//! in and, sometimes, who is on it. The Swift recorder sends the titles of the call apps'
//! windows. macOS only shows window titles to apps with Screen Recording permission, so
//! without it only the app names come through and the guesses stay blank.
//!
//! Everything here is pure so it can be unit-tested without a Mac.

use serde::Deserialize;

use crate::library::same_person;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Window {
    /// The owning app as macOS names it: "zoom.us", "WhatsApp", "Google Chrome"...
    pub app: String,
    #[serde(default)]
    pub title: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CallHints {
    /// "Zoom", "Meet" or "WhatsApp" when a call window of that app is open.
    pub source_app: Option<String>,
    /// People named in window titles, other than the user.
    pub participants: Vec<String>,
}

const BROWSERS: &[&str] = &[
    "Google Chrome",
    "Safari",
    "Arc",
    "Firefox",
    "Microsoft Edge",
    "Brave Browser",
    "Chromium",
    "Vivaldi",
    "Opera",
];

/// WhatsApp windows that are part of the app, not a call with someone.
const WHATSAPP_SCREENS: &[&str] = &[
    "WhatsApp",
    "WhatsApp Business",
    "Settings",
    "Preferences",
    "Chats",
    "Calls",
    "Status",
    "Communities",
    "Archived",
    "Starred messages",
    "New chat",
    "New group",
    "Profile",
];

pub fn from_windows(windows: &[Window], your_name: &str) -> CallHints {
    // A Zoom meeting window beats a WhatsApp call window beats a Meet tab, since a Meet tab
    // is often left open after the meeting and Zoom's window is only a meeting while it says so.
    let mut best: Option<(u8, &str)> = None;
    let mut participants: Vec<String> = Vec::new();
    for window in windows {
        let title = window.title.trim();
        let (priority, app, person) = match window.app.as_str() {
            "zoom.us" => match zoom_host(title) {
                Some(host) => (3, "Zoom", Some(host)),
                None if title.contains("Meeting") || title.contains("Webinar") => (3, "Zoom", None),
                None => continue,
            },
            "WhatsApp" => match whatsapp_contact(title) {
                Some(contact) => (2, "WhatsApp", Some(contact)),
                None => continue,
            },
            app if BROWSERS.contains(&app) && is_meet_tab(title) => (1, "Meet", None),
            _ => continue,
        };
        if best.is_none_or(|(rank, _)| priority > rank) {
            best = Some((priority, app));
        }
        if let Some(person) = person {
            if !same_person(person, your_name)
                && !participants.iter().any(|p| same_person(p, person))
            {
                participants.push(person.to_string());
            }
        }
    }
    CallHints {
        source_app: best.map(|(_, app)| app.to_string()),
        participants,
    }
}

/// Zoom titles a meeting after its host unless the host changed the topic:
/// "Dana Cohen's Zoom Meeting" or "Dana Cohen's Personal Meeting Room".
fn zoom_host(title: &str) -> Option<&str> {
    for suffix in ["Zoom Meeting", "Personal Meeting Room"] {
        let Some(before) = title.strip_suffix(suffix) else {
            continue;
        };
        for possessive in ["'s ", "’s "] {
            if let Some(name) = before.strip_suffix(possessive) {
                let name = name.trim();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
    }
    None
}

/// WhatsApp's main window is just "WhatsApp"; a call window is titled after the contact.
fn whatsapp_contact(title: &str) -> Option<&str> {
    let plausible = !title.is_empty()
        && title.chars().count() <= 60
        && !title.contains("WhatsApp")
        && !WHATSAPP_SCREENS.contains(&title);
    plausible.then_some(title)
}

/// Google Meet tabs are titled "Meet – abc-defg-hij" (Chrome adds " - Google Chrome").
fn is_meet_tab(title: &str) -> bool {
    title.starts_with("Meet – ")
        || title.starts_with("Meet - ")
        || title.starts_with("Meet — ")
        || title.contains("Google Meet")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(app: &str, title: &str) -> Window {
        Window {
            app: app.into(),
            title: title.into(),
        }
    }

    #[test]
    fn reads_the_host_off_a_zoom_meeting() {
        let hints = from_windows(
            &[
                win("zoom.us", "Zoom Workplace"),
                win("zoom.us", "Dana Cohen's Zoom Meeting"),
                win("Finder", "Downloads"),
            ],
            "Noa Levi",
        );
        assert_eq!(hints.source_app.as_deref(), Some("Zoom"));
        assert_eq!(hints.participants, vec!["Dana Cohen"]);

        // The user's own meeting room doesn't make them a participant.
        let mine = from_windows(
            &[win("zoom.us", "Noa Levi’s Personal Meeting Room")],
            "noa levi",
        );
        assert_eq!(mine.source_app.as_deref(), Some("Zoom"));
        assert!(mine.participants.is_empty());

        // A topic the host typed names nobody, and an idle Zoom window isn't a call.
        assert_eq!(
            from_windows(&[win("zoom.us", "Weekly sync")], ""),
            CallHints::default()
        );
        assert_eq!(
            from_windows(&[win("zoom.us", "Zoom Meeting")], "")
                .source_app
                .as_deref(),
            Some("Zoom")
        );
    }

    #[test]
    fn names_a_whatsapp_contact_but_not_the_app_itself() {
        let hints = from_windows(&[win("WhatsApp", "WhatsApp"), win("WhatsApp", "Dana")], "");
        assert_eq!(hints.source_app.as_deref(), Some("WhatsApp"));
        assert_eq!(hints.participants, vec!["Dana"]);
        assert_eq!(
            from_windows(
                &[win("WhatsApp", "WhatsApp"), win("WhatsApp", "Settings")],
                ""
            ),
            CallHints::default()
        );
    }

    #[test]
    fn spots_meet_in_a_browser_but_prefers_a_live_zoom_window() {
        let meet = from_windows(
            &[win("Google Chrome", "Meet – abc-defg-hij - Google Chrome")],
            "",
        );
        assert_eq!(meet.source_app.as_deref(), Some("Meet"));
        assert_eq!(
            from_windows(&[win("Safari", "Gmail - Inbox")], ""),
            CallHints::default()
        );

        let both = from_windows(
            &[
                win("Google Chrome", "Meet – abc-defg-hij - Google Chrome"),
                win("zoom.us", "Zoom Meeting"),
            ],
            "",
        );
        assert_eq!(both.source_app.as_deref(), Some("Zoom"));
    }

    #[test]
    fn without_screen_recording_permission_titles_are_empty() {
        let hints = from_windows(&[win("zoom.us", ""), win("WhatsApp", "")], "");
        assert_eq!(hints, CallHints::default());
    }
}
