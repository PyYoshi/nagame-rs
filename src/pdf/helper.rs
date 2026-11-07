//! 補助プロセスの処理。PDFiumを読み込み、本体から受け取った記述子のPDFを開いて、
//! ページをmemfdへBGRAで描く。PDFiumを呼ぶのはこのプロセスの1本のスレッドだけ。

use super::{
    BASE_DPI, Failure, HELPER_FD, HELPER_LIMIT, Identity, LIBRARY_ENV, MAX_SIDE, Mode, Request,
    Response, ipc,
};
use pdfium_render::prelude::*;
use std::{
    collections::VecDeque,
    fs::File,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::{Path, PathBuf},
};

/// 開いたままにする文書の数。見開きと前後のページで同じ文書を使い回す。
const DOCUMENTS: usize = 2;
/// 画像主体とみなす、1枚の画像がページを覆う割合。
const DOMINANT_COVERAGE: f32 = 0.9;
/// 文字・図形を含むページで、基準解像度に画像の解像度を反映する画像の覆う割合。
const LARGE_COVERAGE: f32 = 0.5;
/// 画像の解像度が極端に低い場合の下限（1ptあたりの画素数。36 DPI）。
const MIN_SCALE: f32 = 0.5;
/// 寸法を読めないページの仮の論理寸法（US Letter）。
const PLACEHOLDER: [f32; 2] = [612., 792.];

/// 補助プロセスの本体。本体がソケットを閉じたら終了する。
pub fn run() -> i32 {
    // 本体が先に終了しても残らないようにする。
    // SAFETY: prctlは呼び出したプロセス自身の属性だけを変える。
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
    }
    let Some(fd) = std::env::var(HELPER_FD)
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
    else {
        return 2;
    };
    // SAFETY: 本体がexec前に複製した、このプロセスだけが持つ記述子。
    let socket = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: 自分の記述子にCLOEXECを付けるだけ。
    unsafe {
        libc::fcntl(socket.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
    }
    if let Some(limit) = std::env::var(HELPER_LIMIT)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        let limit = libc::rlimit {
            rlim_cur: limit,
            rlim_max: limit,
        };
        // SAFETY: 自分のプロセスのデータ領域の上限を設定する。
        unsafe {
            libc::setrlimit(libc::RLIMIT_DATA, &limit);
        }
    }
    let library = std::env::var_os(LIBRARY_ENV).map(PathBuf::from);
    let Some(pdfium) = library.as_deref().and_then(load) else {
        let _ = reply(&socket, &Response::Failed(Failure::Library), None);
        return 1;
    };
    if reply(&socket, &Response::Ready, None).is_err() {
        return 1;
    }
    let mut documents: VecDeque<(Identity, PdfDocument<'static>)> = VecDeque::new();
    loop {
        let Ok((payload, fds)) = ipc::recv(&socket, None) else {
            return 0;
        };
        let Ok(request) = serde_json::from_slice::<Request>(&payload) else {
            return 1;
        };
        let file = fds.into_iter().next().map(File::from);
        let (response, memory) = match request {
            Request::Open => match file.map(|file| document(pdfium, &mut documents, file)) {
                Some(Ok(document)) => (pages(document), None),
                Some(Err(failure)) => (Response::Failed(failure), None),
                None => (Response::Failed(Failure::Io), None),
            },
            Request::Render { page, mode } => {
                match file.map(|file| document(pdfium, &mut documents, file)) {
                    Some(Ok(document)) => match render(document, page, mode) {
                        Ok((response, memory)) => (response, Some(memory)),
                        Err(failure) => (Response::Failed(failure), None),
                    },
                    Some(Err(failure)) => (Response::Failed(failure), None),
                    None => (Response::Failed(Failure::Io), None),
                }
            }
            Request::Trim => {
                documents.clear();
                crate::memory::release_free_memory();
                (Response::Trimmed, None)
            }
            #[cfg(test)]
            Request::Hang => {
                std::thread::sleep(std::time::Duration::from_secs(600));
                (Response::Trimmed, None)
            }
            #[cfg(test)]
            Request::Abort => std::process::abort(),
        };
        if reply(&socket, &response, memory.as_ref()).is_err() {
            return 0;
        }
    }
}

fn reply(socket: &OwnedFd, response: &Response, memory: Option<&OwnedFd>) -> std::io::Result<()> {
    let payload = serde_json::to_vec(response).expect("responses serialize");
    let fds: Vec<_> = memory.iter().map(|fd| fd.as_raw_fd()).collect();
    ipc::send(socket, &payload, &fds)
}

