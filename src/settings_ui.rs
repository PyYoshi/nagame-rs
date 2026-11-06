//! Viewerの設定下書きを編集するUIとキー操作。
//! セレクターの強調候補と、決定済みの下書き値は分けて保持する。

use crate::bindings::Shortcut;
use crate::{
    Command, Viewer,
    settings::{BoundaryAction, Settings, SortOrder},
};
use gpui::{prelude::*, *};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Navigation,
    Display,
    Slideshow,
    Keyboard,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Boundary,
    PreviousFirst,
    ParentFirst,
    Sort,
    Reverse,
    Natural,
    Skip,
    Fit,
    Upscale,
    LockZoom,
    Background,
    Manga,
    Rtl,
    Landscape,
    ReverseControls,
    Interval,
    Prefetch,
    Wheel,
    ShrinkAlgorithm,
    EnlargeAlgorithm,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Choice {
    Boundary(BoundaryAction),
    Sort(SortOrder),
    Color(u32),
    Algorithm(crate::resample::Algorithm),
}
impl Choice {
    fn label(self) -> String {
        match self {
            Self::Boundary(v) => v.label().into(),
            Self::Sort(v) => v.label().into(),
            Self::Algorithm(v) => v.label().into(),
            Self::Color(v) => match v {
                0x181a1f => "ダーク（#181A1F）".into(),
                0 => "黒（#000000）".into(),
                0x404040 => "グレー（#404040）".into(),
                0xffffff => "白（#FFFFFF）".into(),
                _ => format!("現在の色（#{v:06X}）"),
            },
        }
    }
}
#[derive(Clone, Copy)]
pub(super) struct Selector {
    field: Field,
    highlighted: usize,
    position: Point<Pixels>,
}
impl Tab {
    fn label(self) -> &'static str {
        match self {
            Self::Navigation => "移動・並び順",
            Self::Display => "表示・漫画",
            Self::Slideshow => "読み込み・スライド",
            Self::Keyboard => "キー設定",
        }
    }
    pub fn fields(self) -> &'static [Field] {
        use Field::*;
        match self {
            Self::Navigation => &[
                Boundary,
                PreviousFirst,
                ParentFirst,
                Sort,
                Reverse,
                Natural,
                Skip,
            ],
            Self::Display => &[
                Fit,
                Upscale,
                LockZoom,
                Background,
                Manga,
                Rtl,
                Landscape,
                ReverseControls,
                ShrinkAlgorithm,
                EnlargeAlgorithm,
            ],
            Self::Slideshow => &[Interval, Prefetch, Wheel],
            Self::Keyboard => &[],
        }
    }
}
impl Field {
    fn label(self) -> &'static str {
        use Field::*;
        match self {
            Boundary => "最初・最後の画像に到達した時",
            PreviousFirst => "前のフォルダは先頭画像を開く",
            ParentFirst => "親フォルダは先頭画像を開く",
            Sort => "ファイルの並び順",
            Reverse => "逆順に並べる",
            Natural => "名前の数字は数値順に並べる",
            Skip => "スキップする画像の枚数",
            Fit => "画像を開いた時にフィットする",
            Upscale => "フィット時に小さな画像も拡大する",
            LockZoom => "画像を送っても倍率を維持する",
            Background => "背景色",
            Manga => "漫画モード（見開き表示）",
            Rtl => "漫画を右から左に読む",
            Landscape => "横長の画像は単独で表示する",
            ReverseControls => "右から左の漫画は左右の操作を逆にする",
            Interval => "スライドショーの表示秒数",
            Prefetch => "前後の画像を先読みする",
            Wheel => "ホイールでズーム（Ctrlで画像送り）",
            ShrinkAlgorithm => "縮小表示の補間方法",
            EnlargeAlgorithm => "拡大表示の補間方法",
        }
    }
    fn choices(self, s: &Settings) -> Vec<Choice> {
        match self {
            Self::Boundary => [
                BoundaryAction::Ask,
                BoundaryAction::Wrap,
                BoundaryAction::Folder,
                BoundaryAction::Stop,
            ]
            .into_iter()
            .map(Choice::Boundary)
            .collect(),
            Self::Sort => SortOrder::ALL.into_iter().map(Choice::Sort).collect(),
            Self::ShrinkAlgorithm | Self::EnlargeAlgorithm => crate::resample::Algorithm::ALL
                .into_iter()
                .map(Choice::Algorithm)
                .collect(),
            Self::Background => {
                let mut colors = vec![0x181a1f, 0, 0x404040, 0xffffff];
                if !colors.contains(&s.background) {
                    colors.push(s.background);
                }
                colors.into_iter().map(Choice::Color).collect()
            }
            _ => vec![],
        }
    }
    fn selected_choice(self, s: &Settings) -> Option<Choice> {
        match self {
            Self::Boundary => Some(Choice::Boundary(s.boundary)),
            Self::Sort => Some(Choice::Sort(s.sort)),
            Self::Background => Some(Choice::Color(s.background)),
            Self::ShrinkAlgorithm => Some(Choice::Algorithm(s.shrink_algorithm)),
            Self::EnlargeAlgorithm => Some(Choice::Algorithm(s.enlarge_algorithm)),
            _ => None,
        }
    }
    fn choose(self, s: &mut Settings, choice: Choice) {
        match (self, choice) {
            (Self::Boundary, Choice::Boundary(v)) => s.boundary = v,
            (Self::Sort, Choice::Sort(v)) => s.sort = v,
            (Self::Background, Choice::Color(v)) => s.background = v,
            (Self::ShrinkAlgorithm, Choice::Algorithm(v)) => s.shrink_algorithm = v,
            (Self::EnlargeAlgorithm, Choice::Algorithm(v)) => s.enlarge_algorithm = v,
            _ => {}
        }
    }
    fn numeric(self) -> bool {
        matches!(self, Self::Skip | Self::Interval)
    }
    fn value(self, s: &Settings) -> String {
        use Field::*;
        let boolean = match self {
            Boundary => return s.boundary.label().into(),
            ShrinkAlgorithm => return s.shrink_algorithm.label().into(),
            EnlargeAlgorithm => return s.enlarge_algorithm.label().into(),
            Sort => return s.sort.label().into(),
            Skip => return s.skip_count.to_string(),
            Interval => return s.slideshow_seconds.to_string(),
            Background => return format!("#{:06X}", s.background),
            PreviousFirst => s.previous_folder_first,
            ParentFirst => s.parent_folder_first,
            Reverse => s.reverse,
            Natural => s.natural_sort,
            Fit => s.fit_on_open,
            Upscale => s.fit_upscale,
            LockZoom => s.lock_zoom,
            Manga => s.manga,
            Rtl => s.right_to_left,
            Landscape => s.landscape_single,
            ReverseControls => s.manga_reverse_controls,
            Prefetch => s.prefetch,
            Wheel => s.wheel_zoom,
        };
        if boolean { "有効" } else { "無効" }.into()
    }
    pub fn adjust(self, s: &mut Settings, direction: i32) {
        use Field::*;
        match self {
            ShrinkAlgorithm => s.shrink_algorithm = s.shrink_algorithm.next(direction),
            EnlargeAlgorithm => s.enlarge_algorithm = s.enlarge_algorithm.next(direction),
            Boundary => {
                for _ in 0..if direction < 0 { 3 } else { 1 } {
                    s.boundary = s.boundary.next();
                }
            }
            Sort => {
                let index = SortOrder::ALL
                    .iter()
                    .position(|v| *v == s.sort)
                    .unwrap_or(0) as i32;
                s.sort = SortOrder::ALL[(index + direction.signum()).rem_euclid(5) as usize];
            }
            Background => {
                let colors = [0x181a1f, 0x000000, 0x404040, 0xffffff];
                let index = colors.iter().position(|v| *v == s.background).unwrap_or(0) as i32;
                s.background = colors[(index + direction.signum()).rem_euclid(4) as usize];
            }
            Skip => {
                s.skip_count = (s.skip_count as i64 + i64::from(direction)).clamp(1, 10000) as u32
            }
            Interval => {
                s.slideshow_seconds =
                    (s.slideshow_seconds as i64 + i64::from(direction)).clamp(1, 3600) as u32
            }
            _ => {
                let value = match self {
                    PreviousFirst => &mut s.previous_folder_first,
                    ParentFirst => &mut s.parent_folder_first,
                    Reverse => &mut s.reverse,
                    Natural => &mut s.natural_sort,
                    Fit => &mut s.fit_on_open,
                    Upscale => &mut s.fit_upscale,
                    LockZoom => &mut s.lock_zoom,
                    Manga => &mut s.manga,
                    Rtl => &mut s.right_to_left,
                    Landscape => &mut s.landscape_single,
                    ReverseControls => &mut s.manga_reverse_controls,
                    Prefetch => &mut s.prefetch,
                    Wheel => &mut s.wheel_zoom,
                    _ => unreachable!(),
                };
                *value = !*value;
            }
        }
    }
}
impl Viewer {
    fn open_selector(&mut self, field: Field, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = &self.settings_draft else {
            return;
        };
        let choices = field.choices(draft);
        if choices.is_empty() {
            return;
        }
        if self
            .settings_selector
            .is_some_and(|selector| selector.field == field)
        {
            self.settings_selector = None;
            cx.notify();
            return;
        }
        let index = self
            .settings_tab
            .fields()
            .iter()
            .position(|f| *f == field)
            .unwrap_or(0);
        let position = self
            .settings_scroll
            .bounds_for_item(index)
            .map(|bounds| {
                let offset = self.settings_scroll.offset();
                point(
                    bounds.right() + offset.x - px(292.),
                    bounds.bottom() + offset.y,
                )
            })
            .unwrap_or_else(|| point(px(300.), px(150.)));
        self.settings_selected = index;
        self.settings_scroll.scroll_to_item(index);
        self.settings_selector = Some(Selector {
            field,
            highlighted: choices
                .iter()
                .position(|v| Some(*v) == field.selected_choice(draft))
                .unwrap_or(0),
            position,
        });
        window.focus(&self.focus);
        cx.notify();
    }
    fn choose_setting(&mut self, field: Field, choice: Choice, cx: &mut Context<Self>) {
        if let Some(draft) = &mut self.settings_draft {
            field.choose(draft, choice);
        }
        self.settings_selector = None;
        self.settings_error = None;
        cx.notify();
    }
    fn selector_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(mut selector) = self.settings_selector else {
            return false;
        };
        let choices = selector
            .field
            .choices(self.settings_draft.as_ref().unwrap_or(&self.settings));
        match key {
            "escape" | "tab" => self.settings_selector = None,
            "up" | "left" => {
                selector.highlighted = (selector.highlighted + choices.len() - 1) % choices.len();
                self.settings_selector = Some(selector);
            }
            "down" | "right" => {
                selector.highlighted = (selector.highlighted + 1) % choices.len();
                self.settings_selector = Some(selector);
            }
            "home" => {
                selector.highlighted = 0;
                self.settings_selector = Some(selector);
            }
            "end" => {
                selector.highlighted = choices.len() - 1;
                self.settings_selector = Some(selector);
            }
            "enter" | "space" => {
                self.choose_setting(selector.field, choices[selector.highlighted], cx)
            }
            _ => {}
        }
        cx.notify();
        cx.stop_propagation();
        true
    }
    fn selector_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let selector = self.settings_selector.unwrap();
        let draft = self.settings_draft.as_ref().unwrap_or(&self.settings);
        let current = selector.field.selected_choice(draft);
        anchored()
            .position(selector.position)
            .snap_to_window_with_margin(px(8.))
            .child(
                div()
                    .id("settings-selector-popup")
                    .w(px(280.))
                    .p_1()
                    .bg(rgb(0x292f3a))
                    .border_1()
                    .border_color(rgb(0x8b9dbb))
                    .rounded_sm()
                    .shadow_lg()
                    .occlude()
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.settings_selector = None;
                        cx.notify();
                    }))
                    .children(selector.field.choices(draft).into_iter().enumerate().map(
                        |(index, choice)| {
                            div()
                                .id(("option", index))
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .py_2()
                                .cursor_pointer()
                                .bg(rgb(if index == selector.highlighted {
                                    0x465574
                                } else {
                                    0x292f3a
                                }))
                                .hover(|style| style.bg(rgb(0x465574)))
                                .child(div().w(px(18.)).child(if Some(choice) == current {
                                    "✓"
                                } else {
                                    ""
                                }))
                                .child(choice.label())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.choose_setting(selector.field, choice, cx)
                                }))
                        },
                    )),
            )
            .into_any_element()
    }
    fn edit_setting(&mut self, field: Field, direction: i32, cx: &mut Context<Self>) {
        if let Some(draft) = &mut self.settings_draft {
            field.adjust(draft, direction);
        }
        self.settings_selected = self
            .settings_tab
            .fields()
            .iter()
            .position(|f| *f == field)
            .unwrap_or(0);
        self.settings_error = None;
        cx.notify();
    }
    pub(super) fn settings_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        if self.selector_key(key, cx) {
            return;
        }
        if let Some(action) = self.capture_binding {
            if key == "escape" {
                self.capture_binding = None;
            } else if !event.is_held
                && !matches!(key, "shift" | "ctrl" | "alt" | "cmd" | "super" | "fn")
            {
                if let Some(draft) = &mut self.settings_draft {
                    draft.shortcuts.retain(|_, value| *value != action);
                    draft.shortcuts.insert(event.keystroke.unparse(), action);
                }
                self.capture_binding = None;
            }
            self.settings_scroll.scroll_to_item(self.settings_selected);
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if key == "4" {
            self.settings_tab = Tab::Keyboard;
            self.settings_selected = 0;
            self.settings_scroll.scroll_to_item(self.settings_selected);
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if self.settings_tab == Tab::Keyboard && !matches!(key, "1" | "2" | "3") {
            let count = Shortcut::ALL.len();
            match key {
                "escape" => self.command(Command::CancelSettings, window, cx),
                "enter" => self.command(Command::SaveSettings, window, cx),
                "up" => self.settings_selected = (self.settings_selected + count - 1) % count,
                "down" => self.settings_selected = (self.settings_selected + 1) % count,
                "tab" => {
                    self.settings_selected = if event.keystroke.modifiers.shift {
                        (self.settings_selected + count - 1) % count
                    } else {
                        (self.settings_selected + 1) % count
                    }
                }
                "space" | "right" => {
                    self.capture_binding = Some(Shortcut::ALL[self.settings_selected])
                }
                "delete" => {
                    let action = Shortcut::ALL[self.settings_selected];
                    if let Some(draft) = &mut self.settings_draft {
                        draft.shortcuts.retain(|_, a| *a != action);
                    }
                }
                _ => {}
            }
            self.settings_scroll.scroll_to_item(self.settings_selected);
            cx.notify();
            cx.stop_propagation();
            return;
        }
        let count = self.settings_tab.fields().len().max(1);
        match key {
            "escape" => self.command(Command::CancelSettings, window, cx),
            "enter" => self.command(Command::SaveSettings, window, cx),
            "1" | "2" | "3" => {
                self.settings_tab = match key {
                    "1" => Tab::Navigation,
                    "2" => Tab::Display,
                    _ => Tab::Slideshow,
                };
                self.settings_selected = 0;
            }
            "up" => self.settings_selected = (self.settings_selected + count - 1) % count,
            "down" => self.settings_selected = (self.settings_selected + 1) % count,
            "tab" => {
                self.settings_selected = if event.keystroke.modifiers.shift {
                    (self.settings_selected + count - 1) % count
                } else {
                    (self.settings_selected + 1) % count
                }
            }
            "left" | "right" | "space" => {
                let field = self.settings_tab.fields()[self.settings_selected];
                if field
                    .selected_choice(self.settings_draft.as_ref().unwrap_or(&self.settings))
                    .is_some()
                {
                    self.open_selector(field, window, cx);
                } else {
                    self.edit_setting(field, if key == "left" { -1 } else { 1 }, cx);
                }
            }
            _ => {}
        }
        self.settings_scroll.scroll_to_item(self.settings_selected);
        cx.notify();
        cx.stop_propagation();
    }
    pub(super) fn settings_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let draft = self.settings_draft.as_ref().unwrap_or(&self.settings);
        let rows = self
            .settings_tab
            .fields()
            .iter()
            .copied()
            .enumerate()
            .map(|(index, field)| {
                let value = field.value(draft);
                let button = |id: &'static str, label: String, direction: i32| {
                    div()
                        .id(id)
                        .px_3()
                        .py_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .bg(rgb(0x373d49))
                        .hover(|style| style.bg(rgb(0x465574)))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.edit_setting(field, direction, cx)
                        }))
                };
                let control = if field.numeric() {
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(button("minus", "−".into(), -1))
                        .child(value)
                        .child(button("plus", "＋".into(), 1))
                        .into_any_element()
                } else if field.selected_choice(draft).is_some() {
                    div()
                        .id("selector")
                        .w(px(280.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .px_3()
                        .py_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .bg(rgb(
                            if self
                                .settings_selector
                                .is_some_and(|selector| selector.field == field)
                            {
                                0x465574
                            } else {
                                0x373d49
                            },
                        ))
                        .hover(|style| style.bg(rgb(0x465574)))
                        .child(value)
                        .child("▾")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_selector(field, window, cx)
                        }))
                        .into_any_element()
                } else {
                    button("value", value, 1).into_any_element()
                };
                div()
                    .id(field.label())
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(0x393e48))
                    .bg(rgb(if index == self.settings_selected {
                        0x2e3748
                    } else {
                        0x252a33
                    }))
                    .child(field.label())
                    .child(control)
            })
            .collect::<Vec<_>>();
        let rows = if self.settings_tab == Tab::Keyboard {
            Shortcut::ALL
                .iter()
                .copied()
                .enumerate()
                .map(|(index, action)| {
                    let keys = draft
                        .shortcuts
                        .iter()
                        .filter(|(_, value)| **value == action)
                        .map(|(key, _)| key.clone())
                        .collect::<Vec<_>>()
                        .join(" / ");
                    let selected = self.settings_selected == index;
                    div()
                        .id(action.label())
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .bg(rgb(if selected { 0x2e3748 } else { 0x252a33 }))
                        .child(action.label())
                        .child(
                            div()
                                .id("binding")
                                .px_3()
                                .py_1()
                                .cursor_pointer()
                                .bg(rgb(0x373d49))
                                .child(if self.capture_binding == Some(action) {
                                    "キーを押してください…".to_owned()
                                } else if keys.is_empty() {
                                    "未割り当て".to_owned()
                                } else {
                                    keys
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.capture_binding = Some(action);
                                    this.settings_selected = index;
                                    cx.notify();
                                })),
                        )
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        } else {
            rows.into_iter()
                .map(IntoElement::into_any_element)
                .collect::<Vec<_>>()
        };

        div().id("settings-overlay").absolute().inset_0().bg(rgba(0x00000099)).flex().items_center().justify_center().occlude()
            .on_scroll_wheel(cx.listener(|this,_,_,cx|{if this.settings_selector.take().is_some(){cx.notify();}cx.stop_propagation();}))
            .child(div().id("settings-dialog").w(px(760.)).max_w(relative(0.96)).h(px(660.)).max_h(relative(0.94)).flex().flex_col().overflow_hidden().p_5().rounded_lg().bg(rgb(0x252a33)).border_1().border_color(rgb(0x606a79)).text_color(rgb(0xe3e6ec)).text_sm()
                .child(div().flex_none().text_xl().mb_3().child("設定"))
                .child(div().flex_none().flex().gap_2().mb_4().children([Tab::Navigation,Tab::Display,Tab::Slideshow,Tab::Keyboard].into_iter().map(|tab|
                    div().id(tab.label()).px_3().py_2().rounded_sm().cursor_pointer().bg(rgb(if tab==self.settings_tab {0x465574}else{0x343b48})).child(tab.label())
                        .on_click(cx.listener(move |this,_,_,cx| {this.settings_selector=None;this.settings_tab=tab;this.settings_selected=0;this.capture_binding=None;this.settings_scroll.scroll_to_item(0);cx.notify();})))))
                .when(self.settings_tab==Tab::Keyboard,|panel|panel.child(div().text_xs().mb_3().child("変更する項目を選んでSpace、続いて割り当てるキーを押してください。既存の割り当ては置き換わります。Escでキー入力を中止できます。")))
                .child(div().flex_none().mb_2().text_xs().text_color(rgb(0xb8c4d7)).child(format!("このタブには {} 項目あります。右のバー・ホイール・↑↓でスクロールできます。", rows.len())))
                .child(div().flex().flex_1().min_h_0().gap_2()
                    .child(div().id("settings-rows").flex_1().min_w_0().h_full().overflow_y_scroll().track_scroll(&self.settings_scroll).children(rows))
                    .child(self.settings_scrollbar(cx)))
                .when_some(self.settings_error.clone(),|panel,error|panel.child(div().mt_3().text_color(rgb(0xffa0a0)).child(error)))
                .child(div().flex_none().mt_4().text_xs().text_color(rgb(0xa9b3c2)).child("1〜4: タブ / ↑↓: 項目 / Space: 選択・変更 / 選択中のEnter: 決定、Esc: 中止 / Enter: 保存"))
                .child(div().flex_none().flex().justify_between().mt_4()
                    .child(self.button("初期値に戻す",Command::ResetSettings,cx))
                    .child(div().flex().gap_3().child(self.button("キャンセル",Command::CancelSettings,cx)).child(self.button("保存して適用",Command::SaveSettings,cx)))))
            .when(self.settings_selector.is_some(),|overlay|overlay.child(self.selector_panel(cx)))
            .into_any_element()
    }
}
