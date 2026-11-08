//! PDFに埋め込まれていないフォントを、fontconfigで選んでPDFiumへ渡す（補助プロセス内）。
//!
//! PDFium既定のフォント探索はfontconfigを使わず、日本語の代替を固定の名前表と類似度で選ぶ。
//! そのため明朝の指定にもゴシック体が選ばれることがあった。ここでは書体名・文字セットから
//! 言語と書体の種類（明朝／ゴシック）を推定し、fontconfigの優先順位でその言語に対応する
//! フォントを選ぶ。`pdfium-render` の `PdfiumCustomFontProvider` はUTF-8でない書体名
//! （Shift_JISの「ＭＳ 明朝」等）を扱えず、TTCの書体も選べないため、`FPDF_SYSFONTINFO` を直接実装する。

use fontconfig_sys as fc;
use pdfium_render::prelude::*;
use std::{
    cell::RefCell,
    ffi::{CStr, CString, c_char, c_int, c_uchar, c_uint, c_ulong, c_void},
    path::PathBuf,
};

/// fpdf_sysfontinfo.hの文字セット。
const CHARSET_SYMBOL: c_int = 2;
const CHARSET_SHIFTJIS: c_int = 128;
const CHARSET_HANGEUL: c_int = 129;
const CHARSET_JOHAB: c_int = 130;
const CHARSET_GB2312: c_int = 134;
const CHARSET_BIG5: c_int = 136;
const CHARSET_GREEK: c_int = 161;
const CHARSET_VIETNAMESE: c_int = 163;
const CHARSET_HEBREW: c_int = 177;
const CHARSET_ARABIC: c_int = 178;
const CHARSET_CYRILLIC: c_int = 204;
const CHARSET_THAI: c_int = 222;
/// fpdf_sysfontinfo.hのpitch_family。
const PITCH_FIXED: c_int = 1;
const FAMILY_ROMAN: c_int = 0x10;
/// TTCの表タグ（PDFiumはこれでTTC全体を求める）。
const TAG_TTCF: c_uint = u32::from_be_bytes(*b"ttcf");

/// fontconfigへの問い合わせ。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Query {
    /// PDFが指定した書体名（UTF-8で読めた場合）。
    pub family: Option<String>,
    /// 書体名で見つからない場合の総称（serif・sans-serif・monospace）。
    pub generic: &'static str,
    /// 必要な言語（fontconfigの言語タグ）。
    pub lang: Option<&'static str>,
    /// fontconfigの太さ。
    pub weight: c_int,
    pub italic: bool,
}

/// 書体名に含まれていれば言語が分かる語（小文字。Shift_JISの「明朝」「ゴシック」も含む）。
const JAPANESE: &[&[u8]] = &[
    b"mincho",
    b"msgothic",
    b"ms-gothic",
    b"ms gothic",
    b"mspgothic",
    b"ms-pgothic",
    b"ms pgothic",
    b"ryumin",
    b"heisei",
    b"kozuka",
    b"hiragino",
    b"meiryo",
    b"yugothic",
    b"yu gothic",
    b"ipagothic",
    b"ipamincho",
    b"ipaexg",
    b"ipaexm",
    b"takao",
    "明朝".as_bytes(),
    "ゴシック".as_bytes(),
    b"\x96\xbe\x92\xa9",
    b"\x83\x53\x83\x56\x83\x62\x83\x4e",
];
const SIMPLIFIED_CHINESE: &[&[u8]] = &[
    b"simsun",
    b"simhei",
    b"songti",
    b"stsong",
    b"stheiti",
    b"fangsong",
    b"kaiti",
    b"yahei",
    b"dengxian",
    "宋体".as_bytes(),
    "黑体".as_bytes(),
];
const TRADITIONAL_CHINESE: &[&[u8]] = &[b"mingliu", b"dfkai", b"jhenghei", "明體".as_bytes()];
const KOREAN: &[&[u8]] = &[
    b"batang",
    b"gulim",
    b"dotum",
    b"gungsuh",
    b"malgun",
    b"hygothic",
    b"myeongjo",
    "바탕".as_bytes(),
    "굴림".as_bytes(),
    "돋움".as_bytes(),
];
/// 明朝系（セリフ）を示す語。ゴシック系より先に調べる（「HeiseiMin」等）。
const SERIF: &[&[u8]] = &[
    b"mincho",
    b"heiseimin",
    b"ryumin",
    b"ipaexm",
    b"ming",
    b"song",
    b"batang",
    b"myeongjo",
    b"serif",
    b"times",
    b"roman",
    b"century",
    b"georgia",
    b"garamond",
    "明朝".as_bytes(),
    "明體".as_bytes(),
    "宋".as_bytes(),
    "바탕".as_bytes(),
    b"\x96\xbe\x92\xa9",
];
const SANS: &[&[u8]] = &[
    b"gothic",
    b"kaku",
    b"maru",
    b"ipaexg",
    b"sans",
    b"arial",
    b"helvetica",
    b"verdana",
    b"meiryo",
    b"simhei",
    b"heiti",
    b"yahei",
    b"jhenghei",
    b"gulim",
    b"dotum",
    b"malgun",
    "ゴシック".as_bytes(),
    "黑".as_bytes(),
    "굴림".as_bytes(),
    "돋움".as_bytes(),
    b"\x83\x53\x83\x56\x83\x62\x83\x4e",
];
const SYMBOLS: &[&[u8]] = &[
    b"wingdings",
    b"webdings",
    b"symbol",
    b"zapfdingbats",
    b"dingbats",
    b"marlett",
    b"mt extra",
];
const BOLD: &[&[u8]] = &[
    b"bold", b"heavy", b"black", b"-w6", b"-w7", b"-w8", b"-w9", b"w6", b"w7", b"w8", b"w9",
];

