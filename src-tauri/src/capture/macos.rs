use super::{CaptureBackend, CaptureError, RawFrame, Rect};
use core_media_rs::cm_time::CMTime;
use core_video_rs::cv_pixel_buffer::lock::LockTrait;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use screencapturekit::{
    shareable_content::SCShareableContent,
    stream::{
        configuration::{pixel_format::PixelFormat, SCStreamConfiguration},
        content_filter::SCContentFilter,
        output_trait::SCStreamOutputTrait,
        output_type::SCStreamOutputType,
        SCStream,
    },
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::Instant,
};

// ── Frame message sent from the capture callback ──────────────────────────────

struct RawFrameMsg {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    received_at: Instant,
}

// ── SCStreamOutputTrait impl ──────────────────────────────────────────────────

struct FrameCollector {
    tx: mpsc::Sender<RawFrameMsg>,
    is_paused: Arc<AtomicBool>,
}

impl SCStreamOutputTrait for FrameCollector {
    fn did_output_sample_buffer(
        &self,
        sample_buffer: core_media_rs::cm_sample_buffer::CMSampleBuffer,
        of_type: SCStreamOutputType,
    ) {
        if !matches!(of_type, SCStreamOutputType::Screen) {
            return;
        }
        if self.is_paused.load(Ordering::Relaxed) {
            return;
        }

        let received_at = Instant::now();

        let pixel_buffer = match sample_buffer.get_pixel_buffer() {
            Ok(pb) => pb,
            Err(e) => {
                log::debug!("get_pixel_buffer skipped (init frame): {:?}", e);
                return;
            }
        };

        let width = pixel_buffer.get_width();
        let bytes_per_row = pixel_buffer.get_bytes_per_row() as usize;

        let lock = match pixel_buffer.lock() {
            Ok(l) => l,
            Err(e) => {
                log::warn!("CVPixelBuffer lock failed: {:?}", e);
                return;
            }
        };

        let raw = lock.as_slice(); // BGRA bytes, row-padded to bytes_per_row

        // bytes_per_row may be larger than width*4 due to alignment padding.
        let stride = if bytes_per_row >= width as usize * 4 {
            bytes_per_row
        } else {
            width as usize * 4
        };
        let actual_height = if stride > 0 {
            (raw.len() / stride).min(pixel_buffer.get_height() as usize)
        } else {
            pixel_buffer.get_height() as usize
        };

        // Convert BGRA → RGBA, stripping row-stride padding.
        let mut rgba = Vec::with_capacity(width as usize * actual_height * 4);
        for row in 0..actual_height {
            let row_start = row * stride;
            let row_end = row_start + width as usize * 4;
            if row_end > raw.len() {
                break;
            }
            for bgra in raw[row_start..row_end].chunks_exact(4) {
                rgba.push(bgra[2]); // R
                rgba.push(bgra[1]); // G
                rgba.push(bgra[0]); // B
                rgba.push(bgra[3]); // A
            }
        }

        let _ = self.tx.send(RawFrameMsg {
            rgba,
            width,
            height: actual_height as u32,
            received_at,
        });
    }
}

// ── Backend ───────────────────────────────────────────────────────────────────

pub struct MacOSCaptureBackend {
    start_time: Option<Instant>,
    paused_at: Option<Instant>,
    total_paused_ms: u64,
    stream: Option<SCStream>,
    frame_rx: Option<mpsc::Receiver<RawFrameMsg>>,
    is_paused: Arc<AtomicBool>,
}

// SAFETY: MacOSCaptureBackend is only ever accessed through the Mutex in
// CaptureState, ensuring exclusive access from one thread at a time.
// SCStream's underlying ObjC object is reference-counted and safe to transfer
// between threads.
unsafe impl Send for MacOSCaptureBackend {}
unsafe impl Sync for MacOSCaptureBackend {}

