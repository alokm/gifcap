/// Colour quantisation — inlined from src-tauri/src/compression/quantize.rs.
/// Kept as a standalone copy so the spike has no Tauri dep.

#[derive(Clone, Copy)]
pub enum Quantizer {
    Fast,
    Balanced,
    HighQuality,
}

/// Quantize a single RGBA frame.
/// Returns `(palette_rgb, indexed_pixels)`.
/// `palette_rgb` is a flat RGB byte array (3 bytes per colour, up to 256 entries).
pub fn quantize_frame(
    rgba: &[u8],
    width: u32,
    height: u32,
    num_colors: usize,
    quantizer: Quantizer,
    dither: bool,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    match quantizer {
        Quantizer::Fast => quantize_fast(rgba, num_colors),
        Quantizer::Balanced | Quantizer::HighQuality => {
            quantize_quality(rgba, width, height, num_colors, quantizer, dither)
        }
    }
}

fn quantize_fast(rgba: &[u8], num_colors: usize) -> Result<(Vec<u8>, Vec<u8>), String> {
    use color_quant::NeuQuant;
    let nq = NeuQuant::new(10, num_colors, rgba); // sample_factor=10 (fast)
    let palette = nq.color_map_rgb();
    let indices: Vec<u8> = rgba
        .chunks_exact(4)
        .map(|px| nq.index_of(px) as u8)
        .collect();
    Ok((palette, indices))
}

fn quantize_quality(
    rgba: &[u8],
    width: u32,
    height: u32,
    num_colors: usize,
    quantizer: Quantizer,
    dither: bool,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    use quantette::{ImagePipeline, QuantizeMethod};

    // quantette 0.3 takes an image::RgbImage — strip the alpha channel first.
    let rgb: Vec<u8> = rgba
        .chunks_exact(4)
        .flat_map(|px| [px[0], px[1], px[2]])
        .collect();
    let rgb_img = image::RgbImage::from_raw(width, height, rgb)
        .ok_or("failed to create RgbImage from RGB bytes")?;

    let mut pipeline = ImagePipeline::try_from(&rgb_img)
        .map_err(|e| format!("quantette pipeline error: {e}"))?;

    let method = match quantizer {
        Quantizer::HighQuality => QuantizeMethod::kmeans(),
        _ => QuantizeMethod::wu(),
    };

    // PaletteSize is backed by u16 with MAX = 256.  Passing `256u8` would
    // wrap to 0 — use PaletteSize::MAX for 256 colours, u8 cast otherwise.
    let palette_sz = if num_colors >= 256 {
        quantette::PaletteSize::MAX
    } else {
        quantette::PaletteSize::from(num_colors as u8)
    };

    pipeline
        .palette_size(palette_sz)
        .dither(dither)
        .quantize_method(method);

    let (palette, indices) = pipeline.indexed_palette();

    let palette_rgb: Vec<u8> = palette.iter().flat_map(|c| [c.red, c.green, c.blue]).collect();

    Ok((palette_rgb, indices))
}
