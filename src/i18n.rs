//! Fluentのメッセージ。デコーダーのベンチマークを含む全ビルドに埋め込む。
//! 言語は保存した手動指定を優先し、なければOSの主言語（日本語なら日本語、それ以外は英語）に従う。

use i18n_embed::{LanguageLoader, fluent::FluentLanguageLoader};
use rust_embed::RustEmbed;
use std::sync::LazyLock;

/// 表示言語の設定値。Systemは `resolved` でOSの主言語に置き換える。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Language {
    #[default]
    #[serde(rename = "system")]
    System,
    #[serde(rename = "ja")]
    Japanese,
    #[serde(rename = "en")]
    English,
}

impl Language {
    pub fn label(self) -> String {
        match self {
            Self::System => tr!("language-system"),
            Self::Japanese => tr!("language-ja"),
            Self::English => tr!("language-en"),
        }
    }

    fn code(self) -> &'static str {
        match self.resolved() {
            Self::Japanese => "ja",
            Self::English => "en",
            Self::System => unreachable!("system language must be resolved"),
        }
    }

    pub fn resolved(self) -> Self {
        match self {
            Self::System => from_primary_language(
                i18n_embed::DesktopLanguageRequester::requested_languages().first(),
            ),
            language => language,
        }
    }
}

fn from_primary_language(
    language: Option<&i18n_embed::unic_langid::LanguageIdentifier>,
) -> Language {
    if language.is_some_and(|language| language.language.as_str() == "ja") {
        Language::Japanese
    } else {
        Language::English
    }
}

#[derive(RustEmbed)]
#[folder = "locales/"]
struct Localizations;

static LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    let loader = FluentLanguageLoader::new("nagame", "en".parse().expect("valid English locale"));
    loader
        .load_languages(
            &Localizations,
            &[Language::System.code().parse().expect("valid locale")],
        )
        .expect("embedded messages must be available");
    loader
});

pub(crate) fn loader() -> &'static FluentLanguageLoader {
    &LOADER
}

/// 保存済みの言語を適用する。設定画面で編集中の値で呼ばない（保存前に表示言語が変わってしまう）。
pub fn activate(language: Language) {
    loader()
        .load_languages(
            &Localizations,
            &[language.code().parse().expect("valid built-in locale")],
        )
        .expect("embedded locale must be available");
}

// キーと名前付き引数をリテラルのまま渡し、fl!のコンパイル時検証の対象にする。
macro_rules! tr {
    ($id:literal $(, $($args:tt)*)?) => {
        i18n_embed_fl::fl!($crate::i18n::loader(), $id $(, $($args)*)?)
    };
}
pub(crate) use tr;

/// 情報画面に出す元ファイルの情報と表示寸法。数値は翻訳文と分けて渡す。
pub struct ImageInformation<'a> {
    pub path: &'a std::path::Path,
    pub format: &'a str,
    pub bytes: u64,
    pub original: [u32; 2],
    pub displayed: [f32; 2],
    /// Noneはアニメーションの1周目をデコード中で、まだ数が分からない。
    pub frames: Option<usize>,
    pub pages: usize,
}

/// 情報画面に出す、PDFのページの情報。
pub struct PdfInformation<'a> {
    pub path: &'a std::path::Path,
    pub bytes: u64,
    /// 1から数えたページ番号と総ページ数。
    pub page: usize,
    pub total: usize,
    /// ページの論理寸法（pt）。
    pub size: [f32; 2],
    pub displayed: [f32; 2],
    /// 基準解像度。
    pub dpi: f32,
    pub vector: bool,
    pub pages: usize,
}

pub fn pdf_information(info: PdfInformation<'_>) -> String {
    let millimeters = |points: f32| format!("{:.1}", points / 72. * 25.4);
    tr!(
        "pdf-information",
        path = info.path.display().to_string(),
        file_size = file_size(info.bytes),
        page = grouped(info.page as u64),
        total = grouped(info.total as u64),
        page_width = millimeters(info.size[0]),
        page_height = millimeters(info.size[1]),
        width = pixels(info.displayed[0]),
        height = pixels(info.displayed[1]),
        dpi = format!("{:.0}", info.dpi),
        kind = if info.vector { "vector" } else { "image" },
        pages = info.pages
    )
}

