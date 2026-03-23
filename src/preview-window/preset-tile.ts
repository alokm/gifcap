import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { Window } from "@tauri-apps/api/window";

interface EncodeProgressEvent { progress: number; }
interface EncodeCompleteEvent { gif_base64: string; file_size_bytes: number; }
interface EncodeErrorEvent    { error: string; }

const previewArea = document.getElementById("preview-area")!;
const spinner     = document.getElementById("spinner")!;
const statusText  = document.getElementById("status-text")!;
const btnSave     = document.getElementById("btn-save") as HTMLButtonElement;
const btnDiscard  = document.getElementById("btn-discard") as HTMLButtonElement;

let gifBase64: string | null = null;

await listen<EncodeProgressEvent>("encode-progress", (event) => {
  statusText.textContent = `Encoding… ${Math.round(event.payload.progress * 100)}%`;
});

await listen<EncodeCompleteEvent>("encode-complete", (event) => {
  const { gif_base64, file_size_bytes } = event.payload;
  gifBase64 = gif_base64;

  spinner.remove();
  const img = document.createElement("img");
  img.src = `data:image/gif;base64,${gif_base64}`;
  previewArea.appendChild(img);

  const kb = (file_size_bytes / 1024).toFixed(1);
  statusText.textContent = `${kb} KB`;
  btnSave.disabled = false;
});

await listen<EncodeErrorEvent>("encode-error", (event) => {
  spinner.remove();
  previewArea.innerHTML =
    `<span style="color:#ff3b30;font-size:14px;">${event.payload.error}</span>`;
  statusText.textContent = "Encode failed";
});

btnSave.addEventListener("click", async () => {
  if (!gifBase64) {
    alert("gifBase64 is null — encode result was not received");
    return;
  }
  const now = new Date();
  const ts = now.getFullYear().toString()
    + String(now.getMonth() + 1).padStart(2, "0")
    + String(now.getDate()).padStart(2, "0")
    + "-"
    + String(now.getHours()).padStart(2, "0")
    + String(now.getMinutes()).padStart(2, "0")
    + String(now.getSeconds()).padStart(2, "0");

  try {
    // Hide the always-on-top capture window so the native save dialog
    // can appear and be interacted with freely; restore in finally.
    const captureWin = await Window.getByLabel("capture");
    await captureWin?.hide();
    let path: string | null = null;
    try {
      path = await saveDialog({
        defaultPath: `gif-${ts}.gif`,
        filters: [{ name: "GIF", extensions: ["gif"] }],
      });
    } finally {
      await captureWin?.show();
    }
    if (path) await invoke("save_gif", { gifBase64, path });
  } catch (err) {
    alert(`Save failed: ${err}`);
  }
});

btnDiscard.addEventListener("click", async () => {
  await invoke("discard_recording");
  window.close();
});

invoke("start_preset_encode");