/// PDFiumを読み込む。フォントの探索先は既定の代わりに使われるため、既定の4つに
/// 利用者のフォントディレクトリーを加えて、存在するものだけを渡す。
fn load(library: &Path) -> Option<&'static Pdfium> {
    let bindings = Pdfium::bind_to_library(library).ok()?;
    let mut directories: Vec<PathBuf> = [
        "/usr/share/fonts",
        "/usr/share/X11/fonts/Type1",
        "/usr/share/X11/fonts/TTF",
        "/usr/local/share/fonts",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.as_ref().map(|home| home.join(".local/share")));
    directories.extend(data.map(|data| data.join("fonts")));
    directories.extend(home.map(|home| home.join(".fonts")));
    let directories: Vec<String> = directories
        .into_iter()
        .filter(|path| path.is_dir())
        .filter_map(|path| path.to_str().map(str::to_owned))
        .collect();
    let paths: Vec<&str> = directories.iter().map(String::as_str).collect();
    let config = PdfiumLibraryConfig::new()
        .set_user_font_paths(&paths)
        .ok()?;
    // 補助プロセスの終了までPDFiumを使い続けるため、所有者を持たせず保持する。
    Some(Box::leak(Box::new(Pdfium::new_with_config(
        bindings, config,
    ))))
}

/// 記述子のPDFを開く。同じファイル（デバイス・iノード・大きさ・更新日時）なら使い回す。
fn document<'a>(
    pdfium: &'static Pdfium,
    documents: &'a mut VecDeque<(Identity, PdfDocument<'static>)>,
    file: File,
) -> Result<&'a PdfDocument<'static>, Failure> {
    let identity = Identity::of(&file).ok_or(Failure::Io)?;
    if let Some(index) = documents.iter().position(|(id, _)| *id == identity) {
        let entry = documents.remove(index).expect("index is in range");
        documents.push_front(entry);
    } else {
        let document = pdfium
            .load_pdf_from_reader(file, None)
            .map_err(|error| match error {
                PdfiumError::PdfiumLibraryInternalError(
                    PdfiumInternalError::PasswordError | PdfiumInternalError::SecurityError,
                ) => Failure::Encrypted,
                PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FileError) => {
                    Failure::Io
                }
                _ => Failure::Corrupt,
            })?;
        documents.push_front((identity, document));
        documents.truncate(DOCUMENTS);
    }
    Ok(&documents.front().expect("pushed above").1)
}

fn pages(document: &PdfDocument) -> Response {
    let pages = document.pages();
    if pages.is_empty() {
        return Response::Failed(Failure::Empty);
    }
    // 寸法を読めないページは仮の寸法（US Letter）にし、そのページの描画だけを失敗させる。
    // 有限でない値はJSONへ書けず、応答全体が壊れるため必ず置き換える。
    let sizes = pages
        .as_range()
        .map(|index| {
            pages
                .page_size(index)
                .ok()
                .map(|rect| [rect.width().value, rect.height().value])
                .filter(|size| size.iter().all(|value| value.is_finite() && *value > 0.))
                .unwrap_or(PLACEHOLDER)
        })
        .collect();
    Response::Opened { pages: sizes }
}

/// ページの内容から決めた基準解像度。
struct Kind {
    /// 文字・図形を含む（画像主体でない）。
    vector: bool,
    /// 1ptあたりの画素数。
    scale: f32,
}

/// 画像主体（1枚の画像がページの90%以上を覆い、見える文字・図形がない）なら、ページを覆う
/// 画像の解像度の最大値。それ以外は基準DPIと、ページの半分以上を覆う画像の解像度の大きい方。
fn classify(page: &PdfPage, area: f32) -> Kind {
    let mut coverage: f32 = 0.;
    let mut large: f32 = 0.;
    let mut other = false;
    visit(
        page.objects().iter(),
        area,
        1.,
        &mut coverage,
        &mut large,
        &mut other,
        0,
    );
    let base = BASE_DPI / 72.;
    if !other && coverage >= DOMINANT_COVERAGE && large > 0. {
        Kind {
            vector: false,
            scale: large.max(MIN_SCALE),
        }
    } else {
        Kind {
            vector: true,
            scale: base.max(large),
        }
    }
}

/// objectsを調べる。stretchは、objectsの座標の面積をページの面積へ直す倍率（フォームXObjectの
/// 中ではフォームの行列の行列式。PDFiumはフォーム内のオブジェクトの範囲をフォームの座標で返す）。
fn visit<'a>(
    objects: impl Iterator<Item = PdfPageObject<'a>>,
    area: f32,
    stretch: f32,
    coverage: &mut f32,
    large: &mut f32,
    other: &mut bool,
    depth: u8,
) {
    for object in objects {
        match &object {
            PdfPageObject::Image(image) => {
                let (Ok(bounds), Ok(width), Ok(height)) =
                    (object.bounds(), image.width(), image.height())
                else {
                    continue;
                };
                let rect = bounds.to_rect();
                let placed = rect.width().value.abs() * rect.height().value.abs() * stretch;
                if placed <= 0. || width <= 0 || height <= 0 {
                    continue;
                }
                let covered = (placed / area).min(1.);
                *coverage = coverage.max(covered);
                if covered >= LARGE_COVERAGE {
                    // 回転していても面積で比べれば向きに依存しない。
                    let scale = ((width as f32 * height as f32) / placed).sqrt();
                    *large = large.max(scale);
                }
            }
            PdfPageObject::Text(text) => {
                if text.is_visible() {
                    *other = true;
                }
            }
            // スキャン画像をフォームXObjectで包むPDFがあるため、中身も調べる。
            PdfPageObject::XObjectForm(form) if depth < 4 => {
                let inner = form
                    .matrix()
                    .map_or(1., |matrix| matrix.determinant().abs());
                visit(
                    form.iter(),
                    area,
                    stretch * inner,
                    coverage,
                    large,
                    other,
                    depth + 1,
                );
            }
            _ => *other = true,
        }
    }
}

