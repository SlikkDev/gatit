//! Google Drive upload. Every meeting folder is mirrored to Drive as
//! `Gatit/YYYY-MM-DD/HH-MM Title/`, with the same files as the local folder, so calls are
//! organized by date and time and the library can later be synced between machines.
//!
//! Gatit asks Google for the `drive.file` scope only: it can see and change the files it
//! created itself and nothing else in the Drive. It signs in with the user's own OAuth client
//! (created in Google Cloud, see docs/google-drive.md); the client id and secret and the
//! refresh token live in the OS keychain, the access token only in memory. Per-meeting sync
//! state is kept in the meeting's `drive.json`, which is never uploaded.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use base64::Engine;
use chrono::{DateTime, Local};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::io::ReaderStream;

use crate::library::{write_atomic, Meeting, Status};
use crate::settings;
use crate::state::{library_changed, state};

const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const API: &str = "https://www.googleapis.com/drive/v3";
const UPLOAD_API: &str = "https://www.googleapis.com/upload/drive/v3";
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
/// The top-level folder in the user's Drive.
const ROOT_FOLDER: &str = "Gatit";
/// Sync state, kept in each meeting folder and never uploaded.
const STATE_FILE: &str = "drive.json";
const KEYCHAIN_ACCOUNT: &str = "google-drive";
/// How long the browser sign-in may take before the attempt is abandoned.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);
/// A failed upload is retried after 1, 2, 4... minutes, this many times, then waits for the
/// next change to the meeting or the next launch.
const MAX_RETRIES: u32 = 6;

/// Everything needed to talk to Drive as the user, stored as one keychain item.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Credentials {
    pub client_id: String,
    pub client_secret: String,
    /// Empty once Google rejects it: the user has to sign in again.
    pub refresh_token: String,
    /// The Google account's address, for display.
    pub email: String,
}

/// What the Settings window shows about the connection.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Account {
    pub connected: bool,
    pub email: String,
    /// The OAuth client is saved, so signing in again doesn't need the id and secret retyped.
    pub has_client: bool,
    pub client_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Progress {
    pub id: String,
    pub label: String,
    pub percent: f64,
}

/// What has already been uploaded for one meeting (its `drive.json`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncState {
    pub folder_id: String,
    pub folder_name: String,
    pub web_link: String,
    pub files: HashMap<String, FileState>,
    pub synced_at: Option<DateTime<Local>>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileState {
    pub id: String,
    pub size: u64,
    /// Local modification time, seconds since the Unix epoch.
    pub modified: u64,
}

