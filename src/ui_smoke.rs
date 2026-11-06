//! Opt-in real-window UI regression. Never enabled in a normal build.
use crate::{
    Viewer,
    bindings::Shortcut,
    settings::{BoundaryAction, Settings},
    settings_ui::Tab,
};
use gpui::*;
use std::time::Duration;

fn key(handle: WindowHandle<Viewer>, cx: &mut AsyncApp, value: &str) {
    let handle: AnyWindowHandle = handle.into();
    handle
        .update(cx, |_, window, cx| {
            window.dispatch_keystroke(Keystroke::parse(value).unwrap(), cx);
        })
        .unwrap();
}

pub fn start(handle: WindowHandle<Viewer>, cx: &mut App) {
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        if std::env::var_os("NAGAME_UI_STRESS").is_some() {
            executor.timer(Duration::from_millis(300)).await;
            for direction in ["right", "left", "right", "left"] {
                let before = handle.read_with(cx, |v, _| v.album.index).unwrap();
                for _ in 0..350 {
                    key(handle, cx, direction);
                    executor.timer(Duration::from_millis(8)).await;
                }
                executor.timer(Duration::from_millis(150)).await;
                handle
                    .read_with(cx, |v, _| {
                        assert!(v.error.is_none(), "{:?}", v.error);
                        assert!(v.image.is_some());
                        assert!(!v.loading);
                        assert!(if direction == "right" {
                            v.album.index > before
                        } else {
                            v.album.index < before
                        });
                    })
                    .unwrap();
                eprintln!("PASS: {direction}: 350 navigation events with rendered frames");
            }
            let any: AnyWindowHandle = handle.into();
            any.update(cx, |_, window, _| window.remove_window())
                .unwrap();
            return;
        }
        if std::env::var_os("NAGAME_UI_RESTART").is_some() {
            executor.timer(Duration::from_millis(300)).await;
            handle
                .read_with(cx, |viewer, _| {
                    assert_eq!(viewer.settings.boundary, BoundaryAction::Wrap);
                    assert!(viewer.settings.manga);
                    assert_eq!(
                        viewer.settings.shrink_algorithm,
                        crate::resample::Algorithm::Nearest
                    );
                    assert_eq!(
                        viewer.settings.enlarge_algorithm,
                        crate::resample::Algorithm::Nearest
                    );
                    assert_eq!(viewer.settings.shortcuts.get("q"), Some(&Shortcut::Rotate));
                    assert!(!viewer.settings.shortcuts.contains_key("r"));
                    assert_eq!(viewer.pages, 2);
                })
                .unwrap();
            eprintln!("PASS: saved settings restored after process restart");
            let any: AnyWindowHandle = handle.into();
            any.update(cx, |_, window, _| window.remove_window())
                .unwrap();
            return;
        }
        executor.timer(Duration::from_millis(300)).await;
        key(handle, cx, "ctrl-,");
        executor.timer(Duration::from_millis(100)).await;
        key(handle, cx, "right");
        key(handle, cx, "down");
        handle.read_with(cx, |v,_| {
            assert!(v.settings_selector.is_some());
            assert_eq!(v.settings_draft.as_ref().unwrap().boundary,BoundaryAction::Ask);
        }).unwrap();
        key(handle,cx,"escape");
        handle.read_with(cx, |v,_| {
            assert!(v.settings_selector.is_none());
            assert!(v.settings_draft.is_some());
            assert_eq!(v.settings_draft.as_ref().unwrap().boundary,BoundaryAction::Ask);
        }).unwrap();
        key(handle,cx,"space");key(handle,cx,"down");key(handle,cx,"enter");
        handle.read_with(cx, |v,_| {
            assert!(v.settings_selector.is_none());
            assert_eq!(v.settings_draft.as_ref().unwrap().boundary,BoundaryAction::Wrap);
            assert_eq!(v.settings.boundary,BoundaryAction::Ask);
        }).unwrap();
        key(handle, cx, "escape");
        handle
            .read_with(cx, |viewer, _| {
                assert_eq!(viewer.settings.boundary, BoundaryAction::Ask)
            })
            .unwrap();
        key(handle, cx, "ctrl-,");
        executor.timer(Duration::from_millis(100)).await;
        key(handle, cx, "right");
        key(handle, cx, "down");
        key(handle, cx, "enter");
        key(handle, cx, "enter");
        executor.timer(Duration::from_millis(150)).await;
        assert_eq!(
            Settings::read(&Settings::path().unwrap()).unwrap().boundary,
            BoundaryAction::Wrap
        );
        eprintln!("PASS: selector open, uncommitted highlight, Escape, selection, cancel, apply and disk persistence");
        key(handle, cx, "ctrl-,");
        executor.timer(Duration::from_millis(100)).await;
        key(handle, cx, "2");
        handle
            .read_with(cx, |viewer, _| assert!(viewer.settings_tab == Tab::Display))
            .unwrap();
        executor.timer(Duration::from_millis(100)).await;
        for _ in 0..4 {
            key(handle, cx, "down");
        }
        key(handle, cx, "space");
        handle
            .read_with(cx, |viewer, _| {
                assert!(viewer.settings_draft.as_ref().unwrap().manga)
            })
            .unwrap();
        key(handle, cx, "enter");
        executor.timer(Duration::from_millis(200)).await;
        handle
            .read_with(cx, |viewer, _| {
                assert!(viewer.settings.manga);
                assert_eq!(viewer.pages, 2);
                assert_eq!(viewer.dimensions, [800., 600.]);
            })
            .unwrap();
        eprintln!("PASS: display tab, manga toggle and real spread rendering");
        for (input, index, pages, dimensions) in [
            ("space", 2, 1, [800., 400.]),
            ("space", 3, 2, [800., 600.]),
            ("right", 2, 1, [800., 400.]),
            ("right", 0, 2, [800., 600.]),
            ("left", 2, 1, [800., 400.]),
            ("right", 0, 2, [800., 600.]),
        ] {
            key(handle, cx, input);
            executor.timer(Duration::from_millis(150)).await;
            handle
                .read_with(cx, |v, _| {
                    assert_eq!(v.album.index, index);
                    assert_eq!(v.pages, pages);
                    assert_eq!(v.dimensions, dimensions);
                    assert!(v.error.is_none());
                })
                .unwrap();
        }
        eprintln!("PASS: manga forward/backward, landscape boundaries and RTL controls");
        key(handle, cx, "i");
        handle
            .read_with(cx, |v, _| assert!(v.info_visible))
            .unwrap();
        key(handle, cx, "escape");
        key(handle, cx, "f10");
        handle
            .read_with(cx, |v, _| assert!(v.active_menu.is_some()))
            .unwrap();
        key(handle, cx, "right");
        key(handle, cx, "escape");
        key(handle, cx, "ctrl-,");
        key(handle, cx, "4");
        for _ in 0..Shortcut::ALL
            .iter()
            .position(|a| *a == Shortcut::Rotate)
            .unwrap()
        {
            key(handle, cx, "down");
        }
        executor.timer(Duration::from_millis(100)).await;
        handle
            .read_with(cx, |v, _| {
                assert!(f32::from(v.settings_scroll.max_offset().height) > 0.);
                assert!(f32::from(v.settings_scroll.offset().y) < 0.);
            })
            .unwrap();
        if std::env::var_os("NAGAME_UI_SNAPSHOT").is_some() {
            eprintln!("SNAPSHOT: settings keyboard");
            executor.timer(Duration::from_secs(3)).await;
        }
        key(handle, cx, "space");
        key(handle, cx, "q");
        key(handle, cx, "enter");
        executor.timer(Duration::from_millis(200)).await;
        key(handle, cx, "r");
        handle
            .read_with(cx, |v, _| assert_eq!(v.dimensions, [800., 600.]))
            .unwrap();
        key(handle, cx, "q");
        executor.timer(Duration::from_millis(200)).await;
        handle
            .read_with(cx, |v, _| assert_eq!(v.dimensions, [600., 800.]))
            .unwrap();
        assert_eq!(
            Settings::read(&Settings::path().unwrap())
                .unwrap()
                .shortcuts
                .get("q"),
            Some(&Shortcut::Rotate)
        );
        eprintln!("PASS: menus, image information, custom key capture and replacement dispatch");
        key(handle, cx, "ctrl-,");
        key(handle, cx, "2");
        for _ in 0..8 {
            key(handle, cx, "down");
        }
        executor.timer(Duration::from_millis(100)).await;
        key(handle, cx, "right");
        if std::env::var_os("NAGAME_UI_SELECTOR_SNAPSHOT").is_some() {
            eprintln!("SNAPSHOT: interpolation selector");
            executor.timer(Duration::from_secs(3)).await;
        }
        key(handle, cx, "down");
        key(handle, cx, "enter");
        key(handle, cx, "down");
        key(handle, cx, "right");
        key(handle, cx, "down");
        key(handle, cx, "enter");
        key(handle, cx, "enter");
        executor.timer(Duration::from_millis(150)).await;
        for _ in 0..6 {
            key(handle, cx, "-");
        }
        for _ in 0..100 {
            executor.timer(Duration::from_millis(10)).await;
            if handle
                .read_with(cx, |v, _| v.resampling.ready_id().is_some())
                .unwrap()
            {
                break;
            }
        }
        handle
            .read_with(cx, |v, _| {
                assert_eq!(
                    v.settings.shrink_algorithm,
                    crate::resample::Algorithm::Nearest
                );
                assert!(v.resampling.ready_id().is_some());
                assert_ne!(
                    v.resampling.ready_id(),
                    v.image.as_ref().map(|image| image.id)
                );
            })
            .unwrap();
        key(handle, cx, "q");
        executor.timer(Duration::from_millis(200)).await;
        eprintln!(
            "PASS: settings auto-scroll, persisted interpolation selection and display resampling"
        );
        for algorithm in [crate::resample::Algorithm::Linear,crate::resample::Algorithm::Cubic,crate::resample::Algorithm::Lanczos3] {
            let old=handle.read_with(cx,|v,_|v.resampling.ready_id()).unwrap();
            handle.update(cx,|v,_,cx| {
                v.settings.shrink_algorithm=algorithm;
                v.settings.enlarge_algorithm=algorithm;
                cx.notify();
            }).unwrap();
            for _ in 0..200 {
                executor.timer(Duration::from_millis(10)).await;
                if handle.read_with(cx,|v,_|v.resampling.ready_id().is_some() && v.resampling.ready_id()!=old).unwrap(){break;}
            }
            handle.read_with(cx,|v,_| {
                assert_ne!(v.resampling.ready_id(),old,"{algorithm:?} did not finish");
                let image=v.resampling.ready_image().expect("filtered display image missing");
                let pixels=image.as_bytes(0).unwrap().as_chunks::<4>().0;
                assert!(pixels.iter().all(|p|p[3]==255),"{algorithm:?}: opaque image lost alpha");
                assert!(pixels.iter().any(|p|p[0]>200 || p[2]>200),"{algorithm:?}: image lost color");
            }).unwrap();
            executor.timer(Duration::from_millis(100)).await;
            eprintln!("PASS: {algorithm:?} filtered display preserves opacity and color");
            if std::env::var_os("NAGAME_UI_FILTER_SNAPSHOT").is_some() {
                eprintln!("SNAPSHOT: {algorithm:?}");
                executor.timer(Duration::from_secs(3)).await;
            }
        }
        let any: AnyWindowHandle = handle.into();
        any.update(cx, |_, window, _| window.remove_window())
            .unwrap();
    })
    .detach();
}
