use crate::{
    commands::capture::CaptureState,
    compression::worker::encode_fast,
};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

#[tauri::command]
pub async fn encode_preset(
    app: AppHandle,
    state: State<'_, CaptureState>,
) -> Result<(), String> {
    let frames = state.frames.lock().unwrap();
    if frames.is_empty() {
        return Err("No frames to encode".into());
    }
    drop(frames);

    if app.get_webview_window("preview").is_none() {
        tauri::WebviewWindowBuilder::new(
            &app,
            "preview",
            tauri::WebviewUrl::App("preview-window/index.html".into()),
        )
        .title("GifCap — Preview")
        .inner_size(700.0, 500.0)
        .resizable(true)
        .build()
        .map_err(|e| format!("failed to open preview window: {e}"))?;
    }

    Ok(())
}

#[tauri::command]
pub async fn start_preset_encode(
    app: AppHandle,
    state: State<'_, CaptureState>,
) -> Result<(), String> {
    let frames = state.frames.lock().unwrap().clone();
    let fps = *state.recording_fps.lock().unwrap();

    if frames.is_empty() {
        return Err("No frames to encode".into());
    }

    encode_fast(app, Arc::new(frames), fps);
    Ok(())
}

#[tauri::command]
pub async fn save_gif(gif_base64: String, path: String) -> Result<(), String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let bytes = STANDARD.decode(&gif_base64).map_err(|e| e.to_string())?;
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn discard_recording(state: State<'_, CaptureState>) -> Result<(), String> {
    let mut frames = state.frames.lock().unwrap();
    frames.clear();
    frames.shrink_to_fit();
    Ok(())
}
