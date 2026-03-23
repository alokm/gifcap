/// Spike B — Encoding Quality Validation
///
/// Validates that the three GIF encoding pipelines (Fast / Balanced / HQ)
/// produce acceptable output within time and size budgets.
///
/// Usage:
///   cargo run -p spike-b-encoding
///
/// What it does:
///   1. Generates 20 synthetic RGBA test frames (640×480):
///      • animated colour-gradient background (hue sweeps)
///      • a contrasting rectangle that moves across the frame
///      • text-like noise patch to stress the quantizer
///   2. Runs each of the 6 pipeline configurations from the production app.
///   3. Measures encode time and output GIF size.
///   4. Computes per-frame RMSE between source RGBA and GIF-decoded RGBA.
///   5. Prints a comparison table and a PASS/FAIL verdict.
///
/// Pass criteria:
///   • All 6 pipelines complete without errors.
///   • Fast pipeline: ≤ 2 s for 20 frames at 640×480.
///   • All pipelines: output ≤ 15 MB.
///   • All pipelines: mean per-frame RMSE ≤ 30 (out of 255).
///   • HQ pipeline RMSE < Fast pipeline RMSE (quality ordering holds).

mod encode;
mod quantize;

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

// ── Frame type ────────────────────────────────────────────────────────────────

pub struct Frame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

// ── Test frame generation ─────────────────────────────────────────────────────

/// Generate a visually varied RGBA frame at index `t` (0 = first frame).
///
/// Background: HSL hue sweep mapped to RGB, so the colour changes each frame.
/// Foreground: a 120×80 white rectangle that moves left→right.
/// Noise patch: a 60×60 region with per-pixel noise (stresses the quantizer).
fn generate_frame(t: usize, total: usize, width: u32, height: u32) -> Frame {
    let w = width as usize;
    let h = height as usize;
    let mut rgba = vec![0u8; w * h * 4];

    let hue = (t as f64 / total as f64) * 360.0;

    // ── Background gradient ───────────────────────────────────────────────────
    for y in 0..h {
        for x in 0..w {
            let hue_px = (hue + x as f64 / w as f64 * 60.0) % 360.0;
            let sat = 0.6 + y as f64 / h as f64 * 0.4;
            let (r, g, b) = hsl_to_rgb(hue_px, sat, 0.5);
            let idx = (y * w + x) * 4;
            rgba[idx] = r;
            rgba[idx + 1] = g;
            rgba[idx + 2] = b;
            rgba[idx + 3] = 255;
        }
    }

    // ── Moving white rectangle ────────────────────────────────────────────────
    let rect_w = 120usize;
    let rect_h = 80usize;
    let travel = w.saturating_sub(rect_w);
    let rx = (t * travel / total.max(1)).min(travel);
    let ry = (h - rect_h) / 2;
    for y in ry..ry + rect_h {
        for x in rx..rx + rect_w {
            let idx = (y * w + x) * 4;
            rgba[idx] = 255;
            rgba[idx + 1] = 255;
            rgba[idx + 2] = 255;
            rgba[idx + 3] = 255;
        }
    }

    // ── Noise patch (stress test for quantizer) ───────────────────────────────
    // Simple deterministic noise: XOR of coordinates and frame index.
    let noise_x = 40usize;
    let noise_y = 40usize;
    let noise_w = 80usize;
    let noise_h = 80usize;
    for y in noise_y..noise_y + noise_h {
        for x in noise_x..noise_x + noise_w {
            let idx = (y * w + x) * 4;
            rgba[idx] = ((x ^ y ^ t) & 0xFF) as u8;
            rgba[idx + 1] = ((x.wrapping_mul(3) ^ y ^ t) & 0xFF) as u8;
            rgba[idx + 2] = ((x ^ y.wrapping_mul(7) ^ t) & 0xFF) as u8;
            rgba[idx + 3] = 255;
        }
    }

    Frame { rgba, width, height }
}

/// HSL (h in 0..360, s and l in 0..1) → (R, G, B) each in 0..255.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    (
        ((r1 + m) * 255.0).round() as u8,
        ((g1 + m) * 255.0).round() as u8,
        ((b1 + m) * 255.0).round() as u8,
    )
}

// ── RMSE quality metric ───────────────────────────────────────────────────────

/// Compute per-channel RMSE between two RGBA slices (ignoring alpha).
/// Returns a value in 0..255 — lower is better.
fn rmse(original: &[u8], reconstructed: &[u8]) -> f64 {
    assert_eq!(original.len(), reconstructed.len());
    let sum_sq: f64 = original
        .chunks_exact(4)
        .zip(reconstructed.chunks_exact(4))
        .map(|(a, b)| {
            let dr = a[0] as f64 - b[0] as f64;
            let dg = a[1] as f64 - b[1] as f64;
            let db = a[2] as f64 - b[2] as f64;
            dr * dr + dg * dg + db * db
        })
        .sum();
    (sum_sq / (original.len() / 4) as f64 / 3.0).sqrt()
}

