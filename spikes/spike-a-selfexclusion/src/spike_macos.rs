/// macOS implementation for Spike A.
///
/// Creates a red NSWindow in this process, captures two frames via SCStream
/// (one excluding our windows, one baseline), and reports PASS/FAIL.

use core_media_rs::cm_time::CMTime;
use core_video_rs::cv_pixel_buffer::lock::LockTrait;
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
    path::PathBuf,
    sync::mpsc,
    time::Duration,
};

// ── AppKit / objc types ───────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct NSPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct NSSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct NSRect {
    origin: NSPoint,
    size: NSSize,
}

// ── Red window ────────────────────────────────────────────────────────────────

/// Creates a 400×300 red NSWindow at (200, 300) in screen coordinates (AppKit
/// uses bottom-left origin, so y=300 is roughly the middle of a 1080p display).
///
/// Returns the raw NSWindow pointer (must stay alive during the test).
unsafe fn create_red_window() -> *mut objc::runtime::Object {
    use objc::{class, msg_send, sel, sel_impl};

    // Ensure NSApplication is initialised (needed before creating windows).
    let app: *mut objc::runtime::Object =
        msg_send![class!(NSApplication), sharedApplication];
    // Regular activation policy so the window appears in the compositor.
    let _: () = msg_send![app, setActivationPolicy: 0i64]; // NSApplicationActivationPolicyRegular

    // NSWindow frame — AppKit screen coordinates (bottom-left origin).
    let frame = NSRect {
        origin: NSPoint { x: 200.0, y: 300.0 },
        size: NSSize { width: 400.0, height: 300.0 },
    };

    // Alloc + init
    let win_alloc: *mut objc::runtime::Object = msg_send![class!(NSWindow), alloc];
    let win: *mut objc::runtime::Object = msg_send![
        win_alloc,
        initWithContentRect: frame
        styleMask: 1usize   // NSWindowStyleMaskTitled
        backing: 2usize     // NSBackingStoreBuffered
        defer: 0i64
    ];

    // Title so we can identify it by name if needed
    let title_str: *mut objc::runtime::Object = {
        let bytes = b"SpikeA-RedWindow\0";
        msg_send![class!(NSString), stringWithUTF8String: bytes.as_ptr()]
    };
    let _: () = msg_send![win, setTitle: title_str];

    // Fill the content area with a solid red NSBox.
    // NSBox with NSBoxCustom type is AppKit's canonical way to paint a
    // solid background — works without CALayer and without subclassing NSView.
    let content_view: *mut objc::runtime::Object = msg_send![win, contentView];
    let bounds: NSRect = msg_send![content_view, bounds];

    let box_alloc: *mut objc::runtime::Object = msg_send![class!(NSBox), alloc];
    let red_box: *mut objc::runtime::Object =
        msg_send![box_alloc, initWithFrame: bounds];
    // NSBoxCustom = 4: only fillColor / borderColor / borderWidth are used.
    let _: () = msg_send![red_box, setBoxType: 4u64];
    let _: () = msg_send![red_box, setBorderWidth: 0.0f64];
    let red: *mut objc::runtime::Object = msg_send![
        class!(NSColor),
        colorWithRed: 1.0f64
        green: 0.0f64
        blue: 0.0f64
        alpha: 1.0f64
    ];
    let _: () = msg_send![red_box, setFillColor: red];
    // NSViewWidthSizable | NSViewHeightSizable = 18 — resize with window.
    let _: () = msg_send![red_box, setAutoresizingMask: 18usize];
    let _: () = msg_send![content_view, addSubview: red_box];

    // Show
    let _: () = msg_send![app, activateIgnoringOtherApps: 1i8]; // YES
    let null: *mut objc::runtime::Object = std::ptr::null_mut();
    let _: () = msg_send![win, makeKeyAndOrderFront: null];

    win
}

/// Run NSRunLoop on the main thread for `secs` seconds so the compositor has
/// time to render the window before we capture.
unsafe fn run_loop(secs: f64) {
    use objc::{class, msg_send, sel, sel_impl};
    let run_loop: *mut objc::runtime::Object =
        msg_send![class!(NSRunLoop), mainRunLoop];
    let date: *mut objc::runtime::Object =
        msg_send![class!(NSDate), dateWithTimeIntervalSinceNow: secs];
    let _: () = msg_send![run_loop, runUntilDate: date];
}