/// 1辺と出力の上限に収まる寸法。limitedは出力の上限で縮めたか。
fn fit(size: [f32; 2], scale: f32, limit: u64) -> (u32, u32, f32, bool) {
    let longest = size[0].max(size[1]);
    let mut scale = scale.min(MAX_SIDE as f32 / longest);
    let mut limited = false;
    let bytes = |scale: f32| {
        let w = (size[0] * scale).round().max(1.) as u64;
        let h = (size[1] * scale).round().max(1.) as u64;
        w * h * 4
    };
    if bytes(scale) > limit {
        limited = true;
        scale *= (limit as f64 / bytes(scale) as f64).sqrt() as f32;
        while bytes(scale) > limit && scale > 0. {
            scale *= 0.99;
        }
    }
    let width = ((size[0] * scale).round().max(1.) as u32).min(MAX_SIDE);
    let height = ((size[1] * scale).round().max(1.) as u32).min(MAX_SIDE);
    (width, height, scale, limited)
}

fn render(document: &PdfDocument, index: u32, mode: Mode) -> Result<(Response, OwnedFd), Failure> {
    let pages = document.pages();
    let index = i32::try_from(index).map_err(|_| Failure::Page)?;
    let page = pages.get(index).map_err(|_| Failure::Page)?;
    let size = [page.width().value, page.height().value];
    if !size.iter().all(|value| value.is_finite() && *value > 0.) {
        return Err(Failure::Page);
    }
    let kind = classify(&page, size[0] * size[1]);
    let (width, height, scale, limited) = match mode {
        Mode::Base { limit } => fit(size, kind.scale, limit),
        Mode::Scale { scale, limit } => fit(size, scale, limit),
        Mode::Size { width, height } => {
            if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
                return Err(Failure::Page);
            }
            (width, height, width as f32 / size[0], false)
        }
    };
    let bytes = width as usize * height as usize * 4;
    let memory = memfd(bytes).map_err(|_| Failure::Page)?;
    {
        let mut mapping = Mapping::new(&memory, bytes).map_err(|_| Failure::Page)?;
        // BGRAは1画素4バイトなので、PDFiumの行の長さ（4バイト単位へ切り上げ）は幅×4と一致し、
        // 本体は行の詰め物なしで読める。
        let mut bitmap = PdfBitmap::from_bytes(
            width as i32,
            height as i32,
            PdfBitmapFormat::BGRA,
            mapping.as_mut(),
        )
        .map_err(|_| Failure::Page)?;
        let config = PdfRenderConfig::new()
            .set_fixed_size(width as i32, height as i32)
            .set_format(PdfBitmapFormat::BGRA)
            .set_reverse_byte_order(false)
            .set_clear_color(PdfColor::WHITE)
            .render_annotations(true)
            .render_form_data(true)
            .limit_render_image_cache_size(true);
        page.render_into_bitmap_with_config(&mut bitmap, &config)
            .map_err(|_| Failure::Page)?;
    }
    Ok((
        Response::Rendered {
            width,
            height,
            scale,
            vector: kind.vector,
            limited,
        },
        memory,
    ))
}

fn memfd(bytes: usize) -> std::io::Result<OwnedFd> {
    // SAFETY: 名前はNUL終端の定数。成功時は新しい記述子を返す。
    let fd = unsafe { libc::memfd_create(c"nagame-pdf".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: memfd_createが返した記述子の所有者になる。
    let memory = unsafe { OwnedFd::from_raw_fd(fd) };
    File::from(memory.try_clone()?).set_len(bytes as u64)?;
    Ok(memory)
}

/// memfdを読み書きできるように割り当てた領域。破棄すると割り当てを外す。
struct Mapping {
    pointer: *mut u8,
    length: usize,
}
impl Mapping {
    fn new(memory: &OwnedFd, length: usize) -> std::io::Result<Self> {
        // SAFETY: lengthはset_lenした大きさ以下で、共有の読み書き可能な割り当てを作る。
        let pointer = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                memory.as_raw_fd(),
                0,
            )
        };
        if pointer == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self {
            pointer: pointer.cast(),
            length,
        })
    }
    fn as_mut(&mut self) -> &mut [u8] {
        // SAFETY: mmapが返したlengthバイトの領域で、このMappingだけが参照する。
        unsafe { std::slice::from_raw_parts_mut(self.pointer, self.length) }
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: newで割り当てた領域を一度だけ外す。
        unsafe {
            libc::munmap(self.pointer.cast(), self.length);
        }
    }
}
