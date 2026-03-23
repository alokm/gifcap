use crate::capture::{new_backend, CaptureError, Rect};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

pub struct CaptureState {
    pub backend: Mutex<Option<Box<dyn crate::capture::CaptureBackend>>>,
    pub frames: Mutex<Vec<crate::capture::RawFrame>>,
    pub recording_fps: Mutex<f64>,
}

impl Default for CaptureState {
    fn default() -> Self {
        Self {
            backend: Mutex::new(None),
            frames: Mutex::new(Vec::new()),
            recording_fps: Mutex::new(10.0),
        }
    }
}

#[tauri::command]
pub async fn start_recording(
    state: State<'_, CaptureState>,
    region: Rect,
    fps: f64,
) -> Result<(), String> {
    let mut backend = new_backend();
    backend
        .request_permission()
        .map_err(|e: CaptureError| e.to_string())?;
    backend
        .start(region, fps)
        .map_err(|e: CaptureError| e.to_string())?;

    *state.backend.lock().unwrap() = Some(backend);
    *state.recording_fps.lock().unwrap() = fps;
    Ok(())
}

#[tauri::command]
pub async fn stop_recording(
    app: AppHandle,
    state: State<'_, CaptureState>,
) -> Result<(), String> {
    let backend = state.backend.lock().unwrap().take();
    if let Some(b) = backend {
        let frames = b.stop();
        let count = frames.len();
        *state.frames.lock().unwrap() = frames;
        // Update the frame counter in the capture window.
        let _ = app.emit("frame-captured", count);
        Ok(())
    } else {
        Err("No active recording".into())
    }
}

#[tauri::command]
pub async fn pause_recording(state: State<'_, CaptureState>) -> Result<(), String> {
    if let Some(b) = state.backend.lock().unwrap().as_mut() {
        b.pause();
        Ok(())
    } else {
        Err("No active recording".into())
    }
}

#[tauri::command]
pub async fn resume_recording(state: State<'_, CaptureState>) -> Result<(), String> {
    if let Some(b) = state.backend.lock().unwrap().as_mut() {
        b.resume();
        Ok(())
    } else {
        Err("No active recording".into())
    }
}