fn contains(text: &[u8], words: &[&[u8]]) -> bool {
    words
        .iter()
        .any(|word| text.windows(word.len()).any(|window| window == *word))
}

/// 書体名に語を含むか。英字は大文字小文字を区別しない。Shift_JISの2バイト目は英字と同じ値を
/// 取り得るため、多バイトの語は元のバイト列で比べる。
fn named(face: &[u8], lower: &[u8], words: &[&[u8]]) -> bool {
    contains(lower, words) || contains(face, words)
}

/// PDFiumの要求を問い合わせに変える。記号フォントはNone（PDFium内蔵のフォントを使う）。
pub fn query(
    weight: c_int,
    italic: bool,
    charset: c_int,
    pitch: c_int,
    face: &[u8],
) -> Option<Query> {
    // 記号フォントはPDFium内蔵のフォントを使う。PDFiumは文字セットを変えて問い合わせ直すため、
    // よく使われる記号フォントは名前でも除く（DejaVu Sans等へ置き換えない）。
    if charset == CHARSET_SYMBOL || named(face, &face.to_ascii_lowercase(), SYMBOLS) {
        return None;
    }
    // 埋め込みの部分フォントには「ABCDEF+」の接頭辞が付く。
    let face = match face.iter().position(|&b| b == b'+') {
        Some(6) => &face[7..],
        _ => face,
    };
    let lower = face.to_ascii_lowercase();
    let lang = match charset {
        CHARSET_SHIFTJIS => Some("ja"),
        CHARSET_HANGEUL | CHARSET_JOHAB => Some("ko"),
        CHARSET_GB2312 => Some("zh-cn"),
        CHARSET_BIG5 => Some("zh-tw"),
        CHARSET_GREEK => Some("el"),
        CHARSET_VIETNAMESE => Some("vi"),
        CHARSET_HEBREW => Some("he"),
        CHARSET_ARABIC => Some("ar"),
        CHARSET_CYRILLIC => Some("ru"),
        CHARSET_THAI => Some("th"),
        _ if named(face, &lower, JAPANESE) => Some("ja"),
        _ if named(face, &lower, KOREAN) => Some("ko"),
        _ if named(face, &lower, TRADITIONAL_CHINESE) => Some("zh-tw"),
        _ if named(face, &lower, SIMPLIFIED_CHINESE) => Some("zh-cn"),
        _ => None,
    };
    // 書体名での推定は日中韓の書体だけ（欧文の「SansSerif」「Century Gothic」等を取り違えない）。
    // 欧文はPDFが示すフラグに従う。ゴシック系の語を先に調べる（「MS Sans Serif」等）。
    let cjk = matches!(lang, Some("ja" | "ko" | "zh-cn" | "zh-tw"));
    let generic = if cjk && named(face, &lower, SANS) {
        "sans-serif"
    } else if cjk && named(face, &lower, SERIF) {
        "serif"
    } else if pitch & PITCH_FIXED != 0 {
        "monospace"
    } else if pitch & FAMILY_ROMAN != 0 {
        "serif"
    } else {
        "sans-serif"
    };
    let weight = if named(face, &lower, BOLD) {
        700
    } else if (100..=900).contains(&weight) {
        weight
    } else {
        400
    };
    let family = std::str::from_utf8(face)
        .ok()
        .and_then(|name| name.split(',').next())
        .map(|name| name.replace('-', " ").trim().to_owned())
        .filter(|name| !name.is_empty());
    Some(Query {
        family,
        generic,
        lang,
        weight: fontconfig_weight(weight),
        italic: italic || lower.windows(6).any(|w| w == b"italic"),
    })
}

