//! ファイルを復号し、向きと回転を反映したBGRAフレームへ変換する。
//! GIFは先頭フレームの先行通知とフレーム間の中止に対応する。

use crate::i18n::tr;
use image::{AnimationDecoder, Frame, ImageDecoder, ImageFormat, ImageReader, Limits};
use std::{fs::File, io::BufReader, path::Path};

const BUDGET: u64 = 128 * 1024 * 1024;

/// 表示用BGRAフレームと、向き・追加回転を反映した寸法。
/// Frameのバッファ型はRgbaImageだが、色順はRGBAではない。
pub struct Decoded {
    pub frames: Vec<Frame>,
    pub dimensions: [f32; 2],
}

fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(BUDGET);
    limits
}

pub fn decode(path: &Path, turns: u8) -> Result<Decoded, String> {
    decode_progressive(path, turns, |_| {}, || false)
}

/// GIFの先頭をpreviewへ通知し、全フレームを返す。
/// cancelledはGIFのフレーム間で確認し、静止画像の復号途中は中断しない。
pub fn decode_progressive(
    path: &Path,
    turns: u8,
    mut preview: impl FnMut(Frame),
    cancelled: impl Fn() -> bool,
) -> Result<Decoded, String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(ext.as_str(), "heic" | "heif" | "hif" | "avif") {
        return finish(vec![Frame::new(decode_heif(path)?)], turns);
    }
    let mut reader = ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    reader.limits(limits());
    let frames = if reader.format() == Some(ImageFormat::Gif) {
        let mut decoder = image::codecs::gif::GifDecoder::new(BufReader::new(
            File::open(path).map_err(|e| e.to_string())?,
        ))
        .map_err(|e| e.to_string())?;
        decoder.set_limits(limits()).map_err(|e| e.to_string())?;
        let mut frames = Vec::new();
        let mut bytes = 0u64;
        for frame in decoder.into_frames() {
            if cancelled() {
                return Err("cancelled".into());
            }
            let frame = frame.map_err(|e| e.to_string())?;
            bytes += frame.buffer().len() as u64;
            if bytes > BUDGET || frames.len() >= 1000 {
                return Err(tr!("gif-limit"));
            }
            if frames.is_empty() {
                preview(convert(frame.clone(), turns));
            }
            frames.push(frame);
        }
        frames
    } else {
        let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
        let orientation = decoder.orientation().map_err(|e| e.to_string())?;
        let (width, height) = decoder.dimensions();
        if u64::from(width) * u64::from(height) * 4 > BUDGET {
            return Err(tr!("image-limit"));
        }
        let mut image = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
        image.apply_orientation(orientation);
        vec![Frame::new(image.into_rgba8())]
    };
    finish(frames, turns)
}

fn convert(frame: Frame, turns: u8) -> Frame {
    let delay = frame.delay();
    let pixels = frame.into_buffer();
    let mut pixels = match turns % 4 {
        1 => image::imageops::rotate90(&pixels),
        2 => image::imageops::rotate180(&pixels),
        3 => image::imageops::rotate270(&pixels),
        _ => pixels,
    };
    for pixel in pixels.as_flat_samples_mut().samples.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Frame::from_parts(pixels, 0, 0, delay)
}

fn finish(frames: Vec<Frame>, turns: u8) -> Result<Decoded, String> {
    let converted: Vec<_> = frames.into_iter().map(|f| convert(f, turns)).collect();
    let first = converted.first().ok_or(tr!("no-frames"))?;
    let dimensions = [
        first.buffer().width() as f32,
        first.buffer().height() as f32,
    ];
    Ok(Decoded {
        frames: converted,
        dimensions,
    })
}

