//! 実ウィンドウでのUI回帰テスト。`ui-smoke` featureでビルドしたときだけ含まれ、
//! 通常のビルドでは無効。
//! 環境変数 `NAGAME_UI_*` でシナリオを選ぶ。失敗はパニックとして終了コードに表れる。
use crate::{
    Command, Viewer,
    bindings::Shortcut,
    i18n::{Language, tr},
    settings::{BoundaryAction, Settings},
    settings_ui::Tab,
};
use gpui::*;
use std::time::Duration;

async fn wait_for_folder_image(
    handle: WindowHandle<Viewer>,
    cx: &mut AsyncApp,
    executor: &BackgroundExecutor,
    directory: &std::path::Path,
    index: usize,
) {
    for _ in 0..200 {
        executor.timer(Duration::from_millis(10)).await;
        if handle
            .read_with(cx, |v, _| {
                v.album.directory == directory && v.album.index == index && !v.loading
            })
            .unwrap()
        {
            handle
                .read_with(cx, |v, _| {
                    assert!(v.error.is_none(), "{:?}", v.error);
                    assert!(v.image.is_some());
                    let filename = if (index == 0) != v.settings.reverse {
                        "01.png"
                    } else {
                        "03.png"
                    };
                    assert_eq!(v.album.entries[index].path.file_name().unwrap(), filename);
                })
                .unwrap();
            // 次の操作の前に、デコードした画像をウィンドウに描画させる。
            executor.timer(Duration::from_millis(100)).await;
            return;
        }
    }
    panic!("Timed out waiting for {directory:?}, image {index}");
}

/// アプリ内でキー入力を発行する。デスクトップ全体には入力を送らない。
fn key(handle: WindowHandle<Viewer>, cx: &mut AsyncApp, value: &str) {
    let handle: AnyWindowHandle = handle.into();
    handle
        .update(cx, |_, window, cx| {
            window.dispatch_keystroke(Keystroke::parse(value).unwrap(), cx);
        })
        .unwrap();
}

async fn wait_for_window_state(
    handle: WindowHandle<Viewer>,
    cx: &mut AsyncApp,
    executor: &BackgroundExecutor,
    predicate: impl Fn(&Viewer, &Window) -> bool,
) {
    for _ in 0..300 {
        executor.timer(Duration::from_millis(10)).await;
        if handle
            .update(cx, |viewer, window, _| predicate(viewer, window))
            .unwrap()
        {
            executor.timer(Duration::from_millis(100)).await;
            return;
        }
    }
    panic!("Timed out waiting for window state");
}

/// 96×64で、フレームごとに色が変わるGIFを書く（30ms間隔）。
fn write_gif(path: &std::path::Path, frames: usize, offset: u8) {
    let mut encoder = image::codecs::gif::GifEncoder::new(std::fs::File::create(path).unwrap());
    for i in 0..frames {
        encoder
            .encode_frame(image::Frame::from_parts(
                image::RgbaImage::from_pixel(
                    96,
                    64,
                    image::Rgba([offset.wrapping_add(i as u8 * 9), 90, 200, 255]),
                ),
                0,
                0,
                image::Delay::from_numer_denom_ms(30, 1),
            ))
            .unwrap();
    }
}

/// predicateが成り立つまで最大5秒待つ。whatはタイムアウト時のメッセージに使う。
async fn wait_for(
    handle: WindowHandle<Viewer>,
    cx: &mut AsyncApp,
    executor: &BackgroundExecutor,
    what: &str,
    predicate: impl Fn(&Viewer) -> bool,
) {
    for _ in 0..500 {
        executor.timer(Duration::from_millis(10)).await;
        if handle.read_with(cx, |v, _| predicate(v)).unwrap() {
            return;
        }
    }
    panic!("Timed out waiting for {what}");
}

/// 画面に出ているフレームをdurationの間調べ、表示された異なるフレームの数を返す。
async fn shown_frames(
    handle: WindowHandle<Viewer>,
    cx: &mut AsyncApp,
    executor: &BackgroundExecutor,
    duration: Duration,
) -> usize {
    let mut seen = std::collections::HashSet::new();
    let end = std::time::Instant::now() + duration;
    while std::time::Instant::now() < end {
        executor.timer(Duration::from_millis(10)).await;
        handle
            .read_with(cx, |v, _| {
                assert!(v.error.is_none(), "{:?}", v.error);
                let (_, image, frame) = v.player.as_ref().expect("animation player").current();
                seen.insert((image.id, frame));
            })
            .unwrap();
    }
    seen.len()
}