/// 情報画面の整数。5桁以上は3桁ごとにカンマで区切る（日本語・英語とも）。4桁以下は区切らず、
/// 1920 × 1080 のような寸法を見慣れた形に保つ。
fn grouped(value: u64) -> String {
    let digits = value.to_string();
    if digits.len() < 5 {
        return digits;
    }
    let mut text = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

/// 表示寸法（画素数）。浮動小数の寸法を整数へ丸めて区切る。
fn pixels(value: f32) -> String {
    grouped(value.round().max(0.) as u64)
}

/// 単位付きの概数と、区切り付きの正確なバイト数。1 KB未満はバイト数だけ。
fn file_size(bytes: u64) -> String {
    if bytes < 1000 {
        tr!(
            "info-file-size-bytes",
            count = bytes,
            bytes = grouped(bytes)
        )
    } else {
        tr!(
            "info-file-size",
            size = approximate_size(bytes),
            bytes = grouped(bytes)
        )
    }
}

/// 1 KB以上の大きさの概数。単位は1000倍ごと（1 MB = 1,000,000バイト。macOSやGNOMEの
/// ファイル表示と同じ）で、100未満は小数1桁にする。浮動小数の誤差で丸めの向きが変わらない
/// よう、整数で四捨五入する。EBまであればu64のすべての値を1000未満で表せる。
fn approximate_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["KB", "MB", "GB", "TB", "PB", "EB"];
    let bytes = u128::from(bytes);
    let rounded = |divisor: u128, scale: u128| (bytes * scale + divisor / 2) / divisor;
    let mut unit = 0;
    let mut divisor = 1000;
    // 四捨五入すると1000になる値も次の単位で表す（1000 KBではなく1.0 MB）。
    while rounded(divisor, 1) >= 1000 && unit + 1 < UNITS.len() {
        divisor *= 1000;
        unit += 1;
    }
    let tenths = rounded(divisor, 10);
    if tenths < 1000 {
        format!("{}.{} {}", tenths / 10, tenths % 10, UNITS[unit])
    } else {
        format!("{} {}", rounded(divisor, 1), UNITS[unit])
    }
}

/// ステータスバーに出す、表示中の画像の状態。
pub struct ImageStatus<'a> {
    pub page_range: &'a str,
    pub total: usize,
    pub dimensions: [f32; 2],
    pub percent: f32,
    pub fit: bool,
    pub filename: &'a str,
    pub slideshow: bool,
}

pub fn settings_read_error(error: &str) -> String {
    tr!("settings-read-error", error = error)
}

pub fn settings_save_error(error: &str) -> String {
    tr!("settings-save-error", error = error)
}

pub fn error_status(error: &str) -> String {
    tr!("error-status", error = error)
}

pub fn settings_error_status(error: &str) -> String {
    tr!("settings-error-status", error = error)
}

pub fn resampling_error(error: &str) -> String {
    tr!("resampling-error", error = error)
}

pub fn window_open_error(error: &str) -> String {
    tr!("window-open-error", error = error)
}

pub fn clipboard_write_error(error: &str) -> String {
    tr!("clipboard-write-error", error = error)
}

pub fn clipboard_copy_error(error: &str) -> String {
    tr!("clipboard-copy-error", error = error)
}

pub fn current_color(color: u32) -> String {
    tr!("current-color", color = format!("{color:06X}"))
}

pub fn settings_row_count(count: usize) -> String {
    tr!("settings-row-count", count = count)
}

pub fn clipboard_tool_error(tool: &str, error: &str) -> String {
    tr!("clipboard-tool-error", tool = tool, error = error)
}

pub fn saved_image(path: &std::path::Path) -> String {
    tr!("saved-image", path = path.display().to_string())
}

