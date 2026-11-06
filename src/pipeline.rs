//! Bounded decoded-image LRU; one worker, cancellable animation loads and neighbour prefetch.
use crate::decoder;
use crate::i18n::tr;
use gpui::RenderImage;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Instant, SystemTime},
};
const CACHE_BYTES: usize = 256 * 1024 * 1024;
#[derive(Clone, PartialEq, Eq)]
struct Key {
    path: PathBuf,
    turns: u8,
    modified: Option<SystemTime>,
    size: u64,
    second: Option<(PathBuf, Option<SystemTime>, u64)>,
    rtl: bool,
    landscape_single: bool,
}
impl Key {
    fn new(path: PathBuf, turns: u8) -> Self {
        let meta = std::fs::metadata(&path).ok();
        Self {
            path,
            turns,
            modified: meta.as_ref().and_then(|m| m.modified().ok()),
            size: meta.map(|m| m.len()).unwrap_or(0),
            second: None,
            rtl: false,
            landscape_single: false,
        }
    }
}
/// キャッシュとViewerが共有する元解像度の表示画像（BGRA）。
/// pagesは見開き合成の成否を反映した、実際の表示ページ数。
pub struct Loaded {
    pub image: Arc<RenderImage>,
    pub dimensions: [f32; 2],
    pub bytes: usize,
    pub pages: usize,
}
impl Loaded {
    fn from_decoded(decoded: decoder::Decoded) -> Arc<Self> {
        let bytes = decoded.frames.iter().map(|f| f.buffer().len()).sum();
        Arc::new(Self {
            image: Arc::new(RenderImage::new(decoded.frames)),
            dimensions: decoded.dimensions,
            bytes,
            pages: 1,
        })
    }
}
/// generationはViewerの選択世代。secondは見開き候補で、必ず合成するとは限らない。
pub struct Request {
    pub generation: u64,
    pub path: PathBuf,
    pub turns: u8,
    pub neighbours: Vec<PathBuf>,
    pub reload: bool,
    pub second: Option<PathBuf>,
    pub rtl: bool,
    pub landscape_single: bool,
}
/// 同じgenerationに先行フレームと最終結果が届く場合がある。
/// complete=falseはGIFの先頭フレーム、trueは処理完了の応答。
pub struct Response {
    pub generation: u64,
    pub result: Result<Arc<Loaded>, String>,
    pub complete: bool,
    pub elapsed_ms: u128,
    pub cached: bool,
}
pub struct Loader {
    sender: mpsc::Sender<Request>,
    active: Arc<AtomicU64>,
}
impl Loader {
    pub fn cancel(&self, generation: u64) {
        self.active.store(generation, Ordering::Relaxed);
    }
    pub fn send(&self, request: Request) -> Result<(), mpsc::SendError<Request>> {
        self.active.store(request.generation, Ordering::Relaxed);
        self.sender.send(request)
    }
}
impl Drop for Loader {
    fn drop(&mut self) {
        self.active.store(u64::MAX, Ordering::Relaxed);
    }
}
#[derive(Default)]
struct Cache {
    entries: VecDeque<(Key, Arc<Loaded>)>,
    bytes: usize,
}
impl Cache {
    fn get(&mut self, key: &Key) -> Option<Arc<Loaded>> {
        let i = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(i)?;
        let result = entry.1.clone();
        self.entries.push_back(entry);
        Some(result)
    }
    fn insert(&mut self, key: Key, value: Arc<Loaded>) {
        if let Some(i) = self.entries.iter().position(|(k, _)| k == &key) {
            self.bytes -= self.entries.remove(i).unwrap().1.bytes;
        }
        if value.bytes > CACHE_BYTES {
            return;
        }
        while self.bytes + value.bytes > CACHE_BYTES || self.entries.len() >= 8 {
            if let Some((_, old)) = self.entries.pop_front() {
                self.bytes -= old.bytes;
            } else {
                break;
            }
        }
        self.bytes += value.bytes;
        self.entries.push_back((key, value));
    }
}
fn compose(
    first: &Arc<Loaded>,
    second: &Arc<Loaded>,
    rtl: bool,
    landscape_single: bool,
) -> Result<Arc<Loaded>, String> {
    if landscape_single && second.dimensions[0] > second.dimensions[1] {
        return Ok(first.clone());
    }
    let w1 = first.dimensions[0] as u32;
    let w2 = second.dimensions[0] as u32;
    let height = first.dimensions[1].max(second.dimensions[1]) as u32;
    let width = w1 + w2;
    if width > 16384
        || height > 16384
        || u64::from(width) * u64::from(height) * 4 > 128 * 1024 * 1024
    {
        return Ok(first.clone());
    }
    let mut pixels = image::RgbaImage::new(width, height);
    for (page, x_offset) in [
        (first, if rtl { w2 } else { 0 }),
        (second, if rtl { 0 } else { w1 }),
    ] {
        let page_width = page.dimensions[0] as usize;
        let bytes = page.image.as_bytes(0).ok_or(tr!("spread-data-missing"))?;
        for (y, row) in bytes.chunks_exact(page_width * 4).enumerate() {
            let start = (y * width as usize + x_offset as usize) * 4;
            pixels.as_flat_samples_mut().samples[start..start + row.len()].copy_from_slice(row);
        }
    }
    let mut result = Loaded::from_decoded(decoder::Decoded {
        frames: vec![image::Frame::new(pixels)],
        dimensions: [width as f32, height as f32],
    });
    Arc::get_mut(&mut result).unwrap().pages = 2;
    Ok(result)
}
fn rotate(page: &Arc<Loaded>, turns: u8) -> Result<Arc<Loaded>, String> {
    if turns.is_multiple_of(4) {
        return Ok(page.clone());
    }
    let (width, height) = (page.dimensions[0] as u32, page.dimensions[1] as u32);
    let mut frames = Vec::new();
    for index in 0..page.image.frame_count() {
        let pixels = image::RgbaImage::from_raw(
            width,
            height,
            page.image
                .as_bytes(index)
                .ok_or(tr!("frame-missing"))?
                .to_vec(),
        )
        .ok_or(tr!("image-size-invalid"))?;
        let pixels = match turns % 4 {
            1 => image::imageops::rotate90(&pixels),
            2 => image::imageops::rotate180(&pixels),
            _ => image::imageops::rotate270(&pixels),
        };
        frames.push(image::Frame::from_parts(
            pixels,
            0,
            0,
            page.image.delay(index),
        ));
    }
    let dimensions = if turns % 2 == 1 {
        [height as f32, width as f32]
    } else {
        page.dimensions
    };
    let mut rotated = Loaded::from_decoded(decoder::Decoded { frames, dimensions });
    Arc::get_mut(&mut rotated).unwrap().pages = page.pages;
    Ok(rotated)
}

