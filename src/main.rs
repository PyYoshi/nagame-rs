//! Nagameの起動とViewer状態、入力、非同期結果の反映、描画。
//! UI補助モジュールも同じViewerへ実装を追加する。設計はdocs/architecture.mdを参照。

mod bindings;
mod decoder;
mod i18n;
mod menu_ui;
mod model;
mod pipeline;
mod resample;
mod scrollbar;
mod settings;
mod settings_ui;
#[cfg(feature = "ui-smoke")]
mod ui_smoke;
mod viewer_io;

use gpui::{prelude::*, *};
use i18n::tr;
use model::{Album, Viewport};
use settings::{BoundaryAction, Settings};
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

const BAR: f32 = 48.;
const FOOTER: f32 = 30.;

#[derive(Clone, Copy, Debug)]
enum Command {
    Open,
    Menu(menu_ui::Menu),
    OpenFolder,
    SaveAs,
    CopyImage,
    CopyPath,
    Reveal,
    Info,
    ReadingDirection,
    Sort(settings::SortOrder),
    ReverseSort,
    Previous,
    Next,
    First,
    Last,
    ZoomIn,
    ZoomOut,
    Fit,
    Actual,
    Rotate,
    Fullscreen,
    Slide,
    Reload,
    Wrap,
    Dismiss,
    PreviousFolder,
    NextFolder,
    PreviousSibling,
    NextSibling,
    Parent,
    Settings,
    SaveSettings,
    CancelSettings,
    ResetSettings,
    Manga,
    SkipPrevious,
    SkipNext,
}

/// UI状態の所有者。ワーカーの結果はtick等で世代を照合してから反映する。
struct Viewer {
    focus: FocusHandle,
    album: Album,
    view: Viewport,
    image: Option<Arc<RenderImage>>,
    dimensions: [f32; 2],
    retired: Vec<Arc<RenderImage>>,
    resampling: resample::State,
    turns: u8,
    generation: u64,
    loading: bool,
    error: Option<String>,
    requests: pipeline::Loader,
    responses: mpsc::Receiver<pipeline::Response>,
    ticker: Option<Task<()>>,
    slide: bool,
    next_slide: Instant,
    last_scroll: Instant,
    drag: Option<(Point<Pixels>, [f32; 2])>,
    force_reload: bool,
    boundary: Option<isize>,
    active_menu: Option<menu_ui::Menu>,
    menu_anchor: [f32; 2],
    menu_selected: usize,
    info_visible: bool,
    info_text: Option<String>,
    notice: Option<String>,
    pages: usize,
    settings: Settings,
    settings_draft: Option<Settings>,
    settings_tab: settings_ui::Tab,
    settings_selected: usize,
    settings_selector: Option<settings_ui::Selector>,
    settings_scroll: ScrollHandle,
    settings_scroll_drag: Option<f32>,
    capture_binding: Option<bindings::Shortcut>,
    settings_error: Option<String>,
}