impl SyncState {
    pub fn load(folder: &Path) -> Self {
        std::fs::read(folder.join(STATE_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save(&self, folder: &Path) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        write_atomic(&folder.join(STATE_FILE), &json)
    }

    /// The part the window shows.
    pub fn summary(&self) -> Value {
        json!({
            "web_link": self.web_link,
            "synced_at": self.synced_at,
            "error": self.error,
        })
    }
}

pub struct Drive {
    credentials: Mutex<Option<Credentials>>,
    /// The current access token and when it expires.
    token: Mutex<Option<(String, Instant)>>,
    uploads: UnboundedSender<String>,
    queued: Mutex<HashSet<String>>,
    retries: Mutex<HashMap<String, u32>>,
    pub progress: Mutex<Option<Progress>>,
    http: reqwest::Client,
}

impl Drive {
    pub fn new(uploads: UnboundedSender<String>) -> Self {
        Self {
            credentials: Mutex::new(load_credentials()),
            token: Mutex::new(None),
            uploads,
            queued: Mutex::new(HashSet::new()),
            retries: Mutex::new(HashMap::new()),
            progress: Mutex::new(None),
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
        }
    }

    fn credentials(&self) -> Option<Credentials> {
        self.credentials.lock().unwrap().clone()
    }

    pub fn is_connected(&self) -> bool {
        self.credentials()
            .is_some_and(|c| !c.refresh_token.is_empty())
    }

    pub fn account(&self) -> Account {
        match self.credentials() {
            Some(c) => Account {
                connected: !c.refresh_token.is_empty(),
                email: c.email,
                has_client: !c.client_id.is_empty(),
                client_id: c.client_id,
            },
            None => Account::default(),
        }
    }

    fn store(&self, credentials: Option<Credentials>) -> Result<(), String> {
        let json = match &credentials {
            Some(c) => serde_json::to_string(c).map_err(|e| e.to_string())?,
            None => String::new(),
        };
        settings::set_secret(KEYCHAIN_ACCOUNT, &json)?;
        *self.credentials.lock().unwrap() = credentials;
        *self.token.lock().unwrap() = None;
        Ok(())
    }

    pub fn disconnect(&self) -> Result<(), String> {
        self.queued.lock().unwrap().clear();
        self.store(None)
    }

    /// Opens the browser for Google's consent screen and waits for it to come back with a
    /// code, which is exchanged for tokens. With no client id given, the saved one is used.
    pub async fn connect(&self, client_id: &str, client_secret: &str) -> Result<Account, String> {
        let saved = self.credentials().unwrap_or_default();
        let client_id = pick(client_id, &saved.client_id);
        let client_secret = pick(client_secret, &saved.client_secret);
        if client_id.is_empty() || client_secret.is_empty() {
            return Err("Paste the OAuth client ID and client secret first.".into());
        }

        let verifier = random_token(32)?;
        let state = random_token(16)?;
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| format!("Couldn't open a port for the sign-in: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let redirect_uri = format!("http://127.0.0.1:{port}");
        let url = url::Url::parse_with_params(
            AUTH_URL,
            &[
                ("client_id", client_id.as_str()),
                ("redirect_uri", redirect_uri.as_str()),
                ("response_type", "code"),
                ("scope", SCOPE),
                ("code_challenge", &pkce_challenge(&verifier)),
                ("code_challenge_method", "S256"),
                ("access_type", "offline"),
                ("prompt", "consent"),
                ("state", state.as_str()),
            ],
        )
        .map_err(|e| e.to_string())?;
        open::that_detached(url.as_str()).map_err(|e| format!("Couldn't open the browser: {e}"))?;

        let code = tokio::time::timeout(SIGN_IN_TIMEOUT, wait_for_code(listener, &state))
            .await
            .map_err(|_| "The sign-in took too long. Try again.".to_string())??;

        let tokens = token_request(
            &self.http,
            &[
                ("code", code.as_str()),
                ("client_id", client_id.as_str()),
                ("client_secret", client_secret.as_str()),
                ("redirect_uri", redirect_uri.as_str()),
                ("grant_type", "authorization_code"),
                ("code_verifier", verifier.as_str()),
            ],
        )
        .await
        .map_err(|e| e.message())?;
        let refresh_token = tokens["refresh_token"]
            .as_str()
            .filter(|t| !t.is_empty())
            .ok_or("Google didn't return a refresh token. Remove Gatit's access at myaccount.google.com/permissions and connect again.")?
            .to_string();
        let access_token = tokens["access_token"].as_str().unwrap_or("").to_string();

        let about: Value = self
            .http
            .get(format!("{API}/about?fields=user"))
            .bearer_auth(&access_token)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("Couldn't reach Google Drive: {e}"))?
            .json()
            .await
            .unwrap_or(Value::Null);
        let email = about["user"]["emailAddress"]
            .as_str()
            .unwrap_or("")
            .to_string();

        self.store(Some(Credentials {
            client_id,
            client_secret,
            refresh_token,
            email,
        }))?;
        *self.token.lock().unwrap() = Some((access_token, expiry(&tokens)));
        self.retries.lock().unwrap().clear();
        Ok(self.account())
    }

    async fn access_token(&self) -> Result<String, String> {
        if let Some((token, expires)) = self.token.lock().unwrap().clone() {
            if expires > Instant::now() + Duration::from_secs(60) {
                return Ok(token);
            }
        }
        let creds = self
            .credentials()
            .filter(|c| !c.refresh_token.is_empty())
            .ok_or("Google Drive isn't connected.")?;
        let tokens = token_request(
            &self.http,
            &[
                ("client_id", creds.client_id.as_str()),
                ("client_secret", creds.client_secret.as_str()),
                ("refresh_token", creds.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ],
        )
        .await;
        let tokens = match tokens {
            Ok(t) => t,
            Err(TokenError::InvalidGrant) => {
                // Revoked, or expired (Google Cloud projects still "in testing" get 7-day
                // tokens). Keep the client so signing in again is one click.
                let _ = self.store(Some(Credentials {
                    refresh_token: String::new(),
                    ..creds
                }));
                return Err("Google Drive needs you to sign in again, in Settings.".into());
            }
            Err(e) => return Err(e.message()),
        };
        let token = tokens["access_token"]
            .as_str()
            .ok_or("Google didn't return an access token.")?
            .to_string();
        *self.token.lock().unwrap() = Some((token.clone(), expiry(&tokens)));
        Ok(token)
    }

    async fn request(
        &self,
        method: reqwest::Method,
        url: String,
    ) -> Result<reqwest::RequestBuilder, String> {
        let token = self.access_token().await?;
        Ok(self
            .http
            .request(method, url)
            .bearer_auth(token)
            .timeout(Duration::from_secs(60)))
    }

    async fn json(&self, request: reqwest::RequestBuilder) -> Result<Value, String> {
        let response = request
            .send()
            .await
            .map_err(|e| format!("Couldn't reach Google Drive: {e}"))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            // The token stopped working early; the next call fetches a fresh one.
            *self.token.lock().unwrap() = None;
        }
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        Ok(body)
    }

    async fn find_folder(&self, parent: &str, name: &str) -> Result<Option<String>, String> {
        let q = format!(
            "name = '{}' and mimeType = '{FOLDER_MIME}' and '{}' in parents and trashed = false",
            q_escape(name),
            q_escape(parent)
        );
        let found = self
            .json(
                self.request(reqwest::Method::GET, format!("{API}/files"))
                    .await?
                    .query(&[
                        ("q", q.as_str()),
                        ("fields", "files(id)"),
                        ("pageSize", "5"),
                    ]),
            )
            .await?;
        Ok(found["files"][0]["id"].as_str().map(String::from))
    }

    async fn create_folder(&self, parent: &str, name: &str) -> Result<String, String> {
        let created = self
            .json(
                self.request(reqwest::Method::POST, format!("{API}/files?fields=id"))
                    .await?
                    .json(&json!({ "name": name, "mimeType": FOLDER_MIME, "parents": [parent] })),
            )
            .await?;
        created["id"]
            .as_str()
            .map(String::from)
            .ok_or("Google Drive didn't return the new folder's id.".into())
    }

    async fn ensure_folder(&self, parent: &str, name: &str) -> Result<String, String> {
        match self.find_folder(parent, name).await? {
            Some(id) => Ok(id),
            None => self.create_folder(parent, name).await,
        }
    }

    /// Whether a file we created is still there (not deleted or trashed by the user).
    async fn exists(&self, id: &str) -> Result<bool, String> {
        let response = self
            .request(
                reqwest::Method::GET,
                format!("{API}/files/{id}?fields=trashed"),
            )
            .await?
            .send()
            .await
            .map_err(|e| format!("Couldn't reach Google Drive: {e}"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        Ok(body["trashed"] != true)
    }

    /// The files in a folder, by name: (id, size in bytes).
    async fn children(&self, parent: &str) -> Result<HashMap<String, (String, u64)>, String> {
        let q = format!("'{}' in parents and trashed = false", q_escape(parent));
        let listed = self
            .json(
                self.request(reqwest::Method::GET, format!("{API}/files"))
                    .await?
                    .query(&[
                        ("q", q.as_str()),
                        ("fields", "files(id,name,size)"),
                        ("pageSize", "200"),
                    ]),
            )
            .await?;
        let mut files = HashMap::new();
        for file in listed["files"].as_array().into_iter().flatten() {
            if let (Some(id), Some(name)) = (file["id"].as_str(), file["name"].as_str()) {
                let size = file["size"]
                    .as_str()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                files.insert(name.to_string(), (id.to_string(), size));
            }
        }
        Ok(files)
    }

    async fn rename(&self, id: &str, name: &str) -> Result<(), String> {
        self.json(
            self.request(
                reqwest::Method::PATCH,
                format!("{API}/files/{id}?fields=id"),
            )
            .await?
            .json(&json!({ "name": name })),
        )
        .await
        .map(|_| ())
    }

    async fn trash(&self, id: &str) -> Result<(), String> {
        self.json(
            self.request(
                reqwest::Method::PATCH,
                format!("{API}/files/{id}?fields=id"),
            )
            .await?
            .json(&json!({ "trashed": true })),
        )
        .await
        .map(|_| ())
    }

    /// Uploads a file (replacing `existing` when given) in one resumable-upload session,
    /// streaming it from disk. Returns the file's Drive id.
    async fn upload(
        &self,
        parent: &str,
        existing: Option<&str>,
        path: &Path,
        name: &str,
        on_progress: impl Fn(u64) + Send + Sync + 'static,
    ) -> Result<String, String> {
        let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        let mime = mime_for(name);
        let (method, url, metadata) = match existing {
            Some(id) => (
                reqwest::Method::PATCH,
                format!("{UPLOAD_API}/files/{id}?uploadType=resumable&fields=id"),
                json!({}),
            ),
            None => (
                reqwest::Method::POST,
                format!("{UPLOAD_API}/files?uploadType=resumable&fields=id"),
                json!({ "name": name, "parents": [parent] }),
            ),
        };
        let started = self
            .request(method, url)
            .await?
            .header("X-Upload-Content-Type", mime)
            .header("X-Upload-Content-Length", size)
            .json(&metadata)
            .send()
            .await
            .map_err(|e| format!("Couldn't reach Google Drive: {e}"))?;
        if !started.status().is_success() {
            let status = started.status();
            let body: Value = started.json().await.unwrap_or(Value::Null);
            return Err(api_error(status, &body));
        }
        let session = started
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or("Google Drive didn't start the upload.")?
            .to_string();

        let file = tokio::fs::File::open(path)
            .await
            .map_err(|e| e.to_string())?;
        let mut sent = 0u64;
        let body = ReaderStream::with_capacity(file, 256 * 1024).inspect(move |chunk| {
            if let Ok(chunk) = chunk {
                sent += chunk.len() as u64;
                on_progress(sent);
            }
        });
        let token = self.access_token().await?;
        let response = self
            .http
            .put(session)
            .bearer_auth(token)
            .header(reqwest::header::CONTENT_LENGTH, size)
            .header(reqwest::header::CONTENT_TYPE, mime)
            .body(reqwest::Body::wrap_stream(body))
            .send()
            .await
            .map_err(|e| format!("The upload to Google Drive failed: {e}"))?;
        let status = response.status();
        let uploaded: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(api_error(status, &uploaded));
        }
        uploaded["id"]
            .as_str()
            .map(String::from)
            .or_else(|| existing.map(String::from))
            .ok_or("Google Drive didn't return the uploaded file's id.".into())
    }
}

fn pick(given: &str, saved: &str) -> String {
    let given = given.trim();
    if given.is_empty() { saved } else { given }.to_string()
}

fn load_credentials() -> Option<Credentials> {
    serde_json::from_str(&settings::secret(KEYCHAIN_ACCOUNT)?).ok()
}

fn expiry(tokens: &Value) -> Instant {
    Instant::now() + Duration::from_secs(tokens["expires_in"].as_u64().unwrap_or(3600))
}

enum TokenError {
    InvalidGrant,
    Other(String),
}

impl TokenError {
    fn message(self) -> String {
        match self {
            TokenError::InvalidGrant => "Google rejected the sign-in (invalid_grant).".into(),
            TokenError::Other(m) => m,
        }
    }
}

async fn token_request(http: &reqwest::Client, form: &[(&str, &str)]) -> Result<Value, TokenError> {
    let response = http
        .post(TOKEN_URL)
        .form(form)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| TokenError::Other(format!("Couldn't reach Google: {e}")))?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(body);
    }
    let error = body["error"].as_str().unwrap_or("");
    if error == "invalid_grant" {
        return Err(TokenError::InvalidGrant);
    }
    let description = body["error_description"].as_str().unwrap_or("");
    Err(TokenError::Other(format!(
        "Google sign-in failed ({status}): {error} {description}"
    )))
}

fn api_error(status: reqwest::StatusCode, body: &Value) -> String {
    match body["error"]["message"].as_str() {
        Some(message) => format!("Google Drive returned {status}: {message}"),
        None => format!("Google Drive returned {status}."),
    }
}

/// Escapes a value for Drive's `q` search syntax.
fn q_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

fn mime_for(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "m4a" => "audio/mp4",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "json" => "application/json",
        "md" => "text/markdown",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}

fn base64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn random_token(bytes: usize) -> Result<String, String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| e.to_string())?;
    Ok(base64url(&buf))
}