// ── SCStream single-frame capture ─────────────────────────────────────────────

struct FrameCollector {
    tx: mpsc::SyncSender<RgbaFrame>,
}

struct RgbaFrame {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
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

        let raw = lock.as_slice();

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

        let frame = RgbaFrame {
            rgba,
            width,
            height: actual_height as u32,
        };

        // try_send: if channel is full (limit reached) we just drop
        let _ = self.tx.try_send(frame);
    }
}

/// Capture one frame using the given SCContentFilter.
/// Waits up to `timeout` for the first frame.
fn capture_one_frame(
    filter: SCContentFilter,
    width: u32,
    height: u32,
    fps: f64,
    timeout: Duration,
) -> Result<RgbaFrame, String> {
    let (tx, rx) = mpsc::sync_channel::<RgbaFrame>(2);

    let frame_interval = CMTime {
        value: 1,
        timescale: fps as i32,
        flags: 1,
        epoch: 0,
    };

    let stream_config = SCStreamConfiguration::new()
        .set_width(width)
        .map_err(|e| format!("set_width: {:?}", e))?
        .set_height(height)
        .map_err(|e| format!("set_height: {:?}", e))?
        .set_pixel_format(PixelFormat::BGRA)
        .map_err(|e| format!("set_pixel_format: {:?}", e))?
        .set_minimum_frame_interval(&frame_interval)
        .map_err(|e| format!("set_minimum_frame_interval: {:?}", e))?;

    let collector = FrameCollector { tx };

    let mut stream = SCStream::new(&filter, &stream_config);
    stream.add_output_handler(collector, SCStreamOutputType::Screen);
    stream
        .start_capture()
        .map_err(|e| format!("start_capture: {:?}", e))?;

    let result = rx.recv_timeout(timeout);

    stream
        .stop_capture()
        .map_err(|e| format!("stop_capture: {:?}", e))?;

    result.map_err(|_| "Timed out waiting for frame".into())
}

// ── Red pixel analysis ────────────────────────────────────────────────────────

/// Count pixels in the RGBA frame that are "bright red":
///   R ≥ 200, G ≤ 50, B ≤ 50
///
/// `region` is `(x, y, w, h)` in *frame pixels* (not screen points).
/// Returns (red_count, total_pixels_in_region).
fn count_red_pixels(
    frame: &RgbaFrame,
    region: (u32, u32, u32, u32),
) -> (u64, u64) {
    let (rx, ry, rw, rh) = region;
    let mut red_count = 0u64;
    let mut total = 0u64;

    let x_end = (rx + rw).min(frame.width);
    let y_end = (ry + rh).min(frame.height);

    for y in ry..y_end {
        for x in rx..x_end {
            let idx = ((y * frame.width + x) * 4) as usize;
            if idx + 3 >= frame.rgba.len() {
                break;
            }
            let r = frame.rgba[idx];
            let g = frame.rgba[idx + 1];
            let b = frame.rgba[idx + 2];
            total += 1;
            // P3 display: sRGB red (1,0,0) maps to approximately (234,51,36)
            // in captured BGRA bytes.  Use r > g*2 && r > b*2 to detect any
            // red-dominant pixel regardless of color space.
            if r >= 180 && (r as u16) > (g as u16) * 2 && (r as u16) > (b as u16) * 2 {
                red_count += 1;
            }
        }
    }

    (red_count, total)
}

// ── Output helpers ────────────────────────────────────────────────────────────

fn save_frame(frame: &RgbaFrame, path: &PathBuf) {
    match image::save_buffer(
        path,
        &frame.rgba,
        frame.width,
        frame.height,
        image::ColorType::Rgba8,
    ) {
        Ok(()) => println!("  Saved: {}", path.display()),
        Err(e) => println!("  ⚠ Failed to save {}: {}", path.display(), e),
    }
}