impl Viewer {
    fn new(path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (settings, settings_error) =
            match Settings::path().and_then(|path| Settings::read(&path)) {
                Ok(settings) => (settings, None),
                Err(error) => (Settings::default(), Some(error)),
            };
        i18n::activate(settings.language);
        let focus = cx.focus_handle();
        window.focus(&focus);
        let (requests, responses) = pipeline::worker();
        let mut viewer = Self {
            focus,
            album: Album::default(),
            view: Viewport {
                fit: settings.fit_on_open,
                fit_upscale: settings.fit_upscale,
                ..Viewport::default()
            },
            image: None,
            dimensions: [1., 1.],
            retired: vec![],
            resampling: resample::State::default(),
            turns: 0,
            generation: 0,
            loading: false,
            error: None,
            requests,
            responses,
            ticker: None,
            slide: false,
            next_slide: Instant::now(),
            last_scroll: Instant::now(),
            drag: None,
            force_reload: false,
            boundary: None,
            active_menu: None,
            menu_anchor: [0., menu_ui::HEIGHT],
            menu_selected: 0,
            info_visible: false,
            info_text: None,
            notice: None,
            pages: 1,
            settings,
            settings_draft: None,
            settings_tab: settings_ui::Tab::Navigation,
            settings_selected: 0,
            settings_selector: None,
            settings_scroll: ScrollHandle::new(),
            settings_scroll_drag: None,
            capture_binding: None,
            settings_error,
        };
        let executor = cx.background_executor().clone();
        viewer.ticker = Some(cx.spawn(async move |entity, cx| {
            loop {
                executor.timer(Duration::from_millis(8)).await;
                if entity.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        }));
        if let Some(path) = path {
            viewer.open(path, cx);
        }
        viewer
    }

    fn area(window: &Window) -> [f32; 2] {
        let size = window.viewport_size();
        [
            f32::from(size.width).max(1.),
            (f32::from(size.height) - menu_ui::HEIGHT - BAR - FOOTER).max(1.),
        ]
    }

    fn clear_image(&mut self) {
        self.resampling.clear(&mut self.retired);
        if let Some(image) = self.image.take() {
            self.retired.push(image);
        }
    }

    fn open(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        self.resolve_album(
            move || {
                let folder = path.is_dir();
                let mut album = Album::open(&path)?;
                album.sort(&settings);
                if folder {
                    album.index = 0;
                }
                Ok(album)
            },
            cx,
        );
    }

    fn resolve_album(
        &mut self,
        work: impl FnOnce() -> Result<Album, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.generation += 1;
        let generation = self.generation;
        self.requests.cancel(generation);
        self.boundary = None;
        self.loading = true;
        self.error = None;
        let task = cx.background_executor().spawn(async move { work() });
        cx.spawn(async move |viewer, cx| {
            let result = task.await;
            let _ = viewer.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(album) => {
                        this.album = album;
                        this.load(true, cx);
                    }
                    Err(error) => {
                        this.loading = false;
                        this.error = Some(error);
                        this.slide = false;
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn load(&mut self, reset: bool, cx: &mut Context<Self>) {
        let Some(path) = self.album.current().map(PathBuf::from) else {
            self.clear_image();
            self.generation += 1;
            self.requests.cancel(self.generation);
            self.loading = false;
            self.error = None;
            cx.notify();
            return;
        };
        self.clear_image();
        self.pages = 1;
        if reset {
            self.turns = 0;
            if !self.settings.lock_zoom {
                self.view = self.initial_view();
            }
        }
        self.drag = None;
        self.boundary = None;
        self.generation += 1;
        self.loading = true;
        self.error = None;
        self.notice = None;
        if self
            .requests
            .send(pipeline::Request {
                generation: self.generation,
                path,
                turns: self.turns,
                neighbours: if self.settings.prefetch {
                    self.album.neighbours()
                } else {
                    vec![]
                },
                reload: std::mem::take(&mut self.force_reload),
                second: if self.settings.manga {
                    self.album.paths.get(self.album.index + 1).cloned()
                } else {
                    None
                },
                rtl: self.settings.right_to_left,
                landscape_single: self.settings.landscape_single,
            })
            .is_err()
        {
            self.loading = false;
            self.error = Some(tr!("loader-stopped"));
        }
        cx.notify();
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.resampling.tick(&mut self.retired) {
            cx.notify();
        }
        while let Ok(response) = self.responses.try_recv() {
            if response.generation != self.generation {
                continue;
            }
            self.loading = false;
            match response.result {
                Ok(decoded) => {
                    self.dimensions = decoded.dimensions;
                    self.pages = decoded.pages;
                    self.clear_image();
                    self.image = Some(decoded.image.clone());
                    if std::env::var_os("NAGAME_PROFILE").is_some() {
                        eprintln!(
                            "load {} ms cached={} complete={}",
                            response.elapsed_ms, response.cached, response.complete
                        );
                    }
                    self.next_slide = Instant::now()
                        + Duration::from_secs(u64::from(self.settings.slideshow_seconds));
                }
                Err(error) => {
                    self.error = Some(error);
                    self.slide = false;
                }
            }
            cx.notify();
        }
        if self.slide
            && !self.loading
            && self.album.paths.len() > 1
            && Instant::now() >= self.next_slide
        {
            if self.album.index + self.pages >= self.album.paths.len() {
                self.at_boundary(1, cx);
            } else {
                self.album.step(self.pages as isize);
                self.load(true, cx);
            }
        }
    }

    fn prompt(&mut self, cx: &mut Context<Self>) {
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(tr!("open-image").into()),
        });
        cx.spawn(async move |entity, cx| {
            let result = selection.await;
            let _ = entity.update(cx, |this, cx| match result {
                Ok(Ok(Some(paths))) => {
                    if let Some(path) = paths.into_iter().next() {
                        this.open(path, cx);
                    }
                }
                Ok(Ok(None)) => {}
                Ok(Err(error)) => {
                    this.error = Some(error.to_string());
                    cx.notify();
                }
                Err(error) => {
                    this.error = Some(error.to_string());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        if !matches!(command, Command::Menu(_)) {
            self.active_menu = None;
        }
        match command {
            Command::Menu(menu) => {
                self.active_menu = if self.active_menu == Some(menu) {
                    None
                } else {
                    Some(menu)
                };
                self.menu_selected = 0;
                self.menu_anchor = [
                    match menu {
                        menu_ui::Menu::File => 0.,
                        menu_ui::Menu::List => 82.,
                        menu_ui::Menu::View => 164.,
                        menu_ui::Menu::Other => 246.,
                    },
                    menu_ui::HEIGHT,
                ];
            }
            Command::OpenFolder => self.prompt_folder(cx),
            Command::SaveAs => self.save_as(cx),
            Command::CopyImage => self.copy_image(cx),
            Command::CopyPath => {
                if let Some(path) = self.album.current() {
                    cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
                    self.notice = Some(tr!("path-copied"));
                }
            }
            Command::Reveal => {
                if !self.album.directory.as_os_str().is_empty() {
                    cx.open_with_system(&self.album.directory);
                }
            }
            Command::Info => self.show_information(cx),
            Command::ReadingDirection => {
                let mut settings = self.settings.clone();
                settings.right_to_left = !settings.right_to_left;
                self.settings_draft = Some(settings);
                self.apply_settings(cx);
            }
            Command::Sort(sort) => {
                let mut settings = self.settings.clone();
                settings.sort = sort;
                self.settings_draft = Some(settings);
                self.apply_settings(cx);
            }
            Command::ReverseSort => {
                let mut settings = self.settings.clone();
                settings.reverse = !settings.reverse;
                self.settings_draft = Some(settings);
                self.apply_settings(cx);
            }
            Command::Settings => {
                self.settings_selector = None;
                self.settings_scroll_drag = None;
                self.slide = false;
                self.settings_draft = Some(self.settings.clone());
                self.capture_binding = None;
            }
            Command::SaveSettings => {
                self.settings_selector = None;
                self.capture_binding = None;
                self.apply_settings(cx);
            }
            Command::CancelSettings => {
                self.settings_selector = None;
                self.settings_scroll_drag = None;
                self.settings_draft = None;
                self.capture_binding = None;
                self.settings_error = None;
            }
            Command::ResetSettings => {
                self.settings_selector = None;
                self.capture_binding = None;
                self.settings_draft = Some(Settings::default());
                self.settings_error = None;
            }
            Command::Manga => {
                let mut settings = self.settings.clone();
                settings.manga = !settings.manga;
                self.settings_draft = Some(settings);
                self.apply_settings(cx);
            }
            Command::SkipPrevious | Command::SkipNext => {
                let delta = if matches!(command, Command::SkipPrevious) {
                    -(self.settings.skip_count as isize)
                } else {
                    self.settings.skip_count as isize
                };
                if !self.album.paths.is_empty() {
                    self.album.index = (self.album.index as isize + delta)
                        .clamp(0, self.album.paths.len() as isize - 1)
                        as usize;
                    self.load(true, cx);
                }
            }
            Command::Open => self.prompt(cx),
            Command::PreviousFolder
            | Command::NextFolder
            | Command::PreviousSibling
            | Command::NextSibling
            | Command::Parent => {
                let album = self.album.clone();
                let settings = self.settings.clone();
                self.resolve_album(
                    move || {
                        let previous =
                            matches!(command, Command::PreviousFolder | Command::PreviousSibling);
                        let mut target = match command {
                            Command::Parent => album.parent(),
                            _ => album.folder(
                                if previous { -1 } else { 1 },
                                matches!(command, Command::PreviousSibling | Command::NextSibling),
                            ),
                        }?;
                        target.sort(&settings);
                        target.index = if (matches!(command, Command::Parent)
                            && !settings.parent_folder_first)
                            || (previous && !settings.previous_folder_first)
                        {
                            target.paths.len().saturating_sub(1)
                        } else {
                            0
                        };
                        Ok(target)
                    },
                    cx,
                );
            }

            Command::Dismiss => {
                self.boundary = None;
            }
            Command::Wrap => {
                if let Some(direction) = self.boundary.take() {
                    self.album.index = if direction > 0 {
                        0
                    } else {
                        self.album.paths.len().saturating_sub(1)
                    };
                    self.load(true, cx);
                }
            }
            Command::Previous | Command::Next => {
                let direction = if matches!(command, Command::Next) {
                    1
                } else {
                    -1
                };
                if !self.album.paths.is_empty()
                    && ((direction > 0 && self.album.index + self.pages >= self.album.paths.len())
                        || (direction < 0 && self.album.index == 0))
                {
                    self.at_boundary(direction, cx);
                } else {
                    if direction < 0 && self.settings.manga {
                        let mut album = self.album.clone();
                        let landscape_single = self.settings.landscape_single;
                        self.resolve_album(
                            move || {
                                let mut step = 2.min(album.index);
                                if landscape_single {
                                    for offset in 1..=step {
                                        let [w, h] = decoder::dimensions(
                                            &album.paths[album.index - offset],
                                        )?;
                                        if w > h {
                                            step = 1;
                                            break;
                                        }
                                    }
                                }
                                album.index = album.index.saturating_sub(step);
                                Ok(album)
                            },
                            cx,
                        );
                    } else {
                        self.album.step(direction * self.pages as isize);
                        self.load(true, cx);
                    }
                }
            }
            Command::First | Command::Last => {
                self.album.index = if matches!(command, Command::First) {
                    0
                } else {
                    self.album.paths.len().saturating_sub(1)
                };
                self.load(true, cx);
            }
            Command::ZoomIn | Command::ZoomOut => {
                let factor = if matches!(command, Command::ZoomIn) {
                    1.25
                } else {
                    0.8
                };
                self.view
                    .zoom_by(factor, self.dimensions, Self::area(window));
            }
            Command::Fit => {
                self.view = Viewport {
                    fit: true,
                    fit_upscale: self.settings.fit_upscale,
                    ..Viewport::default()
                }
            }
            Command::Actual => {
                self.view = Viewport {
                    fit: false,
                    zoom: 1.,
                    fit_upscale: self.settings.fit_upscale,
                    pan: [0.; 2],
                }
            }
            Command::Rotate => {
                self.turns = (self.turns + 1) % 4;
                self.view = self.initial_view();
                self.load(false, cx);
            }
            Command::Fullscreen => window.toggle_fullscreen(),
            Command::Slide => {
                self.slide = !self.slide && !self.album.paths.is_empty();
                self.next_slide = Instant::now()
                    + Duration::from_secs(u64::from(self.settings.slideshow_seconds));
            }
            Command::Reload => {
                self.force_reload = true;
                if let Some(path) = self.album.current().map(PathBuf::from).or_else(|| {
                    (!self.album.directory.as_os_str().is_empty())
                        .then(|| self.album.directory.clone())
                }) {
                    self.open(path, cx);
                }
            }
        }
        cx.notify();
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let mods = &event.keystroke.modifiers;
        if self.settings_draft.is_some() {
            self.settings_key(event, window, cx);
            return;
        }
        if self.active_menu.is_some() {
            self.menu_key(key, window, cx);
            return;
        }
        if self.info_visible {
            if matches!(key, "escape" | "enter") {
                self.info_visible = false;
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        if let Some(action) = self
            .settings
            .shortcuts
            .get(&event.keystroke.unparse())
            .copied()
        {
            self.command(self.shortcut_command(action), window, cx);
            cx.stop_propagation();
            return;
        }
        if !(mods.control || mods.platform || mods.alt)
            && (matches!(key, "pageup" | "pagedown" | "home" | "end")
                || (mods.shift && matches!(key, "left" | "right" | "up" | "down")))
        {
            let area = Self::area(window);
            let axis = if matches!(key, "left" | "right")
                || (mods.shift && matches!(key, "pageup" | "pagedown" | "home" | "end"))
            {
                0
            } else {
                1
            };
            let offset = match key {
                "home" => f32::MAX,
                "end" => f32::MIN,
                "pageup" => area[axis] * 0.5,
                "pagedown" => -area[axis] * 0.5,
                "left" | "up" => 32.,
                _ => -32.,
            };
            self.view.pan[axis] += offset;
            self.view.clamp(self.dimensions, area);
            cx.notify();
            cx.stop_propagation();
            return;
        }
        let command = match key {
            "f10" => Some(Command::Menu(menu_ui::Menu::File)),
            "escape" => {
                self.slide = false;
                self.boundary = None;
                if window.is_fullscreen() {
                    window.toggle_fullscreen();
                }
                cx.notify();
                None
            }
            _ => None,
        };
        if let Some(command) = command {
            self.command(command, window, cx);
            cx.stop_propagation();
        }
    }

    fn shortcut_command(&self, action: bindings::Shortcut) -> Command {
        use bindings::Shortcut::*;
        match action {
            Open => Command::Open,
            OpenFolder => Command::OpenFolder,
            Previous => {
                if self.reversed_manga_controls() {
                    Command::Next
                } else {
                    Command::Previous
                }
            }
            Next => {
                if self.reversed_manga_controls() {
                    Command::Previous
                } else {
                    Command::Next
                }
            }
            Advance => Command::Next,
            First => Command::First,
            Last => Command::Last,
            PreviousFolder => Command::PreviousFolder,
            NextFolder => Command::NextFolder,
            PreviousSibling => Command::PreviousSibling,
            NextSibling => Command::NextSibling,
            Parent => Command::Parent,
            SkipPrevious => Command::SkipPrevious,
            SkipNext => Command::SkipNext,
            ZoomIn => Command::ZoomIn,
            ZoomOut => Command::ZoomOut,
            Fit => Command::Fit,
            Actual => Command::Actual,
            Rotate => Command::Rotate,
            Fullscreen => Command::Fullscreen,
            Slide => Command::Slide,
            Manga => Command::Manga,
            Settings => Command::Settings,
            Info => Command::Info,
            Reload => Command::Reload,
            SaveAs => Command::SaveAs,
            CopyImage => Command::CopyImage,
            CopyPath => Command::CopyPath,
        }
    }

    fn reversed_manga_controls(&self) -> bool {
        self.settings.manga && self.settings.right_to_left && self.settings.manga_reverse_controls
    }

    fn initial_view(&self) -> Viewport {
        Viewport {
            fit: self.settings.fit_on_open,
            fit_upscale: self.settings.fit_upscale,
            ..Viewport::default()
        }
    }

    fn at_boundary(&mut self, direction: isize, cx: &mut Context<Self>) {
        match self.settings.boundary {
            BoundaryAction::Ask => {
                self.slide = false;
                self.boundary = Some(direction);
            }
            BoundaryAction::Stop => {
                self.slide = false;
            }
            BoundaryAction::Wrap => {
                self.album.index = if direction > 0 {
                    0
                } else {
                    self.album.paths.len().saturating_sub(1)
                };
                self.load(true, cx);
            }
            BoundaryAction::Folder => {
                let album = self.album.clone();
                let settings = self.settings.clone();
                self.resolve_album(
                    move || {
                        let mut target = album.folder(direction, false)?;
                        target.sort(&settings);
                        target.index = if direction < 0 && !settings.previous_folder_first {
                            target.paths.len().saturating_sub(1)
                        } else {
                            0
                        };
                        Ok(target)
                    },
                    cx,
                );
            }
        }
        cx.notify();
    }

    fn apply_settings(&mut self, cx: &mut Context<Self>) {
        let Some(mut settings) = self.settings_draft.clone() else {
            return;
        };
        settings.normalize();
        if let Err(error) = Settings::path().and_then(|path| settings.save(&path)) {
            self.settings_error = Some(error);
            cx.notify();
            return;
        }
        let album = self.album.clone();
        let sorting = settings.clone();
        i18n::activate(settings.language);
        self.settings = settings;
        self.settings_draft = None;
        self.settings_error = None;
        self.resolve_album(
            move || {
                let mut album = album;
                album.sort(&sorting);
                Ok(album)
            },
            cx,
        );
    }

    fn button<L: Into<SharedString>>(
        &self,
        label: L,
        command: Command,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<L> {
        let label = label.into();
        div()
            .id(SharedString::from(format!("{command:?}")))
            .flex_none()
            .px_3()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .hover(|style| style.bg(rgb(0x363a42)))
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| this.command(command, window, cx)))
    }
}

impl Render for Viewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.retired.is_empty() {
            // Rendering can run several times before GPUI submits its atlas uploads.
            // Removing a texture here invalidates those queued uploads. Keep it
            // through presentation and one additional frame so Blade has also
            // synchronized the previous GPU submission before releasing it.
            let retired = std::mem::take(&mut self.retired);
            let viewer = cx.weak_entity();
            window.on_next_frame(move |window, _| {
                window.refresh();
                window.on_next_frame(move |window, cx| {
                    let current = viewer.upgrade().map(|viewer| {
                        let viewer = viewer.read(cx);
                        (
                            viewer.image.as_ref().map(|image| image.id),
                            viewer.resampling.ready_id(),
                        )
                    });
                    for image in retired {
                        // The decoded-image cache may have selected this same
                        // image again before cleanup. Its atlas entry is live.
                        if !current.is_some_and(|(source, scaled)| {
                            source == Some(image.id) || scaled == Some(image.id)
                        }) {
                            let _ = window.drop_image(image);
                        }
                    }
                });
            });
        }
        let area = Self::area(window);
        self.view.clamp(self.dimensions, area);
        let scale = self.view.scale(self.dimensions, area);
        let device_scale = window.scale_factor();
        let target = [
            (self.dimensions[0] * scale * device_scale).round().max(1.) as u32,
            (self.dimensions[1] * scale * device_scale).round().max(1.) as u32,
        ];
        let algorithm = if scale * device_scale < 1. {
            self.settings.shrink_algorithm
        } else {
            self.settings.enlarge_algorithm
        };
        let display_image = self.image.clone().map(|image| {
            self.resampling
                .prepare(image, target, algorithm, &mut self.retired)
        });
        let path = self.album.current();
        let filename = path
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .or_else(|| {
                (!self.album.directory.as_os_str().is_empty())
                    .then(|| self.album.directory.display().to_string())
            });
        window.set_window_title(&format!(
            "{}Nagame",
            filename
                .as_ref()
                .map(|n| format!("{n} — "))
                .unwrap_or_default()
        ));
        let mut status = if self.loading {
            tr!("loading")
        } else if let Some(error) = &self.error {
            i18n::error_status(error)
        } else if let Some(error) = &self.settings_error {
            i18n::settings_error_status(error)
        } else if let Some(notice) = &self.notice {
            notice.clone()
        } else if self.image.is_some() {
            let page_range = if self.pages > 1 {
                format!("{}–{}", self.album.index + 1, self.album.index + self.pages)
            } else {
                (self.album.index + 1).to_string()
            };
            i18n::image_status(i18n::ImageStatus {
                page_range: &page_range,
                total: self.album.paths.len(),
                dimensions: self.dimensions,
                percent: scale * 100.,
                fit: self.view.fit,
                filename: &filename.unwrap_or_default(),
                slideshow: self.slide,
            })
        } else {
            tr!("status-open-hint")
        };

        if self.resampling.fallback {
            status.push_str(&tr!("resampling-fallback"));
        } else if let Some(error) = &self.resampling.error {
            status.push_str(&i18n::resampling_error(error));
        } else if self.resampling.pending() {
            status.push_str(&tr!("resampling-pending"));
        }
        let content = if let Some(image) = &display_image {
            let w = target[0] as f32 / device_scale;
            let h = target[1] as f32 / device_scale;
            img(image.clone())
                .id(("photo", self.generation as usize))
                .absolute()
                .left(px((((area[0] - w) / 2. + self.view.pan[0]) * device_scale)
                    .round()
                    / device_scale))
                .top(px((((area[1] - h) / 2. + self.view.pan[1]) * device_scale)
                    .round()
                    / device_scale))
                .w(px(w))
                .h(px(h))
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .flex_col()
                .justify_center()
                .items_center()
                .gap_3()
                .text_color(rgb(0x9299a5))
                .child(div().text_3xl().child(if self.loading {
                    tr!("loading")
                } else {
                    if self.album.directory.as_os_str().is_empty() {
                        "Nagame".to_owned()
                    } else {
                        tr!("empty-folder")
                    }
                }))
                .child(tr!("empty-hint"))
                .child(tr!("key-hint"))
                .into_any_element()
        };

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(self.settings.background))
            .text_color(rgb(0xe3e6ec))
            .text_sm()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                if this.settings_draft.is_some() || this.info_visible {
                    cx.stop_propagation();
                    return;
                }
                if let Some(path) = paths.paths().first() {
                    this.open(path.clone(), cx);
                }
            }))
            .child(self.menu_bar(cx))
            .child(
                div()
                    .id("toolbar")
                    .h(px(BAR))
                    .flex_none()
                    .flex()
                    .overflow_x_scroll()
                    .items_center()
                    .px_2()
                    .gap_1()
                    .bg(rgb(0x25282f))
                    .child(self.button(tr!("open"), Command::Open, cx))
                    .child(self.button(tr!("settings"), Command::Settings, cx))
                    .child(self.button(tr!("parent"), Command::Parent, cx))
                    .child(self.button(tr!("previous-folder-short"), Command::PreviousFolder, cx))
                    .child(self.button(tr!("next-folder-short"), Command::NextFolder, cx))
                    .child(self.button(
                        "〈",
                        if self.reversed_manga_controls() {
                            Command::Next
                        } else {
                            Command::Previous
                        },
                        cx,
                    ))
                    .child(self.button(
                        "〉",
                        if self.reversed_manga_controls() {
                            Command::Previous
                        } else {
                            Command::Next
                        },
                        cx,
                    ))
                    .child(self.button("−", Command::ZoomOut, cx))
                    .child(self.button("＋", Command::ZoomIn, cx))
                    .child(self.button(tr!("fit"), Command::Fit, cx))
                    .child(self.button("100%", Command::Actual, cx))
                    .child(self.button(tr!("rotate-right"), Command::Rotate, cx))
                    .child(self.button(tr!("fullscreen"), Command::Fullscreen, cx))
                    .child(self.button(
                        if self.slide {
                            tr!("stop")
                        } else {
                            tr!("slideshow-short")
                        },
                        Command::Slide,
                        cx,
                    )),
            )
            .child(
                div()
                    .id("viewport")
                    .relative()
                    .w_full()
                    .h(px(area[1]))
                    .overflow_hidden()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, _| {
                            window.focus(&this.focus);
                            this.drag = Some((event.position, this.view.pan));
                            if event.click_count == 2 {
                                window.toggle_fullscreen();
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            if this.settings_draft.is_none() && !this.info_visible {
                                this.active_menu = Some(menu_ui::Menu::View);
                                this.menu_selected = 0;
                                this.menu_anchor = [
                                    f32::from(event.position.x).clamp(
                                        0.,
                                        (f32::from(window.viewport_size().width) - 300.).max(0.),
                                    ),
                                    f32::from(event.position.y).min(
                                        (f32::from(window.viewport_size().height) - 350.)
                                            .max(menu_ui::HEIGHT),
                                    ),
                                ];
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.drag = None),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                        if event.pressed_button != Some(MouseButton::Left) {
                            this.drag = None;
                            return;
                        }
                        if let Some((origin, pan)) = this.drag {
                            this.view.pan = [
                                pan[0] + f32::from(event.position.x - origin.x),
                                pan[1] + f32::from(event.position.y - origin.y),
                            ];
                            this.view.clamp(this.dimensions, Self::area(window));
                            cx.notify();
                        }
                    }))
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                        if this.settings_draft.is_some() {
                            cx.stop_propagation();
                            return;
                        }
                        let delta = f32::from(event.delta.pixel_delta(px(24.)).y);
                        if delta == 0. {
                            return;
                        }
                        if event.modifiers.control != this.settings.wheel_zoom {
                            this.view.zoom_by(
                                if delta > 0. { 1.1 } else { 1. / 1.1 },
                                this.dimensions,
                                Self::area(window),
                            );
                            cx.notify();
                        } else if this.last_scroll.elapsed() > Duration::from_millis(140) {
                            this.last_scroll = Instant::now();
                            this.command(
                                if delta > 0. {
                                    Command::Previous
                                } else {
                                    Command::Next
                                },
                                window,
                                cx,
                            );
                        }
                        cx.stop_propagation();
                    }))
                    .child(content),
            )
            .child(
                div()
                    .h(px(FOOTER))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px_3()
                    .bg(rgb(0x25282f))
                    .overflow_hidden()
                    .text_xs()
                    .child(status),
            )
            .when(self.active_menu.is_some(), |root| {
                root.child(self.menu_panel(cx))
            })
            .when(self.info_visible, |root| {
                root.child(self.information_panel(cx))
            })
            .when(self.settings_draft.is_some(), |root| {
                root.child(self.settings_panel(cx))
            })
            .when_some(
                self.boundary.filter(|_| self.settings_draft.is_none()),
                |root, direction| {
                    root.child(
                        div()
                            .absolute()
                            .top(px(100.))
                            .left(px(100.))
                            .p_4()
                            .rounded_md()
                            .bg(rgb(0x30343c))
                            .border_1()
                            .border_color(rgb(0x707785))
                            .child(if direction > 0 {
                                tr!("last-image-prompt")
                            } else {
                                tr!("first-image-prompt")
                            })
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .mt_3()
                                    .child(self.button(
                                        if direction > 0 {
                                            tr!("go-next-folder")
                                        } else {
                                            tr!("go-previous-folder")
                                        },
                                        if direction > 0 {
                                            Command::NextFolder
                                        } else {
                                            Command::PreviousFolder
                                        },
                                        cx,
                                    ))
                                    .child(self.button(
                                        if direction > 0 {
                                            tr!("wrap-first")
                                        } else {
                                            tr!("wrap-last")
                                        },
                                        Command::Wrap,
                                        cx,
                                    ))
                                    .child(self.button(tr!("cancel"), Command::Dismiss, cx)),
                            ),
                    )
                },
            )
    }
}