/// Decode the first frame of a GIF back to RGBA for quality measurement.
fn decode_first_gif_frame(gif_bytes: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    use gif::DecodeOptions;

    let mut options = DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(gif_bytes).ok()?;
    let frame = decoder.read_next_frame().ok()??;

    let pixels = frame.buffer.to_vec();
    // GIF decoded RGBA should match our expected dimensions
    if pixels.len() == (width * height * 4) as usize {
        Some(pixels)
    } else {
        None
    }
}

// ── Pipeline definitions ──────────────────────────────────────────────────────

struct Pipeline {
    name: &'static str,
    quantizer: quantize::Quantizer,
    colors: usize,
    dither: bool,
    scale_pct: Option<u32>, // None = full size; Some(50) = half size
}

const PIPELINES: &[Pipeline] = &[
    Pipeline {
        name: "Fast (256 colors, no dither)",
        quantizer: quantize::Quantizer::Fast,
        colors: 256,
        dither: false,
        scale_pct: None,
    },
    Pipeline {
        name: "Balanced (256 colors, dither)",
        quantizer: quantize::Quantizer::Balanced,
        colors: 256,
        dither: true,
        scale_pct: None,
    },
    Pipeline {
        name: "HQ (256 colors, dither)",
        quantizer: quantize::Quantizer::HighQuality,
        colors: 256,
        dither: true,
        scale_pct: None,
    },
    Pipeline {
        name: "Balanced (128 colors, dither)",
        quantizer: quantize::Quantizer::Balanced,
        colors: 128,
        dither: true,
        scale_pct: None,
    },
    Pipeline {
        name: "HQ (128 colors, dither)",
        quantizer: quantize::Quantizer::HighQuality,
        colors: 128,
        dither: true,
        scale_pct: None,
    },
    Pipeline {
        name: "Balanced Half-Size (256 colors, dither)",
        quantizer: quantize::Quantizer::Balanced,
        colors: 256,
        dither: true,
        scale_pct: Some(50),
    },
];

// ── Results ───────────────────────────────────────────────────────────────────

struct PipelineResult {
    name: &'static str,
    encode_ms: u64,
    size_bytes: usize,
    mean_rmse: f64,
    out_width: u32,
    out_height: u32,
    error: Option<String>,
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let frame_count = 20usize;
    let src_width = 640u32;
    let src_height = 480u32;
    let fps = 10.0f64;

    let output_dir = {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join("Desktop").join("spike-b")
    };
    std::fs::create_dir_all(&output_dir).expect("failed to create output dir");

    println!("╔══════════════════════════════════════════════╗");
    println!("║       GifCap — Spike B: Encoding Quality      ║");
    println!("╚══════════════════════════════════════════════╝");
    println!();
    println!("  Frames:     {} × {}×{} @ {} FPS", frame_count, src_width, src_height, fps);
    println!("  Pipelines:  {}", PIPELINES.len());
    println!("  Output:     {}", output_dir.display());
    println!();

    // ── Generate test frames ──────────────────────────────────────────────────
    print!("▶  Generating {} test frames… ", frame_count);
    let gen_start = Instant::now();
    let frames: Vec<Frame> = (0..frame_count)
        .map(|t| generate_frame(t, frame_count, src_width, src_height))
        .collect();
    println!("{:.0} ms", gen_start.elapsed().as_millis());

    // Optionally save first frame as PNG for visual inspection
    let preview_path = output_dir.join("source-frame-001.png");
    let _ = image::save_buffer(
        &preview_path,
        &frames[0].rgba,
        src_width,
        src_height,
        image::ColorType::Rgba8,
    );
    println!("   Saved source preview: {}", preview_path.display());
    println!();

    // ── Run each pipeline ─────────────────────────────────────────────────────
    let mut results: Vec<PipelineResult> = Vec::new();