/// GIFの全フレーム保持・表示補間・逐次デコード・高速な画像送り・メモリー逼迫への対応を確かめる。
async fn animation_check(
    handle: WindowHandle<Viewer>,
    cx: &mut AsyncApp,
    executor: &BackgroundExecutor,
) {
    use crate::memory::{self, Budget};
    let root = std::env::temp_dir().join(format!("nagame-ui-animation-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    write_gif(&root.join("a.gif"), 24, 0);
    image::RgbaImage::from_pixel(96, 64, image::Rgba([20, 160, 40, 255]))
        .save(root.join("b.png"))
        .unwrap();
    write_gif(&root.join("c.gif"), 12, 100);
    let open = |cx: &mut AsyncApp, name: &str| {
        let path = root.join(name);
        handle.update(cx, |v, _, cx| v.open(path, cx)).unwrap();
    };

    open(cx, "a.gif");
    wait_for(handle, cx, executor, "resident animation", |v| {
        !v.loading && v.player.as_ref().is_some_and(|p| p.clip().is_some())
    })
    .await;
    let shown = shown_frames(handle, cx, executor, Duration::from_millis(700)).await;
    assert!(
        shown >= 8,
        "only {shown} frames were shown in 700 ms at 30 ms per frame"
    );
    handle
        .read_with(cx, |v, _| {
            let player = v.player.as_ref().unwrap();
            assert_eq!(player.frames(), Some(24));
            assert!(!player.streaming());
            assert!(v.ledger.held() >= 96 * 64 * 4 * 24);
        })
        .unwrap();
    eprintln!("PASS: resident GIF plays every frame from chunked images");

    handle
        .update(cx, |v, _, cx| {
            v.settings.enlarge_algorithm = crate::resample::Algorithm::Nearest;
            v.settings.shrink_algorithm = crate::resample::Algorithm::Nearest;
            v.view.fit = false;
            v.view.zoom = 3.;
            cx.notify();
        })
        .unwrap();
    wait_for(handle, cx, executor, "resampled animation", |v| {
        let chunks = v
            .player
            .as_ref()
            .and_then(|p| p.clip())
            .map(|c| c.images.len());
        chunks.is_some() && v.resampling.ready_images().count() == chunks.unwrap()
    })
    .await;
    handle
        .update(cx, |v, window, _| {
            let image = v.resampling.ready_image().unwrap();
            let expected = (96. * 3. * window.scale_factor()).round() as u32;
            assert_eq!(u32::from(image.size(0).width), expected);
            assert!(!v.resampling.fallback);
        })
        .unwrap();
    assert!(shown_frames(handle, cx, executor, Duration::from_millis(300)).await >= 4);
    eprintln!("PASS: nearest-neighbour resampling covers every chunk of a resident GIF");

    let frame = 96 * 64 * 4;
    memory::override_budget(Some(Budget {
        resident: frame * 4,
        ahead: frame * 6,
        ..Budget::from_total(0)
    }));
    key(handle, cx, "f5");
    wait_for(handle, cx, executor, "streaming animation", |v| {
        !v.loading && v.player.as_ref().is_some_and(|p| p.streaming())
    })
    .await;
    let mut most = 0;
    for _ in 0..3 {
        assert!(shown_frames(handle, cx, executor, Duration::from_millis(500)).await >= 6);
        handle
            .read_with(cx, |v, _| {
                let held: u64 = v.player.as_ref().unwrap().chunks().map(|c| c.bytes).sum();
                most = most.max(held);
                assert!(v.resampling.fallback, "streaming frames use GPU scaling");
            })
            .unwrap();
    }
    assert!(most <= frame * (6 + 1 + 16), "streaming held {most} bytes");
    eprintln!(
        "PASS: over-budget GIF streams across loops, releasing played chunks ({most} bytes held at most)"
    );

    for (budget, label) in [(None, "resident"), (Some(frame * 4), "streaming")] {
        memory::override_budget(budget.map(|resident| Budget {
            resident,
            ahead: frame * 6,
            ..Budget::from_total(0)
        }));
        open(cx, "a.gif");
        wait_for(handle, cx, executor, "album", |v| {
            !v.loading && v.album.entries.len() == 3
        })
        .await;
        for direction in ["right", "left"] {
            for _ in 0..120 {
                key(handle, cx, direction);
                executor.timer(Duration::from_millis(8)).await;
            }
        }
        wait_for(handle, cx, executor, "settled navigation", |v| {
            !v.loading && v.image.is_some()
        })
        .await;
        executor.timer(Duration::from_millis(300)).await;
        handle
            .read_with(cx, |v, _| assert!(v.error.is_none(), "{:?}", v.error))
            .unwrap();
        eprintln!("PASS: {label}: 240 navigation events across GIF and PNG with chunk retirement");
    }

    // 全フレームを保持できる（24フレームが収まる）が、先読みの上限より大きいため、
    // 逼迫の段階2で逐次デコードへ移る。
    memory::override_budget(Some(Budget {
        ahead: frame * 6,
        ..Budget::from_total(0)
    }));
    open(cx, "a.gif");
    wait_for(handle, cx, executor, "resident animation", |v| {
        !v.loading && v.player.as_ref().is_some_and(|p| p.clip().is_some())
    })
    .await;
    // 開き直して、全フレームのClipをキャッシュにも入れる。
    key(handle, cx, "f5");
    wait_for(handle, cx, executor, "cached resident animation", |v| {
        !v.loading && v.player.as_ref().is_some_and(|p| p.clip().is_some())
    })
    .await;
    let clip_bytes = frame * 24;
    assert!(handle.read_with(cx, |v, _| v.ledger.held()).unwrap() >= clip_bytes);
    memory::signal_pressure();
    executor.timer(Duration::from_millis(50)).await;
    memory::signal_pressure();
    wait_for(handle, cx, executor, "pressure response", |v| {
        v.pressure_level == 2 && v.player.as_ref().is_some_and(|p| p.streaming())
    })
    .await;
    // キャッシュもClipを手放すので、GIFのフレームが実際に解放される。
    wait_for(handle, cx, executor, "released clip", |v| {
        v.ledger.held() < clip_bytes
    })
    .await;
    assert!(shown_frames(handle, cx, executor, Duration::from_millis(500)).await >= 6);
    handle
        .read_with(cx, |v, _| {
            assert_eq!(v.notice.as_deref(), Some(tr!("memory-reduced").as_str()));
            assert!(v.resampling.ready_images().next().is_none());
            assert!(v.resampling.fallback);
        })
        .unwrap();
    eprintln!(
        "PASS: memory pressure trims caches, drops resampling and streams the resident GIF ({} of {clip_bytes} bytes held)",
        handle.read_with(cx, |v, _| v.ledger.held()).unwrap()
    );
    memory::override_budget(None);
    std::fs::remove_dir_all(root).unwrap();
}

/// 現在のRSS（MiB）。
fn rss_mib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("VmRSS:"))
                .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
        })
        .map_or(0, |kib| kib / 1024)
}

