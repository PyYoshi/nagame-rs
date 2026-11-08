//! メニューバー・右クリックメニューの描画とキー操作。実行はCommandへ集約する。

use crate::bindings::{DEFAULT_KEYS, Shortcut};
use crate::i18n::tr;
use crate::{Command, Viewer, settings::SortOrder};
use gpui::{prelude::*, *};
pub const HEIGHT: f32 = 28.;
/// メニューバーの見出し1つの幅。見出しの位置からメニューを開く位置を決める。
const TITLE_WIDTH: f32 = 80.;
/// メニューバーの左右の余白（8px）と見出しの間隔（4px）。
const BAR_PADDING: f32 = 8.;
const TITLE_GAP: f32 = 4.;
/// メニューの幅。
const WIDTH: f32 = 340.;
/// メニューを画面の端から離す距離。収まらない場合は、この余白を残して画面内へずらす。
const SCREEN_MARGIN: f32 = 4.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    File,
    Navigate,
    Sort,
    View,
    /// 画像の上の右クリックメニュー。メニューバーには出さず、`Command::Menu` でも開かない
    /// （`Viewer::open_context_menu` がクリック位置に開く）。
    Context,
}
impl Menu {
    /// メニューバーに並べる順。
    const BAR: [Self; 4] = [Self::File, Self::Navigate, Self::Sort, Self::View];
    /// メニューバーの見出し。右クリックメニューには見出しがない。
    fn label(self) -> String {
        match self {
            Self::File => tr!("menu-file"),
            Self::Navigate => tr!("menu-navigate"),
            Self::Sort => tr!("menu-sort"),
            Self::View => tr!("menu-view"),
            Self::Context => unreachable!("the context menu has no title"),
        }
    }
    /// メニューバーの見出しの下（メニューを開く左上の位置）。
    pub fn anchor(self) -> [f32; 2] {
        let index = Self::BAR
            .iter()
            .position(|m| *m == self)
            .expect("only menu bar menus open under a title");
        [
            BAR_PADDING + index as f32 * (TITLE_WIDTH + TITLE_GAP),
            HEIGHT,
        ]
    }
}

/// メニューの行の左に付ける印。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    None,
    /// オン・オフを切り替える項目。trueなら✓。
    Check(bool),
    /// 並び順のように1つだけを選ぶ項目。trueなら●。
    Radio(bool),
}

enum Entry {
    Item {
        label: String,
        command: Command,
        mark: Mark,
    },
    Separator,
}
impl Entry {
    fn item(label: String, command: Command) -> Self {
        Self::Item {
            label,
            command,
            mark: Mark::None,
        }
    }
    fn marked(label: String, command: Command, mark: Mark) -> Self {
        Self::Item {
            label,
            command,
            mark,
        }
    }
    fn command(&self) -> Option<Command> {
        match self {
            Self::Item { command, .. } => Some(*command),
            Self::Separator => None,
        }
    }
}

/// commandを実行するキー（candidatesは、その操作に割り当てたキーと操作）から、メニューに出す
/// 1つを選ぶ。
/// - 「次（Space）」は、読む方向にかかわらず進む別名なので、「次の画像」のキーより後にする。
///   利用者が「次の画像」のキーを変えても、そのキーを表示する。
/// - 既定の割り当て（キーと操作の組）は `bindings::DEFAULTS` の並びを優先する。
/// - 利用者が加えたキーは、修飾キーの少ない順、同じならキーの文字列順。
fn preferred_key<'a>(
    candidates: impl IntoIterator<Item = (&'a String, Shortcut)>,
) -> Option<&'a String> {
    candidates
        .into_iter()
        .min_by_key(|(key, action)| {
            let preference = DEFAULT_KEYS
                .iter()
                .position(|(default, default_action)| default == *key && default_action == action)
                .unwrap_or(usize::MAX);
            let modifiers = Keystroke::parse(key).map_or(0, |keystroke| {
                let m = keystroke.modifiers;
                [m.control, m.alt, m.shift, m.platform, m.function]
                    .into_iter()
                    .filter(|pressed| *pressed)
                    .count()
            });
            (*action == Shortcut::Advance, preference, modifiers, *key)
        })
        .map(|(key, _)| key)
}

