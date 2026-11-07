//! ファイルを復号し、向きと回転を反映したBGRAフレームへ変換する。
//! GIFは `GifFrames` で1フレームずつ取り出し、アニメーションの逐次再生に使う。

use crate::i18n::tr;
use image::{AnimationDecoder, Frame, ImageDecoder, ImageFormat, ImageReader, Limits};
use std::{fs::File, io::BufReader, path::Path};

/// 呼び出し側が上限を決めない場合（性能計測・テスト）の展開上限。1辺16,384ピクセルのRGBA。
pub const DEFAULT_LIMIT: u64 = 16384 * 16384 * 4;

/// 表示用BGRAフレームと、向き・追加回転を反映した寸法。
/// Frameのバッファ型はRgbaImageだが、色順はRGBAではない。
pub struct Decoded {
    pub frames: Vec<Frame>,
    pub dimensions: [f32; 2],
}

fn limits(max_alloc: u64) -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(max_alloc);
    limits
}

fn limit_error(limit: u64) -> String {
    let limit = limit / (1024 * 1024);
    tr!("image-limit", limit = limit)
}

/// 内容から判定したGIFか。拡張子は見ない。
#[allow(dead_code)] // decode-benchもこのモジュールを使うが、アニメーションは開かない。
pub fn is_gif(path: &Path) -> bool {
    ImageReader::open(path)
        .and_then(|reader| reader.with_guessed_format())
        .is_ok_and(|reader| reader.format() == Some(ImageFormat::Gif))
}

/// 全フレームを返す。GIFの合計とその他の画像は `DEFAULT_LIMIT` まで。
/// decode-benchとテストが使う。
#[allow(dead_code)]
pub fn decode(path: &Path, turns: u8) -> Result<Decoded, String> {
    decode_limited(path, turns, DEFAULT_LIMIT, true, || false)
}

/// 静止画、またはGIFの先頭フレーム（all_framesがfalseの場合）を返す。
/// limitは1枚の展開上限。all_framesのGIFは全フレームの合計にも適用する。
/// cancelledはGIFのフレーム間で確認し、静止画像の復号途中は中断しない。
pub fn decode_limited(
    path: &Path,
    turns: u8,
    limit: u64,
    all_frames: bool,
    cancelled: impl Fn() -> bool,
) -> Result<Decoded, String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(ext.as_str(), "heic" | "heif" | "hif" | "avif") {
        return finish(vec![Frame::new(decode_heif(path, limit)?)], turns);
    }
    let mut reader = ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    reader.limits(limits(limit));
    if reader.format() == Some(ImageFormat::Gif) {
        let mut frames = Vec::new();
        let mut bytes = 0u64;
        for frame in gif_frames(path, turns, limit)? {
            if cancelled() {
                return Err("cancelled".into());
            }
            let frame = frame?;
            bytes += frame.buffer().len() as u64;
            if bytes > limit {
                return Err(limit_error(limit));
            }
            frames.push(frame);
            if !all_frames {
                break;
            }
        }
        return dimensions_of(frames);
    }
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) * 4 > limit {
        return Err(limit_error(limit));
    }
    let mut image = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    image.apply_orientation(orientation);
    finish(vec![Frame::new(image.into_rgba8())], turns)
}

/// GIFのフレームを先頭から順に、回転済みBGRAで返す。各フレームはキャンバス全体の大きさ。
/// GIFの差分描画と「前の状態に戻す」処理は前のフレームに依存するため、途中から読めない。
pub struct GifFrames {
    frames: image::Frames<'static>,
    turns: u8,
    limit: u64,
    done: bool,
}
impl Iterator for GifFrames {
    type Item = Result<Frame, String>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let result = match self.frames.next()? {
            Ok(frame) => {
                let (width, height) = frame.buffer().dimensions();
                check_size(width, height)
                    .and_then(|()| {
                        if u64::from(width) * u64::from(height) * 4 > self.limit {
                            Err(limit_error(self.limit))
                        } else {
                            Ok(())
                        }
                    })
                    .map(|()| convert(frame, self.turns))
            }
            Err(error) => Err(error.to_string()),
        };
        self.done = result.is_err();
        Some(result)
    }
}

/// limitは1フレームの展開上限。デコーダー内部ではキャンバスと作業用に約3枚分を確保する。
pub fn gif_frames(path: &Path, turns: u8, limit: u64) -> Result<GifFrames, String> {
    let mut decoder = image::codecs::gif::GifDecoder::new(BufReader::new(
        File::open(path).map_err(|e| e.to_string())?,
    ))
    .map_err(|e| e.to_string())?;
    decoder
        .set_limits(limits(limit.saturating_mul(3)))
        .map_err(|e| e.to_string())?;
    Ok(GifFrames {
        frames: decoder.into_frames(),
        turns,
        limit,
        done: false,
    })
}