pub fn image_information(info: ImageInformation<'_>) -> String {
    tr!(
        "image-information",
        path = info.path.display().to_string(),
        format = info.format,
        file_size = file_size(info.bytes),
        original_width = grouped(u64::from(info.original[0])),
        original_height = grouped(u64::from(info.original[1])),
        width = pixels(info.displayed[0]),
        height = pixels(info.displayed[1]),
        frames = info
            .frames
            .map_or_else(|| "…".to_owned(), |n| grouped(n as u64)),
        pages = info.pages
    )
}

pub fn image_status(status: ImageStatus<'_>) -> String {
    let mode = match (status.fit, status.slideshow) {
        (false, false) => "normal",
        (true, false) => "fit",
        (false, true) => "slideshow",
        (true, true) => "fit-slideshow",
    };
    tr!(
        "image-status",
        mode = mode,
        page_range = status.page_range,
        total = status.total,
        width = status.dimensions[0],
        height = status.dimensions[1],
        percent = format!("{:.0}", status.percent),
        filename = status.filename
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use fluent_bundle::{FluentArgs, FluentBundle, FluentResource};

    fn without_isolates(text: &str) -> String {
        text.replace(['\u{2068}', '\u{2069}'], "")
    }

    /// メッセージ（値と属性）が参照する変数名。
    fn variables<'a>(
        message: &fluent_syntax::ast::Message<&'a str>,
    ) -> std::collections::BTreeSet<&'a str> {
        use fluent_syntax::ast::{
            CallArguments, Expression, InlineExpression, Pattern, PatternElement,
        };
        type Names<'a> = std::collections::BTreeSet<&'a str>;
        fn pattern<'a>(pattern: &Pattern<&'a str>, names: &mut Names<'a>) {
            for element in &pattern.elements {
                if let PatternElement::Placeable { expression } = element {
                    walk(expression, names);
                }
            }
        }
        fn walk<'a>(expression: &Expression<&'a str>, names: &mut Names<'a>) {
            match expression {
                Expression::Select { selector, variants } => {
                    inline(selector, names);
                    for variant in variants {
                        pattern(&variant.value, names);
                    }
                }
                Expression::Inline(expression) => inline(expression, names),
            }
        }
        fn inline<'a>(expression: &InlineExpression<&'a str>, names: &mut Names<'a>) {
            match expression {
                InlineExpression::VariableReference { id } => {
                    names.insert(id.name);
                }
                InlineExpression::Placeable { expression } => walk(expression, names),
                InlineExpression::FunctionReference { arguments, .. }
                | InlineExpression::TermReference {
                    arguments: Some(arguments),
                    ..
                } => call(arguments, names),
                _ => {}
            }
        }
        fn call<'a>(arguments: &CallArguments<&'a str>, names: &mut Names<'a>) {
            for argument in &arguments.positional {
                inline(argument, names);
            }
            for argument in &arguments.named {
                inline(&argument.value, names);
            }
        }
        let mut names = Names::new();
        if let Some(value) = &message.value {
            pattern(value, &mut names);
        }
        for attribute in &message.attributes {
            pattern(&attribute.value, &mut names);
        }
        names
    }

    #[test]
    fn catalogs_have_matching_keys_and_format_every_message() {
        let mut baseline_ids = None;
        let mut catalog_variables = std::collections::BTreeMap::new();
        for (language, source) in [
            ("ja", include_str!("../locales/ja/nagame.ftl")),
            ("en", include_str!("../locales/en/nagame.ftl")),
        ] {
            let syntax = fluent_syntax::parser::parse(source).expect("valid Fluent syntax");
            let resource =
                FluentResource::try_new(source.to_owned()).expect("valid Fluent resource");
            let ids: std::collections::BTreeSet<_> = syntax
                .body
                .iter()
                .filter_map(|entry| {
                    if let fluent_syntax::ast::Entry::Message(message) = entry {
                        Some(message.id.name)
                    } else {
                        None
                    }
                })
                .collect();
            if let Some(expected) = &baseline_ids {
                assert_eq!(expected, &ids);
            } else {
                baseline_ids = Some(ids);
            }
            let names: std::collections::BTreeMap<_, _> = syntax
                .body
                .iter()
                .filter_map(|entry| match entry {
                    fluent_syntax::ast::Entry::Message(message) => {
                        Some((message.id.name, variables(message)))
                    }
                    _ => None,
                })
                .collect();
            catalog_variables.insert(language, names);
            let mut bundle = FluentBundle::new(vec![language.parse().unwrap()]);
            bundle.add_resource(resource).expect("unique message IDs");
            let mut args = FluentArgs::new();
            for key in [
                "count",
                "total",
                "bytes",
                "frames",
                "pages",
                "original_width",
                "original_height",
                "width",
                "height",
                "page",
            ] {
                args.set(key, 2);
            }
            for key in [
                "color",
                "error",
                "tool",
                "path",
                "format",
                "page_range",
                "percent",
                "filename",
                "limit",
                "page_width",
                "page_height",
                "dpi",
                "size",
                "file_size",
                "key",
            ] {
                args.set(key, "test {literal}");
            }
            args.set("mode", "normal");
            args.set("kind", "image");
            for entry in syntax.body {
                if let fluent_syntax::ast::Entry::Message(message) = entry {
                    let id = message.id.name;
                    let pattern = bundle.get_message(id).unwrap().value().unwrap();
                    let mut errors = Vec::new();
                    let rendered = bundle.format_pattern(pattern, Some(&args), &mut errors);
                    assert!(errors.is_empty(), "{id}: {errors:?}");
                    assert!(!rendered.is_empty(), "{id}");
                }
            }

            // fl!は名前付き引数を検証するが、引数をすべて省略した呼び出しは通してしまう。
            // その穴を埋めるため、列挙したファイルの引数なしの呼び出しを実際のリソースで確認する。
            for source in [
                include_str!("main.rs"),
                include_str!("animation.rs"),
                include_str!("bindings.rs"),
                include_str!("decoder.rs"),
                include_str!("menu_ui.rs"),
                include_str!("model.rs"),
                include_str!("pdf/mod.rs"),
                include_str!("pipeline.rs"),
                include_str!("resample.rs"),
                include_str!("settings.rs"),
                include_str!("settings_ui.rs"),
                include_str!("ui_smoke.rs"),
                include_str!("viewer_io.rs"),
                include_str!("window_state.rs"),
            ] {
                for call in source.split("tr!(").skip(1) {
                    let Some(call) = call.trim_start().strip_prefix('"') else {
                        continue;
                    };
                    let Some((id, rest)) = call.split_once('"') else {
                        continue;
                    };
                    if rest.trim_start().starts_with(')') {
                        let pattern = bundle.get_message(id).unwrap().value().unwrap();
                        let mut errors = Vec::new();
                        bundle.format_pattern(pattern, None, &mut errors);
                        assert!(errors.is_empty(), "no-argument call {id}: {errors:?}");
                    }
                }
            }
        }
        // fl!がコンパイル時に照合するのは英語の文言の引数だけ。日本語に古い変数名が残ると、
        // 実行時に `{$name}` がそのまま表示される。日本語は英語の変数の一部だけを使ってよい
        // （英語の複数形の選択にだけ使う変数があるため）。
        let english = &catalog_variables["en"];
        for (id, names) in &catalog_variables["ja"] {
            assert!(
                names.is_subset(&english[id]),
                "{id}: ja uses {names:?}, en passes {:?}",
                english[id]
            );
        }
    }

    #[test]
    fn plurals_fallback_and_worker_threads_use_embedded_locales() {
        let english = FluentLanguageLoader::new("nagame", "en".parse().unwrap());
        english
            .load_languages(&Localizations, &["en".parse().unwrap()])
            .unwrap();
        assert!(i18n_embed_fl::fl!(english, "settings-row-count", count = 1).contains("One item"));
        assert!(
            without_isolates(&i18n_embed_fl::fl!(
                english,
                "settings-row-count",
                count = 2
            ))
            .contains("2 items")
        );
        let byte = without_isolates(&i18n_embed_fl::fl!(
            english,
            "info-file-size-bytes",
            count = 1,
            bytes = "1"
        ));
        assert_eq!(byte, "1 byte");
        let unsupported: i18n_embed::unic_langid::LanguageIdentifier = "fr".parse().unwrap();
        let fallback = english.select_languages(&[unsupported]);
        assert_eq!(fallback.get("open-image"), "Open image");
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    assert!(
                        i18n_embed_fl::fl!(english, "error-status", error = "worker")
                            .contains("worker")
                    )
                });
            }
        });
    }

    #[test]
    fn information_numbers_are_grouped_and_sizes_have_units() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(9999), "9999");
        assert_eq!(grouped(10_000), "10,000");
        assert_eq!(grouped(12_345_678), "12,345,678");
        assert_eq!(grouped(u64::MAX), "18,446,744,073,709,551,615");
        assert_eq!(pixels(1919.6), "1920");
        assert_eq!(pixels(16384.), "16,384");
        assert_eq!(approximate_size(1000), "1.0 KB");
        assert_eq!(approximate_size(1500), "1.5 KB");
        // ちょうど半分は切り上げる（浮動小数では2.0 KBになっていた）。
        assert_eq!(approximate_size(1050), "1.1 KB");
        assert_eq!(approximate_size(2050), "2.1 KB");
        assert_eq!(approximate_size(98_765), "98.8 KB");
        assert_eq!(approximate_size(99_949), "99.9 KB");
        // 小数1桁で100.0になる値から、整数で表す。
        assert_eq!(approximate_size(99_950), "100 KB");
        assert_eq!(approximate_size(100_000), "100 KB");
        assert_eq!(approximate_size(999_499), "999 KB");
        // 四捨五入すると1000 KBになる大きさは、次の単位で表す。
        assert_eq!(approximate_size(999_500), "1.0 MB");
        assert_eq!(approximate_size(999_999), "1.0 MB");
        assert_eq!(approximate_size(12_345_678), "12.3 MB");
        assert_eq!(approximate_size(5_000_000_000), "5.0 GB");
        assert_eq!(approximate_size(999_999_999_999_999), "1.0 PB");
        assert_eq!(approximate_size(u64::MAX), "18.4 EB");
        let large = without_isolates(&file_size(12_345_678));
        assert!(
            large.contains("12.3 MB") && large.contains("12,345,678"),
            "{large}"
        );
        let small = without_isolates(&file_size(999));
        assert!(small.contains("999") && !small.contains("KB"), "{small}");
        let kilobyte = without_isolates(&file_size(1000));
        assert!(
            kilobyte.contains("1.0 KB") && kilobyte.contains("1000"),
            "{kilobyte}"
        );
    }

    #[test]
    fn system_language_selects_japanese_only_for_japanese_and_honors_overrides() {
        for (tag, expected) in [
            ("ja", Language::Japanese),
            ("ja-JP", Language::Japanese),
            ("en-US", Language::English),
            ("fr", Language::English),
            ("zh-Hans", Language::English),
        ] {
            assert_eq!(from_primary_language(Some(&tag.parse().unwrap())), expected);
        }
        assert_eq!(from_primary_language(None), Language::English);
        assert_eq!(Language::Japanese.resolved(), Language::Japanese);
        assert_eq!(Language::English.resolved(), Language::English);
    }

    #[test]
    fn formatted_messages_preserve_user_text_without_interpreting_it() {
        let path = std::path::Path::new("/images/画像 {error} 🖼.png");
        let message = without_isolates(&image_information(ImageInformation {
            path,
            format: "PNG",
            bytes: 1024,
            original: [400, 600],
            displayed: [800., 600.],
            frames: Some(1),
            pages: 2,
        }));
        assert!(message.contains(&path.display().to_string()));
        assert!(message.contains("400 × 600"));
        assert!(message.contains("800 × 600"));
        assert!(message.contains("1.0 KB") && message.contains("1024"));
        let detail = "a {filename} cannot be opened";
        assert!(without_isolates(&error_status(detail)).ends_with(detail));
    }
}
