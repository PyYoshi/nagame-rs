//! デコード済み画像の予算連動LRU、単一のワーカー、アニメーションの逐次デコードの開始、
//! 前後の画像の先読み。PDFのページは補助プロセスで描き、通常の画像と同じく扱う。
use crate::animation::{self, Clip, Stream};
use crate::decoder;
use crate::i18n::tr;
use crate::memory::{self, Budget, Holder, Ledger};
use crate::model::Entry;
use crate::pdf;
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
/// キャッシュのキー。更新日時か大きさが変わったファイルは、別の画像として読み直す。
#[derive(Clone, PartialEq, Eq)]
struct Key {
    path: PathBuf,
    /// PDFのページ番号。
    page: Option<u32>,
    turns: u8,
    modified: Option<SystemTime>,
    size: u64,
    second: Option<(PathBuf, Option<u32>, Option<SystemTime>, u64)>,
    rtl: bool,
    landscape_single: bool,
    /// GIFの先頭フレームだけの静止画（見開きのページ）。アニメーションの結果と区別する。
    first_frame: bool,
    /// 見開きで揃えたPDFの倍率（f32のビット列）。基準解像度ならNone。
    scale: Option<u32>,
}
impl Key {
    fn new(entry: &Entry, turns: u8) -> Self {
        let meta = std::fs::metadata(&entry.path).ok();
        Self {
            path: entry.path.clone(),
            page: entry.page.map(|page| page.index),
            turns,
            modified: meta.as_ref().and_then(|m| m.modified().ok()),
            size: meta.map(|m| m.len()).unwrap_or(0),
            second: None,
            rtl: false,
            landscape_single: false,
            first_frame: false,
            scale: None,
        }
    }
    /// 見開きのページ。GIFは先頭フレームだけを静止画として使う。
    fn page(entry: &Entry, gif: bool) -> Self {
        Self {
            first_frame: gif,
            ..Self::new(entry, 0)
        }
    }
}

/// PDFのページから描いた画像。見開きの合成結果では両ページの情報をまとめる。
#[derive(Clone, Debug)]
pub struct PdfPage {
    pub path: PathBuf,
    pub page: u32,
    /// ページの論理寸法（pt）。
    pub size: [f32; 2],
    /// 1ptあたりの画素数。
    pub scale: f32,
    /// 文字・図形を含むページ。固有の画素寸法がないため、表示に合わせて描き直す。
    pub vector: bool,
    /// メモリー逼迫中に予算に合わせて縮めた。逼迫が収まった後の再表示で描き直す。
    pub reduced: bool,
    /// 見開きの合成結果なら、2ページ目と並べ方（表示解像度で描き直すときに使う）。
    pub spread: Option<Spread>,
}

/// 見開きの2ページ目。1ページ目はPdfPageのpageとsize。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spread {
    pub page: u32,
    pub size: [f32; 2],
    /// 右から読む（2ページ目を左に置く）。
    pub rtl: bool,
}

/// 逼迫中に縮めたPDFのページで、もう逼迫していない。キャッシュから使わず描き直す。
fn stale(loaded: &Loaded) -> bool {
    loaded.pdf.as_ref().is_some_and(|pdf| pdf.reduced) && !memory::under_pressure()
}

/// PDFのページを補助プロセスで描く。
fn render_page(
    entry: &Entry,
    mode: pdf::Mode,
    class: pdf::Class,
    cancelled: impl Fn() -> bool,
) -> Result<Arc<Loaded>, String> {
    let page = entry.page.ok_or(tr!("pdf-page-error"))?;
    let pressure = memory::under_pressure();
    let rendered = pdf::render(&entry.path, page.index, mode, class, cancelled)
        .map_err(pdf::Failure::message)?;
    let (width, height) = rendered.pixels.dimensions();
    let mut loaded = Loaded::from_decoded(decoder::Decoded {
        frames: vec![image::Frame::new(rendered.pixels)],
        dimensions: [width as f32, height as f32],
    });
    Arc::get_mut(&mut loaded).unwrap().pdf = Some(PdfPage {
        path: entry.path.clone(),
        page: page.index,
        size: page.size,
        scale: rendered.scale,
        vector: rendered.vector,
        reduced: rendered.limited && pressure,
        spread: None,
    });
    Ok(loaded)
}

