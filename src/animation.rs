//! アニメーションの逐次デコードと再生。
//! フレームを数枚ずつの `RenderImage`（チャンク）にまとめ、Viewerが再生位置のフレームを描画する。
//! 全フレームが予算内なら保持して繰り返し再生し、超えたら再生済みのチャンクを捨てて
//! ループごとにファイルから読み直す（逐次デコード）。切り替えに先頭からの読み直しは要らない。

use crate::decoder;
use gpui::RenderImage;
use image::Frame;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, PoisonError, Weak},
    time::{Duration, Instant},
};

/// 1チャンクの最大フレーム数。GPUアトラスの登録・解放はチャンク単位で起きる。
const CHUNK_FRAMES: usize = 16;
/// 1チャンクの最大容量。大きなフレームは少ない枚数でチャンクにする。
const CHUNK_BYTES: u64 = 16 * 1024 * 1024;
/// 逐次デコードで、時間・容量の目標に関係なく先に用意するフレーム数。
const MIN_AHEAD_FRAMES: usize = 2;
const TARGET_MIN: Duration = Duration::from_millis(500);
const TARGET_START: Duration = Duration::from_secs(1);
const TARGET_MAX: Duration = Duration::from_secs(2);
/// 先読み不足が起きないまま、この時間が過ぎたら先読み時間を減らす。
const RELAX_AFTER: Duration = Duration::from_secs(10);

/// 再生に使う遅延。10ms以下は100msとして扱う（主要ブラウザと同じ）。保存には元の値を使う。
pub fn playback_delay(delay: image::Delay) -> Duration {
    let delay = Duration::from(delay);
    if delay <= Duration::from_millis(10) {
        Duration::from_millis(100)
    } else {
        delay
    }
}

/// 連続するフレームをまとめたGPU画像。
pub struct Chunk {
    pub image: Arc<RenderImage>,
    /// 先頭フレームの通し番号。ループしても増え続ける。
    pub seq: u64,
    pub bytes: u64,
    delays: Vec<Duration>,
}
impl Chunk {
    pub fn frames(&self) -> usize {
        self.delays.len()
    }
}

/// 全フレームを保持したアニメーション。キャッシュとPlayerで共有する。
pub struct Clip {
    pub chunks: Vec<Arc<Chunk>>,
    /// chunksと同じ順の画像。表示補間と保存に使う。
    pub images: Vec<Arc<RenderImage>>,
    pub frames: usize,
    pub bytes: u64,
}
impl Clip {
    fn new(chunks: Vec<Arc<Chunk>>) -> Self {
        Self {
            images: chunks.iter().map(|c| c.image.clone()).collect(),
            frames: chunks.iter().map(|c| c.frames()).sum(),
            bytes: chunks.iter().map(|c| c.bytes).sum(),
            chunks,
        }
    }
}

/// フレーム単体、全フレーム保持、先読みの上限。
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub frame: u64,
    pub resident: u64,
    pub ahead: u64,
}

/// 再開位置。保持していたアニメーションを逐次デコードへ移すときに使う。
#[derive(Clone, Copy, Default)]
pub struct Resume {
    /// ループ内のフレーム番号。
    pub frame: usize,
    /// そのフレームに付ける通し番号。
    pub seq: u64,
    /// 1ループのフレーム数（既知の場合）。
    pub frames: Option<usize>,
}

struct State {
    ready: VecDeque<Arc<Chunk>>,
    clip: Option<Arc<Clip>>,
    frames: Option<usize>,
    streaming: bool,
    finished: bool,
    error: Option<String>,
    /// 画面に出ているフレームの通し番号（Player → デコーダー）。
    playing: u64,
    target: Duration,
    low_memory: bool,
    stop: bool,
    /// デコードスレッドが終了した。
    exited: bool,
}
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

