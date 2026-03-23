use color_quant::NeuQuant;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Quantizer {
    Fast,
    Balanced,
    HighQuality,
}

/// Quantize a single RGBA frame.
/// Returns (palette_bytes, indexed_pixels).
/// palette_bytes is a flat RGB array (3 bytes per color).
pub fn quantize_frame(
    rgba: &[u8],
    width: u32,
    height: u32,
    num_colors: usize,
    quantizer: &Quantizer,
    dither: bool,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let num_pixels = (width * height) as usize;
    debug_assert_eq!(rgba.len(), num_pixels * 4, "RGBA buffer size mismatch");

    match quantizer {
        Quantizer::Fast => quantize_fast(rgba, num_colors),
        Quantizer::Balanced | Quantizer::HighQuality => {
            quantize_quality(rgba, width, height, num_colors, quantizer, dither)
        }
    }
}

fn quantize_fast(rgba: &[u8], num_colors: usize) -> Result<(Vec<u8>, Vec<u8>), String> {
    // NeuQuant sample factor: 1 = best quality, 10 = fast
    let sample_factor = 10;
    let nq = NeuQuant::new(sample_factor, num_colors, rgba);
    let palette = nq.color_map_rgb();

    // Cache [r,g,b,a] → palette index. Screen recordings have many repeated
    // pixel values (UI chrome, solid backgrounds), so the hit rate is high and
    // most pixels avoid the O(num_colors) linear scan inside index_of entirely.
    let mut cache: HashMap<[u8; 4], u8> = HashMap::new();
    let indices: Vec<u8> = rgba
        .chunks_exact(4)
        .map(|px| {
            let key = [px[0], px[1], px[2], px[3]];
            *cache.entry(key).or_insert_with(|| nq.index_of(px) as u8)
        })
        .collect();

    Ok((palette, indices))
}

fn quantize_quality(
    rgba: &[u8],
    width: u32,
    height: u32,
    num_colors: usize,
    quantizer: &Quantizer,
    dither: bool,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    // Use quantette for Wu quantization (Balanced) or Wu + k-means (HighQuality).
    // quantette 0.3: construct via TryFrom<&RgbImage> after stripping alpha.
    use quantette::{ImagePipeline, QuantizeMethod};

    let rgb: Vec<u8> = rgba
        .chunks_exact(4)
        .flat_map(|px| [px[0], px[1], px[2]])
        .collect();
    let rgb_img = image::RgbImage::from_raw(width, height, rgb)
        .ok_or("failed to create RgbImage from RGBA bytes")?;

    let mut pipeline = ImagePipeline::try_from(&rgb_img)
        .map_err(|e| format!("quantette pipeline error: {e}"))?;

    let method = match quantizer {
        Quantizer::HighQuality => QuantizeMethod::kmeans(),
        _ => QuantizeMethod::wu(),
    };

    // PaletteSize is backed by u16 with MAX = 256.  Casting 256 to u8 wraps
    // to 0 and produces an empty palette — use PaletteSize::MAX for 256 colours.
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