/// 見開きの2ページ（PDF）の倍率を揃える。倍率が違うか、合成すると1辺かlimitを超えるとき、
/// 揃えた倍率とlimitで縮めたかを返す。倍率の差が5%未満で上限にも収まればNone。
fn common_scale(first: &Loaded, second: &Loaded, limit: u64) -> Option<(f32, bool)> {
    let (a, b) = (first.pdf.as_ref()?, second.pdf.as_ref()?);
    let width = a.size[0] + b.size[0];
    let height = a.size[1].max(b.size[1]);
    let side = pdf::MAX_SIDE as f32 * 0.99;
    let wanted = a.scale.max(b.scale);
    let mut scale = wanted.min(side / width).min(side / height);
    let bytes = (width * scale).ceil() as f64 * (height * scale).ceil() as f64 * 4.;
    let limited = bytes > limit as f64;
    if limited {
        scale *= (limit as f64 / bytes).sqrt() as f32 * 0.99;
    }
    // 上限で縮めたら、差が小さくても必ず描き直す（合成が上限を超えて1ページへ戻らないように）。
    let close = |value: f32| (value / scale - 1.).abs() < 0.05;
    (scale < wanted || !(close(a.scale) && close(b.scale))).then_some((scale, limited))
}

/// 表示するアニメーション。全フレームを保持したClipか、デコード中のStream。
pub enum Motion {
    Clip(Arc<Clip>),
    Stream(Arc<Stream>),
}
/// pathとturnsは、保存と逐次デコードへの切り替えで元ファイルを読み直すために持つ。
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
    /// PDFのページから描いた画像。
    pub pdf: Option<PdfPage>,
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
            pdf: None,
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
            pdf: None,
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
    pub entry: Entry,
    pub turns: u8,
    pub neighbours: Vec<Entry>,
    pub reload: bool,
    pub second: Option<Entry>,
    pub rtl: bool,
    pub landscape_single: bool,
}
pub struct Response {
    pub generation: u64,
    pub result: Result<Arc<Loaded>, String>,
    /// ワーカーが要求を取り出してから応答するまでの時間（NAGAME_PROFILEのログ用）。
    pub elapsed_ms: u128,
    /// キャッシュから返した。
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
/// ワーカーへの送り口。世代が変わると、古い要求の処理と先読みを打ち切らせる。
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
/// デコード済み画像のLRU（後ろほど新しい）。容量は予算と台帳から決め、項目数も制限する。
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
    /// 入れた項目を残し、容量と項目数に収まるまで他の項目を追い出す。容量を超える画像は入れない。
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
            // 逐次デコードへ移ったアニメーションでも、Clipの先頭チャンクは表示中のまま。
            // 一部でも表示中なら残すとClip全体が残るため、すべての画像が表示中の項目だけを残す。
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
/// 見開きの2ページを1枚に合成する（rtlなら右から読む順）。secondが横長（landscape_single）
/// のときや、合成すると上限を超えるときはfirstだけを返す。
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
    let composed = Arc::get_mut(&mut result).unwrap();
    composed.pages = 2;
    composed.pdf = match (&first.pdf, &second.pdf) {
        (Some(a), Some(b)) => Some(PdfPage {
            vector: a.vector || b.vector,
            reduced: a.reduced || b.reduced,
            spread: Some(Spread {
                page: b.page,
                size: b.size,
                rtl,
            }),
            ..a.clone()
        }),
        _ => None,
    };
    Ok(result)
}
/// 静止画と見開きの合成結果に追加回転を反映する。GIFのアニメーションはデコーダーが回転する。
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
    let result = Arc::get_mut(&mut rotated).unwrap();
    result.pages = page.pages;
    result.pdf = page.pdf.clone();
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
        // まだデコード中。重ねて始めず、次の要求でもう一度待つ。
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
    let (path, turns) = (request.entry.path.clone(), request.turns);
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
                path: request.entry.path.clone(),
                turns: request.turns,
                motion: Motion::Stream(stream),
            },
            bytes,
        )
    }))
}

