//! Display-only resampling. Source pixels remain available for export and zoom changes.
use crate::i18n::tr;
use gpui::{ImageId, RenderImage};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Algorithm {
    #[default]
    Gpu,
    Nearest,
    Linear,
    Cubic,
    Lanczos3,
}
impl Algorithm {
    pub const ALL: [Self; 5] = [
        Self::Gpu,
        Self::Nearest,
        Self::Linear,
        Self::Cubic,
        Self::Lanczos3,
    ];
    pub fn label(self) -> String {
        match self {
            Self::Gpu => tr!("gpu-linear"),
            Self::Nearest => tr!("nearest"),
            Self::Linear => tr!("bilinear"),
            Self::Cubic => tr!("bicubic"),
            Self::Lanczos3 => tr!("lanczos3"),
        }
    }
    pub fn next(self, direction: i32) -> Self {
        let index = Self::ALL.iter().position(|v| *v == self).unwrap_or(0) as i32;
        Self::ALL[(index + direction.signum()).rem_euclid(Self::ALL.len() as i32) as usize]
    }
    fn filter(self) -> image::imageops::FilterType {
        use image::imageops::FilterType::*;
        match self {
            Self::Gpu | Self::Linear => Triangle,
            Self::Nearest => Nearest,
            Self::Cubic => CatmullRom,
            Self::Lanczos3 => Lanczos3,
        }
    }
}
/// 補間対象の画像の集合を識別する。静止画は1枚、アニメーションは全チャンク。
/// 先頭の `ImageId` は集合ごとに一意で、チャンクの並びは変わらない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    source: ImageId,
    count: usize,
    width: u32,
    height: u32,
    algorithm: Algorithm,
}
type Images = Arc<[Arc<RenderImage>]>;
struct Request {
    key: Key,
    sources: Images,
    generation: u64,
}
struct Response {
    key: Key,
    result: Result<Images, String>,
}

fn resize(
    source: &RenderImage,
    width: u32,
    height: u32,
    algorithm: Algorithm,
    cancelled: impl Fn() -> bool,
) -> Result<Arc<RenderImage>, String> {
    let mut frames = Vec::new();
    for index in 0..source.frame_count() {
        if cancelled() {
            return Err("cancelled".into());
        }
        let dimensions = source.size(index);
        let bytes = source.as_bytes(index).ok_or(tr!("frame-missing"))?;
        let original = image::RgbaImage::from_raw(
            u32::from(dimensions.width),
            u32::from(dimensions.height),
            bytes.to_vec(),
        )
        .ok_or(tr!("image-size-invalid"))?;
        let pixels = if algorithm == Algorithm::Nearest {
            image::imageops::resize(&original, width, height, algorithm.filter())
        } else {
            // image's floating-point resampler clamps every channel to 0..=1.
            // Normalize BGRA before premultiplying, then restore u8 after filtering.
            // Premultiplied color avoids transparent RGB causing halos.
            let premultiplied =
                image::Rgba32FImage::from_fn(original.width(), original.height(), |x, y| {
                    let p = original.get_pixel(x, y).0;
                    let a = p[3] as f32 / 255.;
                    image::Rgba([
                        p[0] as f32 / 255. * a,
                        p[1] as f32 / 255. * a,
                        p[2] as f32 / 255. * a,
                        a,
                    ])
                });
            let scaled = image::imageops::resize(&premultiplied, width, height, algorithm.filter());
            image::RgbaImage::from_fn(width, height, |x, y| {
                let p = scaled.get_pixel(x, y).0;
                let a = p[3].clamp(0., 1.);
                if a < 0.5 / 255. {
                    image::Rgba([0, 0, 0, 0])
                } else {
                    image::Rgba([
                        (p[0] * 255. / a).clamp(0., 255.).round() as u8,
                        (p[1] * 255. / a).clamp(0., 255.).round() as u8,
                        (p[2] * 255. / a).clamp(0., 255.).round() as u8,
                        (a * 255.).round() as u8,
                    ])
                }
            })
        };
        frames.push(image::Frame::from_parts(pixels, 0, 0, source.delay(index)));
    }
    if cancelled() {
        return Err("cancelled".into());
    }
    Ok(Arc::new(RenderImage::new(frames)))
}

