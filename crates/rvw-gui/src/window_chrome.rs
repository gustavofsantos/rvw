//! The window frame the app draws itself when the platform does not: the
//! minimize, maximize and close controls, the drag region of the top bar and
//! the invisible resize band along the edges. Everything here is inert when
//! `Window::window_decorations` is `Server`, which is always the case on macOS.

use gpui::prelude::*;
use gpui::{
    canvas, div, point, px, svg, AnyView, App, Bounds, CursorStyle, Decorations, DispatchPhase,
    Div, HitboxBehavior, MouseButton, MouseDownEvent, Pixels, Point, ResizeEdge, Rgba, Size,
    Stateful, Tiling, Window, WindowControlArea,
};

use crate::key_strip::HintCard;
use crate::theme;

/// Width of the invisible resize band along each edge.
const RESIZE_BAND: Pixels = px(6.);
/// Side of the square at each corner that resizes both ways.
const RESIZE_CORNER: Pixels = px(12.);
const CONTROL_SIZE: Pixels = px(28.);
const ICON_SIZE: Pixels = px(14.);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl Control {
    fn icon(self) -> &'static str {
        match self {
            Self::Minimize => "icons/window-minimize.svg",
            Self::Maximize => "icons/window-maximize.svg",
            Self::Restore => "icons/window-restore.svg",
            Self::Close => "icons/window-close.svg",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Minimize => "Minimize",
            Self::Maximize => "Maximize",
            Self::Restore => "Restore",
            Self::Close => "Close",
        }
    }
}

/// Which controls the platform supports; close is always there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Supported {
    pub minimize: bool,
    pub maximize: bool,
}

/// The controls to draw, left to right.
pub fn visible_controls(supported: Supported, maximized: bool) -> Vec<Control> {
    let mut controls = Vec::with_capacity(3);
    if supported.minimize {
        controls.push(Control::Minimize);
    }
    if supported.maximize {
        controls.push(if maximized {
            Control::Restore
        } else {
            Control::Maximize
        });
    }
    controls.push(Control::Close);
    controls
}