fn main() {
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    if path
        .as_deref()
        .is_some_and(|p| p == std::path::Path::new("--help") || p == std::path::Path::new("-h"))
    {
        let language = Settings::path()
            .and_then(|path| Settings::read(&path))
            .map(|settings| settings.language)
            .unwrap_or_default();
        i18n::activate(language);
        println!("{}", tr!("cli-help"));
        return;
    }
    Application::new().run(move |cx: &mut App| {
        cx.on_window_closed(|cx| {
            // X11 invokes this hook while borrowing its event-loop state.
            // Quit on a later turn, after that borrow has been released.
            let executor = cx.background_executor().clone();
            cx.spawn(async move |cx| {
                executor.timer(Duration::from_millis(1)).await;
                let _ = cx.update(|cx| {
                    if cx.windows().is_empty() {
                        cx.quit();
                    }
                });
            })
            .detach();
        })
        .detach();
        let result = cx.open_window(
            WindowOptions {
                app_id: Some("nagame".into()),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1000.), px(740.)),
                    cx,
                ))),
                window_min_size: Some(size(px(1000.), px(300.))),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Viewer::new(path, window, cx)),
        );
        match result {
            Ok(_handle) =>
            {
                #[cfg(feature = "ui-smoke")]
                if std::env::var_os("NAGAME_UI_SMOKE").is_some() {
                    ui_smoke::start(_handle, cx);
                }
            }
            Err(error) => {
                eprintln!("{}", i18n::window_open_error(&error.to_string()));
                cx.quit();
            }
        }
        cx.activate(true);
    });
}