fn pkce_challenge(verifier: &str) -> String {
    base64url(&Sha256::digest(verifier.as_bytes()))
}

/// The page the browser shows once Google has sent it back to Gatit.
const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Gatit</title>\
<body style=\"font-family:-apple-system,sans-serif;text-align:center;padding-top:80px\">\
<h2>Gatit is connected to Google Drive</h2><p>You can close this tab.</p></body>";

/// Pulls the authorization code out of the browser's redirect, which arrives as a plain
/// HTTP request like `GET /?state=abc&code=4/0Ab... HTTP/1.1`. Ok(None) means the request
/// was something else (a favicon, say) and the wait goes on.
fn parse_redirect(request_line: &str, expected_state: &str) -> Result<Option<String>, String> {
    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let url = url::Url::parse(&format!("http://127.0.0.1{path}")).map_err(|e| e.to_string())?;
    let (mut code, mut state, mut error) = (None, None, None);
    for (key, value) in url.query_pairs() {
        match &*key {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            _ => {}
        }
    }
    if let Some(error) = error {
        return Err(format!("Google didn't complete the sign-in: {error}"));
    }
    match code {
        None => Ok(None),
        Some(code) if state.as_deref() == Some(expected_state) => Ok(Some(code)),
        Some(_) => Err("The sign-in response didn't match this attempt.".into()),
    }
}