/// 割り当てたキーの表示。`ctrl-shift-o` を `Ctrl+Shift+O`、矢印キーを記号にする。
fn key_label(keystroke: &str) -> String {
    let Ok(keystroke) = Keystroke::parse(keystroke) else {
        return keystroke.to_owned();
    };
    let mut parts = Vec::new();
    let modifiers = &keystroke.modifiers;
    for (pressed, name) in [
        (modifiers.control, "Ctrl"),
        (modifiers.alt, "Alt"),
        (modifiers.shift, "Shift"),
        (modifiers.platform, "Super"),
        (modifiers.function, "Fn"),
    ] {
        if pressed {
            parts.push(name.to_owned());
        }
    }
    let key = keystroke.key.as_str();
    parts.push(match key {
        "left" => "←".to_owned(),
        "right" => "→".to_owned(),
        "up" => "↑".to_owned(),
        "down" => "↓".to_owned(),
        "pageup" => "PageUp".to_owned(),
        "pagedown" => "PageDown".to_owned(),
        "escape" => "Esc".to_owned(),
        "-" => "−".to_owned(),
        _ => {
            let mut chars = key.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    });
    parts.join("+")
}

impl Viewer {
    /// 各メニューの項目。印は今の設定と状態から作る。
    fn menu_entries(&self, menu: Menu) -> Vec<Entry> {
        use Entry::Separator;
        let s = &self.settings;
        let sort = |label: String, order: SortOrder| {
            Entry::marked(label, Command::Sort(order), Mark::Radio(s.sort == order))
        };
        let manga = Entry::marked(tr!("manga"), Command::Manga, Mark::Check(s.manga));
        let slide = Entry::marked(tr!("slideshow"), Command::Slide, Mark::Check(self.slide));
        match menu {
            Menu::File => vec![
                Entry::item(tr!("open-image-menu"), Command::Open),
                Entry::item(tr!("open-folder-menu"), Command::OpenFolder),
                Entry::item(tr!("reload"), Command::Reload),
                Separator,
                Entry::item(tr!("save-as-menu"), Command::SaveAs),
                Entry::item(tr!("copy-image"), Command::CopyImage),
                Entry::item(tr!("copy-file-path"), Command::CopyPath),
                Entry::item(tr!("reveal-image-folder"), Command::Reveal),
                Separator,
                Entry::item(tr!("settings-menu"), Command::Settings),
            ],
            Menu::Navigate => vec![
                Entry::item(tr!("previous-image"), Command::Previous),
                Entry::item(tr!("next-image"), Command::Next),
                Entry::item(tr!("skip-previous"), Command::SkipPrevious),
                Entry::item(tr!("skip-next"), Command::SkipNext),
                Entry::item(tr!("first-image"), Command::First),
                Entry::item(tr!("last-image"), Command::Last),
                Separator,
                Entry::item(tr!("previous-folder"), Command::PreviousFolder),
                Entry::item(tr!("next-folder"), Command::NextFolder),
                Entry::item(tr!("previous-sibling"), Command::PreviousSibling),
                Entry::item(tr!("next-sibling"), Command::NextSibling),
                Entry::item(tr!("parent-folder"), Command::Parent),
            ],
            Menu::Sort => vec![
                sort(tr!("sort-name"), SortOrder::Name),
                sort(tr!("sort-modified"), SortOrder::Modified),
                sort(tr!("sort-size"), SortOrder::Size),
                sort(tr!("sort-extension"), SortOrder::Extension),
                sort(tr!("random"), SortOrder::Random),
                Separator,
                Entry::marked(
                    tr!("reverse-order"),
                    Command::ReverseSort,
                    Mark::Check(s.reverse),
                ),
            ],
            Menu::View => vec![
                Entry::item(tr!("zoom-in"), Command::ZoomIn),
                Entry::item(tr!("zoom-out"), Command::ZoomOut),
                Entry::item(tr!("fit-window"), Command::Fit),
                Entry::item(tr!("actual-size"), Command::Actual),
                Entry::item(tr!("rotate-right"), Command::Rotate),
                Separator,
                manga,
                Entry::marked(
                    tr!("right-to-left"),
                    Command::ReadingDirection,
                    Mark::Check(s.right_to_left),
                ),
                Separator,
                Entry::item(tr!("fullscreen"), Command::Fullscreen),
                slide,
                Separator,
                Entry::item(tr!("image-info"), Command::Info),
            ],
            // マウスでよく使う、画像に対する操作。フォルダ移動と並び順はメニューバーとキーで行う。
            Menu::Context => vec![
                Entry::item(tr!("previous-image"), Command::Previous),
                Entry::item(tr!("next-image"), Command::Next),
                Separator,
                Entry::item(tr!("fit-window"), Command::Fit),
                Entry::item(tr!("actual-size"), Command::Actual),
                Entry::item(tr!("rotate-right"), Command::Rotate),
                Separator,
                Entry::item(tr!("copy-image"), Command::CopyImage),
                Entry::item(tr!("copy-file-path"), Command::CopyPath),
                Entry::item(tr!("save-as-menu"), Command::SaveAs),
                Entry::item(tr!("reveal-image-folder"), Command::Reveal),
                Separator,
                manga,
                Entry::item(tr!("fullscreen"), Command::Fullscreen),
                slide,
                Separator,
                Entry::item(tr!("image-info"), Command::Info),
            ],
        }
    }
    /// commandを実行するキーのうち、メニューに出す1つ（`preferred_key`）。漫画モードで左右キーを
    /// 入れ替えている場合も、実際にその操作を行うキーを返す。
    pub(super) fn command_key(&self, command: Command) -> Option<String> {
        preferred_key(
            self.settings
                .shortcuts
                .iter()
                .map(|(key, action)| (key, *action))
                .filter(|(_, action)| self.shortcut_command(*action) == command),
        )
        .map(|key| key_label(key))
    }
    /// 右クリックメニューをクリック位置に開く。画面に収まらない分は描画時にずらす。
    pub(super) fn open_context_menu(&mut self, position: [f32; 2]) {
        self.active_menu = Some(Menu::Context);
        self.menu_selected = 0;
        self.menu_anchor = position;
    }
    pub(super) fn menu_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .h(px(HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(TITLE_GAP))
            .px(px(BAR_PADDING))
            .bg(rgb(0x20242b))
            .children(Menu::BAR.into_iter().map(|menu| {
                div()
                    .id(menu as usize)
                    .w(px(TITLE_WIDTH))
                    .flex()
                    .justify_center()
                    .py_1()
                    .cursor_pointer()
                    .bg(rgb(if self.active_menu == Some(menu) {
                        0x404b60
                    } else {
                        0x20242b
                    }))
                    .hover(|s| s.bg(rgb(0x404b60)))
                    .child(menu.label())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.command(Command::Menu(menu), window, cx)
                    }))
            }))
            .into_any_element()
    }
    pub(super) fn menu_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let menu = self.active_menu.unwrap_or(Menu::File);
        // 右クリックメニューを画面の下端・右端で開いても切れないよう、画面内へずらす。
        anchored()
            .position(point(px(self.menu_anchor[0]), px(self.menu_anchor[1])))
            .snap_to_window_with_margin(px(SCREEN_MARGIN))
            .child(self.menu_list(menu, cx))
            .into_any_element()
    }
    fn menu_list(&self, menu: Menu, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            // メニューごとに別のIDにして、スクロール位置を引き継がない。
            .id(("menu-popup", menu as usize))
            .w(px(WIDTH))
            .max_h(relative(0.85))
            .overflow_y_scroll()
            .p_1()
            .rounded_sm()
            .bg(rgb(0x292f3a))
            .border_1()
            .border_color(rgb(0x5a6578))
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.active_menu = None;
                cx.notify();
            }))
            .children(
                self.menu_entries(menu)
                    .into_iter()
                    .enumerate()
                    .map(|(index, entry)| match entry {
                        Entry::Separator => div()
                            .id(index)
                            .my_1()
                            .mx_2()
                            .h(px(1.))
                            .bg(rgb(0x4a5466))
                            .into_any_element(),
                        Entry::Item {
                            label,
                            command,
                            mark,
                        } => {
                            div()
                                .id(index)
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_2()
                                .py_1()
                                .cursor_pointer()
                                .bg(rgb(if index == self.menu_selected {
                                    0x465574
                                } else {
                                    0x292f3a
                                }))
                                .hover(|s| s.bg(rgb(0x465574)))
                                .child(div().w(px(16.)).flex_none().child(match mark {
                                    Mark::Check(true) => "✓",
                                    Mark::Radio(true) => "●",
                                    _ => "",
                                }))
                                .child(div().flex_1().min_w_0().child(label))
                                .children(self.command_key(command).map(|key| {
                                    div().flex_none().text_color(rgb(0x9aa3b2)).child(key)
                                }))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.command(command, window, cx)
                                }))
                                .into_any_element()
                        }
                    }),
            )
    }
    pub(super) fn menu_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let menu = self.active_menu.unwrap_or(Menu::File);
        let entries = self.menu_entries(menu);
        // 区切り線を飛ばして、上下の項目へ移る。
        let step = |from: usize, forward: bool| {
            let count = entries.len();
            let mut index = from;
            for _ in 0..count {
                index = if forward {
                    (index + 1) % count
                } else {
                    (index + count - 1) % count
                };
                if entries[index].command().is_some() {
                    break;
                }
            }
            index
        };
        match key {
            "escape" => self.active_menu = None,
            "up" => self.menu_selected = step(self.menu_selected, false),
            "down" => self.menu_selected = step(self.menu_selected, true),
            // 右クリックメニューでは、隣のメニューへ移らない。
            "left" | "right" if menu != Menu::Context => {
                let count = Menu::BAR.len();
                let index = Menu::BAR.iter().position(|m| *m == menu).unwrap_or(0);
                let index = if key == "left" {
                    (index + count - 1) % count
                } else {
                    (index + 1) % count
                };
                self.active_menu = Some(Menu::BAR[index]);
                self.menu_selected = 0;
                self.menu_anchor = Menu::BAR[index].anchor();
            }
            "enter" => {
                if let Some(command) = entries.get(self.menu_selected).and_then(Entry::command) {
                    self.command(command, window, cx)
                }
            }
            _ => {}
        }
        cx.notify();
        cx.stop_propagation();
    }
}

