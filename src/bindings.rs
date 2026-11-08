//! 設定で割り当て可能な操作と初期キー。Commandへの変換はViewerが行う。

use crate::i18n::tr;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::LazyLock};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shortcut {
    Open,
    OpenFolder,
    Previous,
    Next,
    Advance,
    First,
    Last,
    PreviousFolder,
    NextFolder,
    PreviousSibling,
    NextSibling,
    Parent,
    SkipPrevious,
    SkipNext,
    ZoomIn,
    ZoomOut,
    Fit,
    Actual,
    Rotate,
    Fullscreen,
    Slide,
    Manga,
    Settings,
    Info,
    Reload,
    SaveAs,
    CopyImage,
    CopyPath,
}
impl Shortcut {
    pub const ALL: &'static [Self] = &[
        Self::Open,
        Self::OpenFolder,
        Self::Previous,
        Self::Next,
        Self::Advance,
        Self::First,
        Self::Last,
        Self::PreviousFolder,
        Self::NextFolder,
        Self::PreviousSibling,
        Self::NextSibling,
        Self::Parent,
        Self::SkipPrevious,
        Self::SkipNext,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::Fit,
        Self::Actual,
        Self::Rotate,
        Self::Fullscreen,
        Self::Slide,
        Self::Manga,
        Self::Settings,
        Self::Info,
        Self::Reload,
        Self::SaveAs,
        Self::CopyImage,
        Self::CopyPath,
    ];
    pub fn label(self) -> String {
        match self {
            Self::Open => tr!("open-image"),
            Self::OpenFolder => tr!("open-folder"),
            Self::Previous => tr!("shortcut-previous"),
            Self::Next => tr!("shortcut-next"),
            Self::Advance => tr!("shortcut-advance"),
            Self::First => tr!("first-image"),
            Self::Last => tr!("last-image"),
            Self::PreviousFolder => tr!("previous-folder"),
            Self::NextFolder => tr!("next-folder"),
            Self::PreviousSibling => tr!("previous-sibling"),
            Self::NextSibling => tr!("next-sibling"),
            Self::Parent => tr!("parent-folder"),
            Self::SkipPrevious => tr!("skip-previous"),
            Self::SkipNext => tr!("skip-next"),
            Self::ZoomIn => tr!("zoom-in"),
            Self::ZoomOut => tr!("zoom-out"),
            Self::Fit => tr!("fit-window"),
            Self::Actual => tr!("actual-size"),
            Self::Rotate => tr!("rotate-right"),
            Self::Fullscreen => tr!("fullscreen"),
            Self::Slide => tr!("slideshow"),
            Self::Manga => tr!("manga"),
            Self::Settings => tr!("settings"),
            Self::Info => tr!("image-info"),
            Self::Reload => tr!("reload"),
            Self::SaveAs => tr!("save-as"),
            Self::CopyImage => tr!("copy-image"),
            Self::CopyPath => tr!("copy-path"),
        }
    }
}
/// 初期キー。同じ操作に複数のキーがある場合は、メニューに表示するキーを先に置く。
pub const DEFAULTS: &[(&str, Shortcut)] = {
    use Shortcut::*;
    &[
        ("ctrl-o", Open),
        ("cmd-o", Open),
        ("ctrl-shift-o", OpenFolder),
        ("left", Previous),
        ("right", Next),
        ("space", Advance),
        ("ctrl-home", First),
        ("ctrl-end", Last),
        ("up", PreviousFolder),
        ("down", NextFolder),
        ("ctrl-left", PreviousSibling),
        ("ctrl-right", NextSibling),
        ("alt-up", Parent),
        ("backspace", Parent),
        ("[", SkipPrevious),
        ("]", SkipNext),
        ("+", ZoomIn),
        ("=", ZoomIn),
        ("shift-+", ZoomIn),
        ("shift-=", ZoomIn),
        ("-", ZoomOut),
        ("f", Fit),
        ("0", Fit),
        ("1", Actual),
        ("r", Rotate),
        ("f11", Fullscreen),
        ("s", Slide),
        ("a", Slide),
        ("m", Manga),
        ("ctrl-,", Settings),
        ("cmd-,", Settings),
        ("i", Info),
        ("f5", Reload),
        ("ctrl-s", SaveAs),
        ("cmd-s", SaveAs),
        ("ctrl-c", CopyImage),
        ("cmd-c", CopyImage),
        ("ctrl-shift-c", CopyPath),
    ]
};
/// `DEFAULTS` のキーを、設定に保存する形（`Keystroke::unparse`。Linuxでは `cmd` が `super`）へ
/// そろえたもの。並びは `DEFAULTS` と同じ。
pub static DEFAULT_KEYS: LazyLock<Vec<(String, Shortcut)>> = LazyLock::new(|| {
    DEFAULTS
        .iter()
        .map(|(key, action)| (gpui::Keystroke::parse(key).unwrap().unparse(), *action))
        .collect()
});
pub fn defaults() -> BTreeMap<String, Shortcut> {
    DEFAULT_KEYS.iter().cloned().collect()
}