async fn wait_for_code(listener: TcpListener, expected_state: &str) -> Result<String, String> {
    loop {
        let (mut stream, _) = listener.accept().await.map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 8192];
        let n = stream.read(&mut buf).await.unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..n]);
        let outcome = parse_redirect(request.lines().next().unwrap_or(""), expected_state);
        let (status, body) = match &outcome {
            Ok(Some(_)) => ("200 OK", DONE_PAGE.to_string()),
            Ok(None) => ("404 Not Found", String::new()),
            Err(message) => ("400 Bad Request", message.clone()),
        };
        let reply = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(reply.as_bytes()).await;
        let _ = stream.shutdown().await;
        match outcome {
            Ok(Some(code)) => return Ok(code),
            Ok(None) => continue,
            Err(message) => return Err(message),
        }
    }
}

// ---------- syncing meetings ----------

/// The Drive folder name for a meeting: its start time, then its title.
pub fn folder_name(meeting: &Meeting) -> String {
    let title: String = meeting
        .title
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if c == '/' || c == '\\' { '-' } else { c })
        .take(100)
        .collect();
    let title = title.trim();
    format!(
        "{} {}",
        meeting.started_at.format("%H-%M"),
        if title.is_empty() { "Call" } else { title }
    )
}

/// The files in a meeting folder that belong on Drive: (name, size, modified, path).
fn local_files(folder: &Path) -> Vec<(String, u64, u64, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return vec![];
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let meta = entry.metadata().ok()?;
            if !meta.is_file()
                || name.starts_with('.')
                || name.ends_with(".tmp")
                || name == STATE_FILE
            {
                return None;
            }
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            Some((name, meta.len(), modified, entry.path()))
        })
        .collect();
    files.sort();
    files
}

