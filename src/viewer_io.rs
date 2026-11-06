//! Viewerのファイル選択、画像保存・コピー、情報画面。
//! 出力は表示用リサイズ画像を使わず、元解像度のBGRAからRGBAへ戻す。

use crate::i18n::{self, tr};
use crate::{Command, Viewer};
use gpui::{
    AnyElement, Context, PathPromptOptions, RenderImage, div, prelude::*, px, relative, rgb, rgba,
};
use std::{io::Cursor, path::Path};

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
fn save(image: &RenderImage, path: &Path) -> Result<(), String> {
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
        for index in 0..image.frame_count() {
            encoder
                .encode_frame(image::Frame::from_parts(
                    rgba_pixels(image, index)?,
                    0,
                    0,
                    image.delay(index),
                ))
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
    // Encode fully before touching the destination; a codec error cannot truncate it.
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
// GPUI 0.2.2 advertises only text to other Linux clipboard clients.
// Use the desktop clipboard tools for the actual image/png MIME payload.
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
        let name = format!(
            "{}_view.png",
            self.album
                .current()
                .and_then(|p| p.file_stem())
                .unwrap_or_default()
                .to_string_lossy()
        );
        let parent = if self.album.directory.as_os_str().is_empty() {
            std::path::PathBuf::from(".")
        } else {
            self.album.directory.clone()
        };
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
            let task = executor.spawn(async move { save(&image, &output) });
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
        let Some(path) = self.album.current().map(std::path::PathBuf::from) else {
            self.notice = Some(tr!("no-open-image"));
            return;
        };
        self.info_visible = true;
        self.info_text = Some(tr!("loading-info"));
        let dimensions = self.dimensions;
        let frames = self.image.as_ref().map(|p| p.frame_count()).unwrap_or(0);
        let pages = self.pages;
        let task = cx.background_executor().spawn(async move {
            let metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
            let [w, h] = crate::decoder::dimensions(&path)?;
            Ok::<_, String>(i18n::image_information(i18n::ImageInformation {
                path: &path,
                format: &path
                    .extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_uppercase(),
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
        let data = image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 255, 255])); // BGRA red
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(data)]));
        let root = std::env::temp_dir().join(format!("nagame-export-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for name in ["image.png", "image.jpg", "image.JPE"] {
            let path = root.join(name);
            save(&image, &path).unwrap();
            let decoded = image::load_from_memory(&std::fs::read(&path).unwrap())
                .unwrap()
                .into_rgba8();
            let color = decoded.get_pixel(3, 3).0;
            assert!(color[0] > 240 && color[2] < 10);
        }
        let unsupported = root.join("keep.heic");
        std::fs::write(&unsupported, b"keep original").unwrap();
        assert!(save(&image, &unsupported).is_err());
        assert_eq!(std::fs::read(unsupported).unwrap(), b"keep original");
        let clipboard = image::load_from_memory(&png(&image).unwrap())
            .unwrap()
            .into_rgba8();
        assert_eq!(clipboard.get_pixel(0, 0).0, [255, 0, 0, 255]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