/// プロセスのCPU時間（ユーザー＋システム）を秒で返す。クロック刻みは100Hzとみなす。
fn cpu_seconds() -> f64 {
    std::fs::read_to_string("/proc/self/stat")
        .ok()
        .and_then(|stat| {
            let fields: Vec<_> = stat.rsplit_once(')')?.1.split_whitespace().collect();
            Some(
                (fields.get(11)?.parse::<f64>().ok()? + fields.get(12)?.parse::<f64>().ok()?)
                    / 100.,
            )
        })
        .unwrap_or(0.)
}

/// パスを順に開き、RSS・CPU・台帳の保持量・アニメーションの状態を1秒ごとに報告する。
/// `NAGAME_UI_MEMORY_PROFILE=path[:path...][@秒数]`（既定は6秒）。`pressure` を並べると、
/// その位置でメモリー逼迫を通知する。
async fn memory_profile(
    handle: WindowHandle<Viewer>,
    cx: &mut AsyncApp,
    executor: &BackgroundExecutor,
    spec: &str,
) {
    let (paths, seconds) = spec
        .rsplit_once('@')
        .map_or((spec, 6), |(p, s)| (p, s.parse().unwrap()));
    eprintln!("PROFILE: start rss {} MiB", rss_mib());
    for path in paths.split(':') {
        let path = std::path::PathBuf::from(path);
        if path.as_os_str() == "pressure" {
            // 表示中の画像はそのままにして、逼迫への対応でどれだけ解放されるかを報告する。
            crate::memory::signal_pressure();
            executor.timer(Duration::from_millis(100)).await;
            crate::memory::signal_pressure();
        } else {
            handle
                .update(cx, |v, _, cx| v.open(path.clone(), cx))
                .unwrap();
        }
        let mut peak = 0;
        let mut cpu = cpu_seconds();
        for tick in 0..seconds * 4 {
            executor.timer(Duration::from_millis(250)).await;
            peak = peak.max(rss_mib());
            if tick % 4 == 3 {
                handle
                    .read_with(cx, |v, _| {
                        let mode = match &v.player {
                            Some(p) if p.streaming() => "streaming",
                            Some(p) if p.clip().is_some() => "resident",
                            Some(_) => "decoding",
                            None => "still",
                        };
                        eprintln!(
                            "PROFILE: {} t={}s rss {} MiB (peak {peak}) cpu {:.0}% held {} MiB budget {} MiB {mode} error={:?}",
                            path.file_name().unwrap().to_string_lossy(),
                            (tick + 1) / 4,
                            rss_mib(),
                            (cpu_seconds() - std::mem::replace(&mut cpu, cpu_seconds())) * 100.,
                            v.ledger.held() / crate::memory::MIB,
                            v.budget.total / crate::memory::MIB,
                            v.error,
                        );
                    })
                    .unwrap();
            }
        }
    }
}