/// 1つのアニメーションのデコードスレッド。最後の参照が外れると停止する。
pub struct Stream {
    shared: Arc<Shared>,
}
impl Drop for Stream {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().stop = true;
        self.shared.wake.notify_all();
    }
}
impl Stream {
    /// streamingがfalseなら、全フレームが `limits.resident` に収まる限り保持を試み、
    /// 収まったときに `stored` を呼ぶ（キャッシュへの登録）。
    pub fn start(
        path: PathBuf,
        turns: u8,
        limits: Limits,
        resume: Resume,
        streaming: bool,
        stored: impl FnOnce(Arc<Clip>) + Send + 'static,
    ) -> Arc<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                ready: VecDeque::new(),
                clip: None,
                frames: resume.frames,
                streaming,
                finished: false,
                error: None,
                playing: resume.seq,
                target: TARGET_START,
                low_memory: false,
                stop: false,
                exited: false,
            }),
            wake: Condvar::new(),
        });
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("nagame-animation".into())
            .spawn(move || {
                /// Records the exit even if decoding panics.
                struct Exit(Arc<Shared>);
                impl Drop for Exit {
                    fn drop(&mut self) {
                        let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
                        state.exited = true;
                        drop(state);
                        self.0.wake.notify_all();
                    }
                }
                let _exit = Exit(worker.clone());
                Decoder::new(worker, limits, resume, streaming).run(&path, turns, stored)
            })
            .expect("spawn animation decoder");
        Arc::new(Self { shared })
    }

    /// 停止の完了を待つための参照。デコードスレッドを生かし続けない。
    pub fn watch(&self) -> Watch {
        Watch(Arc::downgrade(&self.shared))
    }

    /// 先頭チャンクかエラーを待つ。cancelledがtrueになったらNone。
    pub fn first(&self, cancelled: impl Fn() -> bool) -> Option<Result<Arc<Chunk>, String>> {
        let mut state = self.shared.state.lock().unwrap();
        loop {
            if let Some(chunk) = state.ready.front() {
                return Some(Ok(chunk.clone()));
            }
            if let Some(error) = &state.error {
                return Some(Err(error.clone()));
            }
            if cancelled() {
                return None;
            }
            state = self
                .shared
                .wake
                .wait_timeout(state, Duration::from_millis(10))
                .unwrap()
                .0;
        }
    }

    fn set_low_memory(&self, low: bool) {
        self.shared.state.lock().unwrap().low_memory = low;
        self.shared.wake.notify_all();
    }
}

