const { invoke, convertFileSrc } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

const state = {
  meetings: [],
  selected: null,
  detail: null,
  searchHits: null, // Map of id -> snippet while a search is active
  recordingSince: null,
  recordingMeetingId: null,
  job: null,
  settings: null,
};

// ---------- helpers ----------

function fmtTime(seconds) {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const mm = String(Math.floor(s / 60) % 60).padStart(2, "0");
  const ss = String(s % 60).padStart(2, "0");
  return h ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

function fmtDuration(seconds) {
  const mins = Math.round(seconds / 60);
  if (mins >= 60) return `${Math.floor(mins / 60)} h ${mins % 60} min`;
  return `${Math.max(mins, seconds > 0 ? 1 : 0)} min`;
}

function dayLabel(date) {
  const today = new Date();
  const d = new Date(date);
  const sameDay = (a, b) => a.toDateString() === b.toDateString();
  if (sameDay(d, today)) return "Today";
  const yesterday = new Date(today);
  yesterday.setDate(today.getDate() - 1);
  if (sameDay(d, yesterday)) return "Yesterday";
  return d.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" });
}

function statusLabel(m) {
  return {
    recording: "Recording",
    recorded: "Waiting to transcribe",
    transcribing: "Transcribing",
    transcribed: "Transcribed",
    summarizing: "Summarizing",
    done: "Done",
    error: "Needs attention",
  }[m.status] || m.status;
}

function el(tag, attrs = {}, children = []) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") node.className = v;
    else if (k === "text") node.textContent = v;
    else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else node.setAttribute(k, v);
  }
  for (const child of [].concat(children)) {
    if (child != null) node.append(child);
  }
  return node;
}

function highlight(text, query) {
  if (!query) return [text];
  const lower = text.toLowerCase();
  const i = lower.indexOf(query.toLowerCase());
  if (i < 0) return [text];
  return [text.slice(0, i), el("mark", { text: text.slice(i, i + query.length) }), text.slice(i + query.length)];
}

function showError(message) {
  const notice = $("notice");
  notice.textContent = message;
  notice.hidden = !message;
}

// ---------- recording ----------

function setRecording(recording) {
  $("record").classList.toggle("recording", recording);
  $("record-label").textContent = recording ? "Stop" : "Record";
  $("record").disabled = false;
  $("meters").hidden = !recording;
  if (!recording) {
    state.recordingSince = null;
    state.recordingMeetingId = null;
    $("meter-mic").style.width = $("meter-system").style.width = "0";
  }
}

// Speech sits roughly between 0.01 and 0.3 RMS; show it on a log scale.
function levelToPercent(level) {
  if (level <= 0.001) return 0;
  return Math.min(100, Math.max(0, ((Math.log10(level) + 3) / 2.5) * 100));
}

listen("recorder", ({ payload }) => {
  switch (payload.event) {
    case "starting":
      $("record").disabled = true;
      showError("");
      break;
    case "started":
      state.recordingSince = Date.now();
      setRecording(true);
      break;
    case "levels":
      $("meter-mic").style.width = levelToPercent(payload.mic) + "%";
      $("meter-system").style.width = levelToPercent(payload.system) + "%";
      break;
    case "error":
      showError(payload.message);
      $("record").disabled = false;
      break;
    case "notice":
      showError(payload.message);
      break;
    case "idle":
      setRecording(false);
      break;
  }
});

$("record").addEventListener("click", async () => {
  $("record").disabled = true;
  try {
    await invoke("toggle_recording");
  } catch (error) {
    showError(String(error));
    $("record").disabled = false;
  }
});

setInterval(() => {
  $("timer").textContent = state.recordingSince ? fmtTime((Date.now() - state.recordingSince) / 1000) : "00:00";
}, 500);

// ---------- meeting list ----------

async function refreshList() {
  state.meetings = await invoke("list_meetings");
  renderList();
}