/// 合成したPDFで、開いた直後の状態、拡大時の描き直し、連続のページ送り、見開き、
/// メモリー逼迫で補助プロセスを止めた後の回復、最終ページから次の文書への移動を確かめる。
async fn pdf_check(handle: WindowHandle<Viewer>, cx: &mut AsyncApp, executor: &BackgroundExecutor) {
    use crate::pdf::synthetic::{Item, Page, pdf};
    let root = std::env::temp_dir().join(format!("nagame-ui-pdf-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    // 文字・図形のA4ページ（150 DPIで1240×1754）と、300 DPIのスキャン（600×800）を交互に並べる。
    let text = Page {
        size: [595., 842.],
        items: vec![
            Item::Rect {
                rgb: [30, 120, 30],
                rect: [50., 50., 200., 100.],
            },
            Item::Text {
                text: "Nagame".into(),
                position: [72., 700.],
                size: 24.,
                invisible: false,
            },
        ],
        ..Page::default()
    };
    // 300 DPIのスキャン。フォームXObjectで包んだものも画像主体として扱うことを確かめる。
    let scan = Page {
        size: [144., 192.],
        items: vec![Item::Image {
            pixels: [600, 800],
            left: [200, 40, 40],
            right: [40, 40, 200],
            rect: [0., 0., 144., 192.],
        }],
        ..Page::default()
    };
    let wrapped = Page {
        size: [144., 192.],
        items: vec![Item::FormImage {
            pixels: [600, 800],
            color: [200, 40, 40],
            rect: [0., 0., 144., 192.],
        }],
        ..Page::default()
    };
    let pages: Vec<_> = (0..240)
        .map(|i| match i % 4 {
            1 => scan.clone(),
            3 => wrapped.clone(),
            _ => text.clone(),
        })
        .collect();
    std::fs::write(root.join("vol1.pdf"), pdf(&pages, false)).unwrap();
    std::fs::write(
        root.join("vol2.pdf"),
        pdf(&[text.clone(), scan.clone()], false),
    )
    .unwrap();
    handle
        .update(cx, |v, _, cx| {
            v.settings.boundary = BoundaryAction::Folder;
            v.open(root.join("vol1.pdf"), cx);
        })
        .unwrap();
    wait_for(handle, cx, executor, "the first PDF page", |v| {
        v.album.document && v.image.is_some() && !v.loading
    })
    .await;
    handle
        .read_with(cx, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            assert_eq!(v.album.entries.len(), 240);
            assert!(v.pdf.as_ref().unwrap().vector);
            assert!(
                v.view.fit_upscale,
                "Text pages fill the window regardless of the setting"
            );
            assert_eq!(v.dimensions, [1240., 1754.]);
        })
        .unwrap();
    // 原寸より拡大すると、補間ではなく表示解像度で描き直す（既定のGPU表示でも）。
    handle
        .update(cx, |v, window, cx| {
            v.command(Command::Actual, window, cx);
            v.command(Command::ZoomIn, window, cx);
        })
        .unwrap();
    wait_for(handle, cx, executor, "the redrawn PDF page", |v| {
        v.resampling
            .ready_image()
            .is_some_and(|image| u32::from(image.size(0).width) > 1300)
    })
    .await;
    eprintln!("PASS: PDF text pages fill the window and are redrawn at the display resolution");
    let start = std::time::Instant::now();
    for _ in 0..200 {
        key(handle, cx, "right");
        executor.timer(Duration::from_millis(8)).await;
    }
    wait_for(handle, cx, executor, "settled PDF navigation", |v| {
        !v.loading && v.image.is_some()
    })
    .await;
    handle
        .read_with(cx, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            assert!(
                v.album.document && v.album.index > 100,
                "index {}",
                v.album.index
            );
        })
        .unwrap();
    eprintln!(
        "PASS: 200 PDF page turns in {:?}, rss {} MiB, PDF helper {} MiB",
        start.elapsed(),
        rss_mib(),
        crate::pdf::helper_rss() / crate::memory::MIB
    );
    handle
        .update(cx, |v, _, cx| {
            v.settings.manga = true;
            v.album.index = 0;
            v.load(true, cx);
        })
        .unwrap();
    wait_for(handle, cx, executor, "a PDF spread", |v| {
        !v.loading && v.pages == 2
    })
    .await;
    handle
        .read_with(cx, |v, _| {
            // 文字のページをスキャンの倍率（300 DPI）に揃えて並べる。
            assert!(v.error.is_none(), "{:?}", v.error);
            assert_eq!(v.dimensions, [3079., 3508.]);
        })
        .unwrap();
    eprintln!("PASS: PDF spreads align the page scales");
    handle
        .update(cx, |v, _, cx| {
            v.settings.manga = false;
            v.load(true, cx);
        })
        .unwrap();
    wait_for(handle, cx, executor, "a single PDF page", |v| {
        !v.loading && v.pages == 1
    })
    .await;
    // 2回の逼迫で補助プロセスを止めても、次のページは起動し直した補助プロセスで描く。
    crate::memory::signal_pressure();
    executor.timer(Duration::from_millis(100)).await;
    crate::memory::signal_pressure();
    wait_for(handle, cx, executor, "the stopped PDF helper", |v| {
        v.pressure_level == 2 && crate::pdf::helper_rss() == 0
    })
    .await;
    key(handle, cx, "right");
    wait_for(
        handle,
        cx,
        executor,
        "a page from the restarted helper",
        |v| !v.loading && v.album.index == 1 && v.image.is_some(),
    )
    .await;
    handle
        .read_with(cx, |v, _| assert!(v.error.is_none(), "{:?}", v.error))
        .unwrap();
    eprintln!("PASS: memory pressure stops the PDF helper and the next page restarts it");
    handle
        .update(cx, |v, _, cx| {
            v.album.index = 3;
            v.load(true, cx);
        })
        .unwrap();
    wait_for(handle, cx, executor, "the wrapped scan", |v| {
        !v.loading && v.album.index == 3 && v.pdf.is_some()
    })
    .await;
    handle
        .read_with(cx, |v, _| {
            assert!(
                !v.pdf.as_ref().unwrap().vector,
                "A scan wrapped in a form is an image page"
            );
            assert_eq!(v.dimensions, [600., 800.]);
        })
        .unwrap();
    eprintln!("PASS: scans wrapped in form XObjects keep their embedded resolution");
    handle
        .update(cx, |v, window, cx| v.command(Command::Last, window, cx))
        .unwrap();
    wait_for(handle, cx, executor, "the last PDF page", |v| {
        !v.loading && v.album.index == 239
    })
    .await;
    key(handle, cx, "right");
    wait_for(handle, cx, executor, "the next PDF", |v| {
        !v.loading && v.album.directory.ends_with("vol2.pdf") && v.image.is_some()
    })
    .await;
    eprintln!("PASS: the last page continues to the next PDF in the folder");
    std::fs::remove_dir_all(root).unwrap();
}