/// OpenTypeの太さ（100〜900）をfontconfigの太さへ変える。
fn fontconfig_weight(weight: c_int) -> c_int {
    use fc::constants::*;
    [
        (150, FC_WEIGHT_THIN),
        (250, FC_WEIGHT_EXTRALIGHT),
        (350, FC_WEIGHT_LIGHT),
        (450, FC_WEIGHT_REGULAR),
        (550, FC_WEIGHT_MEDIUM),
        (650, FC_WEIGHT_DEMIBOLD),
        (750, FC_WEIGHT_BOLD),
        (850, FC_WEIGHT_EXTRABOLD),
    ]
    .into_iter()
    .find(|(limit, _)| weight <= *limit)
    .map_or(FC_WEIGHT_BLACK, |(_, value)| value)
}

fn be16(data: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?) as usize)
}

fn be32(data: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?) as usize)
}

/// フォントファイル内の、index番目の書体の表ディレクトリーの位置（TTCでなければ0）。
pub fn face_offset(data: &[u8], index: u32) -> Option<usize> {
    if data.get(..4)? != b"ttcf" {
        return (index == 0).then_some(0);
    }
    if index as usize >= be32(data, 8)? {
        return None;
    }
    let offset = be32(data, 12 + 4 * index as usize)?;
    (offset + 12 <= data.len()).then_some(offset)
}

/// offsetの書体が持つtagの表。
pub fn table(data: &[u8], offset: usize, tag: u32) -> Option<&[u8]> {
    let count = be16(data, offset + 4)?;
    (0..count).find_map(|i| {
        let record = offset + 12 + 16 * i;
        if be32(data, record)? as u32 != tag {
            return None;
        }
        let start = be32(data, record + 8)?;
        data.get(start..start + be32(data, record + 12)?)
    })
}

/// 選んだフォントファイルの書体。
#[derive(Clone)]
struct Found {
    path: PathBuf,
    index: u32,
    family: CString,
    /// 可変フォント（またはその名前付きインスタンス）。PDFiumは名前付きインスタンスを
    /// 選べず既定のインスタンス（Noto CJKのVFでは最も細い太さ）で描く。
    variable: bool,
}

/// 書体名のデザインの同一性を比べるための形（英数字の小文字、「CJK」を除く）。
/// 「Noto Sans CJK JP」と「Noto Sans JP」は同じデザインの別配布。
fn design(family: &CStr) -> String {
    let lower: String = family
        .to_bytes()
        .iter()
        .filter(|b| b.is_ascii_alphanumeric())
        .map(|b| b.to_ascii_lowercase() as char)
        .collect();
    lower.replace("cjk", "")
}

/// fontconfigの優先順位で並んだ候補から選ぶ。先頭が可変フォントなら、同じデザインの静的な
/// フォントが後にあればそちらを選ぶ（太さを保つため）。別のデザインには替えない（字形が変わる）。
fn choose(candidates: Vec<Found>) -> Option<Found> {
    let first = candidates.first()?;
    let position = if first.variable {
        let wanted = design(&first.family);
        candidates
            .iter()
            .position(|found| !found.variable && design(&found.family) == wanted)
            .unwrap_or(0)
    } else {
        0
    };
    candidates.into_iter().nth(position)
}

/// fontconfigの設定。補助プロセスの終了まで使う。GPUIと同じく `libfontconfig.so.1` を
/// 実行時に読み込む（読み込めなければPDFium既定の探索を使う）。
struct Fontconfig {
    lib: &'static fc::Fc,
    config: *mut fc::FcConfig,
}