function renderList() {
  const nav = $("meetings");
  nav.replaceChildren();
  const query = $("search").value.trim();
  const visible = state.searchHits ? state.meetings.filter((m) => state.searchHits.has(m.id)) : state.meetings;
  if (visible.length === 0) {
    nav.append(el("p", { class: "muted", style: "padding: 8px", text: query ? "No calls mention that." : "No calls yet." }));
    return;
  }
  let lastDay = null;
  for (const m of visible) {
    const day = dayLabel(m.started_at);
    if (day !== lastDay) {
      nav.append(el("h4", { text: day }));
      lastDay = day;
    }
    const time = new Date(m.started_at).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
    const sub = [time, m.duration_secs ? fmtDuration(m.duration_secs) : null, m.source_app || null, m.status === "done" ? null : statusLabel(m)]
      .filter(Boolean)
      .join(" · ");
    const button = el("button", { class: "meeting" + (m.id === state.selected ? " selected" : ""), onclick: () => select(m.id) }, [
      el("span", { class: "name", text: m.title }),
      el("span", { class: "sub", text: sub }),
      state.searchHits ? el("span", { class: "snippet" }, highlight(state.searchHits.get(m.id), query)) : null,
    ]);
    nav.append(button);
  }
}

let searchTimer;
$("search").addEventListener("input", () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(async () => {
    const query = $("search").value.trim();
    if (!query) {
      state.searchHits = null;
    } else {
      const hits = await invoke("search", { query });
      state.searchHits = new Map(hits.map((h) => [h.id, h.snippet]));
    }
    renderList();
  }, 150);
});

listen("library", async ({ payload }) => {
  await refreshList();
  if (payload === state.selected) await loadDetail();
});

// ---------- meeting detail ----------

async function select(id) {
  state.selected = id;
  $("settings").hidden = true;
  renderList();
  await loadDetail();
}

async function loadDetail() {
  if (!state.selected) return showEmpty();
  try {
    state.detail = await invoke("get_meeting", { id: state.selected });
  } catch (error) {
    state.selected = null;
    return showEmpty();
  }
  renderDetail();
}

function showEmpty() {
  $("detail").hidden = true;
  $("empty").hidden = false;
  stopPlayer();
}

function renderDetail() {
  const { meeting, transcript, summary, mic, system, video, video_offset } = state.detail;
  $("empty").hidden = true;
  $("detail").hidden = false;

  $("title").value = meeting.title;
  const started = new Date(meeting.started_at);
  $("when").textContent =
    started.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long", year: "numeric" }) +
    ", " + started.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" }) +
    (meeting.duration_secs ? " · " + fmtDuration(meeting.duration_secs) : "");
  $("source-app").value = meeting.source_app || "";
  $("language").value = meeting.language || "he";
  const pill = $("status-pill");
  pill.textContent = statusLabel(meeting);
  pill.className = "pill " + meeting.status;
  $("error").textContent = meeting.error || "";
  $("error").hidden = !meeting.error;

  const busy = ["recording", "transcribing", "summarizing"].includes(meeting.status);
  $("summarize").disabled = busy || !transcript;
  $("summarize").textContent = summary ? "Summarize again" : "Summarize";
  $("copy-summary").disabled = !summary;
  $("transcribe").disabled = busy;
  $("transcribe").textContent = transcript ? "Re-transcribe" : "Transcribe";
  $("delete").disabled = meeting.status === "recording";
  renderProgress();

  // Summary
  const box = $("summary");
  box.replaceChildren();
  if (summary) {
    box.append(el("h3", { text: "Summary" }), el("p", { text: summary.overview, dir: "auto" }));
    const list = (heading, items, render) => {
      if (!items || items.length === 0) return;
      box.append(el("h3", { text: heading }), el("ul", {}, items.map((item) => el("li", { dir: "auto" }, render ? render(item) : item))));
    };
    list("Key points", summary.key_points);
    list("Decisions", summary.decisions);
    list("Action items", summary.action_items, (a) => [a.text, el("span", { class: "owner", text: a.owner === "me" ? "me" : "them" })]);
    list("Open questions", summary.open_questions);
    if (summary.model) box.append(el("p", { class: "model", text: `Written by ${summary.model}` }));
  } else if (meeting.status === "transcribed") {
    box.append(el("p", { class: "placeholder", text: "No summary yet. Press Summarize, or add your Claude API key in Settings to get summaries automatically." }));
  }

  // Transcript
  const area = $("transcript");
  area.replaceChildren();
  if (transcript && transcript.segments.length) {
    area.append(el("h3", { text: `Transcript · ${transcript.language === "en" ? "English" : "Hebrew"} · ${transcript.model}` }));
    for (const seg of transcript.segments) {
      area.append(
        el("div", { class: "line " + seg.speaker, "data-start": seg.start, "data-end": seg.end, onclick: () => seekTo(seg.start) }, [
          el("span", { class: "time", text: fmtTime(seg.start) }),
          el("span", { class: "who", text: seg.speaker === "me" ? "Me" : "Them" }),
          el("span", { class: "text", text: seg.text }),
        ])
      );
    }
  } else if (transcript) {
    area.append(el("p", { class: "muted", text: "Nothing was heard in this recording." }));
  } else if (meeting.status === "recorded" || meeting.status === "error") {
    area.append(el("p", { class: "muted", text: "Not transcribed yet." }));
  }

  setupPlayer(mic, system, meeting.duration_secs, video, video_offset);
}

