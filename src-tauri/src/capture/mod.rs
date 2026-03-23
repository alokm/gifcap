use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct RawFrame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Timestamp in milliseconds from recording start
    pub timestamp_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    #[error("Capture failed: {0}")]
    CaptureFailed(String),
    #[error("Not initialized")]
    NotInitialized,
}

pub trait CaptureBackend: Send + Sync {
    fn request_permission(&self) -> Result<(), CaptureError>;
    fn start(&mut self, region: Rect, fps: f64) -> Result<(), CaptureError>;
    fn pause(&mut self);
    fn resume(&mut self);
    fn stop(self: Box<Self>) -> Vec<RawFrame>;
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::MacOSCaptureBackend as PlatformBackend;
#[cfg(target_os = "windows")]
pub use windows::WindowsCaptureBackend as PlatformBackend;

/// Create a new platform-specific capture backend.
pub fn new_backend() -> Box<dyn CaptureBackend> {
    Box::new(PlatformBackend::new())
}
