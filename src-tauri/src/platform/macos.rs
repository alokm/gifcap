use std::time::Duration;
use tauri::WebviewWindow;

/// Polls the global cursor position every 50 ms and toggles
/// `ignoresMouseEvents` on the capture window so that:
///
/// - The control strip (top 34 CSS px) and resize-handle border (10 px around
///   all edges) remain fully interactive.
/// - Everything else (the transparent interior) is click-through.
///
/// The window starts in click-through mode; the first poll that detects the
/// cursor over an interactive zone will restore event delivery.
pub fn start_click_through_watcher(win: WebviewWindow) {
    std::thread::spawn(move || {
        const CONTROL_STRIP_H: f64 = 34.0; // height of the control strip in logical px
        const BORDER_W: f64 = 10.0; // width of the resize-handle hit zone in logical px
        const POLL_MS: u64 = 50;

        loop {
            std::thread::sleep(Duration::from_millis(POLL_MS));

            let scale = match win.scale_factor() {
                Ok(s) => s,
                Err(_) => break, // window destroyed
            };
            let pos = match win.outer_position() {
                Ok(p) => p,
                Err(_) => break,
            };
            let size = match win.outer_size() {
                Ok(s) => s,
                Err(_) => break,
            };

            // Window bounds in logical (CSS) pixels.
            let wx = pos.x as f64 / scale;
            let wy = pos.y as f64 / scale;
            let ww = size.width as f64 / scale;
            let wh = size.height as f64 / scale;

            // Cursor in logical screen coordinates (origin: top-left of primary display).
            let (cx, cy) = cursor_position_logical();

            // Interactive zones: control strip (top) or any resize-handle border.
            let inside_window = cx >= wx && cx <= wx + ww && cy >= wy && cy <= wy + wh;
            let interactive = inside_window
                && (cy - wy < CONTROL_STRIP_H   // control strip
                    || cx - wx < BORDER_W        // left border
                    || wx + ww - cx < BORDER_W   // right border
                    || wy + wh - cy < BORDER_W); // bottom border

            let _ = win.set_ignore_cursor_events(!interactive);
        }
    });
}

/// Returns the cursor position in logical screen coordinates using
/// CoreGraphics (no extra crates needed — CoreGraphics is always linked).
fn cursor_position_logical() -> (f64, f64) {
    use std::ffi::c_void;

    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *mut c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
        fn CFRelease(cf: *mut c_void);
    }

    unsafe {
        let event = CGEventCreate(std::ptr::null_mut());
        if event.is_null() {
            return (0.0, 0.0);
        }
        let pt = CGEventGetLocation(event);
        CFRelease(event);
        (pt.x, pt.y)
    }
}
