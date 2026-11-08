//! Viewerのファイル選択、画像保存・コピー、情報画面。
//! 出力は表示用リサイズ画像を使わず、元解像度のBGRAからRGBAへ戻す。

use crate::i18n::{self, tr};
use crate::{Command, Viewer};
use gpui::{
    AnyElement, Context, PathPromptOptions, RenderImage, div, prelude::*, px, relative, rgb, rgba,
};
use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::Arc,
};

/// GIFへ書き出すフレームの出どころ。GIF以外の形式では、`Viewer.image` の先頭フレーム
/// （アニメーションなら最初のコマ）だけを使う。
pub(crate) enum Frames {
    /// 表示中の画像のフレーム（静止画・見開き）。
    Image,
    /// 全フレームを保持したアニメーション。
    Clip(Arc<crate::animation::Clip>),
    /// 全フレームを保持していないアニメーション（1周目のデコード中・逐次デコード中）。
    /// 元ファイルから順に読み直す（回転、1フレームの上限）。
    File(PathBuf, u8, u64),
}

fn rgba_pixels(image: &RenderImage, index: usize) -> Result<image::RgbaImage, String> {
    let size = image.size(index);
    let mut bytes = image
        .as_bytes(index)
        .ok_or(tr!("image-data-missing"))?
        .to_vec();
    for pixel in bytes.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    image::RgbaImage::from_raw(u32::from(size.width), u32::from(size.height), bytes)
        .ok_or(tr!("image-size-invalid"))
}
fn png(image: &RenderImage) -> Result<Vec<u8>, String> {
    let mut output = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba_pixels(image, 0)?)
        .write_to(&mut output, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(output.into_inner())
}
type RgbaFrame = Result<(image::RgbaImage, image::Delay), String>;
/// 先頭からフレームをRGBAと遅延で返す。逐次デコード中のアニメーションはファイルから
/// 1フレームずつ読み直すため、全フレームを展開した状態では持たない。
fn rgba_frames<'a>(
    image: &'a RenderImage,
    frames: &'a Frames,
) -> Result<Box<dyn Iterator<Item = RgbaFrame> + 'a>, String> {
    let of = |image: &'a RenderImage| {
        (0..image.frame_count())
            .map(move |index| Ok((rgba_pixels(image, index)?, image.delay(index))))
    };
    Ok(match frames {
        Frames::Image => Box::new(of(image)),
        Frames::Clip(clip) => Box::new(clip.images.iter().flat_map(move |image| of(image))),
        Frames::File(path, turns, limit) => Box::new(
            crate::decoder::gif_frames(path, *turns, *limit)?.map(|frame| {
                let frame = frame?;
                let delay = frame.delay();
                let mut pixels = frame.into_buffer();
                for pixel in pixels.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
                Ok((pixels, delay))
            }),
        ),
    })
}
/// PDFのページを基準解像度（limitまで）で描き直し、追加回転を反映する。
fn full_page(path: &Path, page: u32, turns: u8, limit: u64) -> Result<Arc<RenderImage>, String> {
    let rendered = crate::pdf::render(
        path,
        page,
        crate::pdf::Mode::Base { limit },
        crate::pdf::Class::Open,
        || false,
    )
    .map_err(crate::pdf::Failure::message)?;
    let pixels = match turns % 4 {
        1 => image::imageops::rotate90(&rendered.pixels),
        2 => image::imageops::rotate180(&rendered.pixels),
        3 => image::imageops::rotate270(&rendered.pixels),
        _ => rendered.pixels,
    };
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
}
/// 拡張子から形式を決めて保存する。GIFは全フレームを無限ループで、それ以外は先頭フレームを
/// 書き出す。
fn save(image: &RenderImage, frames: &Frames, path: &Path) -> Result<(), String> {
    let format = if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jpe"))
    {
        image::ImageFormat::Jpeg
    } else {
        image::ImageFormat::from_path(path).map_err(|_| tr!("save-extension"))?
    };
    if !matches!(
        format,
        image::ImageFormat::Png
            | image::ImageFormat::Jpeg
            | image::ImageFormat::Gif
            | image::ImageFormat::Bmp
            | image::ImageFormat::WebP
            | image::ImageFormat::Tiff
            | image::ImageFormat::Ico
    ) {
        return Err(tr!("save-unsupported"));
    }
    let mut output = Cursor::new(Vec::new());
    if format == image::ImageFormat::Gif {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut output);
        encoder
            .set_repeat(image::codecs::gif::Repeat::Infinite)
            .map_err(|e| e.to_string())?;
        for frame in rgba_frames(image, frames)? {
            let (pixels, delay) = frame?;
            encoder
                .encode_frame(image::Frame::from_parts(pixels, 0, 0, delay))
                .map_err(|e| e.to_string())?;
        }
    } else {
        let pixels = image::DynamicImage::ImageRgba8(rgba_pixels(image, 0)?);
        if format == image::ImageFormat::Jpeg {
            image::DynamicImage::ImageRgb8(pixels.into_rgb8()).write_to(&mut output, format)
        } else {
            pixels.write_to(&mut output, format)
        }
        .map_err(|e| e.to_string())?;
    }
    // 保存先に触れる前にすべてエンコードし、一時ファイルから置き換える。エンコードや書き込みに
    // 失敗しても、既存のファイルが途中で切れることはない。
    let parent = path.parent().ok_or(tr!("save-path-invalid"))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let temporary = parent.join(format!(".nagame-export-{}-{stamp}.tmp", std::process::id()));
    let result = (|| {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(output.get_ref())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|e| e.to_string())
}
// GPUI 0.2.2は、Linuxで他のアプリにクリップボードの内容をテキストとしてしか提示しない。
// image/pngの実データは、デスクトップのクリップボードツール（wl-copy・xclip）で渡す。
#[cfg(target_os = "linux")]
fn publish_png(bytes: &[u8]) -> Result<(), String> {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v == "wayland");
    let (program, args): (&str, &[&str]) = if wayland {
        ("wl-copy", &["--type", "image/png"])
    } else {
        (
            "xclip",
            &["-selection", "clipboard", "-t", "image/png", "-i"],
        )
    };
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            i18n::clipboard_tool_error(
                if wayland { "wl-clipboard" } else { "xclip" },
                &e.to_string(),
            )
        })?;
    let write = child
        .stdin
        .take()
        .ok_or(tr!("clipboard-input-failed"))?
        .write_all(bytes);
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    write.map_err(|e| i18n::clipboard_write_error(&e.to_string()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(i18n::clipboard_copy_error(
            String::from_utf8_lossy(&output.stderr).trim(),
        ))
    }
}
impl Viewer {
    pub(super) fn prompt_folder(&mut self, cx: &mut Context<Self>) {
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(tr!("open-folder").into()),
        });
        cx.spawn(async move |viewer, cx| {
            let result = selection.await;
            let _ = viewer.update(cx, |this, cx| {
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            this.open(path, cx);
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => this.error = Some(error.to_string()),
                    Err(error) => this.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn copy_image(&mut self, cx: &mut Context<Self>) {
        let Some(image) = self.image.clone() else {
            self.notice = Some(tr!("no-open-image"));
            return;
        };
        let task = cx.background_executor().spawn(async move {
            let bytes = png(&image)?;
            #[cfg(target_os = "linux")]
            publish_png(&bytes)?;
            Ok::<_, String>(bytes)
        });
        cx.spawn(async move |viewer, cx| {
            let result = task.await;
            let _ = viewer.update(cx, |this, cx| {
                match result {
                    Ok(_bytes) => {
                        #[cfg(not(target_os = "linux"))]
                        {
                            let image = gpui::Image::from_bytes(gpui::ImageFormat::Png, _bytes);
                            cx.write_to_clipboard(gpui::ClipboardItem::new_image(&image));
                        }
                        this.notice = Some(tr!("image-copied"));
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn save_as(&mut self, cx: &mut Context<Self>) {
        let Some(image) = self.image.clone() else {
            self.notice = Some(tr!("no-open-image"));
            return;
        };
        let frames = match &self.player {
            None => Frames::Image,
            Some(player) => match player.clip() {
                Some(clip) => Frames::Clip(clip.clone()),
                None => {
                    // 再生のために一度デコードできたフレームなので、その後に予算が下がっても
                    // （メモリー逼迫時など）書き出しを失敗させない。
                    let (path, turns) = player.source();
                    Frames::File(path.to_path_buf(), turns, crate::decoder::DEFAULT_LIMIT)
                }
            },
        };
        let stem = self
            .album
            .current_path()
            .and_then(|p| p.file_stem())
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let name = if self.pdf.is_some() {
            // PDFはページ番号を含める。桁は総ページ数に合わせ、最低3桁にする。
            let digits = self.album.entries.len().to_string().len().max(3);
            let first = self.album.index + 1;
            if self.pages > 1 {
                format!("{stem}_p{first:0digits$}-{:0digits$}_view.png", first + 1)
            } else {
                format!("{stem}_p{first:0digits$}_view.png")
            }
        } else {
            format!("{stem}_view.png")
        };
        let parent = self
            .album
            .location()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        // 逼迫中に縮めて描いたPDFのページは、逼迫が収まっていれば現在の予算で描き直して保存する
        // （逼迫中は表示中の画像を保存する）。
        let redraw = self
            .pdf
            .as_ref()
            .filter(|pdf| pdf.reduced && self.pages == 1 && !crate::memory::under_pressure())
            .map(|pdf| (pdf.path.clone(), pdf.page, self.turns, self.budget.still));
        let selection = cx.prompt_for_new_path(&parent, Some(&name));
        let executor = cx.background_executor().clone();
        cx.spawn(async move |viewer, cx| {
            let path = match selection.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return,
                Ok(Err(error)) => {
                    let _ = viewer.update(cx, |this, cx| {
                        this.error = Some(error.to_string());
                        cx.notify();
                    });
                    return;
                }
                Err(error) => {
                    let _ = viewer.update(cx, |this, cx| {
                        this.error = Some(error.to_string());
                        cx.notify();
                    });
                    return;
                }
            };
            let output = path.clone();
            let task = executor.spawn(async move {
                let image = match redraw {
                    Some((path, page, turns, limit)) => full_page(&path, page, turns, limit)?,
                    None => image,
                };
                save(&image, &frames, &output)
            });
            let result = task.await;
            let _ = viewer.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.notice = Some(i18n::saved_image(&path)),
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn show_information(&mut self, cx: &mut Context<Self>) {
        if self.info_visible {
            self.info_visible = false;
            return;
        }
        let Some(entry) = self.album.current().cloned() else {
            self.notice = Some(tr!("no-open-image"));
            return;
        };
        // 描けなかったPDFのページには、画像として読む情報がない。
        if entry.page.is_some() && self.pdf.is_none() {
            self.notice = Some(tr!("no-open-image"));
            return;
        }
        let path = entry.path.clone();
        self.info_visible = true;
        self.info_text = Some(tr!("loading-info"));
        let dimensions = self.dimensions;
        if let (Some(page), Some(pdf)) = (entry.page, &self.pdf) {
            let bytes = std::fs::metadata(&path).map_or(0, |metadata| metadata.len());
            self.info_text = Some(i18n::pdf_information(i18n::PdfInformation {
                path: &path,
                bytes,
                page: page.index as usize + 1,
                total: self.album.entries.len(),
                size: page.size,
                displayed: dimensions,
                dpi: pdf.scale * 72.,
                vector: pdf.vector,
                pages: self.pages,
            }));
            return;
        }
        let frames = match &self.player {
            Some(player) => player.frames(),
            None => self.image.as_ref().map(|p| p.frame_count()),
        };
        let pages = self.pages;
        let task = cx.background_executor().spawn(async move {
            let metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
            let [w, h] = crate::decoder::dimensions(&path)?;
            Ok::<_, String>(i18n::image_information(i18n::ImageInformation {
                path: &path,
                format: &crate::decoder::format_name(&path)?,
                bytes: metadata.len(),
                original: [w, h],
                displayed: dimensions,
                frames,
                pages,
            }))
        });
        cx.spawn(async move |viewer, cx| {
            let result = task.await;
            let _ = viewer.update(cx, |this, cx| {
                this.info_text = Some(result.unwrap_or_else(|e| e));
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn information_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .absolute()
            .inset_0()
            .bg(rgba(0x00000099))
            .flex()
            .items_center()
            .justify_center()
            .occlude()
            .child(
                div()
                    .id("image-info")
                    .w(px(700.))
                    .max_h(relative(0.9))
                    .overflow_y_scroll()
                    .p_5()
                    .bg(rgb(0x252a33))
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(0x606a79))
                    .child(div().text_xl().mb_4().child(tr!("image-info")))
                    .children(
                        self.info_text
                            .clone()
                            .unwrap_or_default()
                            .lines()
                            .map(|line| div().min_h(px(20.)).child(line.to_owned()))
                            .collect::<Vec<_>>(),
                    )
                    .child(div().flex().justify_end().mt_4().child(self.button(
                        tr!("close"),
                        Command::Info,
                        cx,
                    ))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn exported_png_and_jpeg_have_correct_colors_and_codec_failure_preserves_file() {
        let data = image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 255, 255])); // BGRAの赤
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(data)]));
        let root = std::env::temp_dir().join(format!("nagame-export-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for name in ["image.png", "image.jpg", "image.JPE"] {
            let path = root.join(name);
            save(&image, &Frames::Image, &path).unwrap();
            let decoded = image::load_from_memory(&std::fs::read(&path).unwrap())
                .unwrap()
                .into_rgba8();
            let color = decoded.get_pixel(3, 3).0;
            assert!(color[0] > 240 && color[2] < 10);
        }
        let unsupported = root.join("keep.heic");
        std::fs::write(&unsupported, b"keep original").unwrap();
        assert!(save(&image, &Frames::Image, &unsupported).is_err());
        assert_eq!(std::fs::read(unsupported).unwrap(), b"keep original");
        let clipboard = image::load_from_memory(&png(&image).unwrap())
            .unwrap()
            .into_rgba8();
        assert_eq!(clipboard.get_pixel(0, 0).0, [255, 0, 0, 255]);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn animations_export_every_frame_from_memory_or_by_redecoding_the_file() {
        use crate::animation::{Limits, Player, Resume, Stream};
        let root = std::env::temp_dir().join(format!("nagame-export-gif-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.gif");
        {
            let mut encoder =
                image::codecs::gif::GifEncoder::new(std::fs::File::create(&source).unwrap());
            for (color, ms) in [([255, 0, 0, 255], 40), ([0, 0, 255, 255], 80)] {
                encoder
                    .encode_frame(image::Frame::from_parts(
                        image::RgbaImage::from_pixel(3, 2, image::Rgba(color)),
                        0,
                        0,
                        image::Delay::from_numer_denom_ms(ms, 1),
                    ))
                    .unwrap();
            }
        }
        let limits = Limits {
            frame: crate::decoder::DEFAULT_LIMIT,
            resident: u64::MAX,
            ahead: u64::MAX,
        };
        let (sender, stored) = std::sync::mpsc::channel();
        let stream = Stream::start(
            source.clone(),
            1,
            limits,
            Resume::default(),
            false,
            move |c| sender.send(c).unwrap(),
        );
        let first = stream.first(|| false).unwrap().unwrap();
        let clip = stored
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        drop(Player::from_clip(
            clip.clone(),
            source.clone(),
            1,
            std::time::Instant::now(),
        ));
        for (name, frames) in [
            ("clip.gif", Frames::Clip(clip)),
            (
                "file.gif",
                Frames::File(source.clone(), 1, crate::decoder::DEFAULT_LIMIT),
            ),
        ] {
            let path = root.join(name);
            save(&first.image, &frames, &path).unwrap();
            let decoded = crate::decoder::decode(&path, 0).unwrap();
            assert_eq!(decoded.frames.len(), 2, "{name}");
            // 90°回転、元の色（デコード後はBGRA）、遅延が保たれている。
            assert_eq!(decoded.dimensions, [2., 3.], "{name}");
            assert_eq!(
                decoded.frames[0].buffer().get_pixel(0, 0).0,
                [0, 0, 255, 255]
            );
            assert_eq!(decoded.frames[1].delay().numer_denom_ms(), (80, 1));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
