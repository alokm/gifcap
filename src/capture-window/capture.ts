import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";

const appWindow = getCurrentWindow();

let isRecording = false;
let isPaused = false;
let frameCount = 0;
let recordingStartMs = 0;
let timerInterval: ReturnType<typeof setInterval> | null = null;

const captureFrame  = document.getElementById("capture-frame")!;
const controlStrip  = document.getElementById("control-strip")!;
const btnRecord     = document.getElementById("btn-record")   as HTMLButtonElement;
const btnPause      = document.getElementById("btn-pause")    as HTMLButtonElement;
const btnStop       = document.getElementById("btn-stop")     as HTMLButtonElement;
const btnCollapse   = document.getElementById("btn-collapse") as HTMLButtonElement;
const fpsSelect     = document.getElementById("fps-select")   as HTMLSelectElement;
const dimensionsEl  = document.getElementById("dimensions")!;
const timerEl       = document.getElementById("timer")!;
const frameCounterEl = document.getElementById("frame-counter")!;

async function updateDimensions() {
  const size = await appWindow.innerSize();
  dimensionsEl.textContent = `${size.width} × ${size.height}`;
}
updateDimensions();
appWindow.onResized(() => updateDimensions());

btnRecord.addEventListener("click", async () => {
  // SCStreamConfiguration.sourceRect uses logical points, not physical pixels.
  // Convert here so the backend receives coordinates it can pass directly to SCKit.
  const scale = await appWindow.scaleFactor();
  const pos   = (await appWindow.innerPosition()).toLogical(scale);
  const size  = (await appWindow.innerSize()).toLogical(scale);
  const fps   = parseFloat(fpsSelect.value);

  try {
    await invoke("start_recording", {
      region: {
        x: Math.round(pos.x),
        y: Math.round(pos.y),
        width:  Math.round(size.width),
        height: Math.round(size.height),
      },
      fps,
    });
  } catch (err) {
    alert(`Failed to start recording: ${err}`);
    return;
  }

  isRecording = true;
  frameCount  = 0;
  recordingStartMs = Date.now();

  captureFrame.classList.add("recording");
  btnRecord.disabled = true;
  btnPause.disabled  = false;
  btnStop.disabled   = false;

  timerInterval = setInterval(updateTimer, 500);
});

btnPause.addEventListener("click", async () => {
  if (!isPaused) {
    await invoke("pause_recording");
    isPaused = true;
    btnPause.textContent = "Resume";
    captureFrame.classList.remove("recording");
    captureFrame.classList.add("paused");
  } else {
    await invoke("resume_recording");
    isPaused = false;
    btnPause.textContent = "Pause";
    captureFrame.classList.remove("paused");
    captureFrame.classList.add("recording");
  }
});

btnStop.addEventListener("click", async () => {
  await invoke("stop_recording");

  isRecording = false;
  isPaused    = false;
  captureFrame.classList.remove("recording", "paused");
  btnRecord.disabled = false;
  btnPause.disabled  = true;
  btnStop.disabled   = true;
  btnPause.textContent = "Pause";

  if (timerInterval) clearInterval(timerInterval);

  await invoke("encode_preset");
});

btnCollapse.addEventListener("click", () => {
  controlStrip.classList.toggle("collapsed");
  btnCollapse.textContent = controlStrip.classList.contains("collapsed") ? "▼" : "▲";
});

function updateTimer() {
  const elapsed  = Date.now() - recordingStartMs;
  const totalSec = Math.floor(elapsed / 1000);
  const min      = Math.floor(totalSec / 60);
  const sec      = totalSec % 60;
  timerEl.textContent = `${min}:${sec.toString().padStart(2, "0")}`;
}

listen<number>("frame-captured", (event) => {
  frameCount = event.payload;
  frameCounterEl.textContent = `${frameCount} frames`;
});
