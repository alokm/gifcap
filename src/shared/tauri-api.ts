import { invoke } from "@tauri-apps/api/core";

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface CompressionProfile {
  name: string;
  quantizer: "Fast" | "Balanced" | "HighQuality";
  colors: number;
  dither: boolean;
  scale_width?: number;
  scale_height?: number;
  fps_override?: number;
}

export const api = {
  startRecording:   (region: Rect, fps: number) => invoke<void>("start_recording", { region, fps }),
  stopRecording:    ()                           => invoke<void>("stop_recording"),
  pauseRecording:   ()                           => invoke<void>("pause_recording"),
  resumeRecording:  ()                           => invoke<void>("resume_recording"),
  encodePreset:     ()                           => invoke<void>("encode_preset"),
  encodeCustom:     (profile: CompressionProfile) => invoke<string>("encode_custom", { profile }),
  saveGif:          (gifBase64: string, path: string) => invoke<void>("save_gif", { gifBase64, path }),
  discardRecording: ()                           => invoke<void>("discard_recording"),
};
