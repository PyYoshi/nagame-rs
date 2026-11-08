use super::synthetic::{Item, Page, pdf};
use super::*;

/// 補助プロセスとして起動されたときだけ処理を行う。通常のテスト実行では何もしない。
#[test]
fn helper_process() {
    if std::env::var_os("NAGAME_TEST_PDF_HELPER").is_some() {
        std::process::exit(helper::run());
    }
}

fn write(name: &str, bytes: &[u8]) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("nagame-pdf-test-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// テストは並行して動くため、種類ごとの置き換えが起きない開く要求の順番で描く。
fn draw(path: &Path, page: u32, mode: Mode) -> Result<Rendered, Failure> {
    render(path, page, mode, Class::Open, || false)
}

fn pixel(rendered: &Rendered, x: u32, y: u32) -> [u8; 4] {
    rendered.pixels.get_pixel(x, y).0
}

const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
/// BGRAで見た赤と白。
const RED_BGRA: [u8; 4] = [0, 0, 255, 255];
const WHITE_BGRA: [u8; 4] = [255, 255, 255, 255];

#[test]
fn page_sizes_reflect_rotation_and_crop_box() {
    let path = write(
        "sizes.pdf",
        &pdf(
            &[
                Page {
                    size: [200., 100.],
                    ..Page::default()
                },
                Page {
                    size: [200., 100.],
                    rotate: 90,
                    ..Page::default()
                },
                Page {
                    size: [300., 400.],
                    crop: Some([0., 0., 150., 200.]),
                    ..Page::default()
                },
            ],
            false,
        ),
    );
    assert_eq!(
        open(&path).unwrap(),
        vec![[200., 100.], [100., 200.], [150., 200.]]
    );
}

#[test]
fn renders_bgra_on_white_with_page_rotation() {
    let half = Item::Rect {
        rgb: RED,
        rect: [0., 0., 50., 50.],
    };
    let path = write(
        "colors.pdf",
        &pdf(
            &[
                Page {
                    size: [100., 50.],
                    items: vec![half.clone()],
                    ..Page::default()
                },
                Page {
                    size: [100., 50.],
                    rotate: 90,
                    items: vec![half],
                    ..Page::default()
                },
            ],
            false,
        ),
    );
    let flat = draw(
        &path,
        0,
        Mode::Size {
            width: 200,
            height: 100,
        },
    )
    .unwrap();
    assert_eq!(flat.pixels.dimensions(), (200, 100));
    assert_eq!(pixel(&flat, 50, 50), RED_BGRA);
    assert_eq!(pixel(&flat, 150, 50), WHITE_BGRA);
    assert!(flat.vector);
    // 時計回りに90°回すと、ページの左側が上になる。
    let rotated = draw(
        &path,
        1,
        Mode::Size {
            width: 50,
            height: 100,
        },
    )
    .unwrap();
    assert_eq!(pixel(&rotated, 25, 25), RED_BGRA);
    assert_eq!(pixel(&rotated, 25, 75), WHITE_BGRA);
}

#[test]
fn image_pages_use_the_embedded_resolution_and_text_pages_use_the_base_dpi() {
    // 144×192 pt（2×2.67インチ）に600×800画素の画像を敷く。300 DPIのスキャンに相当する。
    let scan = Item::Image {
        pixels: [600, 800],
        left: RED,
        right: BLUE,
        rect: [0., 0., 144., 192.],
    };
    let text = |invisible| Item::Text {
        text: "Nagame".into(),
        position: [10., 10.],
        size: 12.,
        invisible,
    };
    let path = write(
        "kinds.pdf",
        &pdf(
            &[
                Page {
                    size: [144., 192.],
                    items: vec![scan.clone(), text(true)],
                    ..Page::default()
                },
                Page {
                    size: [144., 192.],
                    items: vec![scan, text(false)],
                    ..Page::default()
                },
                Page {
                    size: [72., 144.],
                    items: vec![Item::Rect {
                        rgb: BLUE,
                        rect: [0., 0., 10., 10.],
                    }],
                    ..Page::default()
                },
            ],
            false,
        ),
    );
    let limit = Mode::Base { limit: 1 << 30 };
    // OCRの不可視テキストだけを重ねたページは画像主体。
    let scanned = draw(&path, 0, limit).unwrap();
    assert_eq!(scanned.pixels.dimensions(), (600, 800));
    assert!(!scanned.vector && !scanned.limited);
    assert_eq!(pixel(&scanned, 100, 400), RED_BGRA);
    assert_eq!(pixel(&scanned, 500, 400), [255, 0, 0, 255]);
    // 見える文字を重ねたページは文字・図形を含むページで、画像の解像度も反映する。
    let annotated = draw(&path, 1, limit).unwrap();
    assert!(annotated.vector);
    assert_eq!(annotated.pixels.dimensions(), (600, 800));
    // 図形だけのページは150 DPI。
    let vector = draw(&path, 2, limit).unwrap();
    assert!(vector.vector);
    assert_eq!(vector.pixels.dimensions(), (150, 300));
    // 上限を超えるなら縮め、縮めたことを返す。
    let small = draw(&path, 0, Mode::Base { limit: 100_000 }).unwrap();
    assert!(small.limited);
    let (width, height) = small.pixels.dimensions();
    assert!(u64::from(width) * u64::from(height) * 4 <= 100_000);
    assert!((width as f32 / height as f32 - 0.75).abs() < 0.02);
    // 見開きで倍率を揃えるときは指定の倍率で描く。
    let scaled = draw(
        &path,
        2,
        Mode::Scale {
            scale: 2.,
            limit: 1 << 30,
        },
    )
    .unwrap();
    assert_eq!(scaled.pixels.dimensions(), (144, 288));
}

#[test]
fn scans_wrapped_in_form_xobjects_are_image_pages() {
    let path = write(
        "form.pdf",
        &pdf(
            &[Page {
                size: [144., 192.],
                items: vec![Item::FormImage {
                    pixels: [600, 800],
                    color: RED,
                    rect: [0., 0., 144., 192.],
                }],
                ..Page::default()
            }],
            false,
        ),
    );
    let page = draw(&path, 0, Mode::Base { limit: 1 << 30 }).unwrap();
    assert!(!page.vector);
    assert_eq!(page.pixels.dimensions(), (600, 800));
    assert_eq!(pixel(&page, 300, 400), RED_BGRA);
}

#[test]
fn unreadable_documents_report_the_reason() {
    let page = Page {
        size: [100., 100.],
        ..Page::default()
    };
    let encrypted = write("encrypted.pdf", &pdf(std::slice::from_ref(&page), true));
    assert_eq!(open(&encrypted), Err(Failure::Encrypted));
    let corrupt = write("corrupt.pdf", b"%PDF-1.7\nnot a document");
    assert_eq!(open(&corrupt), Err(Failure::Corrupt));
    let empty = write("empty.pdf", &pdf(&[], false));
    assert!(matches!(
        open(&empty),
        Err(Failure::Empty | Failure::Corrupt)
    ));
    let valid = write("valid.pdf", &pdf(&[page], false));
    assert_eq!(
        draw(&valid, 5, Mode::Base { limit: 1 << 30 }).err(),
        Some(Failure::Page)
    );
    assert_eq!(open(Path::new("/nonexistent/nagame.pdf")), Err(Failure::Io));
}

#[test]
fn crashes_restart_the_helper_and_stop_retrying_the_same_document() {
    let page = Page {
        size: [100., 100.],
        ..Page::default()
    };
    let crashing = write("crashing.pdf", &pdf(std::slice::from_ref(&page), false));
    for _ in 0..CRASH_LIMIT {
        assert!(matches!(
            call(&crashing, Request::Abort, Class::Open, || false),
            Err(Failure::Crashed)
        ));
    }
    // 同じ文書で上限まで異常終了したら、開く要求も補助プロセスへ送らない。
    assert_eq!(open(&crashing), Err(Failure::Crashed));
    // 別の文書は起動し直した補助プロセスで開ける。
    let other = write("after-crash.pdf", &pdf(&[page], false));
    assert_eq!(open(&other).unwrap(), vec![[100., 100.]]);
}

#[test]
fn hung_requests_time_out_and_the_helper_restarts() {
    let page = Page {
        size: [100., 100.],
        ..Page::default()
    };
    let path = write("hang.pdf", &pdf(&[page], false));
    let start = Instant::now();
    assert!(matches!(
        call(&path, Request::Hang, Class::Open, || false),
        Err(Failure::Timeout)
    ));
    assert!(start.elapsed() < REQUEST_TIMEOUT * 3);
    assert_eq!(open(&path).unwrap(), vec![[100., 100.]]);
}

#[test]
fn callers_can_stop_waiting() {
    let page = Page {
        size: [100., 100.],
        ..Page::default()
    };
    let path = write("cancel.pdf", &pdf(&[page], false));
    assert_eq!(
        render(
            &path,
            0,
            Mode::Base { limit: 1 << 30 },
            Class::Prefetch,
            || true
        )
        .err(),
        Some(Failure::Cancelled)
    );
}

#[test]
fn queue_keeps_the_latest_request_of_each_class_in_priority_order() {
    let job = |class| {
        let (reply, result) = mpsc::channel();
        (
            Job {
                class,
                request: Request::Trim,
                file: File::open("/proc/self/exe").unwrap(),
                reply,
                abandoned: Arc::new(AtomicBool::new(false)),
            },
            result,
        )
    };
    let mut queue = Queue::default();
    let (prefetch, _) = job(Class::Prefetch);
    let (display, _) = job(Class::Display);
    let (old, replaced) = job(Class::Current);
    let (current, _) = job(Class::Current);
    let (opening, _) = job(Class::Open);
    for job in [prefetch, display, old, current, opening] {
        queue.push(job);
    }
    assert!(matches!(replaced.try_recv(), Ok(Err(Failure::Cancelled))));
    let order: Vec<_> = std::iter::from_fn(|| queue.pop())
        .map(|job| job.class)
        .collect();
    assert_eq!(
        order,
        [Class::Open, Class::Current, Class::Display, Class::Prefetch]
    );
}

#[test]
fn helpers_report_a_missing_library_and_run_under_the_memory_limit() {
    assert_eq!(
        Helper::start_with(Path::new("/nonexistent/libpdfium.so")).err(),
        Some(Failure::Library)
    );
    let helper = Helper::start().unwrap();
    let limits = std::fs::read_to_string(format!(
        "/proc/{}/limits",
        helper.child.as_ref().unwrap().id()
    ))
    .unwrap();
    let data = limits
        .lines()
        .find(|line| line.starts_with("Max data size"))
        .unwrap();
    let values: Vec<_> = data.split_whitespace().skip(3).take(2).collect();
    let limit = helper_limit().to_string();
    assert_eq!(values, [limit.as_str(), limit.as_str()]);
}

#[test]
fn timed_out_documents_are_skipped_for_background_requests_until_forgotten() {
    let _serial = CLASS_TESTS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let page = Page {
        size: [100., 100.],
        ..Page::default()
    };
    let path = write("slow.pdf", &pdf(&[page], false));
    assert!(matches!(
        call(&path, Request::Hang, Class::Display, || false),
        Err(Failure::Timeout)
    ));
    // 先読み・表示解像度の描き直しでは時間切れになった文書を試さない。利用者が開く要求は試し直す。
    let start = Instant::now();
    assert!(matches!(
        call(&path, Request::Open, Class::Display, || false),
        Err(Failure::Timeout)
    ));
    assert!(start.elapsed() < REQUEST_TIMEOUT);
    assert_eq!(open(&path).unwrap(), vec![[100., 100.]]);
    forget(&path);
    assert!(call(&path, Request::Open, Class::Display, || false).is_ok());
}

#[test]
fn abandoned_long_requests_yield_to_waiting_ones_without_a_timeout_strike() {
    let _serial = CLASS_TESTS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let page = Page {
        size: [100., 100.],
        ..Page::default()
    };
    let abandoned = write("abandoned.pdf", &pdf(std::slice::from_ref(&page), false));
    let waiting = write("waiting.pdf", &pdf(&[page], false));
    let stop = Arc::new(AtomicBool::new(false));
    let hang = {
        let (path, stop) = (abandoned.clone(), stop.clone());
        std::thread::spawn(move || {
            call(&path, Request::Hang, Class::Prefetch, || {
                stop.load(Ordering::Relaxed)
            })
            .err()
        })
    };
    std::thread::sleep(Duration::from_millis(300));
    stop.store(true, Ordering::Relaxed);
    assert_eq!(hang.join().unwrap(), Some(Failure::Cancelled));
    assert_eq!(open(&waiting).unwrap(), vec![[100., 100.]]);
    // 時間切れではなく中断として扱ったので、先読みでもこの文書を試せる。
    assert!(call(&abandoned, Request::Open, Class::Prefetch, || false).is_ok());
}

#[test]
fn helpers_cannot_read_outside_fonts_start_processes_or_open_sockets() {
    let page = Page {
        size: [100., 100.],
        ..Page::default()
    };
    let path = write("probe.pdf", &pdf(&[page], false));
    let Ok(Reply::Probe(probe)) = call(&path, Request::Probe, Class::Open, || false) else {
        panic!("no probe reply");
    };
    assert!(probe.report.seccomp, "{probe:?}");
    assert!(
        probe.fork_denied && probe.socket_denied && probe.pidfd_denied,
        "{probe:?}"
    );
    // Landlockはカーネルの対応に依存する。使えた場合は、フォント以外を読めないこと。
    if probe.report.landlock.is_some() {
        assert!(probe.read_denied, "{probe:?}");
    }
    // 制限の後も描ける。
    assert!(draw(&path, 0, Mode::Base { limit: 1 << 30 }).is_ok());
}

#[test]
fn jpeg_scans_render_at_their_embedded_resolution() {
    let pixels = image::RgbImage::from_fn(300, 400, |x, _| {
        if x < 150 {
            image::Rgb([255, 0, 0])
        } else {
            image::Rgb([0, 0, 255])
        }
    });
    let mut data = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut data, 95)
        .encode_image(&pixels)
        .unwrap();
    // 72×96 pt（1×1.33インチ）に300×400画素。300 DPIのスキャン。
    let path = write(
        "jpeg.pdf",
        &pdf(
            &[Page {
                size: [72., 96.],
                items: vec![Item::Jpeg {
                    data,
                    pixels: [300, 400],
                    rect: [0., 0., 72., 96.],
                }],
                ..Page::default()
            }],
            false,
        ),
    );
    let page = draw(&path, 0, Mode::Base { limit: 1 << 30 }).unwrap();
    assert!(!page.vector);
    assert_eq!(page.pixels.dimensions(), (300, 400));
    // JPEGの誤差を許して、左が赤・右が青（BGRA）。
    let [b, g, r, a] = pixel(&page, 50, 200);
    assert!(
        r > 200 && g < 60 && b < 60 && a == 255,
        "{:?}",
        [b, g, r, a]
    );
    let [b, g, r, _] = pixel(&page, 250, 200);
    assert!(b > 200 && g < 60 && r < 60, "{:?}", [b, g, r]);
}