#[cfg(test)]
mod tests {
    // `super::*` はgpuiの `test` マクロも取り込み、`#[test]` が再帰的に展開されるため名前を絞る。
    use super::{
        BAR_PADDING, HEIGHT, Menu, Shortcut, TITLE_GAP, TITLE_WIDTH, key_label, preferred_key,
    };

    fn pick(candidates: &[(&str, Shortcut)]) -> Option<String> {
        let keys: Vec<(String, Shortcut)> = candidates
            .iter()
            .map(|(key, action)| (key.to_string(), *action))
            .collect();
        preferred_key(keys.iter().map(|(key, action)| (key, *action))).cloned()
    }

    #[test]
    fn menus_show_the_key_the_user_would_press() {
        use Shortcut::*;
        // 既定では「次の画像」は→。Spaceは読む方向にかかわらず進む別名。
        assert_eq!(
            pick(&[("right", Next), ("space", Advance)]).unwrap(),
            "right"
        );
        // 利用者が「次の画像」をNへ変えたら、Spaceではなく N を出す。
        assert_eq!(pick(&[("n", Next), ("space", Advance)]).unwrap(), "n");
        // 同じ操作の既定キーは、DEFAULTSの並び（0よりFが先）で選ぶ。
        assert_eq!(pick(&[("0", Fit), ("f", Fit)]).unwrap(), "f");
        // 既定と同じキーでも、別の操作への割り当てなら既定扱いしない。
        assert_eq!(
            pick(&[("f", Fullscreen), ("f11", Fullscreen)]).unwrap(),
            "f11"
        );
        // 利用者が加えたキーは、修飾キーの少ないものを出す。
        assert_eq!(pick(&[("ctrl-shift-x", Info), ("x", Info)]).unwrap(), "x");
        assert_eq!(pick(&[]), None);
    }

    #[test]
    #[should_panic(expected = "only menu bar menus")]
    fn the_context_menu_has_no_title_position() {
        Menu::Context.anchor();
    }

    #[test]
    fn keys_are_shown_with_modifiers_and_symbols() {
        assert_eq!(key_label("ctrl-shift-o"), "Ctrl+Shift+O");
        assert_eq!(key_label("left"), "←");
        assert_eq!(key_label("alt-up"), "Alt+↑");
        assert_eq!(key_label("f11"), "F11");
        assert_eq!(key_label("ctrl-,"), "Ctrl+,");
        assert_eq!(key_label("["), "[");
        assert_eq!(key_label("-"), "−");
        assert_eq!(key_label("ctrl-home"), "Ctrl+Home");
        assert_eq!(key_label("space"), "Space");
    }

    #[test]
    fn menu_titles_line_up_with_their_popups() {
        assert_eq!(Menu::File.anchor(), [BAR_PADDING, HEIGHT]);
        assert_eq!(
            Menu::View.anchor()[0],
            BAR_PADDING + 3. * (TITLE_WIDTH + TITLE_GAP)
        );
    }
}