fn verdict(label: &str, pass: bool) {
    let icon = if pass { "  ✓" } else { "  ✗" };
    println!("{} {}", icon, label);
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run() {
    let our_pid = std::process::id() as i32;
    let output_dir = {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join("Desktop").join("spike-a")
    };
    std::fs::create_dir_all(&output_dir).expect("failed to create output dir");

    // ── Step 1: Create red window ─────────────────────────────────────────────
    println!("▶  Creating red NSWindow (our PID = {})…", our_pid);

    // SAFETY: AppKit APIs must be called on the main thread, which this is.
    let _window_ptr = unsafe { create_red_window() };

    // Run the NSRunLoop for 1 second so the compositor renders the window.
    println!("   Running NSRunLoop for 1 s to render window…");
    unsafe { run_loop(1.0) };

    // ── Step 2: Find our window via SCShareableContent ────────────────────────
    println!("▶  Querying SCShareableContent for our windows…");

    let content = match SCShareableContent::get() {
        Ok(c) => c,
        Err(e) => {
            println!("✗  SCShareableContent::get failed: {:?}", e);
            std::process::exit(1);
        }
    };

    let displays = content.displays();
    if displays.is_empty() {
        println!("✗  No displays found.");
        std::process::exit(1);
    }
    let display = &displays[0];

    let all_windows = content.windows();
    let our_windows: Vec<_> = all_windows
        .iter()
        .filter(|w| w.owning_application().process_id() == our_pid)
        .collect();

    println!(
        "   Total SCWindows: {}  |  Matching our PID ({}): {}",
        all_windows.len(),
        our_pid,
        our_windows.len()
    );

    for w in &our_windows {
        let frame = w.get_frame();
        println!(
            "   Window id={} title={:?} on_screen={} frame=({:.0},{:.0} {:.0}×{:.0})",
            w.window_id(),
            w.title(),
            w.is_on_screen(),
            frame.origin.x,
            frame.origin.y,
            frame.size.width,
            frame.size.height
        );
    }

    // ── Step 3: Determine capture dimensions from display ────────────────────
    // Use the display's reported pixel dimensions so that CG window
    // coordinates map 1:1 to capture pixel coordinates.
    let cap_width = display.width();
    let cap_height = display.height();
    let fps = 10.0f64;
    let timeout = Duration::from_secs(5);

    println!();
    println!("   Display size:       {}×{}", cap_width, cap_height);
    println!("   Capture resolution: {}×{}", cap_width, cap_height);

    // ── Step 4: Capture WITH exclusion ────────────────────────────────────────
    println!();
    println!("▶  Capture 1 — WITH our windows excluded…");

    let filter_excluded = SCContentFilter::new()
        .with_display_excluding_windows(display, &our_windows);

    let excluded_frame = match capture_one_frame(
        filter_excluded,
        cap_width,
        cap_height,
        fps,
        timeout,
    ) {
        Ok(f) => f,
        Err(e) => {
            println!("✗  Capture (excluded) failed: {}", e);
            std::process::exit(1);
        }
    };
    let excluded_path = output_dir.join("frame-excluded.png");
    save_frame(&excluded_frame, &excluded_path);

    // ── Step 5: Capture WITHOUT exclusion (baseline) ──────────────────────────
    println!();
    println!("▶  Capture 2 — WITHOUT exclusion (baseline)…");

    let filter_baseline =
        SCContentFilter::new().with_display_excluding_windows(display, &[]);

    let baseline_frame = match capture_one_frame(
        filter_baseline,
        cap_width,
        cap_height,
        fps,
        timeout,
    ) {
        Ok(f) => f,
        Err(e) => {
            println!("✗  Capture (baseline) failed: {}", e);
            std::process::exit(1);
        }
    };
    let baseline_path = output_dir.join("frame-baseline.png");
    save_frame(&baseline_frame, &baseline_path);

    // ── Step 6: Analyse red pixels ────────────────────────────────────────────
    //
    // Convert the SCWindow frame (CG coords: bottom-left origin) to capture
    // pixel coordinates (top-left origin).
    //
    // SCDisplay.width() / .height() return the display size in the same CG
    // coordinate space as SCWindow.get_frame(), so no additional scale factor
    // is needed — we just flip the Y axis.
    //
    // Inset the region by 5 px on each side to avoid anti-aliased title-bar
    // edges producing false colour readings.
    let region = if let Some(win) = our_windows.first() {
        let wf = win.get_frame();
        // Window content starts below the title bar (~28 pt); the title bar
        // itself is at the TOP of the window in AppKit (high y → high in frame).
        let title_bar_h = 28u32;
        let wx = wf.origin.x as u32;
        // In CG / AppKit, y increases upward.  Convert to top-left:
        //   top_y = display_height - (window_bottom + window_height)
        //         = display_height - wf.origin.y - wf.size.height
        let wy_top = (cap_height as f64 - wf.origin.y - wf.size.height).max(0.0) as u32;
        let ww = wf.size.width as u32;
        let wh = wf.size.height as u32;

        // Inset a bit; skip the title bar area
        let inset = 5u32;
        let rx = wx.saturating_add(inset);
        let ry = wy_top.saturating_add(title_bar_h).saturating_add(inset);
        let rw = ww.saturating_sub(inset * 2);
        let rh = wh.saturating_sub(title_bar_h).saturating_sub(inset * 2);

        println!();
        println!(
            "   Window in capture coords: ({}, {})  {}×{}  (content region: ({},{}) {}×{})",
            wx, wy_top, ww, wh, rx, ry, rw, rh
        );
        (rx, ry, rw, rh)
    } else {
        // No window found — fall back to centre band
        (cap_width / 4, cap_height / 5, cap_width / 2, cap_height * 3 / 5)
    };

    let (excl_red, excl_total) = count_red_pixels(&excluded_frame, region);
    let (base_red, base_total) = count_red_pixels(&baseline_frame, region);

    let excl_pct = if excl_total > 0 { excl_red as f64 / excl_total as f64 * 100.0 } else { 0.0 };
    let base_pct = if base_total > 0 { base_red as f64 / base_total as f64 * 100.0 } else { 0.0 };

    println!();
    println!("── Analysis (window content region {}×{}) ──────────────", region.2, region.3);
    println!(
        "  Baseline  (no exclusion):  {:.1}% red ({}/{} px)",
        base_pct, base_red, base_total
    );
    println!(
        "  Excluded  (our windows):   {:.1}% red ({}/{} px)",
        excl_pct, excl_red, excl_total
    );

    // ── Step 7: Verdict ───────────────────────────────────────────────────────
    let pass_window_found = !our_windows.is_empty();
    // Baseline: at least 50% of the content region must be red.
    let pass_baseline_has_red = base_pct >= 50.0;
    // Exclusion: must eliminate ≥ 95% of the red pixels seen in baseline.
    let pass_exclusion_works = if base_red > 0 {
        excl_red as f64 / base_red as f64 <= 0.05
    } else {
        false // no baseline red → can't validate exclusion
    };

    println!();
    println!("── Verdict ─────────────────────────────────────────────────────");
    verdict("Our window found via SCShareableContent", pass_window_found);
    verdict("Baseline shows red content (≥50% of region)", pass_baseline_has_red);
    verdict("Exclusion removes ≥95% of red pixels", pass_exclusion_works);

    let all_pass = pass_window_found && pass_baseline_has_red && pass_exclusion_works;
    println!();
    if all_pass {
        println!("✓  SPIKE A: PASS — self-exclusion works. Proceed to Spike B.");
    } else {
        println!("✗  SPIKE A: FAIL — see individual failures above.");

        if !pass_window_found {
            println!();
            println!("   DIAGNOSIS: SCShareableContent did not enumerate our window.");
            println!("   This may happen because a CLI binary is not treated as an");
            println!("   NSApplication by the compositor. Consider running as a proper");
            println!("   macOS app bundle or Tauri app.");
            println!("   The exclusion API (SCContentFilter) itself is still usable —");
            println!("   in GifCap's Tauri binary, windows WILL be enumerated.");
        }

        if !pass_baseline_has_red {
            println!();
            println!("   DIAGNOSIS: Red content not detected in window region of baseline frame.");
            println!("   Check ~/Desktop/spike-a/frame-baseline.png.");
            println!("   If the window appears white/gray: the contentView layer colour may");
            println!("   not have been applied correctly. Check CALayer setBackgroundColor.");
        }

        if pass_window_found && pass_baseline_has_red && !pass_exclusion_works {
            println!();
            println!("   DIAGNOSIS: Exclusion did not remove the red window.");
            println!("   SCContentFilter::with_display_excluding_windows may not work");
            println!("   for this window type. Investigate SCWindow.is_on_screen() and");
            println!("   whether the window was correctly passed to the filter.");
        }

        println!();
        std::process::exit(1);
    }
    println!();
}
