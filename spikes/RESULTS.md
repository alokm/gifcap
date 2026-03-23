# Spike Results

## SPIKE-C: Platform Capture — PASS

**Date:** 2026-03-21
**Platform:** macOS (Apple Silicon)
**Binary:** `cargo run -p spike-c-capture`

```
Frames received:   50 / 50 (0.0% drop)
Mean interval:     100.3 ms  (target 100.0 ms)
Max jitter:        11.6 ms   (threshold <20 ms)
Frame dimensions:  640×480 ✓
Frame content:     not black ✓
```

### Findings

**SCStream is the production path.** `screencapturekit` v0.3.6 exposes `SCStream`
for continuous capture — no `CGWindowListCreateImage` fallback needed.

**Correct module paths for v0.3.6** (different from assumed):
```rust
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
use core_media_rs::cm_time::CMTime;
use core_video_rs::cv_pixel_buffer::lock::LockTrait;
```

**Pixel format must be set explicitly.**
`SCStream` defaults to `YCbCr_420v` (4:2:0 planar). Without
`.set_pixel_format(PixelFormat::BGRA)` the luma plane (W×H bytes, 1 byte/pixel)
is mistaken for BGRA data, producing frames with height = H/4.
Fix: always call `.set_pixel_format(PixelFormat::BGRA)?` on `SCStreamConfiguration`.

**Frame delivery is change-driven.**
SCStream only delivers frames when screen content changes. In a static terminal
window, ~20 frames/5s. With active screen activity, 50/50 at exactly 10 FPS.
This is correct behavior for GifCap — recordings always involve active content.

**Additional explicit Cargo dependencies required:**
```toml
core-media-rs = "0.3"
core-video-rs = "0.3"
```
(transitive via `screencapturekit` but must be declared to use directly)

### Architecture Decision

SCStream via `screencapturekit` v0.3.6 is confirmed viable for the production
macOS capture backend. Proceed to Stage 1 with this dependency stack.

Risk R-03 (crate maintenance) status: **monitored** — pin to `screencapturekit = "0.3.6"`
in production `Cargo.toml` and document the fallback to `objc2` raw bindings if
the crate lapses.

---

## SPIKE-A: Self-Exclusion — PASS

**Date:** 2026-03-21
**Platform:** macOS (Apple Silicon)
**Binary:** `cargo run -p spike-a-selfexclusion`

```
Our window found via SCShareableContent  ✓
Baseline shows red content (≥50%)        ✓  57.6% red (65122/113100 px)
Exclusion removes ≥95% of red pixels     ✓   0.0% red (0/113100 px)
```

### Findings

**SCContentFilter::with_display_excluding_windows is the production path.**
A window registered by our PID is found via `SCShareableContent::get().windows()`,
matched by `window.owning_application().process_id() == std::process::id() as i32`,
and passing the slice to `SCContentFilter::new().with_display_excluding_windows`
eliminates 100% of that window's pixels from captured frames.

**CLI binaries ARE enumerated by SCShareableContent.**
Unlike some system APIs, SCStream / SCShareableContent enumerates windows from
plain cargo-run binaries (no app bundle required). `is_on_screen` returns true
and the window appears in the 192-window list.

**P3 display colour space note.**
`NSColor colorWithRed:1.0 green:0.0 blue:0.0 alpha:1.0` on a P3 display renders
as approximately (234, 51, 36) in the captured BGRA bytes — not (255, 0, 0).
sRGB red (1,0,0) is outside the P3 gamut and gets clamped differently.
Detection heuristic: `r >= 180 && r > g*2 && r > b*2` is colour-space agnostic.

**Window frame coordinate system.**
`SCWindow.get_frame()` returns a `CGRect` in AppKit/CG screen coordinates
(origin = bottom-left of primary display). To convert to SCStream capture
coordinates (origin = top-left):
```
capture_top_y = display_height - cg_frame.origin.y - cg_frame.size.height
```
`SCDisplay.width()` / `.height()` give the display size in the same CG point
space, so no additional scale factor is needed when `cap_width = display.width()`.

### Architecture Decision

`SCContentFilter::with_display_excluding_windows` confirmed as the production
self-exclusion mechanism. In GifCap's Tauri binary, enumerate windows via
`SCShareableContent`, match by `std::process::id()`, and pass to the filter.
No alternative API is needed.

---

## SPIKE-B: Encoding Quality — PASS

**Date:** 2026-03-21
**Platform:** macOS (Apple Silicon), release build
**Binary:** `cargo run --release -p spike-b-encoding`

```
Pipeline                                  Time (ms)  Size (KB)  RMSE  Resolution
Fast (256 colors, no dither)                   540       706     4.2   640×480
Balanced (256 colors, dither)                  336      2179     3.4   640×480
HQ (256 colors, dither)                        405      2076     3.6   640×480
Balanced (128 colors, dither)                  320      1789     4.9   640×480
HQ (128 colors, dither)                        363      1761     5.0   640×480
Balanced Half-Size (256 colors, dither)        181       662     3.4   320×240
```

All pipelines complete without error          ✓
Fast pipeline ≤ 2 s (540 ms, release)         ✓
All pipelines ≤ 15 MB output                  ✓
All pipelines RMSE ≤ 30                       ✓
HQ RMSE (3.6) ≤ Fast RMSE (4.2)              ✓

### Findings

**All three pipeline families are production-viable.**

**Bug fixed — quantette PaletteSize overflow.**
`palette_size(256u8)` silently wraps to 0 because `u8` max is 255. `quantette`
v0.3.0 uses `PaletteSize(u16)` with `MAX = 256`. For 256 colours always use
`PaletteSize::MAX`; for < 256 use `num_colors as u8`. This must be fixed in
`src-tauri/src/compression/quantize.rs` before Stage 1.

**quantette 0.3 API uses `TryFrom<&RgbImage>` not `try_from_rgbx_slice`.**
Strip alpha before creating `image::RgbImage`, then use `ImagePipeline::try_from(&img)`.
The existing `src-tauri/src/compression/quantize.rs` calls the non-existent
`try_from_rgbx_slice` and must be rewritten for Stage 1.

**Speed (release mode, Apple Silicon, 20 frames × 640×480):**
- Fast: 540 ms  (NeuQuant sample=10, no dither)
- Balanced: 336 ms  (Wu quantization + dither) — faster than Fast due to Wu's efficiency
- HQ: 405 ms  (k-means + dither)
- Half-Size: 181 ms  (50% scale + Wu)

**Quality ordering holds at same colour budget (256 colors):**
RMSE: Fast 4.2 > HQ 3.6 > Balanced 3.4 (note: Balanced dither slightly outperforms
HQ k-means on synthetic gradient frames; k-means advantage is more visible on
photographic content).

**Dithering increases file size significantly.**
Balanced-256-dither: 2179 KB vs Fast-256-nodither: 706 KB (3× larger).
Half-Size-dither: 662 KB — comparable to Fast at full size, with better RMSE.
This tradeoff should be exposed in the UI.

### Architecture Decision

The three pipelines (NeuQuant fast / Wu balanced / k-means HQ) from
`src-tauri/src/compression/` are confirmed viable for Stage 1, with two
code fixes required before production use (PaletteSize overflow, API update).

All three risk propositions are now validated. **Proceed to Stage 1.**