/// 環境変数で選んだシナリオを実行し、終わったらウィンドウを閉じる。
pub fn start(handle: WindowHandle<Viewer>, cx: &mut App) {
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        if std::env::var_os("NAGAME_UI_PDF").is_some() {
            executor.timer(Duration::from_millis(300)).await;
            pdf_check(handle, cx, &executor).await;
            let any: AnyWindowHandle = handle.into();
            any.update(cx, |_, window, _| window.remove_window()).unwrap();
            return;
        }
        if let Ok(mode) = std::env::var("NAGAME_UI_WINDOW_STATE") {
            executor.timer(Duration::from_millis(300)).await;
            let expected = [1100., 620.];
            if mode == "resize" {
                handle.update(cx, |_, window, _| window.resize(size(px(expected[0]), px(expected[1])))).unwrap();
                wait_for_window_state(handle, cx, &executor, |viewer, window| {
                    viewer.window_state.size == expected && window.bounds().size == size(px(expected[0]), px(expected[1]))
                }).await;
                handle.update(cx, |_, window, _| window.toggle_fullscreen()).unwrap();
                wait_for_window_state(handle, cx, &executor, |_, window| window.is_fullscreen()).await;
            } else if mode == "restore" {
                let saved = crate::window_state::WindowState::load(&crate::window_state::WindowState::path().unwrap());
                handle.update(cx, |viewer, window, cx| {
                    assert!(!window.is_fullscreen());
                    assert_eq!(window.bounds().size, size(px(expected[0]), px(expected[1])));
                    if cx.compositor_name() == "X11" {
                        let saved = saved.position.unwrap();
                        let actual = viewer.window_state.position.unwrap();
                        assert!((saved[0] - actual[0]).abs() <= 5. && (saved[1] - actual[1]).abs() <= 5., "saved {saved:?}, actual {actual:?}");
                    } else {
                        assert!(viewer.window_state.position.is_none());
                    }
                    window.zoom_window();
                }).unwrap();
                wait_for_window_state(handle, cx, &executor, |viewer, window| window.is_maximized() && viewer.window_state.maximized).await;
            } else {
                assert_eq!(mode, "maximized");
                wait_for_window_state(handle, cx, &executor, |viewer, window| window.is_maximized() && viewer.window_state.maximized).await;
                handle.update(cx, |_, window, _| window.zoom_window()).unwrap();
                wait_for_window_state(handle, cx, &executor, |viewer, window| {
                    !window.is_maximized() && viewer.window_state.size == expected && window.bounds().size == size(px(expected[0]), px(expected[1]))
                }).await;
            }
            handle.update(cx, |viewer, _, cx| {
                assert_eq!(viewer.window_state.size, expected, "Normal geometry was overwritten");
                eprintln!("PASS: {} window state {mode}: {:?}", cx.compositor_name(), viewer.window_state);
            }).unwrap();
            let any: AnyWindowHandle = handle.into();
            any.update(cx, |_, window, _| window.remove_window()).unwrap();
            return;
        }
        if std::env::var_os("NAGAME_UI_FOLDER_NAVIGATION").is_some() {
            for _ in 0..200 {
                executor.timer(Duration::from_millis(10)).await;
                if handle.read_with(cx, |v, _| !v.loading && !v.album.entries.is_empty()).unwrap() {
                    executor.timer(Duration::from_millis(100)).await;
                    break;
                }
            }
            let current = handle.read_with(cx, |v, _| v.album.directory.clone()).unwrap();
            let previous = current.parent().unwrap().join("01");
            for previous_first in [true, false] {
                for reverse in [false, true] {
                    for boundary in [BoundaryAction::Folder, BoundaryAction::Ask] {
                        handle.update(cx, |v, _, cx| {
                            v.settings = Settings {
                                previous_folder_first: previous_first,
                                reverse,
                                boundary,
                                ..Settings::default()
                            };
                            v.album = crate::model::Album::open(&current).unwrap();
                            v.album.sort(&v.settings);
                            v.album.index = 0;
                            v.load(true, cx);
                        }).unwrap();
                        wait_for_folder_image(handle, cx, &executor, &current, 0).await;
                        key(handle, cx, "left");
                        if boundary == BoundaryAction::Ask {
                            handle.read_with(cx, |v, _| assert_eq!(v.boundary, Some(-1))).unwrap();
                            handle.update(cx, |v, window, cx| v.command(Command::BoundaryFolder, window, cx)).unwrap();
                        }
                        wait_for_folder_image(handle, cx, &executor, &previous, 2).await;
                        key(handle, cx, "right");
                        if boundary == BoundaryAction::Ask {
                            handle.read_with(cx, |v, _| assert_eq!(v.boundary, Some(1))).unwrap();
                            handle.update(cx, |v, window, cx| v.command(Command::BoundaryFolder, window, cx)).unwrap();
                        }
                        wait_for_folder_image(handle, cx, &executor, &current, 0).await;
                        key(handle, cx, "up");
                        let folder_index = if previous_first { 0 } else { 2 };
                        wait_for_folder_image(handle, cx, &executor, &previous, folder_index).await;
                        key(handle, cx, "down");
                        wait_for_folder_image(handle, cx, &executor, &current, 0).await;
                        key(handle, cx, "ctrl-left");
                        wait_for_folder_image(handle, cx, &executor, &previous, folder_index).await;
                    }
                }
            }
            eprintln!("PASS: boundary image navigation and explicit folder commands select different entry images, including prompts and reverse sorting");
            let any: AnyWindowHandle = handle.into();
            any.update(cx, |_, window, _| window.remove_window()).unwrap();
            return;
        }
        if let Ok(spec) = std::env::var("NAGAME_UI_MEMORY_PROFILE") {
            executor.timer(Duration::from_millis(300)).await;
            memory_profile(handle, cx, &executor, &spec).await;
            let any: AnyWindowHandle = handle.into();
            any.update(cx, |_, window, _| window.remove_window()).unwrap();
            return;
        }
        if std::env::var_os("NAGAME_UI_ANIMATION").is_some() {
            executor.timer(Duration::from_millis(300)).await;
            animation_check(handle, cx, &executor).await;
            let any: AnyWindowHandle = handle.into();
            any.update(cx, |_, window, _| window.remove_window()).unwrap();
            return;
        }
        if std::env::var_os("NAGAME_UI_STRESS").is_some() {
            executor.timer(Duration::from_millis(300)).await;
            for direction in ["right", "left", "right", "left"] {
                let before = handle.read_with(cx, |v, _| v.album.index).unwrap();
                for _ in 0..350 {
                    key(handle, cx, direction);
                    executor.timer(Duration::from_millis(8)).await;
                }
                executor.timer(Duration::from_millis(150)).await;
                handle
                    .read_with(cx, |v, _| {
                        assert!(v.error.is_none(), "{:?}", v.error);
                        assert!(v.image.is_some());
                        assert!(!v.loading);
                        assert!(if direction == "right" {
                            v.album.index > before
                        } else {
                            v.album.index < before
                        });
                    })
                    .unwrap();
                eprintln!("PASS: {direction}: 350 navigation events with rendered frames");
            }
            let any: AnyWindowHandle = handle.into();
            any.update(cx, |_, window, _| window.remove_window())
                .unwrap();
            return;
        }
        if std::env::var_os("NAGAME_UI_RESTART").is_some() {
            executor.timer(Duration::from_millis(300)).await;
            handle
                .read_with(cx, |viewer, _| {
                    assert_eq!(viewer.settings.boundary, BoundaryAction::Wrap);
                    assert!(viewer.settings.manga);
                    assert_eq!(
                        viewer.settings.shrink_algorithm,
                        crate::resample::Algorithm::Nearest
                    );
                    assert_eq!(
                        viewer.settings.enlarge_algorithm,
                        crate::resample::Algorithm::Nearest
                    );
                    assert_eq!(viewer.settings.shortcuts.get("q"), Some(&Shortcut::Rotate));
                    assert!(!viewer.settings.shortcuts.contains_key("r"));
                    assert_eq!(viewer.pages, 2);
                    assert_eq!(viewer.settings.language, Language::English);
                    assert_eq!(tr!("settings"), "Settings");
                })
                .unwrap();
            eprintln!("PASS: saved settings restored after process restart");
            let any: AnyWindowHandle = handle.into();
            any.update(cx, |_, window, _| window.remove_window())
                .unwrap();
            return;
        }
        executor.timer(Duration::from_millis(300)).await;
        key(handle, cx, "ctrl-,");
        executor.timer(Duration::from_millis(100)).await;
        if std::env::var_os("NAGAME_UI_CHECKBOX_SNAPSHOT").is_some() {
            eprintln!("SNAPSHOT: settings checkboxes");
            executor.timer(Duration::from_secs(10)).await;
        }
        key(handle, cx, "down");
        key(handle, cx, "space");
        handle.read_with(cx, |v, _| {
            assert!(v.settings_draft.as_ref().unwrap().previous_folder_first);
            assert!(!v.settings.previous_folder_first);
        }).unwrap();
        executor.timer(Duration::from_millis(50)).await;
        key(handle, cx, "space");
        handle.read_with(cx, |v, _| assert!(!v.settings_draft.as_ref().unwrap().previous_folder_first)).unwrap();
        key(handle, cx, "space");
        handle.read_with(cx, |v, _| assert!(v.settings_draft.as_ref().unwrap().previous_folder_first)).unwrap();
        key(handle, cx, "escape");
        handle.read_with(cx, |v, _| assert!(!v.settings.previous_folder_first)).unwrap();
        key(handle, cx, "ctrl-,");
        executor.timer(Duration::from_millis(100)).await;
        handle.read_with(cx, |v, _| assert!(!v.settings_draft.as_ref().unwrap().previous_folder_first)).unwrap();
        eprintln!("PASS: Space toggles checkbox draft values and Cancel discards checkbox changes");
        key(handle, cx, "up");
        key(handle, cx, "right");
        key(handle, cx, "down");
        handle.read_with(cx, |v,_| {
            assert!(v.settings_selector.is_some());
            assert_eq!(v.settings_draft.as_ref().unwrap().boundary,BoundaryAction::Ask);
        }).unwrap();
        key(handle,cx,"escape");
        handle.read_with(cx, |v,_| {
            assert!(v.settings_selector.is_none());
            assert!(v.settings_draft.is_some());
            assert_eq!(v.settings_draft.as_ref().unwrap().boundary,BoundaryAction::Ask);
        }).unwrap();
        key(handle,cx,"space");key(handle,cx,"down");key(handle,cx,"enter");
        handle.read_with(cx, |v,_| {
            assert!(v.settings_selector.is_none());
            assert_eq!(v.settings_draft.as_ref().unwrap().boundary,BoundaryAction::Wrap);
            assert_eq!(v.settings.boundary,BoundaryAction::Ask);
        }).unwrap();
        key(handle, cx, "escape");
        handle
            .read_with(cx, |viewer, _| {
                assert_eq!(viewer.settings.boundary, BoundaryAction::Ask)
            })
            .unwrap();
        key(handle, cx, "ctrl-,");
        executor.timer(Duration::from_millis(100)).await;
        key(handle, cx, "right");
        key(handle, cx, "down");
        key(handle, cx, "enter");
        key(handle, cx, "enter");
        executor.timer(Duration::from_millis(150)).await;
        assert_eq!(
            Settings::read(&Settings::path().unwrap()).unwrap().boundary,
            BoundaryAction::Wrap
        );
        eprintln!("PASS: selector open, uncommitted highlight, Escape, selection, cancel, apply and disk persistence");
        key(handle, cx, "ctrl-,");
        executor.timer(Duration::from_millis(100)).await;
        key(handle, cx, "2");
        handle
            .read_with(cx, |viewer, _| assert!(viewer.settings_tab == Tab::Display))
            .unwrap();
        executor.timer(Duration::from_millis(100)).await;
        for _ in 0..4 {
            key(handle, cx, "down");
        }
        key(handle, cx, "space");
        handle
            .read_with(cx, |viewer, _| {
                assert!(viewer.settings_draft.as_ref().unwrap().manga)
            })
            .unwrap();
        key(handle, cx, "enter");
        executor.timer(Duration::from_millis(200)).await;
        handle
            .read_with(cx, |viewer, _| {
                assert!(viewer.settings.manga);
                assert_eq!(viewer.pages, 2);
                assert_eq!(viewer.dimensions, [800., 600.]);
            })
            .unwrap();
        eprintln!("PASS: display tab, manga toggle and real spread rendering");
        for (input, index, pages, dimensions) in [
            ("space", 2, 1, [800., 400.]),
            ("space", 3, 2, [800., 600.]),
            ("right", 2, 1, [800., 400.]),
            ("right", 0, 2, [800., 600.]),
            ("left", 2, 1, [800., 400.]),
            ("right", 0, 2, [800., 600.]),
        ] {
            key(handle, cx, input);
            executor.timer(Duration::from_millis(150)).await;
            handle
                .read_with(cx, |v, _| {
                    assert_eq!(v.album.index, index);
                    assert_eq!(v.pages, pages);
                    assert_eq!(v.dimensions, dimensions);
                    assert!(v.error.is_none());
                })
                .unwrap();
        }
        eprintln!("PASS: manga forward/backward, landscape boundaries and RTL controls");
        key(handle, cx, "i");
        handle
            .read_with(cx, |v, _| assert!(v.info_visible))
            .unwrap();
        key(handle, cx, "escape");
        key(handle, cx, "f10");
        handle
            .read_with(cx, |v, _| {
                assert_eq!(v.active_menu, Some(crate::menu_ui::Menu::File))
            })
            .unwrap();
        // ←で末尾の「表示」へ回り、→で「ファイル」「移動」を経て「並び順」へ移る。
        key(handle, cx, "left");
        handle
            .read_with(cx, |v, _| {
                assert_eq!(v.active_menu, Some(crate::menu_ui::Menu::View))
            })
            .unwrap();
        for _ in 0..3 {
            key(handle, cx, "right");
        }
        // 並び順のメニューで、上下キーは区切り線を飛ばして「逆順」へ移り、上端から下端へ回る。
        for _ in 0..5 {
            key(handle, cx, "down");
        }
        handle
            .read_with(cx, |v, _| {
                assert_eq!(v.active_menu, Some(crate::menu_ui::Menu::Sort));
                assert_eq!(v.menu_selected, 6);
            })
            .unwrap();
        key(handle, cx, "down");
        key(handle, cx, "up");
        handle
            .read_with(cx, |v, _| assert_eq!(v.menu_selected, 6))
            .unwrap();
        key(handle, cx, "escape");
        handle
            .read_with(cx, |v, _| assert!(v.active_menu.is_none()))
            .unwrap();
        key(handle, cx, "ctrl-,");
        key(handle, cx, "4");
        for _ in 0..Shortcut::ALL
            .iter()
            .position(|a| *a == Shortcut::Rotate)
            .unwrap()
        {
            key(handle, cx, "down");
        }
        executor.timer(Duration::from_millis(100)).await;
        handle
            .read_with(cx, |v, _| {
                assert!(f32::from(v.settings_scroll.max_offset().height) > 0.);
                assert!(f32::from(v.settings_scroll.offset().y) < 0.);
            })
            .unwrap();
        if std::env::var_os("NAGAME_UI_SNAPSHOT").is_some() {
            eprintln!("SNAPSHOT: settings keyboard");
            executor.timer(Duration::from_secs(3)).await;
        }
        key(handle, cx, "space");
        key(handle, cx, "q");
        key(handle, cx, "enter");
        executor.timer(Duration::from_millis(200)).await;
        key(handle, cx, "r");
        handle
            .read_with(cx, |v, _| assert_eq!(v.dimensions, [800., 600.]))
            .unwrap();
        key(handle, cx, "q");
        executor.timer(Duration::from_millis(200)).await;
        handle
            .read_with(cx, |v, _| assert_eq!(v.dimensions, [600., 800.]))
            .unwrap();
        assert_eq!(
            Settings::read(&Settings::path().unwrap())
                .unwrap()
                .shortcuts
                .get("q"),
            Some(&Shortcut::Rotate)
        );
        eprintln!("PASS: menus, image information, custom key capture and replacement dispatch");
        key(handle, cx, "ctrl-,");
        key(handle, cx, "2");
        for _ in 0..8 {
            key(handle, cx, "down");
        }
        executor.timer(Duration::from_millis(100)).await;
        key(handle, cx, "right");
        if std::env::var_os("NAGAME_UI_SELECTOR_SNAPSHOT").is_some() {
            eprintln!("SNAPSHOT: interpolation selector");
            executor.timer(Duration::from_secs(3)).await;
        }
        key(handle, cx, "down");
        key(handle, cx, "enter");
        key(handle, cx, "down");
        key(handle, cx, "right");
        key(handle, cx, "down");
        key(handle, cx, "enter");
        key(handle, cx, "enter");
        executor.timer(Duration::from_millis(150)).await;
        for _ in 0..6 {
            key(handle, cx, "-");
        }
        for _ in 0..100 {
            executor.timer(Duration::from_millis(10)).await;
            if handle
                .read_with(cx, |v, _| v.resampling.ready_id().is_some())
                .unwrap()
            {
                break;
            }
        }
        handle
            .read_with(cx, |v, _| {
                assert_eq!(
                    v.settings.shrink_algorithm,
                    crate::resample::Algorithm::Nearest
                );
                assert!(v.resampling.ready_id().is_some());
                assert_ne!(
                    v.resampling.ready_id(),
                    v.image.as_ref().map(|image| image.id)
                );
            })
            .unwrap();
        key(handle, cx, "q");
        executor.timer(Duration::from_millis(200)).await;
        eprintln!(
            "PASS: settings auto-scroll, persisted interpolation selection and display resampling"
        );

        let initial_language = handle.read_with(cx, |v, _| v.settings.language).unwrap();
        let initial_label = tr!("settings");
        key(handle, cx, "ctrl-,");
        key(handle, cx, "2");
        for _ in 0..10 { key(handle, cx, "down"); }
        key(handle, cx, "right");
        key(handle, cx, "end");
        key(handle, cx, "enter");
        assert_eq!(tr!("settings"), initial_label);
        handle.read_with(cx, |v, _| {
            assert_eq!(v.settings_draft.as_ref().unwrap().language, Language::English);
            assert_eq!(v.settings.language, initial_language);
        }).unwrap();
        key(handle, cx, "escape");
        assert_eq!(tr!("settings"), initial_label);

        for (language, selection, expected_label) in [
            (Language::English, "end", "Settings"),
            (Language::Japanese, "home", "設定"),
            (Language::English, "end", "Settings"),
        ] {
            key(handle, cx, "ctrl-,");
            key(handle, cx, "2");
            for _ in 0..10 { key(handle, cx, "down"); }
            key(handle, cx, "right");
            key(handle, cx, selection);
            if language == Language::Japanese { key(handle, cx, "down"); }
            key(handle, cx, "enter");
            key(handle, cx, "enter");
            executor.timer(Duration::from_millis(150)).await;
            assert_eq!(tr!("settings"), expected_label);
            assert_eq!(Settings::read(&Settings::path().unwrap()).unwrap().language, language);
            if std::env::var_os("NAGAME_UI_LANGUAGE_SNAPSHOT").is_some() {
                key(handle, cx, "ctrl-,");
                key(handle, cx, "2");
                eprintln!("SNAPSHOT: language {language:?}");
                executor.timer(Duration::from_secs(3)).await;
                key(handle, cx, "escape");
            }
        }
        eprintln!("PASS: language draft, cancel, Japanese/English apply and persistence");
        for algorithm in [crate::resample::Algorithm::Linear,crate::resample::Algorithm::Cubic,crate::resample::Algorithm::Lanczos3] {
            let old=handle.read_with(cx,|v,_|v.resampling.ready_id()).unwrap();
            handle.update(cx,|v,_,cx| {
                v.settings.shrink_algorithm=algorithm;
                v.settings.enlarge_algorithm=algorithm;
                cx.notify();
            }).unwrap();
            for _ in 0..200 {
                executor.timer(Duration::from_millis(10)).await;
                if handle.read_with(cx,|v,_|v.resampling.ready_id().is_some() && v.resampling.ready_id()!=old).unwrap(){break;}
            }
            handle.read_with(cx,|v,_| {
                assert_ne!(v.resampling.ready_id(),old,"{algorithm:?} did not finish");
                let image=v.resampling.ready_image().expect("filtered display image missing");
                let pixels=image.as_bytes(0).unwrap().as_chunks::<4>().0;
                assert!(pixels.iter().all(|p|p[3]==255),"{algorithm:?}: opaque image lost alpha");
                assert!(pixels.iter().any(|p|p[0]>200 || p[2]>200),"{algorithm:?}: image lost color");
            }).unwrap();
            executor.timer(Duration::from_millis(100)).await;
            eprintln!("PASS: {algorithm:?} filtered display preserves opacity and color");
            if std::env::var_os("NAGAME_UI_FILTER_SNAPSHOT").is_some() {
                eprintln!("SNAPSHOT: {algorithm:?}");
                executor.timer(Duration::from_secs(3)).await;
            }
        }
        let any: AnyWindowHandle = handle.into();
        any.update(cx, |_, window, _| window.remove_window())
            .unwrap();
    })
    .detach();
}
