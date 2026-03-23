use crate::{
    capture::RawFrame,
    compression::{encode_gif, CompressionProfile},
    compression::quantize::Quantizer,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncodeProgressEvent {
    pub progress: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncodeCompleteEvent {
    pub gif_base64: String,
    pub file_size_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncodeErrorEvent {
    pub error: String,
}

pub fn encode_fast(app: AppHandle, frames: Arc<Vec<RawFrame>>, recording_fps: f64) {
    let profile = CompressionProfile {
        name: "Fast".to_string(),
        quantizer: Quantizer::Fast,
        colors: 256,
        dither: false,
        scale_width: None,
        scale_height: None,
        fps_override: Some(recording_fps),
    };

    std::thread::spawn(move || {
        let (tx, mut rx) = mpsc::channel::<f32>(32);

        let app_progress = app.clone();
        std::thread::spawn(move || {
            while let Some(p) = rx.blocking_recv() {
                let _ = app_progress.emit("encode-progress", EncodeProgressEvent { progress: p });
            }
        });

        match encode_gif(&frames, &profile, tx) {
            Ok(bytes) => {
                use base64::{engine::general_purpose::STANDARD, Engine};
                let _ = app.emit(
                    "encode-complete",
                    EncodeCompleteEvent {
                        gif_base64: STANDARD.encode(&bytes),
                        file_size_bytes: bytes.len(),
                    },
                );
            }
            Err(e) => {
                let _ = app.emit("encode-error", EncodeErrorEvent { error: e.to_string() });
            }
        }
    });
}