impl Fontconfig {
    fn load() -> Option<Self> {
        let lib = fc::statics::LIB_RESULT.as_ref().ok()?;
        // SAFETY: 設定とフォントの一覧を読み込むだけ。失敗するとnullを返す。
        let config = unsafe { (lib.FcInitLoadConfigAndFonts)() };
        if config.is_null() {
            return None;
        }
        // 置換（FcDefaultSubstitute等）が内部で使う「現在の設定」にもする。しないと、権限を
        // 絞った後に既定の設定を読み直そうとして失敗し、そのたびに時間がかかる。
        // SAFETY: configは読み込み済みの設定。
        unsafe { (lib.FcConfigSetCurrent)(config) };
        Some(Self { lib, config })
    }

    /// 問い合わせに合う、言語に対応したスケーラブルなフォント。fontconfigの優先順位で選ぶ。
    fn resolve(&self, query: &Query) -> Option<Found> {
        let family = query.family.as_deref().and_then(|f| CString::new(f).ok());
        let generic = CString::new(query.generic).ok()?;
        let lang = query.lang.and_then(|lang| CString::new(lang).ok());
        // SAFETY: 作った問い合わせと結果の集合は、この関数の中で破棄する。fontconfigが返す
        // 文字列は集合が生きている間だけ読み、所有する値へ写す。
        unsafe {
            let pattern = (self.lib.FcPatternCreate)();
            if pattern.is_null() {
                return None;
            }
            if let Some(family) = &family {
                (self.lib.FcPatternAddString)(
                    pattern,
                    fc::constants::FC_FAMILY.as_ptr(),
                    family.as_ptr().cast(),
                );
            }
            (self.lib.FcPatternAddString)(
                pattern,
                fc::constants::FC_FAMILY.as_ptr(),
                generic.as_ptr().cast(),
            );
            if let Some(lang) = &lang {
                (self.lib.FcPatternAddString)(
                    pattern,
                    fc::constants::FC_LANG.as_ptr(),
                    lang.as_ptr().cast(),
                );
            }
            (self.lib.FcPatternAddInteger)(
                pattern,
                fc::constants::FC_WEIGHT.as_ptr(),
                query.weight,
            );
            (self.lib.FcPatternAddInteger)(
                pattern,
                fc::constants::FC_SLANT.as_ptr(),
                if query.italic {
                    fc::constants::FC_SLANT_ITALIC
                } else {
                    fc::constants::FC_SLANT_ROMAN
                },
            );
            (self.lib.FcConfigSubstitute)(self.config, pattern, fc::FcMatchPattern);
            (self.lib.FcDefaultSubstitute)(pattern);
            let mut result = 0;
            let set =
                (self.lib.FcFontSort)(self.config, pattern, 0, std::ptr::null_mut(), &mut result);
            (self.lib.FcPatternDestroy)(pattern);
            if set.is_null() {
                return None;
            }
            let fonts = std::slice::from_raw_parts((*set).fonts, (*set).nfont.max(0) as usize);
            let mut candidates: Vec<Found> = fonts
                .iter()
                .filter_map(|&font| self.accept(font, lang.as_deref()))
                .collect();
            (self.lib.FcFontSetDestroy)(set);
            // 可変フォントの既定のインスタンス（番号0）は可変と報告されない。同じファイルに
            // 可変の項目があれば、そのファイルの項目をすべて可変として扱う。
            let variable: std::collections::HashSet<PathBuf> = candidates
                .iter()
                .filter(|found| found.variable)
                .map(|found| found.path.clone())
                .collect();
            for found in &mut candidates {
                found.variable |= variable.contains(&found.path);
            }
            // 権限を絞った後は、フォントのディレクトリー外を指すシンボリックリンク等を読めない。
            candidates.retain(|found| std::fs::File::open(&found.path).is_ok());
            choose(candidates)
        }
    }