function renderProgress() {
  const job = state.job;
  const show = job && state.detail && job.id === state.detail.meeting.id;
  $("progress").hidden = !show;
  if (show) {
    $("progress-fill").style.width = job.percent + "%";
    $("progress-label").textContent = `${job.label} · ${Math.round(job.percent)}%`;
  }
}

listen("job", ({ payload }) => {
  state.job = payload;
  renderProgress();
});

$("title").addEventListener("change", () => invoke("update_meeting", { id: state.selected, changes: { title: $("title").value } }));
$("title").addEventListener("keydown", (e) => { if (e.key === "Enter") $("title").blur(); });
$("source-app").addEventListener("change", () => invoke("update_meeting", { id: state.selected, changes: { source_app: $("source-app").value } }));

$("summarize").addEventListener("click", () => invoke("summarize_meeting", { id: state.selected }).catch((e) => showError(String(e))));
$("transcribe").addEventListener("click", () =>
  invoke("transcribe_meeting", { id: state.selected, language: $("language").value }).catch((e) => showError(String(e)))
);
$("language").addEventListener("change", () => {
  if (state.detail && !state.detail.transcript) {
    invoke("transcribe_meeting", { id: state.selected, language: $("language").value }).catch((e) => showError(String(e)));
  }
});
$("reveal").addEventListener("click", () => invoke("reveal_meeting", { id: state.selected }).catch((e) => showError(String(e))));
$("copy-summary").addEventListener("click", async () => {
  const { summary, meeting } = state.detail;
  const lines = [`# ${meeting.title}`, "", summary.overview];
  const section = (h, items, f = (x) => x) => { if (items && items.length) lines.push("", `## ${h}`, ...items.map((i) => `- ${f(i)}`)); };
  section("Key points", summary.key_points);
  section("Decisions", summary.decisions);
  section("Action items", summary.action_items, (a) => `[ ] ${a.text} (${a.owner})`);
  section("Open questions", summary.open_questions);
  await invoke("copy_text", { text: lines.join("\n") });
  $("copy-summary").textContent = "Copied";
  setTimeout(() => ($("copy-summary").textContent = "Copy summary"), 1500);
});
$("delete").addEventListener("click", async () => {
  if (!confirm(`Move "${state.detail.meeting.title}" to the Trash?`)) return;
  try {
    await invoke("delete_meeting", { id: state.selected });
    state.selected = null;
    showEmpty();
    await refreshList();
  } catch (error) {
    showError(String(error));
  }
});

