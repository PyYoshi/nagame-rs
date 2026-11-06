//! Fluent-backed messages embedded in every build, including the decoder benchmark.
//! Follow the primary OS language (Japanese, otherwise English) unless overridden.

use i18n_embed::{LanguageLoader, fluent::FluentLanguageLoader};
use rust_embed::RustEmbed;
use std::sync::LazyLock;

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

/// Apply only a saved locale; editing the settings draft must not call this.
pub fn activate(language: Language) {
    loader()
        .load_languages(
            &Localizations,
            &[language.code().parse().expect("valid built-in locale")],
        )
        .expect("embedded locale must be available");
}

// Keep literal keys and named arguments visible to the compile-time validator.
macro_rules! tr {
    ($id:literal $(, $($args:tt)*)?) => {
        i18n_embed_fl::fl!($crate::i18n::loader(), $id $(, $($args)*)?)
    };
}
pub(crate) use tr;

/// Source metadata and display geometry, kept separate from translated prose.
pub struct ImageInformation<'a> {
    pub path: &'a std::path::Path,
    pub format: &'a str,
    pub bytes: u64,
    pub original: [u32; 2],
    pub displayed: [f32; 2],
    pub frames: usize,
    pub pages: usize,
}

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
        bytes = info.bytes,
        original_width = info.original[0],
        original_height = info.original[1],
        width = info.displayed[0],
        height = info.displayed[1],
        frames = info.frames,
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

    #[test]
    fn catalogs_have_matching_keys_and_format_every_message() {
        let mut baseline_ids = None;
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
            ] {
                args.set(key, "test {literal}");
            }
            args.set("mode", "normal");
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

            // fl! validates named arguments, but accepts omitting all arguments.
            // Check every no-argument call against the actual resource to cover that gap.
            for source in [
                include_str!("main.rs"),
                include_str!("bindings.rs"),
                include_str!("decoder.rs"),
                include_str!("menu_ui.rs"),
                include_str!("model.rs"),
                include_str!("pipeline.rs"),
                include_str!("resample.rs"),
                include_str!("settings.rs"),
                include_str!("settings_ui.rs"),
                include_str!("viewer_io.rs"),
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
            frames: 1,
            pages: 2,
        }));
        assert!(message.contains(&path.display().to_string()));
        assert!(message.contains("400 × 600"));
        assert!(message.contains("800 × 600"));
        let detail = "a {filename} cannot be opened";
        assert!(without_isolates(&error_status(detail)).ends_with(detail));
    }
}
