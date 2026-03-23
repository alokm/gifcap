/// Spike C — Platform Capture Validation
///
/// Validates that the OS screen capture API delivers frames reliably at 10 FPS.
///
/// Usage:
///   cargo run -p spike-c-capture
///
/// Output:
///   ~/Desktop/spike-c/frame-001.png   — first captured frame
///   ~/Desktop/spike-c/frame-NNN.png   — last captured frame
///   Terminal report: frame count, mean interval, max jitter, drop %
///
/// Pass criteria (from implementation plan):
///   - ≥ 48 / 50 expected frames received (≤ 4% drop at 10 FPS / 5 sec)
///   - Saved PNGs contain real screen content (not black / garbage)
///   - Mean inter-frame jitter < 20 ms
///
/// Fail paths logged by this spike:
///   macOS:  if screencapturekit v0.3 lacks SCStream → fallback to CGWindowListCreateImage
///   Windows: if WGC doesn't deliver to a non-HWND target → evaluate DXGI Desktop Duplication

#[cfg(target_os = "macos")]
mod capture_macos;
#[cfg(target_os = "windows")]
mod capture_windows;

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

// ── shared frame type ────────────────────────────────────────────────────────

/// A single captured frame: raw RGBA pixels + the instant it arrived.
pub struct CapturedFrame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Wall-clock instant this frame was received by the Rust callback.
    pub received_at: Instant,
}

// ── capture config ───────────────────────────────────────────────────────────

pub struct CaptureConfig {
    /// Top-left corner of the capture region, in screen logical pixels.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// How long to capture for.
    pub duration: Duration,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 640,
            height: 480,
            fps: 10.0,
            duration: Duration::from_secs(5),
        }
    }
}

// ── platform dispatch ────────────────────────────────────────────────────────

fn run_capture(config: CaptureConfig) -> Result<Vec<CapturedFrame>, String> {
    #[cfg(target_os = "macos")]
    return capture_macos::capture(config);

    #[cfg(target_os = "windows")]
    return capture_windows::capture(config);

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    Err("Unsupported platform. Spike C targets macOS and Windows only.".into())
}

