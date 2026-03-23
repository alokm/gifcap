use super::{CaptureBackend, CaptureError, RawFrame, Rect};
use std::time::Instant;

pub struct WindowsCaptureBackend {
    frames: Vec<RawFrame>,
    start_time: Option<Instant>,
    paused_at: Option<Instant>,
    total_paused_ms: u64,
    is_recording: bool,
}

impl WindowsCaptureBackend {
    pub fn new() -> Self {
        Self {
            frames: Vec::new(),
            start_time: None,
            paused_at: None,
            total_paused_ms: 0,
            is_recording: false,
        }
    }
}

impl CaptureBackend for WindowsCaptureBackend {
    fn request_permission(&self) -> Result<(), CaptureError> {
        // Windows Graphics Capture does not require a separate permission dialog
        // on Windows 10 1903+; the OS shows a yellow border around the capture.
        log::info!("Windows: no explicit permission required for Graphics Capture API");
        Ok(())
    }

    fn start(&mut self, region: Rect, fps: f64) -> Result<(), CaptureError> {
        // TODO Stage 1: Initialize Windows.Graphics.Capture.GraphicsCaptureItem
        log::info!(
            "Windows: starting capture region={:?} fps={fps}",
            region
        );
        self.frames.clear();
        self.start_time = Some(Instant::now());
        self.total_paused_ms = 0;
        self.is_recording = true;
        Ok(())
    }

    fn pause(&mut self) {
        if self.is_recording && self.paused_at.is_none() {
            self.paused_at = Some(Instant::now());
            log::info!("Windows: capture paused");
        }
    }

    fn resume(&mut self) {
        if let Some(paused_at) = self.paused_at.take() {
            self.total_paused_ms += paused_at.elapsed().as_millis() as u64;
            log::info!("Windows: capture resumed");
        }
    }

    fn stop(mut self: Box<Self>) -> Vec<RawFrame> {
        self.is_recording = false;
        log::info!("Windows: stopping capture, {} frames collected", self.frames.len());
        std::mem::take(&mut self.frames)
    }
}