    /// fontがlangに対応したスケーラブルなフォントなら、そのファイルと書体。
    unsafe fn accept(&self, font: *mut fc::FcPattern, lang: Option<&CStr>) -> Option<Found> {
        // SAFETY: fontはFcFontSortの集合の要素で、呼び出し元が集合を生かしている。
        unsafe {
            let mut scalable = 0;
            if (self.lib.FcPatternGetBool)(
                font,
                fc::constants::FC_SCALABLE.as_ptr(),
                0,
                &mut scalable,
            ) != fc::FcResultMatch
                || scalable == 0
            {
                return None;
            }
            if let Some(lang) = lang {
                let mut languages = std::ptr::null_mut();
                if (self.lib.FcPatternGetLangSet)(
                    font,
                    fc::constants::FC_LANG.as_ptr(),
                    0,
                    &mut languages,
                ) != fc::FcResultMatch
                    || (self.lib.FcLangSetHasLang)(languages, lang.as_ptr().cast())
                        == fc::FcLangDifferentLang
                {
                    return None;
                }
            }
            let mut file = std::ptr::null_mut();
            if (self.lib.FcPatternGetString)(font, fc::constants::FC_FILE.as_ptr(), 0, &mut file)
                != fc::FcResultMatch
            {
                return None;
            }
            let mut index = 0;
            (self.lib.FcPatternGetInteger)(font, fc::constants::FC_INDEX.as_ptr(), 0, &mut index);
            let mut family = std::ptr::null_mut();
            let family = if (self.lib.FcPatternGetString)(
                font,
                fc::constants::FC_FAMILY.as_ptr(),
                0,
                &mut family,
            ) == fc::FcResultMatch
            {
                CStr::from_ptr(family.cast()).to_owned()
            } else {
                CString::default()
            };
            use std::os::unix::ffi::OsStrExt;
            let path = PathBuf::from(std::ffi::OsStr::from_bytes(
                CStr::from_ptr(file.cast()).to_bytes(),
            ));
            let mut variable = 0;
            (self.lib.FcPatternGetBool)(
                font,
                fc::constants::FC_VARIABLE.as_ptr(),
                0,
                &mut variable,
            );
            let index = index.max(0) as u32;
            Some(Found {
                path,
                // FC_INDEXの上位16ビットは可変フォントの名前付きインスタンス。書体の番号は下位。
                index: index & 0xffff,
                family,
                variable: variable != 0 || index >> 16 != 0,
            })
        }
    }

    /// 設定にフォントがある。
    fn has_fonts(&self) -> bool {
        // SAFETY: 設定が持つ集合を読むだけ（所有権は設定にある）。
        unsafe {
            let set = (self.lib.FcConfigGetFonts)(self.config, fc::FcSetSystem);
            !set.is_null() && (*set).nfont > 0
        }
    }

    /// fontconfigが探すフォントのディレクトリー。
    fn directories(&self) -> Vec<PathBuf> {
        let mut directories = Vec::new();
        // SAFETY: 一覧はこの関数の中で読み終えて破棄する。
        unsafe {
            let list = (self.lib.FcConfigGetFontDirs)(self.config);
            if list.is_null() {
                return directories;
            }
            loop {
                let next = (self.lib.FcStrListNext)(list);
                if next.is_null() {
                    break;
                }
                use std::os::unix::ffi::OsStrExt;
                directories.push(PathBuf::from(std::ffi::OsStr::from_bytes(
                    CStr::from_ptr(next.cast()).to_bytes(),
                )));
            }
            (self.lib.FcStrListDone)(list);
        }
        directories
    }
}

/// PDFiumへ渡した書体。ハンドルは一覧の位置＋1。
struct Font {
    path: PathBuf,
    index: u32,
    family: CString,
    charset: c_int,
    /// ファイル全体と書体の表ディレクトリーの位置。最初に求められたときに読む。
    data: Option<(std::sync::Arc<[u8]>, usize)>,
}

struct State {
    fontconfig: Fontconfig,
    fonts: Vec<Font>,
    /// 問い合わせごとの結果（FcFontSortは全フォントを並べ替えるため繰り返さない）。
    resolved: std::collections::HashMap<Query, Option<Found>>,
    /// 読み込んだフォントファイル。同じファイルの書体・文字セットで共有する。
    files: std::collections::HashMap<PathBuf, std::sync::Arc<[u8]>>,
}

/// PDFiumへ渡す `FPDF_SYSFONTINFO`。先頭の構造体のポインターを自分へ戻して使う。
#[repr(C)]
struct Info {
    base: FPDF_SYSFONTINFO,
    state: RefCell<State>,
}

/// コールバックの `pThis` から状態を得る。
///
/// # Safety
/// `this` は `install` が作った `Info` の先頭でなければならない。
unsafe fn state<'a>(this: *mut FPDF_SYSFONTINFO) -> &'a RefCell<State> {
    // SAFETY: Infoはrepr(C)で、baseが先頭にある。
    unsafe { &(*this.cast::<Info>()).state }
}