/// Read display geometry without decoding the pixel buffer (used for manga boundaries).
pub fn dimensions(path: &Path) -> Result<[u32; 2], String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(ext.as_str(), "heic" | "heif" | "hif" | "avif") {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let _lib = libheif_rs::LibHeif::new();
        let context =
            libheif_rs::HeifContext::read_from_bytes(&bytes).map_err(|e| e.to_string())?;
        let handle = context.primary_image_handle().map_err(|e| e.to_string())?;
        return Ok([handle.width(), handle.height()]);
    }
    let mut reader = ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    reader.limits(limits());
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let (w, h) = decoder.dimensions();
    use image::metadata::Orientation::*;
    let swapped = matches!(
        decoder.orientation().map_err(|e| e.to_string())?,
        Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH
    );
    Ok(if swapped { [h, w] } else { [w, h] })
}

fn decode_heif(path: &Path) -> Result<image::RgbaImage, String> {
    use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let lib = LibHeif::new();
    let context = HeifContext::read_from_bytes(&bytes).map_err(|e| e.to_string())?;
    let handle = context.primary_image_handle().map_err(|e| e.to_string())?;
    let (w, h) = (handle.width(), handle.height());
    if w > 16384 || h > 16384 || u64::from(w) * u64::from(h) * 4 > BUDGET {
        return Err(tr!("heif-limit"));
    }
    // libheif applies container irot/imir transforms and converts high bit depth
    // to the 8-bit RGBA output requested here. HDR tone mapping is separate.
    let image = lib
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgba), None)
        .map_err(|e| e.to_string())?;
    let plane = image
        .planes()
        .interleaved
        .ok_or(tr!("rgba-plane-missing"))?;
    let (w, h) = (image.width(), image.height());
    if w > 16384 || h > 16384 || u64::from(w) * u64::from(h) * 4 > BUDGET {
        return Err(tr!("image-too-large"));
    }
    let mut pixels = image::RgbaImage::new(w, h);
    for (destination, source) in pixels
        .as_flat_samples_mut()
        .samples
        .chunks_exact_mut(w as usize * 4)
        .zip(plane.data.chunks(plane.stride))
    {
        destination.copy_from_slice(&source[..w as usize * 4]);
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_bgra_and_rotates_without_modifying_source() {
        let path = std::env::temp_dir().join(format!("nagame-colour-{}.png", std::process::id()));
        let original = image::RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            }
        });
        original.save(&path).unwrap();
        let before = std::fs::read(&path).unwrap();
        let decoded = decode(&path, 1).unwrap();
        assert_eq!(decoded.dimensions, [1., 2.]);
        assert_eq!(
            decoded.frames[0].buffer().get_pixel(0, 0).0,
            [0, 0, 255, 255]
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn jpe_extension_decodes_as_jpeg() {
        let folder = std::env::temp_dir().join(format!("nagame-jpe-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("test.JPE");
        image::RgbImage::from_pixel(8, 6, image::Rgb([250, 10, 20]))
            .save_with_format(&path, ImageFormat::Jpeg)
            .unwrap();
        let decoded = decode(&path, 0).unwrap();
        assert_eq!(decoded.dimensions, [8., 6.]);
        let pixel = decoded.frames[0].buffer().get_pixel(0, 0).0;
        assert!(pixel[2] > 240 && pixel[0] < 30);
        std::fs::remove_dir_all(folder).unwrap();
    }
    #[test]
    fn corrupt_file_returns_error() {
        let path = std::env::temp_dir().join(format!("nagame-corrupt-{}.png", std::process::id()));
        std::fs::write(&path, b"not an image").unwrap();
        assert!(decode(&path, 0).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn gif_keeps_frames_and_timing() {
        let path =
            std::env::temp_dir().join(format!("nagame-animation-{}.gif", std::process::id()));
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(File::create(&path).unwrap());
            for color in [[255, 0, 0, 255], [0, 255, 0, 255]] {
                encoder
                    .encode_frame(Frame::from_parts(
                        image::RgbaImage::from_pixel(2, 2, image::Rgba(color)),
                        0,
                        0,
                        image::Delay::from_numer_denom_ms(100, 1),
                    ))
                    .unwrap();
            }
        }
        let decoded = decode(&path, 0).unwrap();
        assert_eq!(decoded.frames.len(), 2);
        assert_eq!(decoded.frames[1].delay().numer_denom_ms(), (100, 1));
        assert_eq!(
            decoded.frames[1].buffer().get_pixel(0, 0).0,
            [0, 255, 0, 255]
        );
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
mod modern_format_tests {
    use super::*;
    #[test]
    fn avif_and_heic_decode_real_encoded_images() {
        use libheif_rs::{
            Channel, ColorSpace, CompressionFormat, EncoderQuality, HeifContext, Image, LibHeif,
            RgbChroma,
        };
        let lib = LibHeif::new();
        for (format, extension, bits) in [
            (CompressionFormat::Av1, "avif", 8),
            (CompressionFormat::Av1, "avif", 10),
            (CompressionFormat::Hevc, "heic", 8),
            (CompressionFormat::Hevc, "heic", 10),
        ] {
            let mut image = Image::new(32, 24, ColorSpace::Rgb(RgbChroma::C444)).unwrap();
            for (channel, value) in [
                (Channel::R, 220u8),
                (Channel::G, 20),
                (Channel::B, 10),
                (Channel::Alpha, 96),
            ] {
                image.create_plane(channel, 32, 24, bits).unwrap();
                let mut planes = image.planes_mut();
                let plane = match channel {
                    Channel::R => planes.r.take(),
                    Channel::G => planes.g.take(),
                    Channel::B => planes.b.take(),
                    _ => planes.a.take(),
                }
                .unwrap();
                if bits == 8 {
                    plane.data.fill(value);
                } else {
                    for sample in plane.data.as_chunks_mut::<2>().0 {
                        *sample = (((u32::from(value) * 1023) / 255) as u16).to_le_bytes();
                    }
                }
            }
            let mut context = HeifContext::new().unwrap();
            let mut encoder = lib.encoder_for_format(format).unwrap();
            encoder.set_quality(EncoderQuality::Lossy(90)).unwrap();
            context.encode_image(&image, &mut encoder, None).unwrap();
            let path = std::env::temp_dir().join(format!(
                "nagame-modern-{}.{}",
                std::process::id(),
                extension
            ));
            context.write_to_file(path.to_str().unwrap()).unwrap();
            let encoded_context = HeifContext::read_from_file(path.to_str().unwrap()).unwrap();
            assert_eq!(
                encoded_context
                    .primary_image_handle()
                    .unwrap()
                    .luma_bits_per_pixel(),
                bits
            );
            let decoded = decode(&path, 0).unwrap();
            assert_eq!(decoded.dimensions, [32., 24.]);
            let color = decoded.frames[0].buffer().get_pixel(16, 12).0;
            assert!(
                color[2] > 180 && color[0] < 50,
                "expected red BGRA, got {color:?}"
            );
            assert!(color[3].abs_diff(96) <= 4, "alpha={}", color[3]);
            if let Some(directory) = std::env::var_os("NAGAME_CODEC_FIXTURE_DIR") {
                let directory = std::path::PathBuf::from(directory);
                std::fs::create_dir_all(&directory).unwrap();
                std::fs::copy(&path, directory.join(format!("red-{bits}.{extension}"))).unwrap();
            }
            std::fs::remove_file(path).unwrap();
        }
    }
    #[test]
    fn gif_preview_precedes_completion_and_load_can_cancel() {
        let path =
            std::env::temp_dir().join(format!("nagame-progressive-{}.gif", std::process::id()));
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(File::create(&path).unwrap());
            for _ in 0..3 {
                encoder
                    .encode_frame(Frame::new(image::RgbaImage::new(2, 2)))
                    .unwrap();
            }
        }
        let shown = std::cell::Cell::new(false);
        let result = decode_progressive(&path, 0, |_| shown.set(true), || shown.get());
        assert!(shown.get());
        assert!(result.is_err());
        std::fs::remove_file(path).unwrap();
    }
}