// ---------- playback: both tracks play together, so you hear the whole call ----------

const player = { tracks: [], duration: 0, timer: null };

function stopPlayer() {
  for (const a of player.tracks) { a.pause(); a.src = ""; }
  $("video").pause();
  player.tracks = [];
  clearInterval(player.timer);
  $("player").hidden = true;
}

function setupPlayer(mic, system, duration, video, videoOffset) {
  stopPlayer();
  // The video is muted and follows the audio tracks, which carry the sound.
  const v = $("video");
  player.video = video ? v : null;
  player.videoOffset = videoOffset || 0;
  v.hidden = !video;
  if (video) {
    if (v.dataset.path !== video) {
      v.src = convertFileSrc(video);
      v.dataset.path = video;
    }
    v.currentTime = 0;
  } else {
    v.removeAttribute("src");
    delete v.dataset.path;
    v.load();
  }
  player.tracks = [mic, system].filter(Boolean).map((path) => {
    const audio = new Audio(convertFileSrc(path));
    audio.preload = "metadata";
    return audio;
  });
  if (player.tracks.length === 0) return;
  player.duration = duration || 0;
  player.tracks[0].addEventListener("loadedmetadata", () => {
    if (!player.duration && isFinite(player.tracks[0].duration)) player.duration = player.tracks[0].duration;
  });
  player.tracks[0].addEventListener("ended", () => { $("play").textContent = "▶"; });
  $("player").hidden = false;
  $("play").textContent = "▶";
  $("seek").value = 0;
  $("player-time").textContent = `00:00 / ${fmtTime(player.duration)}`;
  player.timer = setInterval(updatePlayerUi, 250);
}

function updatePlayerUi() {
  if (!player.tracks.length) return;
  const t = player.tracks[0].currentTime;
  if (player.duration) $("seek").value = (t / player.duration) * 100;
  $("player-time").textContent = `${fmtTime(t)} / ${fmtTime(player.duration)}`;
  syncVideo(t);
  let current = null;
  for (const line of $("transcript").querySelectorAll(".line")) {
    const hit = t >= +line.dataset.start && t < +line.dataset.end + 0.5;
    line.classList.toggle("current", hit);
    if (hit && !current) current = line;
  }
}

function seekTo(seconds) {
  for (const a of player.tracks) a.currentTime = seconds;
  syncVideo(seconds, true);
  play();
}

// Keeps the video at the audio's position; it's blank until the call window appeared.
function syncVideo(t, force) {
  const v = player.video;
  if (!v) return;
  const target = t - player.videoOffset;
  const playing = player.tracks.length && !player.tracks[0].paused;
  if (target < 0 || (isFinite(v.duration) && target > v.duration)) {
    if (!v.paused) v.pause();
    return;
  }
  if (force || Math.abs(v.currentTime - target) > 0.4) v.currentTime = target;
  if (playing && v.paused) v.play().catch(() => {});
  if (!playing && !v.paused) v.pause();
}

function play() {
  for (const a of player.tracks) a.play().catch(() => {});
  $("play").textContent = "⏸";
  if (player.tracks.length) syncVideo(player.tracks[0].currentTime, true);
}

$("play").addEventListener("click", () => {
  if (!player.tracks.length) return;
  if (player.tracks[0].paused) play();
  else {
    for (const a of player.tracks) a.pause();
    if (player.video) player.video.pause();
    $("play").textContent = "▶";
  }
});
$("video").addEventListener("click", () => $("play").click());
$("seek").addEventListener("input", () => {
  const t = ($("seek").value / 100) * player.duration;
  for (const a of player.tracks) a.currentTime = t;
  syncVideo(t, true);
});

// ---------- settings ----------

function vocabToText(vocab) {
  return vocab.map((v) => (v.variants.length ? `${v.term} = ${v.variants.join(", ")}` : v.term)).join("\n");
}

