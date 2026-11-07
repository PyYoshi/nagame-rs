//! デコード済み画像の予算連動LRU、単一のワーカー、アニメーションの逐次デコードの開始、近隣の先読み。
use crate::animation::{self, Clip, Stream};
use crate::decoder;
use crate::i18n::tr;
use crate::memory::{self, Budget, Holder, Ledger};
use gpui::{ImageId, RenderImage};
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Instant, SystemTime},
};
/// 予算とは別に、キャッシュの項目数も制限する。
const CACHE_ENTRIES: usize = 8;
#[derive(Clone, PartialEq, Eq)]
struct Key {
    path: PathBuf,
    turns: u8,
    modified: Option<SystemTime>,
    size: u64,
    second: Option<(PathBuf, Option<SystemTime>, u64)>,
    rtl: bool,
    landscape_single: bool,
    /// GIFの先頭フレームだけの静止画（見開きのページ）。アニメーションの結果と区別する。
    first_frame: bool,
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
            first_frame: false,
        }
    }
    /// 見開きのページ。GIFは先頭フレームだけを静止画として使う。
    fn page(path: PathBuf, gif: bool) -> Self {
        Self {
            first_frame: gif,
            ..Self::new(path, 0)
        }
    }
}

/// 表示するアニメーション。全フレームを保持したClipか、デコード中のStream。
pub enum Motion {
    Clip(Arc<Clip>),
    Stream(Arc<Stream>),
}
pub struct Animation {
    pub path: PathBuf,
    pub turns: u8,
    pub motion: Motion,
}

