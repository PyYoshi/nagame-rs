//! メニューバー・右クリックメニューの描画とキー操作。実行はCommandへ集約する。

use crate::i18n::tr;
use crate::{Command, Viewer, settings::SortOrder};
use gpui::{prelude::*, *};
pub const HEIGHT: f32 = 28.;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    File,
    List,
    View,
    Other,
}
impl Menu {
    const ALL: [Self; 4] = [Self::File, Self::List, Self::View, Self::Other];
    fn label(self) -> String {
        match self {
            Self::File => tr!("menu-file"),
            Self::List => tr!("menu-list"),
            Self::View => tr!("menu-view"),
            Self::Other => tr!("menu-other"),
        }
    }
    fn entries(self) -> Vec<(String, Command)> {
        match self {
            Self::File => vec![
                (tr!("open-image-menu"), Command::Open),
                (tr!("open-folder-menu"), Command::OpenFolder),
                (tr!("reload"), Command::Reload),
                (tr!("save-as-menu"), Command::SaveAs),
                (tr!("copy-image"), Command::CopyImage),
                (tr!("copy-file-path"), Command::CopyPath),
                (tr!("reveal-folder"), Command::Reveal),
            ],
            Self::List => vec![
                (tr!("previous-image"), Command::Previous),
                (tr!("next-image"), Command::Next),
                (tr!("skip-previous"), Command::SkipPrevious),
                (tr!("skip-next"), Command::SkipNext),
                (tr!("first-image"), Command::First),
                (tr!("last-image"), Command::Last),
                (tr!("previous-folder"), Command::PreviousFolder),
                (tr!("next-folder"), Command::NextFolder),
                (tr!("previous-sibling"), Command::PreviousSibling),
                (tr!("next-sibling"), Command::NextSibling),
                (tr!("parent-folder"), Command::Parent),
                (tr!("sort-name"), Command::Sort(SortOrder::Name)),
                (tr!("sort-modified"), Command::Sort(SortOrder::Modified)),
                (tr!("sort-size"), Command::Sort(SortOrder::Size)),
                (tr!("sort-extension"), Command::Sort(SortOrder::Extension)),
                (tr!("random"), Command::Sort(SortOrder::Random)),
                (tr!("reverse-order"), Command::ReverseSort),
            ],
            Self::View => vec![
                (tr!("image-info"), Command::Info),
                (tr!("reveal-image-folder"), Command::Reveal),
                (tr!("zoom-in"), Command::ZoomIn),
                (tr!("zoom-out"), Command::ZoomOut),
                (tr!("fit-window"), Command::Fit),
                (tr!("actual-size"), Command::Actual),
                (tr!("rotate-right"), Command::Rotate),
                (tr!("fullscreen"), Command::Fullscreen),
                (tr!("manga"), Command::Manga),
                (tr!("reading-direction"), Command::ReadingDirection),
                (tr!("slideshow"), Command::Slide),
            ],
            Self::Other => vec![
                (tr!("settings-menu"), Command::Settings),
                (tr!("image-info"), Command::Info),
            ],
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
                    .id(menu as usize)
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
                            .id(index)
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