fn handle(index: usize) -> *mut c_void {
    (index + 1) as *mut c_void
}

unsafe extern "C" fn map_font(
    this: *mut FPDF_SYSFONTINFO,
    weight: c_int,
    italic: FPDF_BOOL,
    charset: c_int,
    pitch: c_int,
    face: *const c_char,
    _exact: *mut FPDF_BOOL,
) -> *mut c_void {
    // SAFETY: PDFiumは文字列を呼び出しの間だけ有効にして渡す。
    let face = if face.is_null() {
        &[][..]
    } else {
        unsafe { CStr::from_ptr(face) }.to_bytes()
    };
    let Some(query) = query(weight, italic != 0, charset, pitch, face) else {
        return std::ptr::null_mut();
    };
    // SAFETY: thisはinstallが渡したInfo。PDFiumは補助プロセスの1本のスレッドから呼ぶ。
    let mut state = unsafe { state(this) }.borrow_mut();
    let found = match state.resolved.get(&query) {
        Some(found) => found.clone(),
        None => {
            let found = state.fontconfig.resolve(&query);
            state.resolved.insert(query.clone(), found.clone());
            found
        }
    };
    if std::env::var_os("NAGAME_PROFILE").is_some() {
        // PDFiumから呼ばれる関数の中ではパニックしない（標準エラーが閉じていても続ける）。
        use std::io::Write;
        let _ = writeln!(
            std::io::stderr(),
            "pdf font: {} weight {weight} charset {charset} pitch {pitch:#x} -> {:?} -> {:?}",
            String::from_utf8_lossy(face),
            query,
            found.as_ref().map(|f| (&f.path, f.index, &f.family)),
        );
    }
    let Some(found) = found else {
        return std::ptr::null_mut();
    };
    if let Some(index) = state.fonts.iter().position(|font| {
        font.path == found.path && font.index == found.index && font.charset == charset
    }) {
        return handle(index);
    }
    state.fonts.push(Font {
        path: found.path,
        index: found.index,
        family: found.family,
        charset,
        data: None,
    });
    handle(state.fonts.len() - 1)
}

unsafe extern "C" fn get_font(this: *mut FPDF_SYSFONTINFO, face: *const c_char) -> *mut c_void {
    // SAFETY: map_fontと同じ前提。
    unsafe { map_font(this, 400, 0, 1, 0, face, std::ptr::null_mut()) }
}

unsafe extern "C" fn get_font_data(
    this: *mut FPDF_SYSFONTINFO,
    font: *mut c_void,
    table_tag: c_uint,
    buffer: *mut c_uchar,
    size: c_ulong,
) -> c_ulong {
    // SAFETY: map_fontと同じ前提。
    let mut state = unsafe { state(this) }.borrow_mut();
    let state = &mut *state;
    let Some(font) = (font as usize)
        .checked_sub(1)
        .and_then(|index| state.fonts.get_mut(index))
    else {
        return 0;
    };
    if font.data.is_none() {
        let data = match state.files.get(&font.path) {
            Some(data) => data.clone(),
            None => {
                let Some(data) = std::fs::read(&font.path).ok() else {
                    return 0;
                };
                let data: std::sync::Arc<[u8]> = data.into();
                state.files.insert(font.path.clone(), data.clone());
                data
            }
        };
        let Some(offset) = face_offset(&data, font.index) else {
            return 0;
        };
        font.data = Some((data, offset));
    }
    let (data, offset) = font.data.as_ref().expect("loaded above");
    // WindowsのGDIのGetFontDataと同じ規約。表0は書体の位置からファイルの終わりまで、「ttcf」は
    // TTC全体。PDFiumはその長さの差からTTC内の書体の番号を求める（GetTTCIndex）。
    let bytes: &[u8] = match table_tag {
        0 => &data[*offset..],
        TAG_TTCF if data.starts_with(b"ttcf") => &data[..],
        TAG_TTCF => return 0,
        tag => match table(data, *offset, tag) {
            Some(bytes) => bytes,
            None => return 0,
        },
    };
    // 領域が小さくても先頭を書く（GDIと同じ）。PDFiumはTTCの先頭1 KiBだけを求めて
    // チェックサムを取り、フォントのキャッシュのキーにする。書かないとキーが不定になる。
    if !buffer.is_null() {
        let length = bytes.len().min(size as usize);
        // SAFETY: PDFiumはsizeバイトの領域を渡している。
        unsafe { buffer.copy_from_nonoverlapping(bytes.as_ptr(), length) };
    }
    bytes.len() as c_ulong
}