/// キャッシュとViewerが共有する元解像度の表示画像（BGRA）。
/// pagesは見開き合成の成否を反映した、実際の表示ページ数。
/// アニメーションのimageは先頭チャンク（先頭フレーム）。
pub struct Loaded {
    pub image: Arc<RenderImage>,
    pub dimensions: [f32; 2],
    pub bytes: u64,
    pub pages: usize,
    pub animation: Option<Animation>,
}
impl Loaded {
    fn from_decoded(decoded: decoder::Decoded) -> Arc<Self> {
        let image = Arc::new(RenderImage::new(decoded.frames));
        Arc::new(Self {
            bytes: memory::image_bytes(&image),
            image,
            dimensions: decoded.dimensions,
            pages: 1,
            animation: None,
        })
    }
    fn animated(chunk: &animation::Chunk, animation: Animation, bytes: u64) -> Arc<Self> {
        let size = chunk.image.size(0);
        Arc::new(Self {
            image: chunk.image.clone(),
            dimensions: [u32::from(size.width) as f32, u32::from(size.height) as f32],
            bytes,
            pages: 1,
            animation: Some(animation),
        })
    }
    /// 保持している画像とその容量。チャンクは先頭の画像と重複し得る。
    fn images(&self) -> Vec<(ImageId, u64)> {
        let mut images = vec![(self.image.id, memory::image_bytes(&self.image))];
        if let Some(Animation {
            motion: Motion::Clip(clip),
            ..
        }) = &self.animation
        {
            images.extend(clip.chunks.iter().map(|c| (c.image.id, c.bytes)));
        }
        images
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
pub struct Response {
    pub generation: u64,
    pub result: Result<Arc<Loaded>, String>,
    pub elapsed_ms: u128,
    pub cached: bool,
}
enum Message {
    Load(Request),
    /// 予算に合わせてキャッシュを縮める。aggressiveなら表示中以外をすべて捨てる。
    Trim {
        aggressive: bool,
    },
}
/// 全フレームを保持できたアニメーションのキャッシュ登録。デコードスレッドから届き、
/// ワーカーが次のメッセージを処理するときに反映する。
type Stored = (Key, Arc<Loaded>);
pub struct Loader {
    sender: mpsc::Sender<Message>,
    active: Arc<AtomicU64>,
}
impl Loader {
    pub fn cancel(&self, generation: u64) {
        self.active.store(generation, Ordering::Relaxed);
    }
    pub fn send(&self, request: Request) -> Result<(), mpsc::SendError<Request>> {
        self.active.store(request.generation, Ordering::Relaxed);
        self.sender
            .send(Message::Load(request))
            .map_err(|mpsc::SendError(message)| match message {
                Message::Load(request) => mpsc::SendError(request),
                Message::Trim { .. } => unreachable!("send only wraps loads"),
            })
    }
    pub fn trim(&self, aggressive: bool) {
        let _ = self.sender.send(Message::Trim { aggressive });
    }
}
impl Drop for Loader {
    fn drop(&mut self) {
        self.active.store(u64::MAX, Ordering::Relaxed);
    }
}
struct Cache {
    entries: VecDeque<(Key, Arc<Loaded>)>,
    ledger: Arc<Ledger>,
}
impl Cache {
    fn new(ledger: Arc<Ledger>) -> Self {
        Self {
            entries: VecDeque::new(),
            ledger,
        }
    }
    fn get(&mut self, key: &Key) -> Option<Arc<Loaded>> {
        let i = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(i)?;
        let result = entry.1.clone();
        self.entries.push_back(entry);
        Some(result)
    }
    /// 同じ画像を複数のキーで持っていても一度だけ数える。
    fn bytes(&self) -> u64 {
        let mut seen = HashSet::new();
        self.entries
            .iter()
            .flat_map(|(_, loaded)| loaded.images())
            .filter(|(id, _)| seen.insert(*id))
            .map(|(_, bytes)| bytes)
            .sum()
    }
    /// 予算のうち、Viewerだけが持つ画像を除いた分をキャッシュに使う。
    /// adding（これから入れる画像）はViewerも持っていて外側に数えられているため除く。
    fn capacity(&self, budget: &Budget, adding: &HashSet<ImageId>) -> u64 {
        budget
            .total
            .saturating_sub(self.ledger.outside_except(Holder::Cache, adding))
    }
    /// 表示中の画像だけからなる項目。追い出してもメモリーは減らない。
    fn shown(&self, loaded: &Loaded) -> bool {
        loaded
            .images()
            .iter()
            .all(|(id, _)| self.ledger.holds(Holder::Viewer, *id))
    }
    /// 古い順に、表示中でない項目を優先して1つ追い出す。
    fn evict(&mut self, keep: Option<&Key>) -> bool {
        let candidates = || {
            self.entries
                .iter()
                .enumerate()
                .filter(|(_, (k, _))| Some(k) != keep)
        };
        let index = candidates()
            .find(|(_, (_, loaded))| !self.shown(loaded))
            .or_else(|| candidates().next())
            .map(|(i, _)| i);
        index.and_then(|i| self.entries.remove(i)).is_some()
    }
    /// 台帳を更新する。大きく減ったら、1 MiB未満のフレームが残したヒープもOSへ返す。
    fn publish(&self, before: u64) {
        self.ledger.replace(
            Holder::Cache,
            self.entries.iter().flat_map(|(_, loaded)| loaded.images()),
        );
        if before.saturating_sub(self.bytes()) >= 32 * memory::MIB {
            memory::release_free_memory();
        }
    }
    fn insert(&mut self, key: Key, value: Arc<Loaded>, budget: &Budget) {
        let before = self.bytes();
        if let Some(i) = self.entries.iter().position(|(k, _)| k == &key) {
            self.entries.remove(i);
        }
        let adding = value.images().into_iter().map(|(id, _)| id).collect();
        let capacity = self.capacity(budget, &adding);
        if value.bytes <= capacity {
            self.entries.push_back((key.clone(), value));
            while (self.entries.len() > CACHE_ENTRIES || self.bytes() > capacity)
                && self.evict(Some(&key))
            {}
        }
        self.publish(before);
    }
    fn trim(&mut self, budget: &Budget, aggressive: bool) {
        let before = self.bytes();
        if aggressive {
            // A clip whose player streams again still shows its first chunk; keep only
            // entries whose every image is on screen.
            let entries = std::mem::take(&mut self.entries);
            self.entries = entries
                .into_iter()
                .filter(|(_, loaded)| self.shown(loaded))
                .collect();
        }
        let capacity = self.capacity(budget, &HashSet::new());
        while self.bytes() > capacity && self.evict(None) {}
        self.publish(before);
    }
}
fn compose(
    first: &Arc<Loaded>,
    second: &Arc<Loaded>,
    rtl: bool,
    landscape_single: bool,
    limit: u64,
) -> Result<Arc<Loaded>, String> {
    if landscape_single && second.dimensions[0] > second.dimensions[1] {
        return Ok(first.clone());
    }
    let w1 = first.dimensions[0] as u32;
    let w2 = second.dimensions[0] as u32;
    let height = first.dimensions[1].max(second.dimensions[1]) as u32;
    let width = w1 + w2;
    if width > 16384 || height > 16384 || u64::from(width) * u64::from(height) * 4 > limit {
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

/// 単独表示のGIFを逐次デコードで開く。先頭フレームが揃うか、中止されるまで待つ。
fn open_animation(
    request: &Request,
    key: &Key,
    budget: &Budget,
    store: &mpsc::Sender<Stored>,
    previous: &mut Option<animation::Watch>,
    cancelled: impl Fn() -> bool,
) -> Option<Result<Arc<Loaded>, String>> {
    if let Some(watch) = previous.take()
        && !watch.wait_stopped(&cancelled)
    {
        // Still decoding: wait for it again on the next request instead of overlapping.
        *previous = Some(watch);
        return None;
    }
    if cancelled() {
        return None;
    }
    let limits = animation::Limits {
        frame: budget.still,
        resident: budget.resident,
        ahead: budget.ahead,
    };
    let (path, turns) = (request.path.clone(), request.turns);
    let (store, store_key) = (store.clone(), key.clone());
    let stream = Stream::start(
        path.clone(),
        turns,
        limits,
        animation::Resume::default(),
        false,
        move |clip| {
            let Some(first) = clip.chunks.first() else {
                return;
            };
            let bytes = clip.bytes;
            let loaded = Loaded::animated(
                first,
                Animation {
                    path,
                    turns,
                    motion: Motion::Clip(clip.clone()),
                },
                bytes,
            );
            let _ = store.send((store_key, loaded));
        },
    );
    *previous = Some(stream.watch());
    let first = stream.first(cancelled)?;
    Some(first.map(|chunk| {
        let bytes = chunk.bytes;
        Loaded::animated(
            &chunk,
            Animation {
                path: request.path.clone(),
                turns: request.turns,
                motion: Motion::Stream(stream),
            },
            bytes,
        )
    }))
}

pub fn worker(ledger: Arc<Ledger>) -> (Loader, mpsc::Receiver<Response>) {
    let (sender, input) = mpsc::channel::<Message>();
    let (store, stored) = mpsc::channel::<Stored>();
    let (output, responses) = mpsc::sync_channel(2);
    let active = Arc::new(AtomicU64::new(0));
    let cancellation = active.clone();
    std::thread::spawn(move || {
        let mut cache = Cache::new(ledger.clone());
        let mut previous = None;
        while let Ok(message) = input.recv() {
            let mut request = None;
            let mut trim = None;
            for message in
                std::iter::once(message).chain(std::iter::from_fn(|| input.try_recv().ok()))
            {
                match message {
                    Message::Load(newer) => request = Some(newer),
                    Message::Trim { aggressive } => {
                        trim = Some(aggressive || trim.unwrap_or(false))
                    }
                }
            }
            while let Ok((key, loaded)) = stored.try_recv() {
                cache.insert(key, loaded, &memory::current(ledger.held()));
            }
            if let Some(aggressive) = trim {
                cache.trim(&memory::current(ledger.held()), aggressive);
            }
            let Some(request) = request else {
                continue;
            };
            let start = Instant::now();
            let budget = memory::current(ledger.held());
            let cancelled = || cancellation.load(Ordering::Relaxed) != request.generation;
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
            } else if request.second.is_none() && decoder::is_gif(&request.path) {
                match open_animation(&request, &key, &budget, &store, &mut previous, cancelled) {
                    Some(result) => result,
                    None => continue,
                }
            } else {
                (|| {
                    let page = |path: &PathBuf, cache: &mut Cache| {
                        let gif = request.second.is_some() && decoder::is_gif(path);
                        let key = Key::page(path.clone(), gif);
                        if let Some(page) = (!request.reload).then(|| cache.get(&key)).flatten() {
                            return Ok(page);
                        }
                        let page = Loaded::from_decoded(decoder::decode_limited(
                            path,
                            0,
                            budget.still,
                            false,
                            cancelled,
                        )?);
                        cache.insert(key, page.clone(), &budget);
                        Ok::<_, String>(page)
                    };
                    let first = page(&request.path, &mut cache)?;
                    let loaded = if let Some(second_path) = &request.second {
                        if request.landscape_single
                            && (first.dimensions[0] > first.dimensions[1]
                                || decoder::dimensions(second_path).is_ok_and(|[w, h]| w > h))
                        {
                            first
                        } else {
                            let second = page(second_path, &mut cache)?;
                            compose(
                                &first,
                                &second,
                                request.rtl,
                                request.landscape_single,
                                budget.still,
                            )?
                        }
                    } else {
                        first
                    };
                    rotate(&loaded, request.turns)
                })()
            };
            if cancelled() {
                continue;
            }
            if let Ok(ref loaded) = result
                && loaded.animation.is_none()
            {
                cache.insert(key, loaded.clone(), &budget);
            }
            if output
                .send(Response {
                    generation: request.generation,
                    result,
                    elapsed_ms: start.elapsed().as_millis(),
                    cached: hit,
                })
                .is_err()
            {
                break;
            }
            for path in request.neighbours {
                if cancelled() || memory::under_pressure() {
                    break;
                }
                // Animations are not prefetched: they are decoded while they play.
                if decoder::is_gif(&path) {
                    continue;
                }
                let key = Key::new(path, 0);
                if cache.get(&key).is_some() {
                    continue;
                }
                let budget = memory::current(ledger.held());
                if let Ok(decoded) =
                    decoder::decode_limited(&key.path, 0, budget.still, false, || false)
                {
                    cache.insert(key, Loaded::from_decoded(decoded), &budget);
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
        let (loader, results) = worker(Default::default());
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
mod memory_tests {
    use super::*;
    use std::time::Duration;

    fn still(width: u32, height: u32) -> Arc<Loaded> {
        Loaded::from_decoded(decoder::Decoded {
            frames: vec![image::Frame::new(image::RgbaImage::new(width, height))],
            dimensions: [width as f32, height as f32],
        })
    }

    #[test]
    fn cache_shrinks_for_images_the_viewer_holds_and_counts_shared_entries_once() {
        let ledger = Arc::new(Ledger::default());
        let mut cache = Cache::new(ledger.clone());
        let budget = Budget::from_total(0);
        let mib4 = 4 * memory::MIB;
        let images: Vec<_> = (0..3).map(|_| still(1024, 1024)).collect();
        for (i, image) in images.iter().enumerate() {
            cache.insert(
                Key::new(format!("/missing/{i}.png").into(), 0),
                image.clone(),
                &budget,
            );
        }
        // The same image under a second key (e.g. a spread that fell back to one page).
        cache.insert(
            Key::new("/missing/alias.png".into(), 0),
            images[2].clone(),
            &budget,
        );
        assert_eq!(cache.entries.len(), 4);
        assert_eq!(cache.bytes(), 3 * mib4);
        assert_eq!(ledger.held(), 3 * mib4);
        // The viewer now holds something big that the cache does not have.
        let other = still(1, 1);
        ledger.replace(
            Holder::Viewer,
            [
                (other.image.id, budget.total - 2 * mib4),
                (images[2].image.id, mib4),
            ],
        );
        cache.trim(&budget, false);
        assert!(cache.bytes() <= 2 * mib4, "{}", cache.bytes());
        assert!(
            cache
                .entries
                .iter()
                .any(|(_, l)| Arc::ptr_eq(l, &images[2]))
        );
        cache.trim(&budget, true);
        assert!(
            cache
                .entries
                .iter()
                .all(|(_, l)| Arc::ptr_eq(l, &images[2])),
            "aggressive trim keeps only what the viewer shows"
        );
        // An image the viewer already holds is not counted twice when it is inserted.
        // 68 MiB: double counting would leave only 60 MiB of a 128 MiB budget.
        let big = still(4096, 4352);
        ledger.replace(Holder::Viewer, [(big.image.id, big.bytes)]);
        cache.insert(Key::new("/missing/big.png".into(), 0), big.clone(), &budget);
        assert!(
            cache.entries.iter().any(|(_, l)| Arc::ptr_eq(l, &big)),
            "a {} MiB image fits a {} MiB budget",
            big.bytes / memory::MIB,
            budget.total / memory::MIB
        );
    }

    #[test]
    fn gifs_stream_then_cache_the_complete_clip_and_spread_pages_use_the_first_frame() {
        let root = std::env::temp_dir().join(format!("nagame-gif-cache-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        image::RgbaImage::from_pixel(4, 6, image::Rgba([0, 0, 255, 255]))
            .save(root.join("page.png"))
            .unwrap();
        {
            let file = std::fs::File::create(root.join("anim.gif")).unwrap();
            let mut encoder = image::codecs::gif::GifEncoder::new(file);
            for color in [[255, 0, 0, 255], [0, 255, 0, 255], [255, 255, 0, 255]] {
                encoder
                    .encode_frame(image::Frame::from_parts(
                        image::RgbaImage::from_pixel(4, 6, image::Rgba(color)),
                        0,
                        0,
                        image::Delay::from_numer_denom_ms(30, 1),
                    ))
                    .unwrap();
            }
        }
        let (loader, results) = worker(Default::default());
        let request = |generation, path: &str, second: Option<&str>| Request {
            generation,
            path: root.join(path),
            turns: 0,
            neighbours: vec![],
            reload: false,
            second: second.map(|name| root.join(name)),
            rtl: false,
            landscape_single: false,
        };
        let receive = || results.recv_timeout(Duration::from_secs(10)).unwrap();
        loader.send(request(1, "anim.gif", None)).unwrap();
        let first = receive();
        assert!(!first.cached);
        let loaded = first.result.unwrap();
        let Some(Animation {
            motion: Motion::Stream(stream),
            ..
        }) = &loaded.animation
        else {
            panic!("an uncached GIF starts streaming");
        };
        let mut player = animation::Player::from_stream(
            stream.clone(),
            root.join("anim.gif"),
            0,
            Instant::now(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while player.clip().is_none() && Instant::now() < deadline {
            player.tick(Instant::now(), &mut Vec::new());
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(player.clip().map(|c| c.frames), Some(3));
        drop((player, loaded));
        loader.send(request(2, "anim.gif", None)).unwrap();
        let second = receive();
        assert!(second.cached, "the complete clip was stored");
        assert!(matches!(
            second.result.unwrap().animation,
            Some(Animation {
                motion: Motion::Clip(ref clip),
                ..
            }) if clip.frames == 3
        ));
        loader
            .send(request(3, "page.png", Some("anim.gif")))
            .unwrap();
        let spread = receive().result.unwrap();
        assert_eq!(spread.pages, 2);
        assert!(spread.animation.is_none());
        // LTR: page.png on the left, the GIF's first (red) frame on the right, as BGRA.
        assert_eq!(
            &spread.image.as_bytes(0).unwrap()[16..20],
            &[0, 0, 255, 255]
        );
        loader.send(request(4, "anim.gif", None)).unwrap();
        let again = receive();
        assert!(again.cached);
        assert!(
            again.result.unwrap().animation.is_some(),
            "the spread page did not replace the clip"
        );
        std::fs::remove_dir_all(root).unwrap();
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
        let (loader, results) = worker(Default::default());
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
        let (loader, results) = worker(Default::default());
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
