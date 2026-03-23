/// GIF encoder — inlined from src-tauri/src/compression/mod.rs.
/// Kept as a standalone copy so the spike has no Tauri dep.

use crate::Frame;
use gif::{Encoder, Frame as GifFrame, Repeat};

pub fn encode_gif(
    frames: &[Frame],
    src_width: u32,
    src_height: u32,
    out_width: u32,
    out_height: u32,
    fps: f64,
    colors: usize,
    quantizer: &crate::quantize::Quantizer,
    dither: bool,
) -> Result<Vec<u8>, String> {
    if frames.is_empty() {
        return Err("No frames".into());
    }

    // GIF delay is in 1/100s units
    let frame_delay_cs = (100.0 / fps).round() as u16;

    let mut output = Vec::new();
    {
        let mut encoder = Encoder::new(&mut output, out_width as u16, out_height as u16, &[])
            .map_err(|e| e.to_string())?;
        encoder.set_repeat(Repeat::Infinite).map_err(|e| e.to_string())?;

        for frame in frames {
            // Scale if needed
            let rgba = if out_width != src_width || out_height != src_height {
                let img = image::RgbaImage::from_raw(src_width, src_height, frame.rgba.clone())
                    .ok_or("invalid frame buffer")?;
                image::imageops::resize(
                    &img,
                    out_width,
                    out_height,
                    image::imageops::FilterType::Lanczos3,
                )
                .into_raw()
            } else {
                frame.rgba.clone()
            };

            // Quantize
            let (palette, indices) = crate::quantize::quantize_frame(
                &rgba,
                out_width,
                out_height,
                colors,
                *quantizer,
                dither,
            )?;

            // Build GIF frame with local colour table
            let mut gif_frame = GifFrame::default();
            gif_frame.width = out_width as u16;
            gif_frame.height = out_height as u16;
            gif_frame.delay = frame_delay_cs;
            gif_frame.palette = Some(palette);
            gif_frame.buffer = std::borrow::Cow::Owned(indices);

            encoder.write_frame(&gif_frame).map_err(|e| e.to_string())?;
        }
    }

    Ok(output)
}