function textToVocab(text) {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const [term, rest = ""] = line.split("=").map((s) => s.trim());
      return { term, variants: rest.split(",").map((s) => s.trim()).filter(Boolean) };
    })
    .filter((v) => v.term);
}

async function openSettings() {
  state.settings = await invoke("get_settings");
  const s = state.settings;
  $("s-shortcut").value = s.shortcut;
  $("s-video").checked = s.record_video;
  $("s-language").value = s.default_language;
  $("s-model").value = s.hebrew_model;
  $("s-when").value = s.transcribe_when;
  $("s-vocab").value = vocabToText(s.vocabulary);
  $("s-auto").checked = s.auto_summarize;
  $("s-claude-model").value = s.claude_model;
  $("s-key").value = "";
  $("settings-message").textContent = "";
  await renderModels();
  const status = await invoke("app_status");
  $("key-status").textContent = status.has_api_key
    ? "A key is saved in the Keychain. Paste a new one to replace it, or type \"remove\" to delete it."
    : "No key yet. Keys are stored in the macOS Keychain, never in a file.";
  $("about").textContent = `macOS ${status.macos || "?"} · ${status.arch === "x86_64" ? "Intel" : status.arch}`;
  $("empty").hidden = true;
  $("detail").hidden = true;
  $("settings").hidden = false;
}

async function renderModels() {
  const models = await invoke("model_status");
  const box = $("models");
  box.replaceChildren();
  for (const m of models) {
    box.append(
      el("div", {}, [
        el("span", { text: `${m.label}: ${m.installed ? "installed" : "not downloaded yet"}` }),
        m.installed ? null : el("button", { text: "Download now", onclick: async (e) => {
          e.target.disabled = true;
          try {
            await invoke("download_model", { key: m.key });
            await renderModels();
          } catch (error) {
            $("settings-message").textContent = String(error);
            e.target.disabled = false;
          }
        } }),
      ])
    );
  }
}

$("open-settings").addEventListener("click", openSettings);
$("close-settings").addEventListener("click", () => {
  $("settings").hidden = true;
  if (state.selected) renderDetail(); else showEmpty();
});
$("save-settings").addEventListener("click", async () => {
  const s = {
    ...state.settings,
    shortcut: $("s-shortcut").value.trim(),
    record_video: $("s-video").checked,
    default_language: $("s-language").value,
    hebrew_model: $("s-model").value,
    transcribe_when: $("s-when").value,
    vocabulary: textToVocab($("s-vocab").value),
    auto_summarize: $("s-auto").checked,
    claude_model: $("s-claude-model").value.trim() || state.settings.claude_model,
  };
  try {
    const key = $("s-key").value.trim();
    if (key) await invoke("set_api_key", { key: key.toLowerCase() === "remove" ? "" : key });
    await invoke("save_settings", { settings: s });
    $("settings-message").textContent = "Saved.";
    $("s-key").value = "";
    $("shortcut-hint").textContent = s.shortcut;
  } catch (error) {
    $("settings-message").textContent = String(error);
  }
});
$("edit-prompt").addEventListener("click", () => invoke("open_summary_prompt").catch((e) => ($("settings-message").textContent = String(e))));
$("open-folder").addEventListener("click", () => invoke("open_library_folder"));

// ---------- startup ----------

(async () => {
  const status = await invoke("app_status");
  const settings = await invoke("get_settings");
  $("shortcut-hint").textContent = settings.shortcut.replace("Super", "⌘").replace("Alt", "⌥").replace("Control", "⌃").replace("Shift", "⇧");
  if (status.can_record) {
    showError(status.can_record);
    $("empty-hint").textContent = status.can_record;
  }
  state.job = status.job;
  if (status.recording) {
    state.recordingSince = status.started_at_ms || Date.now();
    setRecording(true);
  }
  await refreshList();
  showEmpty();
})();
