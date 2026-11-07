//! Persist normal window geometry separately from user-editable settings.
use gpui::{Bounds, Pixels, point, px, size};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::Path};

/// GPUI's initial X11 coordinates can be overridden by smart placement. Send a
/// normal application EWMH request after mapping; never use this on Wayland.
pub fn restore_position(window: &gpui::Window, origin: gpui::Point<Pixels>) -> Result<(), String> {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        use x11rb::{
            connection::Connection,
            protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, EventMask},
        };
        let send = || -> Result<(), Box<dyn std::error::Error>> {
            let (connection, screen) = x11rb::connect(None)?;
            let root = connection.setup().roots[screen].root;
            // GPUI 0.2.2's X11 raw window handle is unimplemented. Nagame has
            // one window per process; identify only our own client by its PID.
            let clients = connection
                .intern_atom(false, b"_NET_CLIENT_LIST")?
                .reply()?
                .atom;
            let pid = connection.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom;
            let mut windows = connection
                .get_property(false, root, clients, AtomEnum::WINDOW, 0, 4096)?
                .reply()?
                .value32()
                .map(|values| values.collect::<Vec<_>>())
                .unwrap_or_default();
            if windows.is_empty() {
                windows = connection.query_tree(root)?.reply()?.children;
            }
            let mut id = None;
            for candidate in windows {
                if let Ok(reply) = connection
                    .get_property(false, candidate, pid, AtomEnum::CARDINAL, 0, 1)?
                    .reply()
                    && reply.value32().and_then(|mut values| values.next())
                        == Some(std::process::id())
                {
                    id = Some(candidate);
                    break;
                }
            }
            let id = id.ok_or_else(|| crate::tr!("window-state-x11-not-found"))?;
            let atom = connection
                .intern_atom(false, b"_NET_MOVERESIZE_WINDOW")?
                .reply()?
                .atom;
            let scale = window.scale_factor();
            // StaticGravity, x/y present, source=application. The WM still has
            // the final say; no pager privileges or compositor-specific rules.
            let flags = 10 | (1 << 8) | (1 << 9) | (1 << 12);
            let event = ClientMessageEvent::new(
                32,
                id,
                atom,
                [
                    flags,
                    (f32::from(origin.x) * scale).round() as i32 as u32,
                    (f32::from(origin.y) * scale).round() as i32 as u32,
                    0,
                    0,
                ],
            );
            connection
                .send_event(
                    false,
                    root,
                    EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                    event,
                )?
                .check()?;
            connection.flush()?;
            Ok(())
        };
        send().map_err(|error| error.to_string())?;
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    let _ = (window, origin);
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    pub size: [f32; 2],
    /// Only populated by backends which expose global window coordinates.
    pub position: Option<[f32; 2]>,
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            size: [1000., 740.],
            position: None,
            maximized: false,
        }
    }
}

impl WindowState {
    pub fn path() -> Result<std::path::PathBuf, String> {
        crate::settings::Settings::path().map(|path| path.with_file_name("window-state.json"))
    }

    pub fn load(path: &Path) -> Self {
        let mut state: Self = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        for (value, fallback) in state.size.iter_mut().zip(Self::default().size) {
            if !value.is_finite() || *value <= 0. {
                *value = fallback;
            }
        }
        state.position = state
            .position
            .filter(|position| position.iter().all(|value| value.is_finite()));
        state
    }

