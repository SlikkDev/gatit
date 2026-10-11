// The "Record this call?" pop-up. It closes itself after a while if ignored, and pauses that
// countdown while the pointer is over it.
const { invoke } = window.__TAURI__.core;

const $ = (id) => document.getElementById(id);
const TIMEOUT_MS = 30000;

let remaining = TIMEOUT_MS;
let lastTick = null;
let hovering = false;
let answered = false;

async function answer(record) {
  if (answered) return;
  answered = true;
  $("card").classList.add("leaving");
  await new Promise((resolve) => setTimeout(resolve, 120));
  await invoke("answer_call_prompt", { record });
}

window.loadCall = async () => {
  const call = await invoke("pending_call");
  if (!call) return;
  const what = call.source === "Meet" ? `Call in ${call.app}` : `${call.app} call started`;
  $("title").textContent = what;
  remaining = TIMEOUT_MS;
};

// A timer rather than requestAnimationFrame, which stops while the pop-up is on a Space
// that isn't showing.
function tick() {
  const now = performance.now();
  if (lastTick != null && !hovering) remaining -= now - lastTick;
  lastTick = now;
  $("countdown").style.transform = `scaleX(${Math.max(0, remaining / TIMEOUT_MS)})`;
  if (remaining <= 0) answer(false);
}

$("record").addEventListener("click", () => answer(true));
$("dismiss").addEventListener("click", () => answer(false));
$("card").addEventListener("mouseenter", () => (hovering = true));
$("card").addEventListener("mouseleave", () => (hovering = false));
document.addEventListener("keydown", (e) => {
  if (e.key === "Enter") answer(true);
  if (e.key === "Escape") answer(false);
});

window.loadCall();
tick();
setInterval(tick, 250);
