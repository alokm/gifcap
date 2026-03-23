import { load } from "@tauri-apps/plugin-store";

interface Prefs {
  defaultFps: number;
  autoCollapse: boolean;
  skipPreview: boolean;
}

const DEFAULT_PREFS: Prefs = {
  defaultFps: 10,
  autoCollapse: false,
  skipPreview: false,
};

async function loadPrefs(): Promise<Prefs> {
  const store = await load("prefs.json", { autoSave: true });
  return {
    defaultFps:   (await store.get<number>("defaultFps"))   ?? DEFAULT_PREFS.defaultFps,
    autoCollapse: (await store.get<boolean>("autoCollapse")) ?? DEFAULT_PREFS.autoCollapse,
    skipPreview:  (await store.get<boolean>("skipPreview"))  ?? DEFAULT_PREFS.skipPreview,
  };
}

async function savePrefs(prefs: Prefs) {
  const store = await load("prefs.json", { autoSave: true });
  await store.set("defaultFps",   prefs.defaultFps);
  await store.set("autoCollapse", prefs.autoCollapse);
  await store.set("skipPreview",  prefs.skipPreview);
  await store.save();
}

// Init UI
(async () => {
  const prefs = await loadPrefs();

  const fpsEl          = document.getElementById("pref-fps")           as HTMLSelectElement;
  const autoCollapseEl = document.getElementById("pref-auto-collapse") as HTMLInputElement;
  const skipPreviewEl  = document.getElementById("pref-skip-preview")  as HTMLInputElement;

  fpsEl.value              = String(prefs.defaultFps);
  autoCollapseEl.checked   = prefs.autoCollapse;
  skipPreviewEl.checked    = prefs.skipPreview;

  document.getElementById("btn-save-prefs")?.addEventListener("click", async () => {
    await savePrefs({
      defaultFps:   parseFloat(fpsEl.value),
      autoCollapse: autoCollapseEl.checked,
      skipPreview:  skipPreviewEl.checked,
    });
    window.close();
  });
})();
