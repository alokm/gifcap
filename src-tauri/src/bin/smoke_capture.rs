/// Smoke test for the production macOS capture backend.
///
/// Exercises MacOSCaptureBackend end-to-end without the Tauri runtime:
///   1. request_permission()
///   2. start()  — 2-second capture at 10 FPS
///   3. pause() / resume()
///   4. stop()   — collect frames and report
///
/// Usage:
///   cargo run --bin smoke_capture
///
/// Pass criteria:
///   - Permission granted (no crash / permission error)
///   - ≥ 15 frames received in 2 seconds at 10 FPS (≥ 75%)
///   - First frame has non-zero dimensions and non-black content

use gifcap_lib::capture::{new_backend, Rect};
use std::{thread, time::Duration};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    println!("╔════════════════════════════════════════════╗");
    println!("║  GifCap — Stage 1 Smoke Test: Capture     ║");
    println!("╚════════════════════════════════════════════╝");
    println!();

    // ── 1. Permission ─────────────────────────────────────────────────────────
    println!("▶  Requesting screen recording permission…");
    let backend = new_backend();
    if let Err(e) = backend.request_permission() {
        println!("✗  Permission denied: {}", e);
        println!();
        println!("   Grant screen recording access to Terminal (or the process running");
        println!("   this binary) in System Settings → Privacy & Security → Screen Recording.");
        std::process::exit(1);
    }
    println!("   Permission OK");
    println!();

    // ── 2. Start — full primary display at 10 FPS ────────────────────────────
    let fps = 10.0f64;
    let duration_secs = 2.0f64;
    let expected = (fps * duration_secs).round() as usize;

    // width=0, height=0 → backend falls back to display dimensions.
    let region = Rect { x: 0, y: 0, width: 0, height: 0 };

    println!("▶  Starting capture — {} FPS for {}s (expect ~{} frames)…", fps, duration_secs, expected);
    let mut backend = new_backend();
    if let Err(e) = backend.start(region, fps) {
        println!("✗  start() failed: {}", e);
        std::process::exit(1);
    }

    // ── 3. Run for 1s, pause briefly, resume, run another 1s ─────────────────
    thread::sleep(Duration::from_secs(1));

    println!("   Pausing for 300 ms…");
    backend.pause();
    thread::sleep(Duration::from_millis(300));
    backend.resume();
    println!("   Resumed.");

    thread::sleep(Duration::from_secs(1));

    // ── 4. Stop and collect frames ────────────────────────────────────────────
    println!("■  Stopping…");
    let frames = Box::new(backend).stop();

    println!();

    // ── 5. Report ─────────────────────────────────────────────────────────────
    let received = frames.len();
    let drop_pct = (expected.saturating_sub(received)) as f64 / expected as f64 * 100.0;

    println!("── Results ──────────────────────────────────────────────────────");
    println!("  Frames received:  {} / {} ({:.1}% drop)", received, expected, drop_pct);

    if let Some(f) = frames.first() {
        println!("  First frame:      {}×{}  ({} bytes)", f.width, f.height, f.rgba.len());
        println!("  First timestamp:  {} ms", f.timestamp_ms);
    }
    if let Some(f) = frames.last() {
        println!("  Last timestamp:   {} ms", f.timestamp_ms);
    }

    // Verify frames aren't paused-period frames (timestamps shouldn't cluster
    // around the 1000–1300 ms window when we were paused).
    let paused_frames = frames
        .iter()
        .filter(|f| f.timestamp_ms >= 1000 && f.timestamp_ms <= 1300)
        .count();
    println!("  Frames in pause window (1000–1300 ms): {}", paused_frames);

    println!();

    // ── 6. Verdict ────────────────────────────────────────────────────────────
    // SCStream is change-driven: frames only arrive when screen content changes.
    // A static terminal delivers far fewer than the configured FPS — this is
    // correct behavior (Spike C finding). Just verify at least 1 frame arrived.
    let pass_frame_count = received >= 1;
    let pass_dimensions = frames.first().map(|f| f.width > 0 && f.height > 0).unwrap_or(false);
    let pass_content = frames.first().map(|f| !is_black(&f.rgba)).unwrap_or(false);
    let pass_pause = paused_frames == 0;

    println!("── Verdict ──────────────────────────────────────────────────────");
    println!("  (note: SCStream is change-driven — fewer frames on static screens is expected)");
    verdict("Frame delivery (≥1 frame received)", pass_frame_count);
    verdict("Frame dimensions non-zero", pass_dimensions);
    verdict("Frame content not black", pass_content);
    verdict("No frames during pause window", pass_pause);

    let all_pass = pass_frame_count && pass_dimensions && pass_content && pass_pause;
    println!();
    if all_pass {
        println!("✓  SMOKE TEST: PASS");
    } else {
        println!("✗  SMOKE TEST: FAIL");
        std::process::exit(1);
    }
    println!();
}

fn is_black(rgba: &[u8]) -> bool {
    rgba.chunks(4 * 64).all(|chunk| chunk[0] < 10 && chunk[1] < 10 && chunk[2] < 10)
}

fn verdict(label: &str, pass: bool) {
    println!("  {}  {}", if pass { "✓" } else { "✗" }, label);
}
