//! 設定一覧のスクロールバー。ScrollHandleとバーの位置を相互変換する。

//! Always-visible settings scrollbar. Geometry is read at paint time, after layout.
use crate::Viewer;
use gpui::{prelude::*, *};

#[derive(Clone, Copy)]
struct Geometry {
    top: f32,
    thumb: f32,
    travel: f32,
    maximum: f32,
}
impl Geometry {
    fn new(height: f32, viewport: f32, maximum: f32, offset: f32) -> Self {
        let maximum = maximum.max(0.);
        let thumb =
            (height * viewport / (viewport + maximum).max(1.)).clamp(24_f32.min(height), height);
        let travel = (height - thumb).max(0.);
        Self {
            top: if maximum > 0. {
                (-offset / maximum).clamp(0., 1.) * travel
            } else {
                0.
            },
            thumb,
            travel,
            maximum,
        }
    }
    fn offset(self, top: f32) -> f32 {
        if self.travel > 0. {
            -(top / self.travel).clamp(0., 1.) * self.maximum
        } else {
            0.
        }
    }
}
impl Viewer {
    pub(super) fn settings_scrollbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let handle = self.settings_scroll.clone();
        div()
            .w(px(16.))
            .h_full()
            .flex_none()
            .cursor_pointer()
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let height = f32::from(bounds.size.height).max(0.);
                        let geometry = Geometry::new(
                            height,
                            f32::from(handle.bounds().size.height),
                            f32::from(handle.max_offset().height),
                            f32::from(handle.offset().y),
                        );
                        window.paint_quad(fill(bounds, rgb(0x171b23)));
                        window.paint_quad(fill(
                            Bounds::new(
                                point(bounds.left() + px(3.), bounds.top() + px(geometry.top)),
                                size(px(10.), px(geometry.thumb)),
                            ),
                            rgb(0x9aa9c1),
                        ));
                        let entity_down = entity.clone();
                        let scroll = handle.clone();
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                            if phase != DispatchPhase::Bubble
                                || event.button != MouseButton::Left
                                || !bounds.contains(&event.position)
                            {
                                return;
                            }
                            let y = f32::from(event.position.y - bounds.top());
                            let grab = if y >= geometry.top && y <= geometry.top + geometry.thumb {
                                y - geometry.top
                            } else {
                                geometry.thumb / 2.
                            };
                            scroll.set_offset(point(px(0.), px(geometry.offset(y - grab))));
                            entity_down.update(cx, |viewer, cx| {
                                viewer.settings_scroll_drag = Some(grab);
                                cx.notify();
                            });
                            cx.stop_propagation();
                        });
                        let entity_up = entity.clone();
                        window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                            entity_up.update(cx, |viewer, _| viewer.settings_scroll_drag = None);
                        });
                        let entity_move = entity.clone();
                        let scroll = handle.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
                            if !event.dragging() {
                                return;
                            }
                            entity_move.update(cx, |viewer, cx| {
                                if let Some(grab) = viewer.settings_scroll_drag {
                                    scroll.set_offset(point(
                                        px(0.),
                                        px(geometry.offset(
                                            f32::from(event.position.y - bounds.top()) - grab,
                                        )),
                                    ));
                                    cx.notify();
                                }
                            });
                        });
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::Geometry;
    #[test]
    fn thumb_tracks_offset_and_drag_reaches_both_ends() {
        let g = Geometry::new(300., 300., 900., -450.);
        assert_eq!(g.thumb, 75.);
        assert_eq!(g.top, 112.5);
        assert_eq!(g.offset(g.top), -450.);
        assert_eq!(g.offset(-20.), 0.);
        assert_eq!(g.offset(300.), -900.);
        let g = Geometry::new(300., 300., 0., 0.);
        assert_eq!(g.thumb, 300.);
        assert_eq!(g.offset(100.), 0.);
    }
}
