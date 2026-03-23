/// Spike A — Self-Exclusion Validation
///
/// Validates that SCContentFilter::with_display_excluding_windows correctly
/// removes GifCap's own windows from captured frames.
///
/// Usage:
///   cargo run -p spike-a-selfexclusion
///
/// What it does:
///   1. Creates a bright red (255, 0, 0) NSWindow via AppKit/objc.
///   2. Runs NSRunLoop briefly to let the compositor render the window.
///   3. Finds the window via SCShareableContent, matching by our PID.
///   4. Captures one frame WITH the window excluded.
///   5. Captures one frame WITHOUT exclusion (baseline).
///   6. Saves both PNGs to ~/Desktop/spike-a/.
///   7. Scans the excluded frame at the window's known location for red pixels.
///
/// Pass criteria:
///   - At least one window found matching our PID.
///   - Excluded frame: ≤ 1% red pixels in the window's screen region.
///   - Baseline frame: ≥ 10% red pixels in the window's screen region.
///
/// Fail paths:
///   - No SCWindow found for our PID → CLI binary windows may not be
///     enumerated by SCShareableContent; consider Tauri app spike instead.
///   - Exclusion has no effect → SCContentFilter filter mechanism broken.

#[cfg(target_os = "macos")]
mod spike_macos;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    println!("╔══════════════════════════════════════════════╗");
    println!("║        GifCap — Spike A: Self-Exclusion       ║");
    println!("╚══════════════════════════════════════════════╝");
    println!();

    #[cfg(target_os = "macos")]
    spike_macos::run();

    #[cfg(not(target_os = "macos"))]
    {
        println!("✗  Spike A targets macOS only.");
        std::process::exit(1);
    }
}