unsafe extern "C" fn get_face_name(
    this: *mut FPDF_SYSFONTINFO,
    font: *mut c_void,
    buffer: *mut c_char,
    size: c_ulong,
) -> c_ulong {
    // SAFETY: map_fontと同じ前提。
    let state = unsafe { state(this) }.borrow();
    let Some(font) = (font as usize)
        .checked_sub(1)
        .and_then(|i| state.fonts.get(i))
    else {
        return 0;
    };
    let name = font.family.as_bytes_with_nul();
    if !buffer.is_null() && size as usize >= name.len() {
        // SAFETY: PDFiumはsizeバイト以上の領域を渡している。
        unsafe { buffer.copy_from_nonoverlapping(name.as_ptr().cast(), name.len()) };
    }
    name.len() as c_ulong
}

unsafe extern "C" fn get_font_charset(this: *mut FPDF_SYSFONTINFO, font: *mut c_void) -> c_int {
    // SAFETY: map_fontと同じ前提。
    let state = unsafe { state(this) }.borrow();
    (font as usize)
        .checked_sub(1)
        .and_then(|i| state.fonts.get(i))
        .map_or(0, |font| font.charset)
}

unsafe extern "C" fn delete_font(_this: *mut FPDF_SYSFONTINFO, _font: *mut c_void) {}

unsafe extern "C" fn release(_this: *mut FPDF_SYSFONTINFO) {}

