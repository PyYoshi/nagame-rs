//! メニューバー・右クリックメニューの描画とキー操作。実行はCommandへ集約する。

use crate::{Command, Viewer, settings::SortOrder};
use gpui::{prelude::*, *};
pub const HEIGHT: f32 = 28.;
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    File,
    List,
    View,
    Other,
}
impl Menu {
    const ALL: [Self; 4] = [Self::File, Self::List, Self::View, Self::Other];
    fn label(self) -> &'static str {
        match self {
            Self::File => "ファイル",
            Self::List => "リスト",
            Self::View => "表示",
            Self::Other => "その他",
        }
    }
    fn entries(self) -> Vec<(&'static str, Command)> {
        match self {
            Self::File => vec![
                ("画像を開く…", Command::Open),
                ("フォルダを開く…", Command::OpenFolder),
                ("再読み込み", Command::Reload),
                ("名前を付けて保存…", Command::SaveAs),
                ("画像をコピー", Command::CopyImage),
                ("ファイルのパスをコピー", Command::CopyPath),
                ("フォルダをファイラーで開く", Command::Reveal),
            ],
            Self::List => vec![
                ("前の画像", Command::Previous),
                ("次の画像", Command::Next),
                ("前へスキップ", Command::SkipPrevious),
                ("次へスキップ", Command::SkipNext),
                ("最初の画像", Command::First),
                ("最後の画像", Command::Last),
                ("前のフォルダ", Command::PreviousFolder),
                ("次のフォルダ", Command::NextFolder),
                ("前隣のフォルダ", Command::PreviousSibling),
                ("次隣のフォルダ", Command::NextSibling),
                ("親フォルダ", Command::Parent),
                ("名前順", Command::Sort(SortOrder::Name)),
                ("更新日時順", Command::Sort(SortOrder::Modified)),
                ("サイズ順", Command::Sort(SortOrder::Size)),
                ("拡張子順", Command::Sort(SortOrder::Extension)),
                ("ランダム", Command::Sort(SortOrder::Random)),
                ("逆順", Command::ReverseSort),
            ],
            Self::View => vec![
                ("画像の情報", Command::Info),
                ("画像のあるフォルダを開く", Command::Reveal),
                ("ズームイン", Command::ZoomIn),
                ("ズームアウト", Command::ZoomOut),
                ("ウィンドウに合わせる", Command::Fit),
                ("原寸", Command::Actual),
                ("右回転", Command::Rotate),
                ("全画面", Command::Fullscreen),
                ("漫画モード", Command::Manga),
                ("漫画の読む方向を切り替え", Command::ReadingDirection),
                ("自動スライド", Command::Slide),
            ],
            Self::Other => vec![("設定…", Command::Settings), ("画像の情報", Command::Info)],
        }
    }
}
impl Viewer {
    pub(super) fn menu_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .h(px(HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .bg(rgb(0x20242b))
            .children(Menu::ALL.into_iter().map(|menu| {
                div()
                    .id(menu.label())
                    .px_3()
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
        div()
            .id("menu-popup")
            .absolute()
            .left(px(self.menu_anchor[0]))
            .top(px(self.menu_anchor[1]))
            .w(px(300.))
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
                menu.entries()
                    .into_iter()
                    .enumerate()
                    .map(|(index, (label, command))| {
                        div()
                            .id(label)
                            .px_3()
                            .py_1()
                            .cursor_pointer()
                            .bg(rgb(if index == self.menu_selected {
                                0x465574
                            } else {
                                0x292f3a
                            }))
                            .hover(|s| s.bg(rgb(0x465574)))
                            .child(label)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.command(command, window, cx)
                            }))
                    }),
            )
            .into_any_element()
    }
    pub(super) fn menu_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let menu = self.active_menu.unwrap_or(Menu::File);
        let count = menu.entries().len();
        match key {
            "escape" => self.active_menu = None,
            "up" => self.menu_selected = (self.menu_selected + count - 1) % count,
            "down" => self.menu_selected = (self.menu_selected + 1) % count,
            "left" | "right" => {
                let index = Menu::ALL.iter().position(|m| *m == menu).unwrap_or(0);
                let index = if key == "left" {
                    (index + 3) % 4
                } else {
                    (index + 1) % 4
                };
                self.active_menu = Some(Menu::ALL[index]);
                self.menu_selected = 0;
                self.menu_anchor = [index as f32 * 82., HEIGHT];
            }
            "enter" => self.command(menu.entries()[self.menu_selected].1, window, cx),
            _ => {}
        }
        cx.notify();
        cx.stop_propagation();
    }
}