/// 読み込みワーカーを起動する。溜まった要求は最新のものだけを処理し、応答の後に前後の画像を
/// 先読みする。キャッシュの保持量はledgerへ記録する。
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
            let mut key = Key::new(&request.entry, request.turns);
            key.rtl = request.second.is_some() && request.rtl;
            key.landscape_single = request.second.is_some() && request.landscape_single;
            key.second = request.second.as_ref().map(|entry| {
                let page = Key::new(entry, 0);
                (page.path, page.page, page.modified, page.size)
            });
            let cached = if request.reload {
                None
            } else {
                cache.get(&key).filter(|loaded| !stale(loaded))
            };
            let hit = cached.is_some();
            let result = if let Some(image) = cached {
                Ok(image)
            } else if request.second.is_none()
                && request.entry.page.is_none()
                && decoder::is_gif(&request.entry.path)
            {
                match open_animation(&request, &key, &budget, &store, &mut previous, cancelled) {
                    Some(result) => result,
                    None => continue,
                }
            } else {
                (|| {
                    // scaleは見開きで揃えたPDFの倍率。Noneなら基準解像度で描く。
                    let page = |entry: &Entry, scale: Option<f32>, cache: &mut Cache| {
                        let gif = request.second.is_some()
                            && entry.page.is_none()
                            && decoder::is_gif(&entry.path);
                        let mut key = Key::page(entry, gif);
                        key.scale = scale.map(f32::to_bits);
                        if let Some(page) = (!request.reload)
                            .then(|| cache.get(&key))
                            .flatten()
                            .filter(|page| !stale(page))
                        {
                            return Ok(page);
                        }
                        let page = if entry.page.is_some() {
                            let mode = match scale {
                                Some(scale) => pdf::Mode::Scale {
                                    scale,
                                    limit: budget.still,
                                },
                                None => pdf::Mode::Base {
                                    limit: budget.still,
                                },
                            };
                            render_page(entry, mode, pdf::Class::Current, cancelled)?
                        } else {
                            Loaded::from_decoded(decoder::decode_limited(
                                &entry.path,
                                0,
                                budget.still,
                                false,
                                cancelled,
                            )?)
                        };
                        cache.insert(key, page.clone(), &budget);
                        Ok::<_, String>(page)
                    };
                    let first = page(&request.entry, None, &mut cache)?;
                    let loaded = if let Some(second_entry) = &request.second {
                        if request.landscape_single
                            && (first.dimensions[0] > first.dimensions[1]
                                || second_entry.proportions().is_ok_and(|[w, h]| w > h))
                        {
                            first
                        } else {
                            let mut first = first;
                            let mut second = page(second_entry, None, &mut cache)?;
                            let mut limited = false;
                            if let Some((scale, clamped)) =
                                common_scale(&first, &second, budget.still)
                            {
                                limited = clamped;
                                let differs = |page: &Loaded| {
                                    page.pdf
                                        .as_ref()
                                        .is_some_and(|pdf| (pdf.scale / scale - 1.).abs() > 0.005)
                                };
                                if differs(&first) {
                                    first = page(&request.entry, Some(scale), &mut cache)?;
                                }
                                if differs(&second) {
                                    second = page(second_entry, Some(scale), &mut cache)?;
                                }
                            }
                            let mut composed = compose(
                                &first,
                                &second,
                                request.rtl,
                                request.landscape_single,
                                budget.still,
                            )?;
                            // 逼迫で下がった上限に合わせて縮めた見開きは、逼迫の後に描き直す。
                            if limited
                                && memory::under_pressure()
                                && let Some(pdf) =
                                    Arc::get_mut(&mut composed).and_then(|c| c.pdf.as_mut())
                            {
                                pdf.reduced = true;
                            }
                            composed
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
            for entry in request.neighbours {
                if cancelled() || memory::under_pressure() {
                    break;
                }
                // アニメーションは先読みしない。再生しながらデコードする。
                if entry.page.is_none() && decoder::is_gif(&entry.path) {
                    continue;
                }
                let key = Key::new(&entry, 0);
                if cache.get(&key).is_some_and(|loaded| !stale(&loaded)) {
                    continue;
                }
                let budget = memory::current(ledger.held());
                let loaded = if entry.page.is_some() {
                    render_page(
                        &entry,
                        pdf::Mode::Base {
                            limit: budget.still,
                        },
                        pdf::Class::Prefetch,
                        cancelled,
                    )
                    .ok()
                } else {
                    decoder::decode_limited(&entry.path, 0, budget.still, false, || false)
                        .ok()
                        .map(Loaded::from_decoded)
                };
                if let Some(loaded) = loaded {
                    cache.insert(key, loaded, &budget);
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
            entry: Entry::file(path.clone()),
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
                Key::new(&Entry::file(format!("/missing/{i}.png").into()), 0),
                image.clone(),
                &budget,
            );
        }
        // 同じ画像を別のキーでも持つ（例えば単独表示に戻った見開き）。
        cache.insert(
            Key::new(&Entry::file("/missing/alias.png".into()), 0),
            images[2].clone(),
            &budget,
        );
        assert_eq!(cache.entries.len(), 4);
        assert_eq!(cache.bytes(), 3 * mib4);
        assert_eq!(ledger.held(), 3 * mib4);
        // キャッシュにない大きな画像をViewerが持つ。
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
        // Viewerが持つ画像をキャッシュへ入れても、二重には数えない。
        // 68 MiBなので、二重に数えると128 MiBの予算に60 MiBしか残らず、入らなくなる。
        let big = still(4096, 4352);
        ledger.replace(Holder::Viewer, [(big.image.id, big.bytes)]);
        cache.insert(
            Key::new(&Entry::file("/missing/big.png".into()), 0),
            big.clone(),
            &budget,
        );
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
            entry: Entry::file(root.join(path)),
            turns: 0,
            neighbours: vec![],
            reload: false,
            second: second.map(|name| Entry::file(root.join(name))),
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
        // 左から右へ読む順なので、左がpage.png、右がGIFの先頭（赤）フレーム。BGRAで比べる。
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
            entry: Entry::file(root.join("red.png")),
            turns,
            neighbours: vec![],
            reload: false,
            second: Some(Entry::file(root.join(second))),
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
            entry: Entry::file(root.join("page.png")),
            turns: 0,
            neighbours: vec![],
            reload: false,
            second: second.map(|name| Entry::file(root.join(name))),
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

#[cfg(test)]
mod pdf_tests {
    use super::*;
    use crate::model::{Album, Entry};
    use crate::pdf::synthetic::{Item, Page, pdf};
    use std::time::Duration;

    fn request(generation: u64, entry: Entry, second: Option<Entry>) -> Request {
        Request {
            generation,
            entry,
            turns: 0,
            neighbours: vec![],
            reload: false,
            second,
            rtl: false,
            landscape_single: false,
        }
    }

    #[test]
    fn pdf_pages_load_at_the_base_resolution_cache_and_align_spread_scales() {
        // 先読みはPDFの先読みの種類を使い、他のテストの先読みと置き換え合うため順番に動かす。
        let _serial = pdf::CLASS_TESTS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let root = std::env::temp_dir().join(format!("nagame-pdf-pipeline-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("book.pdf");
        // 1ページ目は300 DPIのスキャン、2ページ目は図形だけ（150 DPI）。同じ大きさのページ。
        let scan = Page {
            size: [144., 192.],
            items: vec![Item::Image {
                pixels: [600, 800],
                left: [0, 0, 0],
                right: [0, 0, 0],
                rect: [0., 0., 144., 192.],
            }],
            ..Page::default()
        };
        let drawing = Page {
            size: [144., 192.],
            items: vec![Item::Rect {
                rgb: [0, 0, 255],
                rect: [0., 0., 10., 10.],
            }],
            ..Page::default()
        };
        std::fs::write(&path, pdf(&[scan, drawing.clone(), drawing], false)).unwrap();
        let album = Album::open(&path).unwrap();
        let (loader, results) = worker(Default::default());
        let receive = || results.recv_timeout(Duration::from_secs(20)).unwrap();

        let mut single = request(1, album.entries[1].clone(), None);
        single.neighbours = vec![album.entries[2].clone()];
        loader.send(single).unwrap();
        let first = receive();
        assert!(!first.cached);
        let loaded = first.result.unwrap();
        assert_eq!(loaded.dimensions, [300., 400.]);
        let info = loaded.pdf.as_ref().unwrap();
        assert!(info.vector && !info.reduced);
        assert_eq!((info.page, info.size), (1, [144., 192.]));
        // 先読みしたページはキャッシュから返る。次の要求を送ると先読みは中止されるので、
        // 描き終わるのを待ってから送る（実際の画像送りも、表示から次の操作までに間がある）。
        std::thread::sleep(Duration::from_secs(1));
        loader
            .send(request(2, album.entries[2].clone(), None))
            .unwrap();
        assert!(receive().cached);

        // 見開きは解像度の高いスキャンのページに倍率を揃えて合成する。
        loader
            .send(request(
                3,
                album.entries[0].clone(),
                Some(album.entries[1].clone()),
            ))
            .unwrap();
        let spread = receive().result.unwrap();
        assert_eq!(spread.pages, 2);
        assert_eq!(spread.dimensions, [1200., 800.]);
        assert!(spread.pdf.as_ref().unwrap().vector);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn spread_scales_shrink_to_fit_even_when_the_pages_already_match() {
        let page = |scale: f32| {
            let mut loaded = Loaded::from_decoded(decoder::Decoded {
                frames: vec![image::Frame::new(image::RgbaImage::new(1, 1))],
                dimensions: [1., 1.],
            });
            Arc::get_mut(&mut loaded).unwrap().pdf = Some(PdfPage {
                path: "/book.pdf".into(),
                page: 0,
                size: [100., 100.],
                scale,
                vector: false,
                reduced: false,
                spread: None,
            });
            loaded
        };
        // 同じ倍率で上限に収まれば揃え直さない。
        assert_eq!(common_scale(&page(2.), &page(2.), 1 << 30), None);
        // 差が5%未満でも、合成（400×200画素、320,000バイト）が上限を超えれば縮める。
        let (scale, limited) = common_scale(&page(2.), &page(1.95), 300_000).unwrap();
        assert!(limited && scale < 1.95);
        assert!((200. * scale).ceil() * (100. * scale).ceil() * 4. <= 300_000.);
        // 倍率の差が大きければ、高い方へ揃える。
        assert_eq!(
            common_scale(&page(4.), &page(2.), 1 << 30),
            Some((4., false))
        );
    }

    #[test]
    fn pages_reduced_under_memory_pressure_are_redrawn_once_it_ends() {
        let page = |reduced| {
            let mut loaded = Loaded::from_decoded(decoder::Decoded {
                frames: vec![image::Frame::new(image::RgbaImage::new(2, 2))],
                dimensions: [2., 2.],
            });
            Arc::get_mut(&mut loaded).unwrap().pdf = Some(PdfPage {
                path: "/book.pdf".into(),
                page: 0,
                size: [1., 1.],
                scale: 2.,
                vector: true,
                reduced,
                spread: None,
            });
            loaded
        };
        assert!(!memory::under_pressure());
        assert!(stale(&page(true)));
        assert!(!stale(&page(false)));
    }
}
