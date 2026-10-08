const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const button = document.getElementById("record");
const timer = document.getElementById("timer");
const message = document.getElementById("message");
const meters = { mic: document.getElementById("mic"), system: document.getElementById("system") };

let startedAt = null;

function setRecording(recording) {
  button.disabled = false;
  button.classList.toggle("recording", recording);
  button.textContent = recording ? "Stop" : "Record";
  if (!recording) {
    startedAt = null;
    meters.mic.style.width = meters.system.style.width = "0";
  }
}

// Speech sits roughly between 0.01 and 0.3 RMS; show it on a log scale.
function levelToPercent(level) {
  if (level <= 0.001) return 0;
  return Math.min(100, Math.max(0, (Math.log10(level) + 3) / 2.5 * 100));
}

listen("recorder", ({ payload }) => {
  switch (payload.event) {
    case "starting":
      button.disabled = true;
      message.textContent = "Starting…";
      break;
    case "started":
      startedAt = Date.now();
      setRecording(true);
      message.textContent = "Recording. Both bars should move during a call.";
      break;
    case "levels":
      meters.mic.style.width = levelToPercent(payload.mic) + "%";
      meters.system.style.width = levelToPercent(payload.system) + "%";
      break;
    case "stopped":
      message.textContent = "Saved.";
      break;
    case "error":
      message.textContent = payload.message;
      break;
    case "idle":
      setRecording(false);
      break;
  }
});

button.addEventListener("click", async () => {
  try {
    if (await invoke("is_recording")) {
      button.disabled = true;
      await invoke("stop_recording");
    } else {
      await invoke("start_recording");
    }
  } catch (error) {
    button.disabled = false;
    message.textContent = String(error);
  }
});

document.getElementById("open").addEventListener("click", () => invoke("open_recordings"));

setInterval(() => {
  if (!startedAt) return (timer.textContent = "00:00");
  const seconds = Math.floor((Date.now() - startedAt) / 1000);
  const hours = Math.floor(seconds / 3600);
  const mm = String(Math.floor(seconds / 60) % 60).padStart(2, "0");
  const ss = String(seconds % 60).padStart(2, "0");
  timer.textContent = hours ? `${hours}:${mm}:${ss}` : `${mm}:${ss}`;
}, 250);

invoke("is_recording").then(setRecording);