pub fn worker() -> (Loader, mpsc::Receiver<Response>) {
    let (sender, input) = mpsc::channel::<Request>();
    let (output, responses) = mpsc::sync_channel(2);
    let active = Arc::new(AtomicU64::new(0));
    let cancellation = active.clone();
    std::thread::spawn(move || {
        let mut cache = Cache::default();
        while let Ok(mut request) = input.recv() {
            while let Ok(newer) = input.try_recv() {
                request = newer;
            }
            let start = Instant::now();
            let mut key = Key::new(request.path.clone(), request.turns);
            key.rtl = request.second.is_some() && request.rtl;
            key.landscape_single = request.second.is_some() && request.landscape_single;
            key.second = request.second.as_ref().map(|path| {
                let page = Key::new(path.clone(), 0);
                (page.path, page.modified, page.size)
            });
            let cached = if request.reload {
                None
            } else {
                cache.get(&key)
            };
            let hit = cached.is_some();
            let result = if let Some(image) = cached {
                Ok(image)
            } else {
                (|| {
                    let first_key = Key::new(request.path.clone(), 0);
                    let first = if !request.reload {
                        cache.get(&first_key)
                    } else {
                        None
                    };
                    let first = match first {
                        Some(first) => first,
                        None => {
                            let decoded = decoder::decode_progressive(
                                &request.path,
                                0,
                                |frame| {
                                    if request.second.is_none() && request.turns == 0 {
                                        let dimensions = [
                                            frame.buffer().width() as f32,
                                            frame.buffer().height() as f32,
                                        ];
                                        let preview = Loaded::from_decoded(decoder::Decoded {
                                            frames: vec![frame],
                                            dimensions,
                                        });
                                        let _ = output.send(Response {
                                            generation: request.generation,
                                            result: Ok(preview),
                                            complete: false,
                                            elapsed_ms: start.elapsed().as_millis(),
                                            cached: false,
                                        });
                                    }
                                },
                                || cancellation.load(Ordering::Relaxed) != request.generation,
                            )?;
                            let first = Loaded::from_decoded(decoded);
                            cache.insert(first_key, first.clone());
                            first
                        }
                    };
                    let loaded = if let Some(second_path) = &request.second {
                        if request.landscape_single
                            && (first.dimensions[0] > first.dimensions[1]
                                || decoder::dimensions(second_path).is_ok_and(|[w, h]| w > h))
                        {
                            first
                        } else {
                            let second_key = Key::new(second_path.clone(), 0);
                            let second = match if request.reload {
                                None
                            } else {
                                cache.get(&second_key)
                            } {
                                Some(second) => second,
                                None => {
                                    let page = Loaded::from_decoded(decoder::decode_progressive(
                                        second_path,
                                        0,
                                        |_| {},
                                        || {
                                            cancellation.load(Ordering::Relaxed)
                                                != request.generation
                                        },
                                    )?);
                                    cache.insert(second_key, page.clone());
                                    page
                                }
                            };
                            compose(&first, &second, request.rtl, request.landscape_single)?
                        }
                    } else {
                        first
                    };
                    rotate(&loaded, request.turns)
                })()
            };
            if cancellation.load(Ordering::Relaxed) != request.generation {
                continue;
            }
            if let Ok(ref loaded) = result {
                cache.insert(key, loaded.clone());
            }
            if output
                .send(Response {
                    generation: request.generation,
                    result,
                    complete: true,
                    elapsed_ms: start.elapsed().as_millis(),
                    cached: hit,
                })
                .is_err()
            {
                break;
            }
            for path in request.neighbours {
                if cancellation.load(Ordering::Relaxed) != request.generation {
                    break;
                }
                // Animated images are not prefetched: their full frame sets would delay navigation.
                if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("gif"))
                {
                    continue;
                }
                let key = Key::new(path, 0);
                if cache.get(&key).is_some() {
                    continue;
                }
                if let Ok(decoded) = decoder::decode(&key.path, 0) {
                    cache.insert(key, Loaded::from_decoded(decoded));
                }
            }
        }
    });
    (Loader { sender, active }, responses)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revisiting_uses_cache_but_changed_file_is_redecoded() {
        let path = std::env::temp_dir().join(format!("nagame-cache-{}.png", std::process::id()));
        image::RgbaImage::new(2, 2).save(&path).unwrap();
        let (loader, results) = worker();
        let request = |generation| Request {
            generation,
            path: path.clone(),
            turns: 0,
            neighbours: vec![],
            reload: false,
            second: None,
            rtl: false,
            landscape_single: false,
        };
        loader.send(request(1)).unwrap();
        let first = results
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        assert!(!first.cached);
        loader.send(request(2)).unwrap();
        let second = results
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        assert!(second.cached);
        assert!(Arc::ptr_eq(&first.result.unwrap(), &second.result.unwrap()));
        image::RgbaImage::new(3, 5).save(&path).unwrap();
        loader.send(request(3)).unwrap();
        let third = results
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        assert!(!third.cached);
        assert_eq!(third.result.unwrap().dimensions, [3., 5.]);
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
mod manga_tests {
    use super::*;
    #[test]
    fn spreads_follow_reading_direction_rotation_and_landscape_options() {
        let root = std::env::temp_dir().join(format!("nagame-spread-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for (name, width, height, color) in [
            ("red.png", 4, 6, [255, 0, 0, 255]),
            ("blue.png", 4, 6, [0, 0, 255, 255]),
            ("wide.png", 8, 4, [0, 255, 0, 255]),
        ] {
            image::RgbaImage::from_pixel(width, height, image::Rgba(color))
                .save(root.join(name))
                .unwrap();
        }
        let (loader, results) = worker();
        let request = |generation, second, turns, landscape_single| Request {
            generation,
            path: root.join("red.png"),
            turns,
            neighbours: vec![],
            reload: false,
            second: Some(root.join(second)),
            rtl: true,
            landscape_single,
        };
        loader.send(request(1, "blue.png", 0, true)).unwrap();
        let pair = results
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .result
            .unwrap();
        assert_eq!(pair.pages, 2);
        assert_eq!(pair.dimensions, [8., 6.]);
        let pixels = pair.image.as_bytes(0).unwrap();
        assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
        assert_eq!(&pixels[16..20], &[0, 0, 255, 255]);
        loader.send(request(2, "blue.png", 1, true)).unwrap();
        let rotated = results
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .result
            .unwrap();
        assert_eq!(rotated.dimensions, [6., 8.]);
        assert_eq!(rotated.pages, 2);
        loader.send(request(3, "wide.png", 0, true)).unwrap();
        let single = results
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .result
            .unwrap();
        assert_eq!(single.pages, 1);
        assert_eq!(single.dimensions, [4., 6.]);
        loader.send(request(4, "wide.png", 0, false)).unwrap();
        let joined = results
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .result
            .unwrap();
        assert_eq!(joined.pages, 2);
        assert_eq!(joined.dimensions, [12., 6.]);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn zero_width_page_is_rejected_without_stopping_the_worker() {
        let root = std::env::temp_dir().join(format!("nagame-zero-width-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        image::RgbaImage::new(4, 6)
            .save(root.join("page.png"))
            .unwrap();
        // 論理スクリーン幅0に1x1の画像ブロックを1つ持つGIF。gifクレートは幅0を拒否しない。
        std::fs::write(
            root.join("zero.gif"),
            [
                b"GIF89a".as_slice(),
                &[0, 0, 1, 0, 0x80, 0, 0],
                &[0, 0, 0, 255, 255, 255],
                &[0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0],
                &[2, 2, 0x44, 1, 0, 0x3b],
            ]
            .concat(),
        )
        .unwrap();
        let (loader, results) = worker();
        let request = |generation, second: Option<&str>| Request {
            generation,
            path: root.join("page.png"),
            turns: 0,
            neighbours: vec![],
            reload: false,
            second: second.map(|name| root.join(name)),
            rtl: false,
            landscape_single: false,
        };
        loader.send(request(1, Some("zero.gif"))).unwrap();
        let rejected = results
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        assert_eq!(rejected.result.err(), Some(tr!("image-size-invalid")));
        loader.send(request(2, None)).unwrap();
        let next = results
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        assert_eq!(next.result.unwrap().dimensions, [4., 6.]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