impl MacOSCaptureBackend {
    pub fn new() -> Self {
        Self {
            start_time: None,
            paused_at: None,
            total_paused_ms: 0,
            stream: None,
            frame_rx: None,
            is_paused: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl CaptureBackend for MacOSCaptureBackend {
    fn request_permission(&self) -> Result<(), CaptureError> {
        // SCShareableContent::get() triggers the TCC prompt on first run and
        // returns an error if the user has not yet granted permission.
        // On macOS 15+ the combined "Screen and System Audio Recording"
        // permission covers this; capturesAudio is set to false in the stream
        // config to keep the prompt screen-only.
        SCShareableContent::get().map_err(|e| {
            CaptureError::PermissionDenied(format!(
                "Screen recording permission not granted. \
                 Open System Settings → Privacy & Security → Screen & System Audio Recording \
                 and enable GifCap, then try again. (detail: {:?})",
                e
            ))
        })?;
        log::info!("macOS: screen recording permission granted");
        Ok(())
    }

    fn start(&mut self, region: Rect, fps: f64) -> Result<(), CaptureError> {
        let our_pid = std::process::id() as i32;

        let content = SCShareableContent::get().map_err(|e| {
            CaptureError::CaptureFailed(format!("SCShareableContent::get: {:?}", e))
        })?;

        let mut displays = content.displays();
        if displays.is_empty() {
            return Err(CaptureError::CaptureFailed("no displays found".into()));
        }
        let display = displays.remove(0);

        // Exclude our own windows by matching process ID.
        let all_windows = content.windows();
        let our_windows: Vec<_> = all_windows
            .iter()
            .filter(|w| w.owning_application().process_id() == our_pid)
            .collect();

        log::info!(
            "macOS: excluding {} own window(s) from capture (PID {})",
            our_windows.len(),
            our_pid
        );

        let filter =
            SCContentFilter::new().with_display_excluding_windows(&display, &our_windows);

        // Use the region dimensions; fall back to the full display if zero.
        let cap_width = if region.width > 0 {
            region.width
        } else {
            display.width()
        };
        let cap_height = if region.height > 0 {
            region.height
        } else {
            display.height()
        };

        // CMTime with value=1, timescale=fps → minimum frame interval of 1/fps seconds.
        let frame_interval = CMTime {
            value: 1,
            timescale: fps.max(1.0) as i32,
            flags: 1,
            epoch: 0,
        };

        // Build the base configuration; these failures are fatal.
        let base_config = SCStreamConfiguration::new()
            .set_width(cap_width)
            .map_err(|e| CaptureError::CaptureFailed(format!("set_width: {:?}", e)))?
            .set_height(cap_height)
            .map_err(|e| CaptureError::CaptureFailed(format!("set_height: {:?}", e)))?
            // BGRA must be explicit — default YCbCr 4:2:0 gives wrong frame dimensions.
            .set_pixel_format(PixelFormat::BGRA)
            .map_err(|e| CaptureError::CaptureFailed(format!("set_pixel_format: {:?}", e)))?
            .set_minimum_frame_interval(&frame_interval)
            .map_err(|e| {
                CaptureError::CaptureFailed(format!("set_minimum_frame_interval: {:?}", e))
            })?
            // Explicitly opt out of audio — macOS 15 shows a combined
            // "Screen and System Audio Recording" prompt for any SCStream
            // unless audio capture is disabled at the configuration level.
            .set_captures_audio(false)
            .map_err(|e| CaptureError::CaptureFailed(format!("set_captures_audio: {:?}", e)))?;

        // Apply sourceRect as a non-fatal crop. set_source_rect consumes the config,
        // so if it fails we rebuild the base config without it (full display scaled
        // to width×height) rather than failing the entire recording.
        let stream_config = match base_config.set_source_rect(CGRect {
            origin: CGPoint {
                x: region.x as f64,
                y: region.y as f64,
            },
            size: CGSize {
                width: cap_width as f64,
                height: cap_height as f64,
            },
        }) {
            Ok(cfg) => {
                log::info!(
                    "macOS: sourceRect ({},{}) {}×{} applied",
                    region.x, region.y, cap_width, cap_height
                );
                cfg
            }
            Err(e) => {
                log::warn!(
                    "macOS: set_source_rect failed — capturing full display: {:?}", e
                );
                SCStreamConfiguration::new()
                    .set_width(cap_width)
                    .map_err(|e| CaptureError::CaptureFailed(format!("set_width: {:?}", e)))?
                    .set_height(cap_height)
                    .map_err(|e| CaptureError::CaptureFailed(format!("set_height: {:?}", e)))?
                    .set_pixel_format(PixelFormat::BGRA)
                    .map_err(|e| CaptureError::CaptureFailed(format!("set_pixel_format: {:?}", e)))?
                    .set_minimum_frame_interval(&frame_interval)
                    .map_err(|e| CaptureError::CaptureFailed(format!("set_minimum_frame_interval: {:?}", e)))?
                    .set_captures_audio(false)
                    .map_err(|e| CaptureError::CaptureFailed(format!("set_captures_audio: {:?}", e)))?
            }
        };

        let (frame_tx, frame_rx) = mpsc::channel::<RawFrameMsg>();

        // Reset pause state for the new recording.
        self.is_paused.store(false, Ordering::Relaxed);
        let is_paused = Arc::clone(&self.is_paused);

        let mut stream = SCStream::new(&filter, &stream_config);
        stream.add_output_handler(
            FrameCollector {
                tx: frame_tx,
                is_paused,
            },
            SCStreamOutputType::Screen,
        );

        stream
            .start_capture()
            .map_err(|e| CaptureError::CaptureFailed(format!("start_capture: {:?}", e)))?;

        log::info!(
            "macOS: SCStream started — {}×{} at {} FPS",
            cap_width,
            cap_height,
            fps
        );

        self.stream = Some(stream);
        self.frame_rx = Some(frame_rx);
        self.start_time = Some(Instant::now());
        self.paused_at = None;
        self.total_paused_ms = 0;

        Ok(())
    }

    fn pause(&mut self) {
        if self.paused_at.is_none() {
            self.is_paused.store(true, Ordering::Relaxed);
            self.paused_at = Some(Instant::now());
            log::info!("macOS: capture paused");
        }
    }

    fn resume(&mut self) {
        if let Some(paused_at) = self.paused_at.take() {
            self.total_paused_ms += paused_at.elapsed().as_millis() as u64;
            self.is_paused.store(false, Ordering::Relaxed);
            log::info!("macOS: capture resumed");
        }
    }

    fn stop(mut self: Box<Self>) -> Vec<RawFrame> {
        let start_time = self.start_time.unwrap_or_else(Instant::now);

        // 1. Signal the output callback to stop accepting new frames immediately.
        //    Any callback already past this check will complete before we proceed.
        self.is_paused.store(true, Ordering::Release);

        // 2. Give in-flight callbacks a window to exit did_output_sample_buffer.
        //    At 10 fps one frame budget is ~100 ms; 50 ms is enough for any callback
        //    already running to finish and see the flag on its next iteration.
        std::thread::sleep(std::time::Duration::from_millis(50));

        // 3. Stop the stream. Use `ref` so the stream stays alive in `self` until
        //    the end of this function — keeping FrameCollector (and its Sender)
        //    valid for the entire drain below.
        if let Some(ref stream) = self.stream {
            if let Err(e) = stream.stop_capture() {
                log::warn!("macOS: stop_capture error: {:?}", e);
            }
        }

        // 4. Drain all frames that arrived before the flag was set.
        let mut frames = Vec::new();
        if let Some(rx) = self.frame_rx.take() {
            while let Ok(msg) = rx.try_recv() {
                let ts_ms = msg
                    .received_at
                    .duration_since(start_time)
                    .as_millis() as u64;
                frames.push(RawFrame {
                    rgba: msg.rgba,
                    width: msg.width,
                    height: msg.height,
                    timestamp_ms: ts_ms,
                });
            }
        }

        log::info!("macOS: capture stopped — {} frames collected", frames.len());
        frames
        // `self` drops here: stream (and FrameCollector) are freed only after
        // the drain is complete and no callbacks can reach the Sender.
    }
}