    /// The first display is the primary display. Clamp restored geometry so a
    /// disconnected monitor or changed scaling does not leave the window lost.
    pub fn restored_bounds(
        &self,
        displays: &[Bounds<Pixels>],
        position_supported: bool,
    ) -> Bounds<Pixels> {
        let fallback = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(740.)));
        let primary = displays.first().copied().unwrap_or(fallback);
        let requested = self
            .size
            .map(|v| if v.is_finite() && v > 0. { v } else { 0. });
        let requested = [
            if requested[0] > 0. {
                requested[0]
            } else {
                1000.
            },
            if requested[1] > 0. {
                requested[1]
            } else {
                740.
            },
        ];
        // Wayland outputs may not be announced when Application::run starts.
        // Let the compositor constrain the size once its first configure arrives.
        if displays.is_empty() {
            return Bounds::new(
                point(px(0.), px(0.)),
                size(
                    px(requested[0].clamp(1000., 16384.)),
                    px(requested[1].clamp(300., 16384.)),
                ),
            );
        }
        let position = self
            .position
            .filter(|p| position_supported && p.iter().all(|v| v.is_finite()));
        let mut display = primary;
        if !position_supported
            && (requested[0] > f32::from(primary.size.width)
                || requested[1] > f32::from(primary.size.height))
        {
            display = displays
                .iter()
                .find(|display| {
                    requested[0] <= f32::from(display.size.width)
                        && requested[1] <= f32::from(display.size.height)
                })
                .or_else(|| {
                    displays.iter().max_by(|a, b| {
                        (f32::from(a.size.width) * f32::from(a.size.height))
                            .total_cmp(&(f32::from(b.size.width) * f32::from(b.size.height)))
                    })
                })
                .copied()
                .unwrap_or(primary);
        }
        let mut overlap = 0.;
        if let Some([x, y]) = position {
            for candidate in displays {
                let width = ((x + requested[0]).min(candidate.right().into())
                    - x.max(candidate.left().into()))
                .max(0.);
                let height = ((y + requested[1]).min(candidate.bottom().into())
                    - y.max(candidate.top().into()))
                .max(0.);
                let area = width * height;
                if area > overlap {
                    display = *candidate;
                    overlap = area;
                }
            }
        }
        let available = [
            f32::from(display.size.width).max(1.),
            f32::from(display.size.height).max(1.),
        ];
        let width = requested[0].clamp(1000_f32.min(available[0]), available[0]);
        let height = requested[1].clamp(300_f32.min(available[1]), available[1]);
        let origin = if let Some([x, y]) = position.filter(|_| overlap > 0.) {
            point(
                px(x.clamp(display.left().into(), f32::from(display.right()) - width)),
                px(y.clamp(display.top().into(), f32::from(display.bottom()) - height)),
            )
        } else {
            point(
                display.left() + px((available[0] - width) / 2.),
                display.top() + px((available[1] - height) / 2.),
            )
        };
        Bounds::new(origin, size(px(width), px(height)))
    }

    pub fn record(
        &mut self,
        bounds: Bounds<Pixels>,
        maximized: bool,
        fullscreen: bool,
        position_supported: bool,
    ) {
        if fullscreen {
            return;
        }
        self.maximized = maximized;
        if maximized {
            return;
        }
        let measured = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
        if measured.iter().all(|v| v.is_finite() && *v > 0.) {
            self.size = measured;
        }
        let origin = [f32::from(bounds.origin.x), f32::from(bounds.origin.y)];
        if position_supported && origin.iter().all(|v| v.is_finite()) {
            self.position = Some(origin);
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other(crate::tr!("window-state-directory-missing")))?;
        fs::create_dir_all(parent)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(std::io::Error::other)?
            .as_nanos();
        let temporary = parent.join(format!(".window-state-{}-{stamp}.tmp", std::process::id()));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    #[test]
    fn restore_handles_monitors_scaling_and_wayland_position_limits() {
        let displays = [
            bounds(0., 0., 1920., 1080.),
            bounds(-1600., 0., 1600., 900.),
        ];
        let mut state = WindowState {
            size: [1100., 600.],
            position: Some([-1500., 60.]),
            maximized: false,
        };
        assert_eq!(
            state.restored_bounds(&displays, true),
            bounds(-1500., 60., 1100., 600.)
        );
        assert_eq!(
            state.restored_bounds(&displays, false),
            bounds(410., 240., 1100., 600.)
        );
        assert_eq!(
            state.restored_bounds(&displays[..1], true),
            bounds(410., 240., 1100., 600.)
        );
        assert_eq!(
            state.restored_bounds(&[], false),
            bounds(0., 0., 1100., 600.)
        );
        assert_eq!(
            state.restored_bounds(&[bounds(0., 0., 800., 600.), displays[0]], false),
            bounds(410., 240., 1100., 600.)
        );
        state.position = Some([1800., 1000.]);
        assert_eq!(
            state.restored_bounds(&displays, true),
            bounds(820., 480., 1100., 600.)
        );
        state.size = [10000., 10000.];
        assert_eq!(
            state.restored_bounds(&[bounds(0., 0., 800., 600.)], true),
            bounds(0., 0., 800., 600.)
        );
        state.size = [f32::NAN, -1.];
        state.position = Some([f32::INFINITY, 0.]);
        assert_eq!(
            state.restored_bounds(&displays, true),
            bounds(460., 170., 1000., 740.)
        );
    }

    #[test]
    fn normal_geometry_survives_maximize_fullscreen_and_restart() {
        let directory =
            std::env::temp_dir().join(format!("nagame-window-state-{}", std::process::id()));
        let path = directory.join("window-state.json");
        let mut state = WindowState::load(&path);
        state.record(bounds(120., 80., 1100., 600.), false, false, true);
        state.record(bounds(0., 0., 1920., 1080.), true, false, true);
        assert_eq!(state.size, [1100., 600.]);
        assert_eq!(state.position, Some([120., 80.]));
        assert!(state.maximized);
        state.record(bounds(0., 0., 1920., 1080.), false, true, true);
        assert!(state.maximized);
        state.save(&path).unwrap();
        assert_eq!(WindowState::load(&path), state);
        state.record(bounds(0., 0., 1200., 650.), false, false, false);
        assert_eq!(state.position, Some([120., 80.]));
        assert_eq!(state.size, [1200., 650.]);
        assert!(!state.maximized);
        fs::write(&path, b"broken JSON").unwrap();
        assert_eq!(WindowState::load(&path), WindowState::default());
        fs::remove_dir_all(directory).unwrap();
    }
}