/// 停止を求められたデコードスレッドの終了を待つ。デコードは1フレームの途中では止まらない
/// ため、待たずに次を始めると、素早い画像送りで大きなフレームのデコードが重なる。
/// 弱い参照なので、デコード済みのフレーム（Clipや未取得のチャンク）を生かし続けない。
pub struct Watch(Weak<Shared>);
impl Watch {
    /// 停止を求められていて、まだ終了していなければ待つ。終了した（または停止を求められて
    /// いない）ならtrue。cancelledがtrueになって待つのをやめたらfalse。
    pub fn wait_stopped(&self, cancelled: impl Fn() -> bool) -> bool {
        let Some(shared) = self.0.upgrade() else {
            return true;
        };
        let mut state = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if !state.stop || state.exited {
                return true;
            }
            if cancelled() {
                return false;
            }
            state = shared
                .wake
                .wait_timeout(state, Duration::from_millis(10))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

struct Decoder {
    shared: Arc<Shared>,
    limits: Limits,
    resume: Resume,
    streaming: bool,
    seq: u64,
    /// 全フレーム保持の候補。逐次デコードへ移ったら捨てる。
    kept: Vec<Arc<Chunk>>,
    kept_bytes: u64,
    /// 公開済みで、まだ画面に出ていないフレーム（通し番号、遅延、容量）。
    unplayed: VecDeque<(u64, Duration, u64)>,
    pending: Vec<Frame>,
    pending_bytes: u64,
    chunk_limit: usize,
    /// 先読みを目標まで補充中か。満たしたら目標の半分を下回るまで休み、まとめて補充する。
    filling: bool,
}
impl Decoder {
    fn new(shared: Arc<Shared>, limits: Limits, resume: Resume, streaming: bool) -> Self {
        Self {
            shared,
            limits,
            resume,
            streaming,
            seq: resume.seq,
            kept: Vec::new(),
            kept_bytes: 0,
            unplayed: VecDeque::new(),
            pending: Vec::new(),
            pending_bytes: 0,
            chunk_limit: 1,
            filling: true,
        }
    }

    fn stopped(&self) -> bool {
        self.shared.state.lock().unwrap().stop
    }

    fn fail(&self, error: String) {
        let mut state = self.shared.state.lock().unwrap();
        state.error = Some(error);
        state.finished = true;
        drop(state);
        self.shared.wake.notify_all();
    }

    fn run(mut self, path: &std::path::Path, turns: u8, stored: impl FnOnce(Arc<Clip>)) {
        let mut skip = self.resume.frame;
        let mut first_lap = true;
        loop {
            let frames = match decoder::gif_frames(path, turns, self.limits.frame) {
                Ok(frames) => frames,
                Err(error) => return self.fail(error),
            };
            let mut count = 0usize;
            for frame in frames {
                if self.stopped() {
                    return;
                }
                let frame = match frame {
                    Ok(frame) => frame,
                    // A broken tail ends the loop where decoding stopped, like browsers do.
                    Err(error) if count > 0 => {
                        self.shared.state.lock().unwrap().error = Some(error);
                        break;
                    }
                    Err(error) => return self.fail(error),
                };
                count += 1;
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                if !self.push(frame) {
                    return;
                }
            }
            self.publish();
            if count == 0 {
                return self.fail(crate::i18n::tr!("no-frames"));
            }
            if first_lap && self.resume.frames.is_none() {
                self.shared.state.lock().unwrap().frames = Some(count);
            }
            if !self.streaming {
                let clip = Arc::new(Clip::new(std::mem::take(&mut self.kept)));
                // Store first: once the player sees the clip it asks the cache to recount.
                stored(clip.clone());
                let mut state = self.shared.state.lock().unwrap();
                state.clip = Some(clip);
                state.finished = true;
                drop(state);
                self.shared.wake.notify_all();
                return;
            }
            if first_lap && count == 1 {
                // A single frame larger than the resident limit: nothing to loop.
                self.shared.state.lock().unwrap().finished = true;
                self.shared.wake.notify_all();
                return;
            }
            first_lap = false;
        }
    }

    /// フレームを保留し、必要ならチャンクを公開して先読みの上限まで待つ。
    /// 停止を求められたらfalse。
    fn push(&mut self, frame: Frame) -> bool {
        let bytes = frame.buffer().len() as u64;
        if !self.streaming
            && (self.kept_bytes + self.pending_bytes + bytes > self.limits.resident
                || self.shared.state.lock().unwrap().low_memory)
        {
            self.streaming = true;
            self.kept.clear();
            self.kept_bytes = 0;
            self.shared.state.lock().unwrap().streaming = true;
        }
        self.pending.push(frame);
        self.pending_bytes += bytes;
        let per_chunk = ((CHUNK_BYTES / bytes.max(1)) as usize).clamp(1, CHUNK_FRAMES);
        if self.pending.len() >= self.chunk_limit.min(per_chunk) || self.running_low() {
            self.publish();
        }
        // Small animations decode at full speed. Past the lookahead size, even a resident
        // attempt stays within `ahead` of playback, so a later switch to streaming leaves
        // at most that much unplayed.
        if self.streaming || self.kept_bytes + self.pending_bytes > self.limits.ahead {
            return self.wait_for_room(bytes);
        }
        !self.stopped()
    }

    /// 公開済みで未再生のフレームが少なく、保留中のフレームをすぐ見せる必要があるか。
    fn running_low(&mut self) -> bool {
        let playing = self.shared.state.lock().unwrap().playing;
        while self
            .unplayed
            .front()
            .is_some_and(|(seq, ..)| *seq <= playing)
        {
            self.unplayed.pop_front();
        }
        self.unplayed.len() < MIN_AHEAD_FRAMES
            || self.unplayed.iter().map(|(_, d, _)| *d).sum::<Duration>() < TARGET_MIN / 2
    }

    fn wait_for_room(&mut self, next: u64) -> bool {
        let mut state = self.shared.state.lock().unwrap();
        loop {
            if state.stop {
                return false;
            }
            let playing = state.playing;
            while self
                .unplayed
                .front()
                .is_some_and(|(seq, ..)| *seq <= playing)
            {
                self.unplayed.pop_front();
            }
            let frames = self.unplayed.len() + self.pending.len();
            let bytes = self.pending_bytes + self.unplayed.iter().map(|(.., b)| b).sum::<u64>();
            // Waking for every played frame decodes one frame at a time on a cold, slow core.
            // Refill in batches instead: after reaching the target, resume below half of it.
            let (target, ahead) = if self.filling {
                (state.target, self.limits.ahead)
            } else {
                (state.target / 2, self.limits.ahead / 2)
            };
            // The time target only paces streaming; a resident attempt is bounded by bytes.
            let time_ok = !self.streaming || {
                let pending: Duration =
                    self.pending.iter().map(|f| playback_delay(f.delay())).sum();
                pending + self.unplayed.iter().map(|(_, d, _)| *d).sum::<Duration>() < target
            };
            let room = frames < MIN_AHEAD_FRAMES
                || (!state.low_memory && time_ok && bytes + next <= ahead);
            self.filling = room;
            if room {
                return true;
            }
            if !self.pending.is_empty() {
                // Never sleep on decoded frames the player cannot see yet.
                drop(state);
                self.publish();
                state = self.shared.state.lock().unwrap();
                continue;
            }
            state = self
                .shared
                .wake
                .wait_timeout(state, Duration::from_millis(100))
                .unwrap()
                .0;
        }
    }

    fn publish(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let frames = std::mem::take(&mut self.pending);
        let delays: Vec<_> = frames.iter().map(|f| playback_delay(f.delay())).collect();
        let bytes = std::mem::take(&mut self.pending_bytes);
        let seq = self.seq;
        let each = bytes / frames.len() as u64;
        for (i, delay) in delays.iter().enumerate() {
            self.unplayed.push_back((seq + i as u64, *delay, each));
        }
        self.seq += frames.len() as u64;
        let chunk = Arc::new(Chunk {
            image: Arc::new(RenderImage::new(frames)),
            seq,
            bytes,
            delays,
        });
        if !self.streaming {
            self.kept.push(chunk.clone());
            self.kept_bytes += bytes;
        }
        self.chunk_limit = (self.chunk_limit * 2).min(CHUNK_FRAMES);
        self.shared.state.lock().unwrap().ready.push_back(chunk);
        self.shared.wake.notify_all();
    }
}

/// Viewerが所有する再生状態。tickで時刻を進め、描画は `current` のフレームを使う。
pub struct Player {
    stream: Option<Arc<Stream>>,
    chunks: VecDeque<Arc<Chunk>>,
    index: usize,
    frame: usize,
    due: Instant,
    clip: Option<Arc<Clip>>,
    frames: Option<usize>,
    streaming: bool,
    stalled: bool,
    target: Duration,
    smooth_since: Instant,
    low_memory: bool,
    path: PathBuf,
    turns: u8,
    chunks_changed: bool,
}
impl Player {
    pub fn from_clip(clip: Arc<Clip>, path: PathBuf, turns: u8, now: Instant) -> Self {
        let mut player = Self::empty(path, turns, now);
        player.chunks = clip.chunks.iter().cloned().collect();
        player.frames = Some(clip.frames);
        player.clip = Some(clip);
        player.start(now);
        player
    }
    /// streamは先頭チャンクを公開済みであること（`Stream::first`）。
    pub fn from_stream(stream: Arc<Stream>, path: PathBuf, turns: u8, now: Instant) -> Self {
        let mut player = Self::empty(path, turns, now);
        player.stream = Some(stream);
        player.pull();
        player.start(now);
        player
    }
    fn empty(path: PathBuf, turns: u8, now: Instant) -> Self {
        Self {
            stream: None,
            chunks: VecDeque::new(),
            index: 0,
            frame: 0,
            due: now,
            clip: None,
            frames: None,
            streaming: false,
            stalled: false,
            target: TARGET_START,
            smooth_since: now,
            low_memory: false,
            path,
            turns,
            chunks_changed: true,
        }
    }
    fn start(&mut self, now: Instant) {
        self.due = now + self.chunks.front().map_or(Duration::ZERO, |c| c.delays[0]);
    }

    fn pull(&mut self) {
        let Some(stream) = &self.stream else {
            return;
        };
        let mut state = stream.shared.state.lock().unwrap();
        if !state.ready.is_empty() {
            self.chunks.extend(state.ready.drain(..));
            self.chunks_changed = true;
        }
        self.streaming = state.streaming;
        self.frames = state.frames.or(self.frames);
        let clip = state.clip.clone();
        let finished = state.finished;
        drop(state);
        if let Some(clip) = clip {
            self.clip = Some(clip);
        }
        if finished && !self.streaming {
            // The decoder exited after keeping every frame (or failed); nothing more arrives.
            self.stream = None;
        }
    }

    /// 時刻を進める。表示フレームが変わったらtrue。逐次デコードで再生済みになった
    /// チャンクはretiredへ移す。
    pub fn tick(&mut self, now: Instant, retired: &mut Vec<Arc<RenderImage>>) -> bool {
        self.pull();
        if self.frames == Some(1) || self.chunks.is_empty() {
            return false;
        }
        let mut changed = false;
        for _ in 0..64 {
            if now < self.due {
                break;
            }
            let chunk = &self.chunks[self.index];
            let next = if self.frame + 1 < chunk.frames() {
                Some((self.index, self.frame + 1))
            } else if self.index + 1 < self.chunks.len() {
                Some((self.index + 1, 0))
            } else if self.clip.is_some() && !self.streaming {
                Some((0, 0))
            } else {
                None
            };
            let Some((index, frame)) = next else {
                if self.streaming && !self.stalled && self.stream.is_some() {
                    // The decoder fell behind playback: buffer more from now on.
                    self.target = (self.target * 3 / 2).min(TARGET_MAX);
                    self.smooth_since = now;
                    self.send_target();
                }
                self.stalled = true;
                break;
            };
            self.index = index;
            self.frame = frame;
            changed = true;
            let delay = self.chunks[index].delays[frame];
            self.due = if self.stalled
                || now.saturating_duration_since(self.due) > Duration::from_secs(1)
            {
                now + delay
            } else {
                self.due + delay
            };
            self.stalled = false;
            if self.streaming {
                while self.index > 0 {
                    if let Some(old) = self.chunks.pop_front() {
                        retired.push(old.image.clone());
                    }
                    self.index -= 1;
                    self.chunks_changed = true;
                }
            }
        }
        if changed && let Some(stream) = &self.stream {
            stream.shared.state.lock().unwrap().playing = self.seq();
            stream.shared.wake.notify_all();
        }
        if self.streaming
            && self.target > TARGET_MIN
            && now.saturating_duration_since(self.smooth_since) >= RELAX_AFTER
        {
            self.target = (self.target * 4 / 5).max(TARGET_MIN);
            self.smooth_since = now;
            self.send_target();
        }
        changed
    }

    fn send_target(&self) {
        if let Some(stream) = &self.stream {
            stream.shared.state.lock().unwrap().target = self.target;
            stream.shared.wake.notify_all();
        }
    }

    fn seq(&self) -> u64 {
        self.chunks[self.index].seq + self.frame as u64
    }

    /// 描画するチャンクの位置、画像、チャンク内のフレーム番号。
    pub fn current(&self) -> (usize, &Arc<RenderImage>, usize) {
        (self.index, &self.chunks[self.index].image, self.frame)
    }
    /// 現在保持しているチャンク。台帳とGPU画像の再利用判定に使う。
    pub fn chunks(&self) -> impl Iterator<Item = &Arc<Chunk>> {
        self.chunks.iter()
    }
    /// 保持中のチャンクが前回から変わったか。読むとリセットする。
    pub fn take_chunks_changed(&mut self) -> bool {
        std::mem::take(&mut self.chunks_changed)
    }
    /// 全フレームを保持していればそのアニメーション。表示補間と保存に使う。
    pub fn clip(&self) -> Option<&Arc<Clip>> {
        self.clip.as_ref().filter(|_| !self.streaming)
    }
    pub fn frames(&self) -> Option<usize> {
        self.frames
    }
    pub fn streaming(&self) -> bool {
        self.streaming
    }
    /// デコードスレッドが動いている（保持の試み中か逐次デコード中）。
    pub fn decoding(&self) -> bool {
        self.stream.is_some()
    }
    pub fn source(&self) -> (&std::path::Path, u8) {
        (&self.path, self.turns)
    }
    /// 保持しているチャンクをすべて退役させる。
    pub fn retire(self, retired: &mut Vec<Arc<RenderImage>>) {
        retired.extend(self.chunks.into_iter().map(|c| c.image.clone()));
    }

    /// メモリ逼迫時に使用量を下げる。逐次デコード中は先読みを最小にし、全フレームを
    /// 保持している場合は現在のチャンクだけ残して逐次デコードへ移る。
    pub fn reduce(&mut self, limits: Limits, retired: &mut Vec<Arc<RenderImage>>) {
        self.low_memory = true;
        if let Some(stream) = &self.stream {
            stream.set_low_memory(true);
            return;
        }
        let (Some(clip), Some(frames)) = (self.clip.as_ref(), self.frames) else {
            return;
        };
        // Streaming a small animation would save little and cost decoding every loop.
        if clip.bytes <= limits.ahead || frames < 2 {
            return;
        }
        self.clip = None;
        let current = self.chunks[self.index].clone();
        for (i, chunk) in self.chunks.drain(..).enumerate() {
            if i != self.index {
                retired.push(chunk.image.clone());
            }
        }
        self.chunks.push_back(current.clone());
        self.index = 0;
        self.chunks_changed = true;
        self.streaming = true;
        let next = current.seq + current.frames() as u64;
        let stream = Stream::start(
            self.path.clone(),
            self.turns,
            limits,
            Resume {
                frame: (next % frames as u64) as usize,
                seq: next,
                frames: Some(frames),
            },
            true,
            |_| {},
        );
        stream.set_low_memory(true);
        stream.shared.state.lock().unwrap().playing = self.seq();
        self.stream = Some(stream);
        self.send_target();
    }
    /// デコードスレッドが致命的なエラーで止まった場合のエラー（ファイルの削除など）。
    /// 1周目の途中の破損はそこでループを終えるだけなので含めない。
    pub fn take_error(&mut self) -> Option<String> {
        let stream = self.stream.as_ref()?;
        let mut state = stream.shared.state.lock().unwrap();
        if state.finished && state.clip.is_none() && state.frames != Some(1) {
            state.error.take()
        } else {
            None
        }
    }
    /// 逼迫が収まったら先読みを戻す。
    pub fn relax(&mut self) {
        if std::mem::take(&mut self.low_memory)
            && let Some(stream) = &self.stream
        {
            stream.set_low_memory(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn gif(name: &str, frames: usize, size: u32, ms: u32) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "nagame-animation-{name}-{}.gif",
            std::process::id()
        ));
        let mut encoder = image::codecs::gif::GifEncoder::new(File::create(&path).unwrap());
        for i in 0..frames {
            encoder
                .encode_frame(Frame::from_parts(
                    image::RgbaImage::from_pixel(
                        size,
                        size,
                        image::Rgba([(i * 20) as u8, 0, 0, 255]),
                    ),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(ms, 1),
                ))
                .unwrap();
        }
        path
    }

    fn limits(resident: u64, ahead: u64) -> Limits {
        Limits {
            frame: decoder::DEFAULT_LIMIT,
            resident,
            ahead,
        }
    }

    /// Plays one frame per call, as if each frame's delay had elapsed.
    fn advance(player: &mut Player, retired: &mut Vec<Arc<RenderImage>>) -> Option<u64> {
        for _ in 0..500 {
            let now = player.due;
            if player.tick(now, retired) {
                return Some(player.seq());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        None
    }

    fn red(player: &Player) -> u8 {
        let (_, image, frame) = player.current();
        // BGRA: red is the third byte.
        image.as_bytes(frame).unwrap()[2]
    }

    #[test]
    fn playback_delay_clamps_tiny_values_like_browsers() {
        assert_eq!(
            playback_delay(image::Delay::from_numer_denom_ms(0, 1)),
            Duration::from_millis(100)
        );
        assert_eq!(
            playback_delay(image::Delay::from_numer_denom_ms(10, 1)),
            Duration::from_millis(100)
        );
        assert_eq!(
            playback_delay(image::Delay::from_numer_denom_ms(20, 1)),
            Duration::from_millis(20)
        );
    }

    #[test]
    fn resident_animation_keeps_growing_chunks_and_loops_without_redecoding() {
        let path = gif("resident", 10, 4, 50);
        let (sender, stored) = std::sync::mpsc::channel();
        let stream = Stream::start(
            path.clone(),
            0,
            limits(u64::MAX, u64::MAX),
            Resume::default(),
            false,
            move |clip| sender.send(clip).unwrap(),
        );
        let first = stream.first(|| false).unwrap().unwrap();
        assert_eq!(first.frames(), 1, "the first frame is published alone");
        let clip = stored.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(clip.frames, 10);
        let sizes: Vec<_> = clip.chunks.iter().map(|c| c.frames()).collect();
        // Frames are published one by one until enough are visible, then in growing chunks.
        assert_eq!(sizes[0], 1);
        assert_eq!(sizes.iter().sum::<usize>(), 10);
        assert!(sizes.len() < 10, "later frames are batched: {sizes:?}");
        assert!(Arc::ptr_eq(&clip.chunks[0], &first));
        let mut player = Player::from_stream(stream, path.clone(), 0, Instant::now());
        let mut retired = Vec::new();
        let mut reds = vec![red(&player)];
        for _ in 0..19 {
            advance(&mut player, &mut retired).unwrap();
            reds.push(red(&player));
        }
        assert!(player.clip().is_some());
        assert!(!player.streaming());
        assert!(retired.is_empty(), "resident chunks stay loaded");
        assert_eq!(&reds[..10], &reds[10..]);
        assert_eq!(reds[1], 20);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn over_budget_animation_streams_in_order_across_loops_and_bounds_lookahead() {
        let path = gif("stream", 12, 8, 20);
        let frame = 8 * 8 * 4;
        // Resident limit fits 3 frames; lookahead bytes fit 4 frames.
        let stream = Stream::start(
            path.clone(),
            0,
            limits(frame * 3, frame * 4),
            Resume::default(),
            false,
            |_| panic!("an over-budget animation must not be stored"),
        );
        stream.first(|| false).unwrap().unwrap();
        let mut player = Player::from_stream(stream, path.clone(), 0, Instant::now());
        let mut retired = Vec::new();
        let mut reds = vec![red(&player)];
        for _ in 0..30 {
            advance(&mut player, &mut retired).expect("stream keeps up");
            reds.push(red(&player));
            let held: u64 = player.chunks().map(|c| c.bytes).sum();
            let state = player.stream.as_ref().unwrap().shared.state.lock().unwrap();
            let queued: u64 = state.ready.iter().map(|c| c.bytes).sum();
            // Played chunks are released; at most the lookahead plus the chunk on screen remain.
            assert!(
                held + queued <= frame * (4 + 1 + CHUNK_FRAMES as u64),
                "held {held} queued {queued}"
            );
        }
        assert!(player.streaming());
        assert!(player.clip().is_none());
        assert!(!retired.is_empty(), "played chunks were retired");
        let expected: Vec<u8> = (0..31).map(|i| ((i % 12) * 20) as u8).collect();
        assert_eq!(reds, expected);
        assert_eq!(player.frames(), Some(12));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_resident_attempt_past_the_lookahead_size_is_paced_by_playback() {
        let path = gif("paced", 20, 8, 20);
        let frame = 8 * 8 * 4;
        // Lookahead fits 3 frames; resident would fit all 20.
        let stream = Stream::start(
            path.clone(),
            0,
            limits(frame * 20, frame * 3),
            Resume::default(),
            false,
            |_| {},
        );
        stream.first(|| false).unwrap().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let decoded = |stream: &Stream| {
            let state = stream.shared.state.lock().unwrap();
            state.ready.iter().map(|c| c.frames()).sum::<usize>()
        };
        // Without playback the decoder stops near the lookahead size instead of decoding all 20.
        assert!(decoded(&stream) <= 4 + 2, "decoded {}", decoded(&stream));
        let mut player = Player::from_stream(stream, path.clone(), 0, Instant::now());
        let mut retired = Vec::new();
        for _ in 0..25 {
            advance(&mut player, &mut retired).unwrap();
        }
        assert!(
            player.clip().is_some(),
            "still resident once every frame was decoded"
        );
        assert!(retired.is_empty());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn low_memory_abandons_a_resident_attempt() {
        let path = gif("abandon", 20, 8, 20);
        let frame = 8 * 8 * 4;
        let stream = Stream::start(
            path.clone(),
            0,
            limits(u64::MAX, frame * 3),
            Resume::default(),
            false,
            |_| panic!("abandoned attempts are not stored"),
        );
        stream.first(|| false).unwrap().unwrap();
        let mut player = Player::from_stream(stream, path.clone(), 0, Instant::now());
        let mut retired = Vec::new();
        player.reduce(limits(u64::MAX, frame * 3), &mut retired);
        for _ in 0..25 {
            advance(&mut player, &mut retired).unwrap();
        }
        assert!(player.streaming());
        assert!(player.clip().is_none());
        assert!(!retired.is_empty());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn reducing_a_resident_animation_resumes_streaming_at_the_next_frame() {
        let path = gif("reduce", 12, 4, 30);
        let (sender, stored) = std::sync::mpsc::channel();
        let stream = Stream::start(
            path.clone(),
            0,
            limits(u64::MAX, u64::MAX),
            Resume::default(),
            false,
            move |clip| sender.send(clip).unwrap(),
        );
        stream.first(|| false).unwrap().unwrap();
        let clip = stored.recv_timeout(Duration::from_secs(10)).unwrap();
        drop(stream);
        let mut player = Player::from_clip(clip, path.clone(), 0, Instant::now());
        let mut retired = Vec::new();
        for _ in 0..4 {
            advance(&mut player, &mut retired).unwrap();
        }
        assert_eq!(red(&player), 80);
        player.reduce(limits(u64::MAX, 4 * 4 * 4 * 3), &mut retired);
        assert!(player.streaming());
        assert_eq!(player.chunks().count(), 1);
        assert!(!retired.is_empty());
        let mut reds = vec![red(&player)];
        for _ in 0..12 {
            advance(&mut player, &mut retired).expect("resumed stream delivers frames");
            reds.push(red(&player));
        }
        let expected: Vec<u8> = (4..17).map(|i| ((i % 12) * 20) as u8).collect();
        assert_eq!(reds, expected);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_single_frame_over_the_resident_limit_is_decoded_once() {
        let path = gif("single", 1, 8, 50);
        let stream = Stream::start(
            path.clone(),
            0,
            limits(1, u64::MAX),
            Resume::default(),
            false,
            |_| panic!("not resident"),
        );
        stream.first(|| false).unwrap().unwrap();
        let watch = stream.watch();
        let mut player = Player::from_stream(stream, path.clone(), 0, Instant::now());
        let mut retired = Vec::new();
        for _ in 0..20 {
            player.tick(Instant::now() + Duration::from_secs(10), &mut retired);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(player.frames(), Some(1));
        assert_eq!(player.chunks().count(), 1, "the frame is not decoded again");
        assert!(player.take_error().is_none());
        drop(player);
        assert!(watch.wait_stopped(|| false));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_watch_does_not_keep_decoded_frames_alive() {
        let path = gif("watch", 6, 8, 20);
        let (sender, stored) = std::sync::mpsc::channel();
        let stream = Stream::start(
            path.clone(),
            0,
            limits(u64::MAX, u64::MAX),
            Resume::default(),
            false,
            move |clip| sender.send(clip).unwrap(),
        );
        let watch = stream.watch();
        let clip = Arc::downgrade(&stored.recv_timeout(Duration::from_secs(10)).unwrap());
        let first = Arc::downgrade(&stream.first(|| false).unwrap().unwrap().image);
        drop(stream);
        assert!(watch.wait_stopped(|| false));
        // The exit guard releases its own reference right after recording the exit.
        let deadline = Instant::now() + Duration::from_secs(2);
        while (clip.upgrade().is_some() || first.upgrade().is_some()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(clip.upgrade().is_none(), "the clip was released");
        assert!(first.upgrade().is_none(), "queued chunks were released");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn decode_errors_before_the_first_frame_are_reported() {
        let path = std::env::temp_dir().join(format!(
            "nagame-animation-broken-{}.gif",
            std::process::id()
        ));
        std::fs::write(&path, b"GIF89a broken").unwrap();
        let stream = Stream::start(
            path.clone(),
            0,
            limits(u64::MAX, u64::MAX),
            Resume::default(),
            false,
            |_| {},
        );
        assert!(stream.first(|| false).unwrap().is_err());
        std::fs::remove_file(path).unwrap();
    }
}