/// Whether anything in the folder changed since it was last uploaded. Decided from local
/// state alone, so checking every meeting at launch costs no requests.
pub fn needs_sync(folder: &Path, meeting: &Meeting) -> bool {
    let sync = SyncState::load(folder);
    if sync.folder_id.is_empty() || sync.error.is_some() || sync.folder_name != folder_name(meeting)
    {
        return true;
    }
    local_files(folder)
        .into_iter()
        .any(|(name, size, modified, _)| {
            sync.files
                .get(&name)
                .is_none_or(|f| f.size != size || f.modified != modified)
        })
}

fn report(app: &AppHandle, id: &str, label: &str, percent: f64) {
    let progress = Progress {
        id: id.into(),
        label: label.into(),
        percent: percent.clamp(0.0, 100.0),
    };
    *state(app).drive.progress.lock().unwrap() = Some(progress.clone());
    let _ = app.emit("drive", Some(progress));
}

fn clear_report(app: &AppHandle) {
    *state(app).drive.progress.lock().unwrap() = None;
    let _ = app.emit("drive", None::<Progress>);
}

/// Brings a meeting's Drive folder up to date: creates the date and meeting folders, renames
/// the folder when the title changed, and uploads new or changed files.
async fn sync_meeting(app: &AppHandle, id: &str) -> Result<(), String> {
    let st = state(app);
    let drive = &st.drive;
    let (folder, meeting) = {
        let lib = st.library();
        (lib.folder(id)?, lib.get(id)?)
    };
    if meeting.status == Status::Recording {
        return Ok(());
    }
    let mut sync = SyncState::load(&folder);
    let name = folder_name(&meeting);

    let root = drive.ensure_folder("root", ROOT_FOLDER).await?;
    let day = drive
        .ensure_folder(&root, &meeting.started_at.format("%Y-%m-%d").to_string())
        .await?;
    let mut folder_id = sync.folder_id.clone();
    if folder_id.is_empty() || !drive.exists(&folder_id).await? {
        // First upload, or the folder was deleted in Drive: look for it by its last known
        // name (the id may have been lost with a reinstall), else start a fresh one.
        let by_old_name = if sync.folder_name.is_empty() {
            None
        } else {
            drive.find_folder(&day, &sync.folder_name).await?
        };
        folder_id = match by_old_name {
            Some(id) => id,
            None => drive.ensure_folder(&day, &name).await?,
        };
        sync.files.clear();
    }
    if sync.folder_name != name {
        drive.rename(&folder_id, &name).await?;
    }
    sync.folder_id = folder_id.clone();
    sync.folder_name = name;
    sync.web_link = format!("https://drive.google.com/drive/folders/{folder_id}");

    let remote = drive.children(&folder_id).await?;
    let mut pending = Vec::new();
    for (file_name, size, modified, path) in local_files(&folder) {
        let on_drive = remote.get(&file_name);
        let recorded = sync.files.get(&file_name);
        let up_to_date = match (on_drive, recorded) {
            (Some(_), Some(f)) => f.size == size && f.modified == modified,
            // Uploaded by another machine, or our state was lost: trust a matching size.
            (Some((_, remote_size)), None) => *remote_size == size,
            (None, _) => false,
        };
        if up_to_date {
            if let Some((remote_id, _)) = on_drive {
                sync.files.insert(
                    file_name,
                    FileState {
                        id: remote_id.clone(),
                        size,
                        modified,
                    },
                );
            }
        } else {
            pending.push((file_name, size, modified, path));
        }
    }
    sync.save(&folder)?;

    let total: u64 = pending.iter().map(|f| f.1).sum::<u64>().max(1);
    let mut done = 0u64;
    for (file_name, size, modified, path) in pending {
        let existing = remote.get(&file_name).map(|(id, _)| id.as_str());
        let label = format!("Uploading {file_name} to Google Drive");
        report(app, id, &label, done as f64 / total as f64 * 100.0);
        let (progress_app, progress_id, progress_label) = (app.clone(), id.to_string(), label);
        let file_id = drive
            .upload(&folder_id, existing, &path, &file_name, move |sent| {
                let percent = (done + sent) as f64 / total as f64 * 100.0;
                report(&progress_app, &progress_id, &progress_label, percent);
            })
            .await?;
        done += size;
        sync.files.insert(
            file_name,
            FileState {
                id: file_id,
                size,
                modified,
            },
        );
        sync.save(&folder)?;
    }
    sync.synced_at = Some(Local::now());
    sync.error = None;
    sync.save(&folder)
}

