pub mod quantize;
pub mod worker;

use crate::capture::RawFrame;
use gif::{Encoder, Frame, Repeat};
use image::imageops::FilterType;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

pub use quantize::Quantizer;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionProfile {
    pub name: String,
    pub quantizer: Quantizer,
    /// Number of palette colors: 16, 32, 64, 128, or 256
    pub colors: u16,
    pub dither: bool,
    /// None = original width; value <= 100 treated as percentage
    pub scale_width: Option<u32>,
    /// None = maintain aspect ratio
    pub scale_height: Option<u32>,
    /// None = use recording FPS
    pub fps_override: Option<f64>,
}

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("Empty frame buffer")]
    EmptyFrames,
    #[error("GIF encode error: {0}")]
    GifError(String),
    #[error("Quantization error: {0}")]
    QuantizeError(String),
    #[error("Image error: {0}")]
    ImageError(#[from] image::ImageError),
}

/// Encode frames to GIF bytes using the given profile.
/// Reports progress (0.0–1.0) via the provided sender.
pub fn encode_gif(
    frames: &[RawFrame],
    profile: &CompressionProfile,
    progress_tx: mpsc::Sender<f32>,
) -> Result<Vec<u8>, EncodeError> {
    if frames.is_empty() {
        return Err(EncodeError::EmptyFrames);
    }

    let source_fps = profile.fps_override.unwrap_or(10.0);
    let frame_delay_cs = (100.0 / source_fps).round() as u16;

    let first = &frames[0];
    let (out_width, out_height) = compute_output_dimensions(
        first.width,
        first.height,
        profile.scale_width,
        profile.scale_height,
    );

    // Phase 1: parallel resize + quantize — each frame is independent.
    let quantized: Vec<(Vec<u8>, Vec<u8>)> = frames
        .par_iter()
        .map(|raw_frame| -> Result<(Vec<u8>, Vec<u8>), EncodeError> {
            let rgba_pixels =
                if out_width != raw_frame.width || out_height != raw_frame.height {
                    let img = image::RgbaImage::from_raw(
                        raw_frame.width,
                        raw_frame.height,
                        raw_frame.rgba.clone(),
                    )
                    .ok_or_else(|| EncodeError::GifError("invalid frame dimensions".into()))?;
                    image::imageops::resize(&img, out_width, out_height, FilterType::Lanczos3)
                        .into_raw()
                } else {
                    raw_frame.rgba.clone()
                };

            let (palette, indices) = quantize::quantize_frame(
                &rgba_pixels,
                out_width,
                out_height,
                profile.colors as usize,
                &profile.quantizer,
                profile.dither,
            )
            .map_err(EncodeError::QuantizeError)?;

            Ok((palette, indices))
        })
        .collect::<Result<_, _>>()?;

    // Phase 2: sequential GIF write (Encoder is not Send).
    // Progress is reported here — the write phase is fast but provides
    // per-frame granularity so the UI doesn't stall at 0%.
    let mut output = Vec::new();
    {
        let mut encoder =
            Encoder::new(&mut output, out_width as u16, out_height as u16, &[])
                .map_err(|e| EncodeError::GifError(e.to_string()))?;
        encoder
            .set_repeat(Repeat::Infinite)
            .map_err(|e| EncodeError::GifError(e.to_string()))?;

        let total = quantized.len() as f32;
        for (i, (palette, indices)) in quantized.into_iter().enumerate() {
            let mut gif_frame = Frame::default();
            gif_frame.width = out_width as u16;
            gif_frame.height = out_height as u16;
            gif_frame.delay = frame_delay_cs;
            gif_frame.palette = Some(palette);
            gif_frame.buffer = std::borrow::Cow::Owned(indices);

            encoder
                .write_frame(&gif_frame)
                .map_err(|e| EncodeError::GifError(e.to_string()))?;

            let _ = progress_tx.try_send((i + 1) as f32 / total);
        }
    }

    Ok(output)
}

fn compute_output_dimensions(
    width: u32,
    height: u32,
    scale_width: Option<u32>,
    scale_height: Option<u32>,
) -> (u32, u32) {
    match (scale_width, scale_height) {
        (None, None) => (width, height),
        (Some(sw), None) => {
            let out_w = if sw <= 100 { (width * sw / 100).max(1) } else { sw };
            let out_h = ((out_w as f64 / width as f64) * height as f64).round() as u32;
            (out_w, out_h.max(1))
        }
        (None, Some(sh)) => {
            let out_h = if sh <= 100 { (height * sh / 100).max(1) } else { sh };
            let out_w = ((out_h as f64 / height as f64) * width as f64).round() as u32;
            (out_w.max(1), out_h)
        }
        (Some(sw), Some(sh)) => {
            let out_w = if sw <= 100 { (width * sw / 100).max(1) } else { sw };
            let out_h = if sh <= 100 { (height * sh / 100).max(1) } else { sh };
            (out_w, out_h)
        }
    }
}