/// fontconfigを読み込めたら、PDFiumのフォント探索をfontconfigによるものへ置き換える。
/// 置き換えたら、fontconfigが探すフォントのディレクトリーを返す。
pub fn install(pdfium: &Pdfium) -> Option<Vec<PathBuf>> {
    let fontconfig = Fontconfig::load()?;
    // フォントが1つもない設定なら、PDFium既定の探索のほうがよい。
    if !fontconfig.has_fonts() {
        return None;
    }
    let directories = fontconfig.directories();
    let info = Box::leak(Box::new(Info {
        base: FPDF_SYSFONTINFO {
            version: 2,
            Release: Some(release),
            EnumFonts: None,
            MapFont: Some(map_font),
            GetFont: Some(get_font),
            GetFontData: Some(get_font_data),
            GetFaceName: Some(get_face_name),
            GetFontCharset: Some(get_font_charset),
            DeleteFont: Some(delete_font),
        },
        state: RefCell::new(State {
            fontconfig,
            fonts: Vec::new(),
            resolved: Default::default(),
            files: Default::default(),
        }),
    }));
    // SAFETY: infoは補助プロセスの終了まで解放しない。PDFiumは初期化済み。
    unsafe {
        pdfium
            .bindings()
            .FPDF_SetSystemFontInfo((info as *mut Info).cast::<FPDF_SYSFONTINFO>());
    }
    Some(directories)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_choose_language_style_and_weight_from_the_charset_and_name() {
        let japanese = query(400, false, CHARSET_SHIFTJIS, 0, b"MS-Mincho").unwrap();
        assert_eq!(japanese.lang, Some("ja"));
        assert_eq!(japanese.generic, "serif");
        assert_eq!(japanese.family.as_deref(), Some("MS Mincho"));
        // Shift_JISの「ＭＳ ゴシック」はUTF-8で読めないが、文字セットと語で判定できる。
        let gothic = query(
            -1,
            false,
            1,
            0,
            b"\x82\x6c\x82\x72 \x83\x53\x83\x56\x83\x62\x83\x4e",
        )
        .unwrap();
        assert_eq!(
            (gothic.lang, gothic.generic, gothic.family),
            (Some("ja"), "sans-serif", None)
        );
        assert_eq!(gothic.weight, fc::constants::FC_WEIGHT_REGULAR);
        let heisei = query(400, false, 0, 0, b"ABCDEF+HeiseiMin-W3").unwrap();
        assert_eq!((heisei.lang, heisei.generic), (Some("ja"), "serif"));
        let bold = query(400, false, 0, 0, b"HeiseiKakuGo-W5,Bold").unwrap();
        assert_eq!(
            (bold.generic, bold.weight),
            ("sans-serif", fc::constants::FC_WEIGHT_BOLD)
        );
        assert_eq!(
            query(400, false, 0, 0, b"SimSun").unwrap().lang,
            Some("zh-cn")
        );
        assert_eq!(
            query(400, false, CHARSET_HANGEUL, 0, b"Gulim")
                .unwrap()
                .lang,
            Some("ko")
        );
        let latin = query(400, true, 0, PITCH_FIXED, b"Courier-Oblique").unwrap();
        assert_eq!(
            (latin.lang, latin.generic, latin.italic),
            (None, "monospace", true)
        );
        assert!(query(400, false, CHARSET_SYMBOL, 0, b"Wingdings").is_none());
        // 記号フォントは、文字セットを変えた問い合わせ直しでも置き換えない。
        assert!(query(400, false, 0, 0, b"Wingdings-Regular").is_none());
        // 欧文は書体名で推定せず、PDFのフラグに従う（「SansSerif」をセリフ体にしない）。
        assert_eq!(
            query(400, false, 0, 0, b"SansSerif").unwrap().generic,
            "sans-serif"
        );
        assert_eq!(
            query(400, false, 0, 0, b"Century Gothic").unwrap().generic,
            "sans-serif"
        );
        assert_eq!(
            query(400, false, 0, FAMILY_ROMAN, b"TimesNewRoman")
                .unwrap()
                .generic,
            "serif"
        );
    }

    /// 表を1つずつ持つ2書体のTTC。
    fn collection() -> Vec<u8> {
        let mut data = b"ttcf".to_vec();
        data.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        data.extend_from_slice(&2u32.to_be_bytes());
        data.extend_from_slice(&20u32.to_be_bytes());
        data.extend_from_slice(&48u32.to_be_bytes());
        for (start, tag, body) in [(20usize, b"name", 76u32), (48, b"cmap", 80)] {
            assert_eq!(data.len(), start);
            data.extend_from_slice(&0x0001_0000u32.to_be_bytes());
            data.extend_from_slice(&1u16.to_be_bytes());
            data.extend_from_slice(&[0; 6]);
            data.extend_from_slice(tag);
            data.extend_from_slice(&0u32.to_be_bytes());
            data.extend_from_slice(&body.to_be_bytes());
            data.extend_from_slice(&4u32.to_be_bytes());
        }
        data.extend_from_slice(b"NAMECMAP");
        data
    }

    #[test]
    fn variable_fonts_yield_only_to_static_fonts_of_the_same_design() {
        let found = |family: &str, variable| Found {
            path: PathBuf::from(format!("/{family}")),
            index: 0,
            family: CString::new(family).unwrap(),
            variable,
        };
        let pick = |candidates| choose(candidates).map(|f| f.family);
        // 同じデザインの静的なフォントがあれば、可変フォントより優先する。
        assert_eq!(
            pick(vec![
                found("Noto Sans CJK JP", true),
                found("Droid Sans Fallback", false),
                found("Noto Sans JP", false),
            ]),
            Some(c"Noto Sans JP".into())
        );
        // 別のデザインには替えない。太さが細くなっても可変フォントを使う。
        assert_eq!(
            pick(vec![
                found("Noto Serif CJK JP", true),
                found("Noto Sans JP", false),
            ]),
            Some(c"Noto Serif CJK JP".into())
        );
        assert_eq!(
            pick(vec![found("IPAexMincho", false)]),
            Some(c"IPAexMincho".into())
        );
        assert!(pick(vec![]).is_none());
    }

    #[test]
    fn collections_expose_each_face_and_its_tables() {
        let data = collection();
        assert_eq!(face_offset(&data, 0), Some(20));
        assert_eq!(face_offset(&data, 1), Some(48));
        assert_eq!(face_offset(&data, 2), None);
        let name = u32::from_be_bytes(*b"name");
        let cmap = u32::from_be_bytes(*b"cmap");
        assert_eq!(table(&data, 20, name), Some(&b"NAME"[..]));
        assert_eq!(table(&data, 20, cmap), None);
        assert_eq!(table(&data, 48, cmap), Some(&b"CMAP"[..]));
        assert_eq!(face_offset(b"\x00\x01\x00\x00", 0), Some(0));
        assert_eq!(face_offset(b"\x00\x01\x00\x00", 1), None);
        assert_eq!(face_offset(b"tt", 0), None);
    }
}