/// Queues a meeting for upload (no-op when Drive isn't connected or uploads are off).
pub fn enqueue(app: &AppHandle, id: &str) {
    let st = state(app);
    if !st.drive.is_connected() || !st.settings().drive_upload {
        return;
    }
    if st.drive.queued.lock().unwrap().insert(id.to_string()) {
        let _ = st.drive.uploads.send(id.to_string());
    }
}

/// Queues every meeting with something not yet uploaded. Runs at launch, when Drive is
/// connected, and when automatic uploads are switched back on.
pub fn resume(app: &AppHandle) {
    let st = state(app);
    if !st.drive.is_connected() || !st.settings().drive_upload {
        return;
    }
    let meetings = st.library().list().unwrap_or_default();
    let folders: Vec<(String, PathBuf, Meeting)> = {
        let lib = st.library();
        meetings
            .into_iter()
            .filter(|m| m.status != Status::Recording)
            .filter_map(|m| Some((m.id.clone(), lib.folder(&m.id).ok()?, m)))
            .collect()
    };
    for (id, folder, meeting) in folders {
        if needs_sync(&folder, &meeting) {
            enqueue(app, &id);
        }
    }
}

/// Moves a deleted meeting's Drive folder to Drive's trash, in the background.
pub fn trash_folder(app: &AppHandle, folder_id: String) {
    if folder_id.is_empty() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = state(&app).drive.trash(&folder_id).await {
            eprintln!("google drive: couldn't trash {folder_id}: {e}");
        }
    });
}