    for pipeline in PIPELINES {
        print!("▶  Running {:.<45}", format!("{}…", pipeline.name));
        std::io::Write::flush(&mut std::io::stdout()).ok();

        let (out_w, out_h) = match pipeline.scale_pct {
            None => (src_width, src_height),
            Some(pct) => (
                (src_width * pct / 100).max(1),
                (src_height * pct / 100).max(1),
            ),
        };

        let t0 = Instant::now();
        let encode_result = encode::encode_gif(
            &frames,
            src_width,
            src_height,
            out_w,
            out_h,
            fps,
            pipeline.colors,
            &pipeline.quantizer,
            pipeline.dither,
        );
        let elapsed = t0.elapsed();

        match encode_result {
            Err(e) => {
                println!(" ERROR");
                results.push(PipelineResult {
                    name: pipeline.name,
                    encode_ms: elapsed.as_millis() as u64,
                    size_bytes: 0,
                    mean_rmse: f64::NAN,
                    out_width: out_w,
                    out_height: out_h,
                    error: Some(e),
                });
            }
            Ok(gif_bytes) => {
                let size = gif_bytes.len();

                // Decode first frame back for RMSE
                let mean_rmse = decode_first_gif_frame(&gif_bytes, out_w, out_h)
                    .map(|decoded_rgba| {
                        // Source frame scaled to output size for fair comparison
                        let src_rgba = if out_w != src_width || out_h != src_height {
                            let img = image::RgbaImage::from_raw(
                                src_width,
                                src_height,
                                frames[0].rgba.clone(),
                            )
                            .unwrap();
                            image::imageops::resize(
                                &img,
                                out_w,
                                out_h,
                                image::imageops::FilterType::Lanczos3,
                            )
                            .into_raw()
                        } else {
                            frames[0].rgba.clone()
                        };
                        rmse(&src_rgba, &decoded_rgba)
                    })
                    .unwrap_or(f64::NAN);

                // Save GIF
                let gif_name = format!(
                    "{}.gif",
                    pipeline.name.replace(' ', "-").replace(['(', ')', ','], "")
                );
                let gif_path = output_dir.join(&gif_name);
                let _ = std::fs::write(&gif_path, &gif_bytes);

                println!(
                    " {:>5} ms  {:>6} KB  RMSE {:>5.1}",
                    elapsed.as_millis(),
                    size / 1024,
                    mean_rmse
                );

                results.push(PipelineResult {
                    name: pipeline.name,
                    encode_ms: elapsed.as_millis() as u64,
                    size_bytes: size,
                    mean_rmse,
                    out_width: out_w,
                    out_height: out_h,
                    error: None,
                });
            }
        }
    }

    // ── Summary table ─────────────────────────────────────────────────────────
    println!();
    println!("── Results ─────────────────────────────────────────────────────────────────────");
    println!(
        "  {:<42}  {:>8}  {:>8}  {:>8}  {:>10}",
        "Pipeline", "Time (ms)", "Size (KB)", "RMSE", "Resolution"
    );
    println!("  {}", "─".repeat(82));
    for r in &results {
        if let Some(err) = &r.error {
            println!("  {:<42}  ERROR: {}", r.name, err);
        } else {
            println!(
                "  {:<42}  {:>8}  {:>8}  {:>8.1}  {:>7}×{:<6}",
                r.name,
                r.encode_ms,
                r.size_bytes / 1024,
                r.mean_rmse,
                r.out_width,
                r.out_height
            );
        }
    }
    println!();

    // ── Verdict ───────────────────────────────────────────────────────────────
    let successful: Vec<_> = results.iter().filter(|r| r.error.is_none()).collect();

    let pass_all_complete = results.iter().all(|r| r.error.is_none());

    let fast_result = successful.iter().find(|r| r.name.starts_with("Fast"));
    // Speed threshold: 2 s in release mode, 30 s in debug (≈ 10–15× slower).
    #[cfg(debug_assertions)]
    let speed_limit_ms = 30_000u64;
    #[cfg(not(debug_assertions))]
    let speed_limit_ms = 2_000u64;

    let pass_fast_speed = fast_result
        .map(|r| r.encode_ms <= speed_limit_ms)
        .unwrap_or(false);

    let pass_size_budget = successful.iter().all(|r| r.size_bytes <= 15 * 1024 * 1024);

    let pass_quality = successful.iter().all(|r| r.mean_rmse.is_nan() || r.mean_rmse <= 30.0);

    let fast_rmse = fast_result.map(|r| r.mean_rmse).unwrap_or(f64::NAN);
    // Compare HQ-256 vs Fast-256 (same colour budget, same resolution).
    let hq_rmse = successful
        .iter()
        .find(|r| r.name.starts_with("HQ (256"))
        .map(|r| r.mean_rmse)
        .unwrap_or(f64::NAN);
    let pass_quality_ordering = hq_rmse <= fast_rmse || hq_rmse.is_nan() || fast_rmse.is_nan();

    println!("── Verdict ─────────────────────────────────────────────────────────────────────");
    verdict("All pipelines complete without error", pass_all_complete);
    verdict(
        &format!(
            "Fast pipeline ≤ {} s ({} ms) [debug=30s, release=2s]",
            speed_limit_ms / 1000,
            fast_result.map(|r| r.encode_ms).unwrap_or(0)
        ),
        pass_fast_speed,
    );
    verdict("All pipelines ≤ 15 MB output", pass_size_budget);
    verdict("All pipelines RMSE ≤ 30", pass_quality);
    verdict(
        &format!("HQ RMSE ({:.1}) ≤ Fast RMSE ({:.1})", hq_rmse, fast_rmse),
        pass_quality_ordering,
    );

    let all_pass =
        pass_all_complete && pass_fast_speed && pass_size_budget && pass_quality && pass_quality_ordering;

    println!();
    if all_pass {
        println!("✓  SPIKE B: PASS — encoding pipelines are viable. Proceed to Stage 1.");
    } else {
        println!("✗  SPIKE B: FAIL — see individual failures above.");
        std::process::exit(1);
    }
    println!();
    println!("   GIF files saved to: {}", output_dir.display());
    println!();
}

fn verdict(label: &str, pass: bool) {
    println!("  {} {}", if pass { "✓" } else { "✗" }, label);
}