// ── main ─────────────────────────────────────────────────────────────────────

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = CaptureConfig::default();
    let target_fps = config.fps;
    let duration_secs = config.duration.as_secs_f64();
    let expected_frames = (target_fps * duration_secs).round() as usize;
    let output_dir = dirs_output();

    println!("╔══════════════════════════════════════════════╗");
    println!("║          GifCap — Spike C: Platform Capture  ║");
    println!("╚══════════════════════════════════════════════╝");
    println!();
    println!("  Region:   {}×{} at ({}, {})", config.width, config.height, config.x, config.y);
    println!("  FPS:      {}", config.fps);
    println!("  Duration: {}s", config.duration.as_secs());
    println!("  Expected: {} frames", expected_frames);
    println!("  Output:   {}", output_dir.display());
    println!();

    std::fs::create_dir_all(&output_dir).expect("failed to create output directory");

    println!("▶  Starting capture…");
    let capture_start = Instant::now();

    let frames = match run_capture(config) {
        Ok(f) => f,
        Err(e) => {
            println!();
            println!("✗  CAPTURE FAILED");
            println!("   {}", e);
            println!();
            println!("── Fail path guidance ──────────────────────────────────");
            #[cfg(target_os = "macos")]
            println!("   macOS: check screencapturekit crate version / API surface.");
            #[cfg(target_os = "macos")]
            println!("   Fallback option: CGWindowListCreateImage via objc2 (polling).");
            #[cfg(target_os = "windows")]
            println!("   Windows: verify WGC frame pool delivers to display capture target.");
            #[cfg(target_os = "windows")]
            println!("   Fallback option: DXGI Desktop Duplication API.");
            std::process::exit(1);
        }
    };

    let total_elapsed = capture_start.elapsed();

    // ── save diagnostic PNGs ─────────────────────────────────────────────────
    if !frames.is_empty() {
        save_frame(&frames[0], &output_dir, 1);
        if frames.len() > 1 {
            save_frame(&frames[frames.len() - 1], &output_dir, frames.len());
        }
    }

    // ── timing analysis ──────────────────────────────────────────────────────
    let received = frames.len();
    let drop_count = expected_frames.saturating_sub(received);
    let drop_pct = drop_count as f64 / expected_frames as f64 * 100.0;

    // Inter-frame intervals
    let intervals: Vec<Duration> = frames
        .windows(2)
        .map(|w| w[1].received_at.duration_since(w[0].received_at))
        .collect();

    let target_interval = Duration::from_secs_f64(1.0 / target_fps);

    let mean_interval = if intervals.is_empty() {
        Duration::ZERO
    } else {
        intervals.iter().sum::<Duration>() / intervals.len() as u32
    };

    let max_jitter = intervals
        .iter()
        .map(|d| {
            let diff = if *d > target_interval {
                *d - target_interval
            } else {
                target_interval - *d
            };
            diff
        })
        .max()
        .unwrap_or(Duration::ZERO);

    // ── report ───────────────────────────────────────────────────────────────
    println!("■  Capture complete ({:.1}s elapsed)", total_elapsed.as_secs_f64());
    println!();
    println!("── Results ─────────────────────────────────────────────────────");
    println!(
        "  Frames received:   {} / {} ({:.1}% drop)",
        received, expected_frames, drop_pct
    );
    println!(
        "  Mean interval:     {:.1} ms  (target {:.1} ms)",
        mean_interval.as_secs_f64() * 1000.0,
        target_interval.as_secs_f64() * 1000.0
    );
    println!("  Max jitter:        {:.1} ms", max_jitter.as_secs_f64() * 1000.0);
    println!("  Output PNGs:       {}", output_dir.display());
    println!();

    // ── pass / fail verdict ──────────────────────────────────────────────────
    let pass_frames = received >= (expected_frames * 96 / 100); // ≥ 96% = ≤ 4% drop
    let pass_jitter = max_jitter.as_millis() < 20;
    let pass_content = frames
        .first()
        .map(|f| !is_black_frame(&f.rgba))
        .unwrap_or(false);

    println!("── Verdict ─────────────────────────────────────────────────────");
    verdict("Frame delivery (≥96%)", pass_frames);
    verdict("Max jitter (<20 ms)", pass_jitter);
    verdict("Frame content (not black)", pass_content);

    let all_pass = pass_frames && pass_jitter && pass_content;
    println!();
    if all_pass {
        println!("✓  SPIKE C: PASS — platform capture is viable. Proceed to Spike A.");
    } else {
        println!("✗  SPIKE C: FAIL — see individual failures above.");
        println!("   Review fail path guidance and update architecture before Stage 1.");
        std::process::exit(1);
    }
    println!();
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn dirs_output() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("Desktop").join("spike-c")
}

fn save_frame(frame: &CapturedFrame, dir: &PathBuf, index: usize) {
    let path = dir.join(format!("frame-{:03}.png", index));
    // Derive height from actual buffer size to guard against any dimension
    // mismatch from the capture layer (stride, alignment, partial frames).
    let pixels_per_row = frame.width as usize * 4;
    let actual_height = if pixels_per_row > 0 {
        (frame.rgba.len() / pixels_per_row) as u32
    } else {
        frame.height
    };
    match image::save_buffer(
        &path,
        &frame.rgba,
        frame.width,
        actual_height,
        image::ColorType::Rgba8,
    ) {
        Ok(()) => println!("  Saved: {} ({}×{})", path.display(), frame.width, actual_height),
        Err(e) => println!("  ⚠ Failed to save {}: {}", path.display(), e),
    }
}

/// Returns true if every pixel in an RGBA buffer is very dark (likely a black frame).
fn is_black_frame(rgba: &[u8]) -> bool {
    // Sample every 64th pixel to keep this fast
    rgba.chunks(4 * 64)
        .all(|chunk| chunk[0] < 10 && chunk[1] < 10 && chunk[2] < 10)
}

fn verdict(label: &str, pass: bool) {
    let icon = if pass { "  ✓" } else { "  ✗" };
    println!("{} {}", icon, label);
}