/// 時計回りにturns×90°回転し、RGBAをBGRAへ並べ替える。遅延は元の値を保つ。
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

/// 幅か高さが0の画像は、行単位の処理（見開き合成など）で扱えないため拒否する。
fn check_size(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err(tr!("image-size-invalid"));
    }
    Ok(())
}

fn finish(frames: Vec<Frame>, turns: u8) -> Result<Decoded, String> {
    dimensions_of(frames.into_iter().map(|f| convert(f, turns)).collect())
}

fn dimensions_of(frames: Vec<Frame>) -> Result<Decoded, String> {
    let first = frames.first().ok_or(tr!("no-frames"))?;
    let (width, height) = first.buffer().dimensions();
    check_size(width, height)?;
    let dimensions = [width as f32, height as f32];
    Ok(Decoded { frames, dimensions })
}

/// 画素を展開せずに、画像の向き情報を反映した幅と高さを読む（利用者の追加回転は含まない）。
/// 見開きで横長のページを単独表示するかの判定と、情報画面に使う。
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
    reader.limits(limits(DEFAULT_LIMIT));
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let (w, h) = decoder.dimensions();
    use image::metadata::Orientation::*;
    let swapped = matches!(
        decoder.orientation().map_err(|e| e.to_string())?,
        Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH
    );
    Ok(if swapped { [h, w] } else { [w, h] })
}

fn decode_heif(path: &Path, limit: u64) -> Result<image::RgbaImage, String> {
    use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let lib = LibHeif::new();
    let context = HeifContext::read_from_bytes(&bytes).map_err(|e| e.to_string())?;
    let handle = context.primary_image_handle().map_err(|e| e.to_string())?;
    let (w, h) = (handle.width(), handle.height());
    if w > 16384 || h > 16384 || u64::from(w) * u64::from(h) * 4 > limit {
        return Err(tr!("heif-limit"));
    }
    // libheifはコンテナのirot/imir（回転・反転）を適用し、高ビット深度もここで要求した
    // 8bitのRGBAへ変換する。HDRのトーンマッピングは行わない。
    let image = lib
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgba), None)
        .map_err(|e| e.to_string())?;
    let plane = image
        .planes()
        .interleaved
        .ok_or(tr!("rgba-plane-missing"))?;
    let (w, h) = (image.width(), image.height());
    check_size(w, h)?;
    if w > 16384 || h > 16384 || u64::from(w) * u64::from(h) * 4 > limit {
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
    fn gif_frames_stream_rotated_bgra_and_limits_apply_per_frame() {
        let path =
            std::env::temp_dir().join(format!("nagame-gif-stream-{}.gif", std::process::id()));
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(File::create(&path).unwrap());
            for (color, ms) in [([255, 0, 0, 255], 0), ([0, 0, 255, 255], 70)] {
                encoder
                    .encode_frame(Frame::from_parts(
                        image::RgbaImage::from_pixel(3, 2, image::Rgba(color)),
                        0,
                        0,
                        image::Delay::from_numer_denom_ms(ms, 1),
                    ))
                    .unwrap();
            }
        }
        assert!(is_gif(&path));
        let frames: Vec<_> = gif_frames(&path, 1, DEFAULT_LIMIT)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].buffer().dimensions(), (2, 3));
        assert_eq!(frames[0].buffer().get_pixel(0, 0).0, [0, 0, 255, 255]);
        assert_eq!(frames[1].buffer().get_pixel(1, 2).0, [255, 0, 0, 255]);
        assert_eq!(frames[0].delay().numer_denom_ms(), (0, 1));
        assert_eq!(frames[1].delay().numer_denom_ms(), (70, 1));
        // 1フレームは24バイト。先頭フレームだけなら上限に収まるが、GIF全体は収まらない。
        let first = decode_limited(&path, 0, 24, false, || false).unwrap();
        assert_eq!(first.frames.len(), 1);
        assert_eq!(first.dimensions, [3., 2.]);
        assert!(decode_limited(&path, 0, 24, true, || false).is_err());
        let mut tiny = gif_frames(&path, 0, 23).unwrap();
        assert!(tiny.next().unwrap().is_err());
        assert!(tiny.next().is_none());
        let cancelled = decode_limited(&path, 0, DEFAULT_LIMIT, true, || true);
        assert_eq!(cancelled.err().as_deref(), Some("cancelled"));
        std::fs::remove_file(path).unwrap();
    }
}