/// The edge or corner to resize from at `point`, or `None` in the middle of
/// the window and on a side that is tiled against the screen.
pub fn edge_at(
    point: Point<Pixels>,
    size: Size<Pixels>,
    band: Pixels,
    corner: Pixels,
    tiling: Tiling,
) -> Option<ResizeEdge> {
    let near_left = point.x < corner && !tiling.left;
    let near_right = point.x >= size.width - corner && !tiling.right;
    let near_top = point.y < corner && !tiling.top;
    let near_bottom = point.y >= size.height - corner && !tiling.bottom;
    match (near_left, near_right, near_top, near_bottom) {
        (true, _, true, _) => return Some(ResizeEdge::TopLeft),
        (_, true, true, _) => return Some(ResizeEdge::TopRight),
        (true, _, _, true) => return Some(ResizeEdge::BottomLeft),
        (_, true, _, true) => return Some(ResizeEdge::BottomRight),
        _ => {}
    }
    if point.y < band && !tiling.top {
        Some(ResizeEdge::Top)
    } else if point.y >= size.height - band && !tiling.bottom {
        Some(ResizeEdge::Bottom)
    } else if point.x < band && !tiling.left {
        Some(ResizeEdge::Left)
    } else if point.x >= size.width - band && !tiling.right {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}

/// The rectangles of the band, edges first and corners last so the corner
/// cursor wins where they overlap. Mirrors `edge_at`.
fn band_rects(
    size: Size<Pixels>,
    band: Pixels,
    corner: Pixels,
    tiling: Tiling,
) -> Vec<(ResizeEdge, Bounds<Pixels>)> {
    let rect = |x, y, w, h| Bounds::new(point(x, y), gpui::size(w, h));
    let (w, h) = (size.width, size.height);
    let mut rects = Vec::with_capacity(8);
    if !tiling.top {
        rects.push((ResizeEdge::Top, rect(px(0.), px(0.), w, band)));
    }
    if !tiling.bottom {
        rects.push((ResizeEdge::Bottom, rect(px(0.), h - band, w, band)));
    }
    if !tiling.left {
        rects.push((ResizeEdge::Left, rect(px(0.), px(0.), band, h)));
    }
    if !tiling.right {
        rects.push((ResizeEdge::Right, rect(w - band, px(0.), band, h)));
    }
    let corners = [
        (
            ResizeEdge::TopLeft,
            px(0.),
            px(0.),
            tiling.top || tiling.left,
        ),
        (
            ResizeEdge::TopRight,
            w - corner,
            px(0.),
            tiling.top || tiling.right,
        ),
        (
            ResizeEdge::BottomLeft,
            px(0.),
            h - corner,
            tiling.bottom || tiling.left,
        ),
        (
            ResizeEdge::BottomRight,
            w - corner,
            h - corner,
            tiling.bottom || tiling.right,
        ),
    ];
    for (edge, x, y, tiled) in corners {
        if !tiled {
            rects.push((edge, rect(x, y, corner, corner)));
        }
    }
    rects
}

fn cursor_for(edge: ResizeEdge) -> CursorStyle {
    match edge {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
    }
}

fn is_client(window: &Window) -> bool {
    matches!(window.window_decorations(), Decorations::Client { .. })
}

/// The invisible resize band, to be the last child of the root so it sits
/// above everything. `None` when the platform draws the frame, or when the
/// window is maximized or full screen. The band's hitboxes are `Normal`, so
/// they do not take hover away from the content under them.
pub fn resize_layer(window: &Window) -> Option<impl IntoElement> {
    let Decorations::Client { tiling } = window.window_decorations() else {
        return None;
    };
    if window.is_maximized() || window.is_fullscreen() {
        return None;
    }
    Some(
        canvas(
            move |bounds, window, _cx| {
                band_rects(bounds.size, RESIZE_BAND, RESIZE_CORNER, tiling)
                    .into_iter()
                    .map(|(edge, rect)| {
                        let at = Bounds::new(bounds.origin + rect.origin, rect.size);
                        (edge, window.insert_hitbox(at, HitboxBehavior::Normal))
                    })
                    .collect::<Vec<_>>()
            },
            move |bounds, hitboxes, window, _cx| {
                for (edge, hitbox) in &hitboxes {
                    window.set_cursor_style(cursor_for(*edge), hitbox);
                }
                let window_size = bounds.size;
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                        return;
                    }
                    let found = edge_at(
                        event.position,
                        window_size,
                        RESIZE_BAND,
                        RESIZE_CORNER,
                        tiling,
                    );
                    if let Some(edge) = found {
                        window.start_window_resize(edge);
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .size_full(),
    )
}

/// Makes `bar` the window's drag handle: a press moves the window, a double
/// click toggles maximize, a right click opens the window menu. Children that
/// must keep their clicks are wrapped in [`no_drag`].
pub fn drag_region(bar: Div, window: &Window) -> Div {
    if !is_client(window) {
        return bar;
    }
    let controls = window.window_controls();
    bar.window_control_area(WindowControlArea::Drag)
        .on_mouse_down(MouseButton::Left, move |event, window, _| {
            if event.click_count == 2 {
                if controls.maximize {
                    window.zoom_window();
                }
            } else {
                window.start_window_move();
            }
        })
        .on_mouse_down(MouseButton::Right, move |event, window, _| {
            if controls.window_menu {
                window.show_window_menu(event.position);
            }
        })
}

/// Keeps a press on `child` from reaching the drag region behind it.
pub fn no_drag(child: impl IntoElement) -> Div {
    div()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(child)
}

/// The controls, or `None` when the platform draws them.
pub fn controls(window: &Window) -> Option<Div> {
    if !is_client(window) {
        return None;
    }
    let platform = window.window_controls();
    let supported = Supported {
        minimize: platform.minimize,
        maximize: platform.maximize,
    };
    let row = visible_controls(supported, window.is_maximized())
        .into_iter()
        .fold(div().flex().items_center().gap(px(4.)), |row, control| {
            row.child(control_button(control))
        });
    Some(row)
}

fn control_button(control: Control) -> Stateful<Div> {
    let closing = control == Control::Close;
    let (ink, wash) = if closing {
        let red = theme::rejected();
        (red, Rgba { a: 0.14, ..red })
    } else {
        (theme::muted(), theme::selected_panel())
    };
    let label = control.label();
    div()
        .id(label)
        .flex()
        .items_center()
        .justify_center()
        .size(CONTROL_SIZE)
        .rounded(theme::CONTROL_RADIUS)
        .cursor_pointer()
        .text_color(ink)
        .hover(move |style| style.bg(wash))
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_click(move |_, window, cx| match control {
            Control::Minimize => window.minimize_window(),
            Control::Maximize | Control::Restore => window.zoom_window(),
            Control::Close => cx.quit(),
        })
        .tooltip(move |_: &mut Window, cx: &mut App| -> AnyView {
            cx.new(|_| HintCard { text: label }).into()
        })
        .child(svg().path(control.icon()).size(ICON_SIZE).text_color(ink))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    const BAND: Pixels = px(6.);
    const CORNER: Pixels = px(12.);

    fn edge(x: f32, y: f32, tiling: Tiling) -> Option<ResizeEdge> {
        edge_at(
            point(px(x), px(y)),
            size(px(1000.), px(800.)),
            BAND,
            CORNER,
            tiling,
        )
    }

    #[test]
    fn the_middle_of_the_window_is_not_an_edge() {
        assert_eq!(edge(500., 400., Tiling::default()), None);
        assert_eq!(edge(6., 400., Tiling::default()), None);
        assert_eq!(
            edge(500., 794.5, Tiling::default()),
            Some(ResizeEdge::Bottom)
        );
        assert_eq!(edge(500., 793.9, Tiling::default()), None);
    }

    #[test]
    fn each_side_and_corner_is_found() {
        let free = Tiling::default();
        assert_eq!(edge(500., 2., free), Some(ResizeEdge::Top));
        assert_eq!(edge(500., 799., free), Some(ResizeEdge::Bottom));
        assert_eq!(edge(2., 400., free), Some(ResizeEdge::Left));
        assert_eq!(edge(999., 400., free), Some(ResizeEdge::Right));
        assert_eq!(edge(2., 2., free), Some(ResizeEdge::TopLeft));
        assert_eq!(edge(999., 2., free), Some(ResizeEdge::TopRight));
        assert_eq!(edge(2., 799., free), Some(ResizeEdge::BottomLeft));
        assert_eq!(edge(999., 799., free), Some(ResizeEdge::BottomRight));
        assert_eq!(edge(10., 10., free), Some(ResizeEdge::TopLeft));
    }

    #[test]
    fn a_tiled_side_offers_no_edge() {
        let top_tiled = Tiling {
            top: true,
            ..Tiling::default()
        };
        assert_eq!(edge(500., 2., top_tiled), None);
        assert_eq!(edge(2., 2., top_tiled), Some(ResizeEdge::Left));
        assert_eq!(edge(500., 799., top_tiled), Some(ResizeEdge::Bottom));
        assert_eq!(edge(2., 2., Tiling::tiled()), None);
    }

    #[test]
    fn the_band_rectangles_agree_with_edge_at() {
        let tiling = Tiling {
            left: true,
            ..Tiling::default()
        };
        let window = size(px(1000.), px(800.));
        let rects = band_rects(window, BAND, CORNER, tiling);
        assert!(rects.iter().all(|(e, _)| *e != ResizeEdge::Left));
        for (found, rect) in rects.iter().rev() {
            let centre = rect.center();
            let at = edge_at(centre, window, BAND, CORNER, tiling);
            assert_eq!(at, Some(*found));
        }
    }

    #[test]
    fn controls_follow_the_platform_and_the_maximized_state() {
        let all = Supported {
            minimize: true,
            maximize: true,
        };
        assert_eq!(
            visible_controls(all, false),
            [Control::Minimize, Control::Maximize, Control::Close]
        );
        assert_eq!(
            visible_controls(all, true),
            [Control::Minimize, Control::Restore, Control::Close]
        );
        let none = Supported {
            minimize: false,
            maximize: false,
        };
        assert_eq!(visible_controls(none, true), [Control::Close]);
    }
}