/// The upload worker: one meeting at a time, retrying failures with growing delays.
pub async fn run(app: AppHandle, mut uploads: UnboundedReceiver<String>) {
    while let Some(id) = uploads.recv().await {
        let st = state(&app);
        st.drive.queued.lock().unwrap().remove(&id);
        if !st.drive.is_connected() || !st.settings().drive_upload {
            continue;
        }
        report(&app, &id, "Uploading to Google Drive", 0.0);
        let result = sync_meeting(&app, &id).await;
        clear_report(&app);
        match result {
            Ok(()) => {
                st.drive.retries.lock().unwrap().remove(&id);
            }
            Err(message) => {
                eprintln!("google drive: {id}: {message}");
                if let Ok(folder) = st.library().folder(&id) {
                    if folder.exists() {
                        let mut sync = SyncState::load(&folder);
                        sync.error = Some(message.clone());
                        let _ = sync.save(&folder);
                    }
                }
                let attempt = {
                    let mut retries = st.drive.retries.lock().unwrap();
                    let attempt = retries.entry(id.clone()).or_insert(0);
                    *attempt += 1;
                    *attempt
                };
                // A sign-in problem can't fix itself; everything else gets another go.
                if attempt <= MAX_RETRIES && st.drive.is_connected() {
                    let (app, id) = (app.clone(), id.clone());
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(60 << (attempt - 1))).await;
                        enqueue(&app, &id);
                    });
                }
            }
        }
        library_changed(&app, &id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn meeting(title: &str) -> Meeting {
        Meeting {
            id: "2026-10-11 14-03-22".into(),
            title: title.into(),
            started_at: Local.with_ymd_and_hms(2026, 10, 11, 14, 3, 22).unwrap(),
            duration_secs: 0.0,
            source_app: String::new(),
            language: "he".into(),
            status: Status::Done,
            error: None,
            title_edited: false,
            participants: vec![],
            participants_edited: false,
        }
    }

    #[test]
    fn folder_names_start_with_the_time() {
        assert_eq!(folder_name(&meeting("Pricing call")), "14-03 Pricing call");
        assert_eq!(folder_name(&meeting("  ")), "14-03 Call");
        assert_eq!(folder_name(&meeting("Q3/Q4 plan")), "14-03 Q3-Q4 plan");
        assert_eq!(
            folder_name(&meeting("תמחור עם הלקוח")),
            "14-03 תמחור עם הלקוח"
        );
    }

    #[test]
    fn pkce_challenge_matches_rfc_7636() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn redirect_parsing() {
        let line = "GET /?state=abc&code=4%2F0AbCd&scope=drive.file HTTP/1.1";
        assert_eq!(parse_redirect(line, "abc").unwrap(), Some("4/0AbCd".into()));
        assert!(parse_redirect(line, "other").is_err());
        assert_eq!(
            parse_redirect("GET /favicon.ico HTTP/1.1", "abc").unwrap(),
            None
        );
        assert!(parse_redirect("GET /?error=access_denied HTTP/1.1", "abc").is_err());
    }

    #[test]
    fn search_queries_escape_quotes() {
        assert_eq!(q_escape("Noa's call"), "Noa\\'s call");
    }

    #[test]
    fn sync_state_tracks_local_changes() {
        let folder = std::env::temp_dir().join(format!("gatit-drive-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("meeting.md"), "# hi").unwrap();
        std::fs::write(folder.join("meeting.tmp"), "half-written").unwrap();
        let m = meeting("Pricing call");

        // Never uploaded.
        assert!(needs_sync(&folder, &m));
        assert_eq!(local_files(&folder).len(), 1, "temp files are skipped");

        let (name, size, modified, _) = local_files(&folder).remove(0);
        let mut sync = SyncState {
            folder_id: "f1".into(),
            folder_name: folder_name(&m),
            ..Default::default()
        };
        sync.files.insert(
            name,
            FileState {
                id: "x".into(),
                size,
                modified,
            },
        );
        sync.save(&folder).unwrap();
        assert!(!needs_sync(&folder, &m));
        assert_eq!(local_files(&folder).len(), 1, "drive.json is skipped");

        // A new file, a renamed meeting, or an earlier failure each need another upload.
        assert!(needs_sync(&folder, &meeting("New title")));
        std::fs::write(folder.join("summary.json"), "{}").unwrap();
        assert!(needs_sync(&folder, &m));
        let _ = std::fs::remove_dir_all(&folder);
    }
}
