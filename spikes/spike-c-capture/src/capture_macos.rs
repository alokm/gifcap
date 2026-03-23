/// macOS capture backend for Spike C — corrected for screencapturekit v0.3.6.
///
/// SPIKE C FINDING: screencapturekit v0.3.6 DOES expose SCStream for
/// continuous capture. The original file used wrong module paths from an
/// older/different API design. Correct paths documented below.
///
/// SCStream is the production path — no CGWindowListCreateImage fallback needed
/// for the capture mechanism itself.

use crate::{CaptureConfig, CapturedFrame};
use core_media_rs::cm_time::CMTime;
use core_video_rs::cv_pixel_buffer::lock::LockTrait;
use screencapturekit::{
    shareable_content::SCShareableContent,
    stream::{
        SCStream,
        configuration::{pixel_format::PixelFormat, SCStreamConfiguration},
        content_filter::SCContentFilter,
        output_trait::SCStreamOutputTrait,
        output_type::SCStreamOutputType,
    },
};
use std::{
    sync::mpsc,
    time::Instant,
};

pub fn capture(config: CaptureConfig) -> Result<Vec<CapturedFrame>, String> {
    capture_via_scstream(&config)
}

fn capture_via_scstream(config: &CaptureConfig) -> Result<Vec<CapturedFrame>, String> {
    // ── 1. Get the primary display ────────────────────────────────────────────
    let mut displays = SCShareableContent::get()
        .map_err(|e| format!("SCShareableContent::get failed: {:?}", e))?
        .displays();

    let display = if displays.is_empty() {
        return Err("No displays found".into());
    } else {
        displays.remove(0)
    };

    // ── 2. Content filter: capture entire display, exclude no windows ─────────
    // For Spike A we will pass GifCap's own window ID here to test self-exclusion.
    // For this spike we capture everything to validate frame delivery.
    let filter = SCContentFilter::new().with_display_excluding_windows(&display, &[]);

    // ── 3. Stream configuration ───────────────────────────────────────────────
    // CMTime for minimum_frame_interval: value=1, timescale=fps means 1/fps seconds.
    let frame_interval = CMTime {
        value: 1,
        timescale: config.fps as i32,
        flags: 1,
        epoch: 0,
    };

    let stream_config = SCStreamConfiguration::new()
        .set_width(config.width)
        .map_err(|e| format!("set_width failed: {:?}", e))?
        .set_height(config.height)
        .map_err(|e| format!("set_height failed: {:?}", e))?
        // Explicitly request packed BGRA — without this SCStream defaults to
        // YCbCr 4:2:0 planar, where the luma plane has W×H bytes (1 byte/pixel)
        // instead of W×H×4, producing garbled frame dimensions.
        .set_pixel_format(PixelFormat::BGRA)
        .map_err(|e| format!("set_pixel_format failed: {:?}", e))?
        .set_minimum_frame_interval(&frame_interval)
        .map_err(|e| format!("set_minimum_frame_interval failed: {:?}", e))?;

    // ── 4. Shared frame buffer + output handler ───────────────────────────────
    let (frame_tx, frame_rx) = mpsc::channel::<CapturedFrame>();

    struct FrameCollector {
        tx: mpsc::Sender<CapturedFrame>,
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

            let received_at = Instant::now();

            // Extract pixel data from CMSampleBuffer → CVPixelBuffer → locked bytes.
            // SCStream delivers frames in kCVPixelFormatType_32BGRA by default.
            // CouldNotGetDataBuffer on the first few callbacks is normal while
            // SCStream initialises — downgrade to debug to avoid log noise.
            let pixel_buffer = match sample_buffer.get_pixel_buffer() {
                Ok(pb) => pb,
                Err(e) => {
                    log::debug!("get_pixel_buffer skipped (likely empty init frame): {:?}", e);
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

            log::debug!(
                "frame buf: width={} height={} bytes_per_row={} raw.len()={}",
                width,
                pixel_buffer.get_height(),
                bytes_per_row,
                raw.len()
            );

            // Derive actual row count from the raw buffer size.
            // CVPixelBuffer stride (bytes_per_row) may be less than width×4
            // on some display configurations — use it only if it makes sense.
            // Fall back to width×4 as the stride if bytes_per_row < width×4.
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
                let row_end = row_start + (width as usize * 4);
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

            let _ = self.tx.send(CapturedFrame {
                rgba,
                width,
                height: actual_height as u32,
                received_at,
            });
        }
    }

    // ── 5. Create stream, add output handler, start ───────────────────────────
    let mut stream = SCStream::new(&filter, &stream_config);
    stream.add_output_handler(FrameCollector { tx: frame_tx }, SCStreamOutputType::Screen);

    stream
        .start_capture()
        .map_err(|e| format!("start_capture failed: {:?}", e))?;

    log::info!(
        "SCStream started — capturing {}×{} at {} FPS for {}s",
        config.width,
        config.height,
        config.fps,
        config.duration.as_secs()
    );

    // ── 6. Collect frames while recording ────────────────────────────────────
    let deadline = Instant::now() + config.duration;
    let mut frames = Vec::new();

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match frame_rx.recv_timeout(remaining) {
            Ok(frame) => frames.push(frame),
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // ── 7. Stop ───────────────────────────────────────────────────────────────
    stream
        .stop_capture()
        .map_err(|e| format!("stop_capture failed: {:?}", e))?;

    log::info!("SCStream stopped — {} frames collected", frames.len());

    Ok(frames)
}