/// 表示専用リサイズの要求と結果を管理する。
/// 退役した画像は呼び出し元へ返し、GPUからの遅延解放はViewerが担う。
pub struct State {
    sender: mpsc::Sender<Request>,
    receiver: mpsc::Receiver<Response>,
    active: Arc<AtomicU64>,
    generation: u64,
    wanted: Option<(Key, Images)>,
    since: Instant,
    sent: bool,
    ready: Option<(Key, Images)>,
    pub fallback: bool,
    pub error: Option<String>,
}
impl Default for State {
    fn default() -> Self {
        let (sender, input) = mpsc::channel::<Request>();
        let (output, receiver) = mpsc::channel();
        let active = Arc::new(AtomicU64::new(0));
        let cancellation = active.clone();
        std::thread::spawn(move || {
            while let Ok(mut request) = input.recv() {
                while let Ok(newer) = input.try_recv() {
                    request = newer;
                }
                if cancellation.load(Ordering::Relaxed) != request.generation {
                    continue;
                }
                let cancelled = || cancellation.load(Ordering::Relaxed) != request.generation;
                let result = request
                    .sources
                    .iter()
                    .map(|source| {
                        resize(
                            source,
                            request.key.width,
                            request.key.height,
                            request.key.algorithm,
                            cancelled,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Images::from);
                if cancellation.load(Ordering::Relaxed) == request.generation
                    && output
                        .send(Response {
                            key: request.key,
                            result,
                        })
                        .is_err()
                {
                    break;
                }
            }
        });
        Self {
            sender,
            receiver,
            active,
            generation: 0,
            wanted: None,
            since: Instant::now(),
            sent: false,
            ready: None,
            fallback: false,
            error: None,
        }
    }
}
impl State {
    #[cfg(feature = "ui-smoke")]
    pub fn ready_image(&self) -> Option<&RenderImage> {
        self.ready.as_ref().map(|(_, images)| images[0].as_ref())
    }
    #[cfg(any(test, feature = "ui-smoke"))]
    pub fn ready_id(&self) -> Option<ImageId> {
        self.ready.as_ref().map(|(_, images)| images[0].id)
    }
    /// 補間済みの画像。台帳とGPU画像の再利用判定に使う。
    pub fn ready_images(&self) -> impl Iterator<Item = &Arc<RenderImage>> {
        self.ready.iter().flat_map(|(_, images)| images.iter())
    }
    pub fn clear(&mut self, retired: &mut Vec<Arc<RenderImage>>) {
        self.generation += 1;
        self.active.store(self.generation, Ordering::Relaxed);
        self.wanted = None;
        self.sent = false;
        self.error = None;
        self.fallback = false;
        if let Some((_, images)) = self.ready.take() {
            retired.extend(images.iter().cloned());
        }
    }
    /// 補間しない状態にする。fallbackは「上限によりGPU表示」を表示するか。
    pub fn disable(&mut self, fallback: bool, retired: &mut Vec<Arc<RenderImage>>) {
        if self.wanted.is_some() || self.ready.is_some() || self.error.is_some() {
            self.clear(retired);
        }
        self.fallback = fallback;
    }
    /// sourcesの補間済み画像があれば返す。なければ元画像をGPUで拡大縮小して表示する。
    /// limitは出力の全フレーム合計の上限。
    pub fn prepare(
        &mut self,
        sources: &[Arc<RenderImage>],
        target: [u32; 2],
        algorithm: Algorithm,
        limit: u64,
        retired: &mut Vec<Arc<RenderImage>>,
    ) -> Option<Images> {
        let [width, height] = target;
        let frames: u64 = sources.iter().map(|s| s.frame_count() as u64).sum();
        let fallback = width == 0
            || height == 0
            || width > 16384
            || height > 16384
            || u64::from(width) * u64::from(height) * 4 * frames > limit;
        let original = sources.first()?.size(0);
        if algorithm == Algorithm::Gpu
            || fallback
            || (width == u32::from(original.width) && height == u32::from(original.height))
        {
            self.disable(algorithm != Algorithm::Gpu && fallback, retired);
            return None;
        }
        let key = Key {
            source: sources[0].id,
            count: sources.len(),
            width,
            height,
            algorithm,
        };
        if self.wanted.as_ref().map(|(k, _)| *k) != Some(key) {
            self.clear(retired);
            self.since = Instant::now();
            self.wanted = Some((key, sources.into()));
        }
        self.ready
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, images)| images.clone())
    }
    pub fn pending(&self) -> bool {
        self.wanted.is_some() && self.ready.is_none() && self.error.is_none()
    }
    pub fn tick(&mut self, retired: &mut Vec<Arc<RenderImage>>) -> bool {
        let mut changed = false;
        while let Ok(response) = self.receiver.try_recv() {
            if self.wanted.as_ref().map(|(key, _)| *key) != Some(response.key) {
                continue;
            }
            match response.result {
                Ok(images) => {
                    if let Some((_, old)) = self.ready.replace((response.key, images)) {
                        retired.extend(old.iter().cloned());
                    }
                }
                Err(error) => self.error = Some(error),
            }
            changed = true;
        }
        if !self.sent
            && self.since.elapsed() >= Duration::from_millis(60)
            && let Some((key, sources)) = &self.wanted
        {
            self.sent = true;
            if self
                .sender
                .send(Request {
                    key: *key,
                    sources: sources.clone(),
                    generation: self.generation,
                })
                .is_err()
            {
                self.error = Some(tr!("resampler-stopped"));
                changed = true;
            }
        }
        changed
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.active.store(u64::MAX, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::{Algorithm, resize};
    use gpui::RenderImage;
    #[test]
    fn stale_resize_results_are_discarded_and_oversized_targets_use_gpu() {
        use super::State;
        use std::{
            sync::Arc,
            time::{Duration, Instant},
        };
        let limit = 128 * 1024 * 1024;
        let source = Arc::new(RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::new(8, 8),
        )]));
        let sources = std::slice::from_ref(&source);
        let mut state = State::default();
        let mut retired = Vec::new();
        assert!(
            state
                .prepare(sources, [16, 16], Algorithm::Gpu, limit, &mut retired)
                .is_none()
        );
        assert!(!state.pending());
        state.prepare(sources, [16, 16], Algorithm::Nearest, limit, &mut retired);
        state.since = Instant::now() - Duration::from_millis(100);
        state.tick(&mut retired);
        std::thread::sleep(Duration::from_millis(10));
        state.prepare(sources, [24, 24], Algorithm::Linear, limit, &mut retired);
        state.tick(&mut retired);
        assert!(state.ready_id().is_none());
        state.since = Instant::now() - Duration::from_millis(100);
        let deadline = Instant::now() + Duration::from_secs(2);
        while state.ready_id().is_none() && Instant::now() < deadline {
            state.tick(&mut retired);
            std::thread::sleep(Duration::from_millis(2));
        }
        let output = state
            .prepare(sources, [24, 24], Algorithm::Linear, limit, &mut retired)
            .unwrap();
        assert_eq!(u32::from(output[0].size(0).width), 24);
        assert_ne!(output[0].id, source.id);
        assert!(
            state
                .prepare(
                    sources,
                    [u32::MAX, u32::MAX],
                    Algorithm::Lanczos3,
                    limit,
                    &mut retired
                )
                .is_none()
        );
        assert!(state.fallback);
        assert!(!state.pending());
        assert_eq!(retired.len(), 1);
        // The output limit covers every frame of every image in the set.
        assert!(
            state
                .prepare(
                    sources,
                    [24, 24],
                    Algorithm::Linear,
                    24 * 24 * 4 - 1,
                    &mut retired
                )
                .is_none()
        );
        assert!(state.fallback);
    }
    #[test]
    fn image_sets_resize_together_and_keep_their_order() {
        use super::State;
        use std::{
            sync::Arc,
            time::{Duration, Instant},
        };
        let chunk = |color| {
            Arc::new(RenderImage::new(vec![
                image::Frame::new(
                    image::RgbaImage::from_pixel(4, 4, image::Rgba(color))
                );
                2
            ]))
        };
        let sources = [chunk([255, 0, 0, 255]), chunk([0, 0, 255, 255])];
        let mut state = State::default();
        let mut retired = Vec::new();
        assert!(
            state
                .prepare(
                    &sources,
                    [8, 8],
                    Algorithm::Nearest,
                    8 * 8 * 4 * 4,
                    &mut retired
                )
                .is_none()
        );
        state.since = Instant::now() - Duration::from_millis(100);
        let deadline = Instant::now() + Duration::from_secs(2);
        while state.ready_id().is_none() && Instant::now() < deadline {
            state.tick(&mut retired);
            std::thread::sleep(Duration::from_millis(2));
        }
        let output = state
            .prepare(
                &sources,
                [8, 8],
                Algorithm::Nearest,
                8 * 8 * 4 * 4,
                &mut retired,
            )
            .unwrap();
        assert_eq!(output.len(), 2);
        assert_eq!(output[1].frame_count(), 2);
        assert_eq!(&output[0].as_bytes(1).unwrap()[..4], &[255, 0, 0, 255]);
        assert_eq!(&output[1].as_bytes(0).unwrap()[..4], &[0, 0, 255, 255]);
        assert_eq!(state.ready_images().count(), 2);
        state.disable(true, &mut retired);
        assert!(state.fallback);
        assert_eq!(retired.len(), 2);
    }
    #[test]
    fn filtered_resize_preserves_opaque_and_translucent_colors() {
        let colors = [
            [17, 83, 201, 255],
            [17, 83, 201, 128],
            [201, 83, 17, 32],
            [0, 0, 0, 0],
        ];
        let source = RenderImage::new(
            colors
                .into_iter()
                .enumerate()
                .map(|(index, color)| {
                    image::Frame::from_parts(
                        image::RgbaImage::from_pixel(8, 6, image::Rgba(color)),
                        0,
                        0,
                        image::Delay::from_numer_denom_ms(25 * (index as u32 + 1), 1),
                    )
                })
                .collect::<Vec<_>>(),
        );
        for algorithm in [
            Algorithm::Nearest,
            Algorithm::Linear,
            Algorithm::Cubic,
            Algorithm::Lanczos3,
        ] {
            for [w, h] in [[16, 12], [4, 3]] {
                let output = resize(&source, w, h, algorithm, || false).unwrap();
                assert_eq!(output.frame_count(), colors.len());
                for (index, color) in colors.iter().enumerate() {
                    assert_eq!(output.delay(index), source.delay(index));
                    for pixel in output.as_bytes(index).unwrap().as_chunks::<4>().0 {
                        for channel in 0..4 {
                            assert!(
                                pixel[channel].abs_diff(color[channel]) <= 1,
                                "{algorithm:?} {w}x{h} frame {index}: expected {color:?}, got {pixel:?}"
                            );
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn nearest_preserves_pixels_linear_filters_and_alpha_has_no_color_halo() {
        let mut pixels = image::RgbaImage::new(2, 1);
        pixels.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        pixels.put_pixel(1, 0, image::Rgba([0, 255, 0, 0]));
        let source = RenderImage::new(vec![image::Frame::from_parts(
            pixels,
            0,
            0,
            image::Delay::from_numer_denom_ms(70, 1),
        )]);
        let nearest = resize(&source, 6, 1, Algorithm::Nearest, || false).unwrap();
        assert_eq!(&nearest.as_bytes(0).unwrap()[4..8], &[255, 0, 0, 255]);
        for algorithm in [Algorithm::Linear, Algorithm::Cubic, Algorithm::Lanczos3] {
            let scaled = resize(&source, 6, 1, algorithm, || false).unwrap();
            assert_eq!(scaled.delay(0).numer_denom_ms(), (70, 1));
            for p in scaled.as_bytes(0).unwrap().as_chunks::<4>().0 {
                if p[3] > 0 {
                    assert_eq!(p[0], 255);
                    assert_eq!(p[1], 0);
                }
            }
        }
        assert!(resize(&source, 6, 1, Algorithm::Linear, || true).is_err());
    }
}
