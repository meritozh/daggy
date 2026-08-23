//! Three-ring interactive allocation map.

use std::sync::{Arc, Mutex};

use daggy_core::{fmt, NodeId, TreeProjection};
use gpui::{
    canvas, div, px, App, Bounds, FillOptions, FillRule, FontWeight, InteractiveElement,
    IntoElement, MouseButton, ParentElement, PathBuilder, PathStyle, Pixels, Point, SharedString,
    StatefulInteractiveElement, Styled, Window,
};

use crate::app_state::AppState;
use crate::theme::Theme;

const RINGS: usize = 3;
const INNER_RADIUS: f32 = 0.30;
const MIN_ANGLE: f32 = 0.7_f32.to_radians();
const RING_GAP: f32 = 1.5;
const SLICE_GAP: f32 = 1.5;

fn ring_band(radius: Pixels, inner: f32, outer: f32) -> (Pixels, Pixels) {
    let inset = px(RING_GAP / 2.0);
    (radius * inner + inset, radius * outer - inset)
}

fn slice_angles(inner: Pixels, outer: Pixels, start: f32, end: f32) -> (f32, f32) {
    let mid = ((inner / px(1.0)) + (outer / px(1.0))) * 0.5;
    let half_gap = (SLICE_GAP * 0.5) / mid.max(1.0);
    let span = end - start;
    if span <= half_gap * 2.0 {
        let center = (start + end) * 0.5;
        let keep = (span * 0.4).max(0.0003);
        (center - keep, center + keep)
    } else {
        (start + half_gap, end - half_gap)
    }
}

#[derive(Clone, Debug)]
struct Wedge {
    id: Option<NodeId>,
    start: f32,
    end: f32,
    inner: f32,
    outer: f32,
    color: u32,
}

#[derive(Clone)]
struct PaintedWedge {
    start: f32,
    end: f32,
    inner: Pixels,
    outer: Pixels,
    color: u32,
}

pub struct Sunburst {
    bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
}

impl Sunburst {
    pub fn new() -> Self {
        Self {
            bounds: Arc::new(Mutex::new(None)),
        }
    }

    pub fn render(
        &mut self,
        projection: &TreeProjection,
        hovered: Option<NodeId>,
        selected: Option<NodeId>,
        center_note: SharedString,
        theme: Theme,
        state: gpui::WeakEntity<AppState>,
    ) -> impl IntoElement {
        let wedges = Arc::new(build_wedges(projection, &theme));
        let highlighted = hovered.or(selected);
        let bounds_store = self.bounds.clone();
        let paint_wedges = wedges.clone();
        let dark = theme.is_dark;
        let painter = canvas(
            move |bounds: Bounds<Pixels>, _window, _cx: &mut App| {
                if let Ok(mut stored) = bounds_store.lock() {
                    *stored = Some(bounds);
                }
                let center = Point {
                    x: bounds.origin.x + bounds.size.width * 0.5,
                    y: bounds.origin.y + bounds.size.height * 0.5,
                };
                let radius = bounds.size.width.min(bounds.size.height) * 0.455;
                let painted = paint_wedges
                    .iter()
                    .map(|wedge| {
                        let (inner, outer) = ring_band(radius, wedge.inner, wedge.outer);
                        let (start, end) = slice_angles(inner, outer, wedge.start, wedge.end);
                        PaintedWedge {
                            start,
                            end,
                            inner,
                            outer,
                            color: if wedge.id == highlighted {
                                emphasize(wedge.color, dark)
                            } else {
                                wedge.color
                            },
                        }
                    })
                    .collect::<Vec<_>>();
                (center, painted)
            },
            move |_, (center, wedges), window, _cx: &mut App| {
                for wedge in wedges {
                    paint_wedge(window, center, &wedge);
                }
            },
        )
        .size_full();

        let focus = projection.focus_node.as_ref();
        let size = focus
            .map(|node| fmt::human_bytes(node.block_size))
            .unwrap_or_else(|| "0 B".into());

        let move_bounds = self.bounds.clone();
        let move_wedges = wedges.clone();
        let move_state = state.clone();
        let hover_state = state.clone();
        let click_bounds = self.bounds.clone();
        let click_wedges = wedges;
        let click_state = state;

        div()
            .id("sunburst-map")
            .relative()
            .size_full()
            .bg(theme.canvas)
            .cursor(gpui::CursorStyle::Arrow)
            .on_mouse_move(move |event, _, cx| {
                let id = hit_test(&move_bounds, &move_wedges, event.position).and_then(|hit| hit);
                let _ = move_state.update(cx, |state, cx| state.set_hovered(id, cx));
            })
            .on_hover(move |inside, _, cx| {
                if !*inside {
                    let _ = hover_state.update(cx, |state, cx| state.set_hovered(None, cx));
                }
            })
            .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                match hit_test(&click_bounds, &click_wedges, event.position) {
                    Some(Some(id)) => {
                        let _ = click_state.update(cx, |state, cx| state.activate_node(id, cx));
                    }
                    Some(None) => {
                        let _ = click_state.update(cx, |state, cx| state.go_up(cx));
                    }
                    None => {}
                }
            })
            .child(painter)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .w(px(220.))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(4.))
                            .child(
                                div()
                                    .font_family("SF Pro Display")
                                    .text_size(px(44.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.text)
                                    .child(SharedString::from(size)),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(if center_note.as_ref() == "calculating" {
                                        theme.warning
                                    } else {
                                        theme.text_secondary
                                    })
                                    .child(center_note),
                            ),
                    ),
            )
    }
}

impl Default for Sunburst {
    fn default() -> Self {
        Self::new()
    }
}

fn build_wedges(projection: &TreeProjection, theme: &Theme) -> Vec<Wedge> {
    let mut wedges = Vec::new();
    if let Some(focus) = projection.focus {
        append_children(
            projection,
            focus,
            0.0,
            std::f32::consts::TAU,
            0,
            0,
            theme,
            &mut wedges,
        );
    }
    wedges
}

fn append_children(
    projection: &TreeProjection,
    parent: NodeId,
    start: f32,
    end: f32,
    depth: usize,
    branch_color: usize,
    theme: &Theme,
    wedges: &mut Vec<Wedge>,
) {
    if depth >= RINGS {
        return;
    }
    let Some(parent) = projection.chart_node(parent) else {
        return;
    };
    if !parent.node.kind.is_dir() {
        return;
    }
    let children = parent
        .children
        .iter()
        .filter_map(|id| {
            projection
                .chart_node(*id)
                .map(|node| (*id, node.node.block_size))
        })
        .collect::<Vec<_>>();
    let accounted = children
        .iter()
        .map(|(_, size)| *size)
        .sum::<u64>()
        .saturating_add(parent.omitted_size);
    if accounted == 0 {
        return;
    }
    let total = accounted;

    let ring_width = (1.0 - INNER_RADIUS) / RINGS as f32;
    let inner = INNER_RADIUS + ring_width * depth as f32;
    let outer = inner + ring_width;
    let mut cursor = start;
    let mut grouped = parent.omitted_size;
    for (index, (id, size)) in children.into_iter().enumerate() {
        let span = (end - start) * size as f32 / total as f32;
        if span < MIN_ANGLE {
            grouped = grouped.saturating_add(size);
            continue;
        }
        let color_index = if depth == 0 { index } else { branch_color };
        let wedge_end = cursor + span;
        wedges.push(Wedge {
            id: Some(id),
            start: cursor,
            end: wedge_end,
            inner,
            outer,
            color: ring_shade(
                theme.sunburst[color_index % theme.sunburst.len()],
                depth,
                index,
            ),
        });
        append_children(
            projection,
            id,
            cursor,
            wedge_end,
            depth + 1,
            color_index,
            theme,
            wedges,
        );
        cursor = wedge_end;
    }
    if grouped > 0 {
        let span = (end - start) * grouped as f32 / total as f32;
        wedges.push(Wedge {
            id: None,
            start: cursor,
            end: cursor + span,
            inner,
            outer,
            color: ring_shade(theme.sunburst_grouped, depth, 0),
        });
    }
}

fn hit_test(
    bounds: &Arc<Mutex<Option<Bounds<Pixels>>>>,
    wedges: &[Wedge],
    position: Point<Pixels>,
) -> Option<Option<NodeId>> {
    let bounds = bounds.lock().ok()?.as_ref().copied()?;
    if !bounds.contains(&position) {
        return None;
    }
    let center = Point {
        x: bounds.origin.x + bounds.size.width * 0.5,
        y: bounds.origin.y + bounds.size.height * 0.5,
    };
    let radius = bounds.size.width.min(bounds.size.height) * 0.455;
    let dx = (position.x - center.x) / px(1.0);
    let dy = (position.y - center.y) / px(1.0);
    let distance = px((dx * dx + dy * dy).sqrt());
    if distance < radius * INNER_RADIUS {
        return Some(None);
    }
    let mut angle = dy.atan2(dx) + std::f32::consts::FRAC_PI_2;
    if angle < 0.0 {
        angle += std::f32::consts::TAU;
    }
    for wedge in wedges.iter().rev() {
        let (inner, outer) = ring_band(radius, wedge.inner, wedge.outer);
        let (start, end) = slice_angles(inner, outer, wedge.start, wedge.end);
        if distance >= inner && distance <= outer && angle >= start && angle <= end {
            return Some(wedge.id);
        }
    }
    None
}

fn paint_wedge(window: &mut Window, center: Point<Pixels>, wedge: &PaintedWedge) {
    if wedge.end <= wedge.start || wedge.outer <= wedge.inner {
        return;
    }
    let steps = (((wedge.end - wedge.start) / std::f32::consts::TAU) * 160.0)
        .ceil()
        .max(4.0) as usize;
    let mut path = PathBuilder::fill().with_style(PathStyle::Fill(
        FillOptions::default().with_fill_rule(FillRule::NonZero),
    ));
    for index in 0..=steps {
        let angle = wedge.start + (wedge.end - wedge.start) * index as f32 / steps as f32;
        let point = polar(center, wedge.outer, angle);
        if index == 0 {
            path.move_to(point);
        } else {
            path.line_to(point);
        }
    }
    for index in 0..=steps {
        let angle = wedge.end - (wedge.end - wedge.start) * index as f32 / steps as f32;
        path.line_to(polar(center, wedge.inner, angle));
    }
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, gpui::rgb(wedge.color));
    }
}

fn polar(center: Point<Pixels>, radius: Pixels, angle: f32) -> Point<Pixels> {
    let radius = radius / px(1.0);
    let angle = angle - std::f32::consts::FRAC_PI_2;
    Point {
        x: center.x + px(angle.cos() * radius),
        y: center.y + px(angle.sin() * radius),
    }
}

const MAX_LIGHTNESS: f32 = 0.56;
const MIN_LIGHTNESS: f32 = 0.24;

fn ring_shade(color: u32, depth: usize, sibling: usize) -> u32 {
    let (hue, saturation, lightness) = rgb_to_hsl(color);
    let sibling_nudge = if depth == 0 {
        0.0
    } else {
        ((sibling % 3) as f32 - 1.0) * 0.03
    };
    let depth_shift = if lightness >= 0.48 {
        -0.07 * depth as f32
    } else {
        0.06 * depth as f32
    };
    let lightness = (lightness + depth_shift + sibling_nudge).clamp(MIN_LIGHTNESS, MAX_LIGHTNESS);
    let saturation = if saturation > 0.08 {
        (saturation + 0.05 * depth as f32).clamp(0.38, 0.88)
    } else {
        saturation
    };
    hsl_to_rgb(hue, saturation, lightness)
}

fn emphasize(color: u32, dark_canvas: bool) -> u32 {
    let (hue, saturation, lightness) = rgb_to_hsl(color);
    let lightness = if dark_canvas {
        (lightness + 0.07).min(MAX_LIGHTNESS)
    } else {
        (lightness - 0.06).max(MIN_LIGHTNESS)
    };
    let saturation = if saturation > 0.08 {
        (saturation + 0.06).min(0.9)
    } else {
        saturation
    };
    hsl_to_rgb(hue, saturation, lightness)
}

fn rgb_to_hsl(color: u32) -> (f32, f32, f32) {
    let red = ((color >> 16) & 0xff) as f32 / 255.0;
    let green = ((color >> 8) & 0xff) as f32 / 255.0;
    let blue = (color & 0xff) as f32 / 255.0;
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let lightness = (max + min) / 2.0;
    if max - min < f32::EPSILON {
        return (0.0, 0.0, lightness);
    }
    let delta = max - min;
    let saturation = if lightness > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let hue = if (max - red).abs() < f32::EPSILON {
        let mut value = (green - blue) / delta;
        if green < blue {
            value += 6.0;
        }
        value
    } else if (max - green).abs() < f32::EPSILON {
        (blue - red) / delta + 2.0
    } else {
        (red - green) / delta + 4.0
    } / 6.0;
    (hue, saturation, lightness)
}

fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> u32 {
    let (red, green, blue) = if saturation < f32::EPSILON {
        (lightness, lightness, lightness)
    } else {
        let q = if lightness < 0.5 {
            lightness * (1.0 + saturation)
        } else {
            lightness + saturation - lightness * saturation
        };
        let p = 2.0 * lightness - q;
        (
            hue_to_rgb(p, q, hue + 1.0 / 3.0),
            hue_to_rgb(p, q, hue),
            hue_to_rgb(p, q, hue - 1.0 / 3.0),
        )
    };
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
    (channel(red) << 16) | (channel(green) << 8) | channel(blue)
}

fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 0.5 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn is_near_white(color: u32) -> bool {
        let red = ((color >> 16) & 0xff) as f32;
        let green = ((color >> 8) & 0xff) as f32;
        let blue = (color & 0xff) as f32;
        let luma = (0.2126 * red + 0.7152 * green + 0.0722 * blue) / 255.0;
        // Cream and paper-white both read as holes in the map.
        luma > 0.78 && red.min(green).min(blue) > 160.0 || red.min(green).min(blue) > 210.0
    }

    #[test]
    fn ring_shades_never_wash_to_white() {
        for theme in [Theme::dark(), Theme::light()] {
            for (index, &base) in theme.sunburst.iter().enumerate() {
                for depth in 0..RINGS {
                    for sibling in 0..6 {
                        let color = ring_shade(base, depth, sibling);
                        assert!(
                            !is_near_white(color),
                            "sunburst[{index}] depth {depth} sibling {sibling} is {color:06x}"
                        );
                        assert!(
                            !is_near_white(emphasize(color, theme.is_dark)),
                            "emphasized sunburst[{index}] depth {depth} is washed out"
                        );
                    }
                }
            }
            for depth in 0..RINGS {
                let grouped = ring_shade(theme.sunburst_grouped, depth, 0);
                assert!(
                    !is_near_white(grouped),
                    "grouped depth {depth} is {grouped:06x}"
                );
            }
        }
    }

    fn file_tree(entries: &[(&str, u64)]) -> daggy_core::Tree {
        use std::ffi::OsStr;
        use std::sync::Arc;

        use daggy_core::{Kind, Node, Tree};

        let mut tree = Tree::new();
        let root = tree.alloc(Node::unscanned_dir(0, None, Arc::from(OsStr::new("/"))));
        tree.root = Some(root);
        for (name, size) in entries {
            let id = tree.alloc(Node::unscanned_dir(
                0,
                Some(root),
                Arc::from(OsStr::new(*name)),
            ));
            if let Some(node) = tree.get_mut(id) {
                node.kind = Kind::File;
                node.size = *size;
                node.block_size = *size;
            }
            tree.get_mut(root).unwrap().children.push(id);
        }
        tree.recompute_upward(root);
        tree
    }

    #[test]
    fn files_do_not_paint_empty_outer_rings() {
        let tree = file_tree(&[("movie.mp4", 63), ("game", 105)]);
        let projection = tree.projection(tree.root, 3, 32, 128, 32);
        let wedges = build_wedges(&projection, &Theme::light());
        assert!(
            wedges
                .iter()
                .all(|wedge| (wedge.inner - INNER_RADIUS).abs() < 0.0001),
            "files have no nested folders, so only the inner ring should draw, got {wedges:?}"
        );
        assert_eq!(wedges.len(), 2);
        assert!(wedges.iter().all(|wedge| wedge.id.is_some()));
    }

    #[test]
    fn omitted_children_share_one_grouped_slice_on_the_same_ring() {
        let tree = file_tree(&[("a", 40), ("b", 30), ("c", 20), ("d", 10)]);
        let projection = tree.projection(tree.root, 3, 2, 128, 32);
        let wedges = build_wedges(&projection, &Theme::light());
        let inner = wedges
            .iter()
            .filter(|wedge| (wedge.inner - INNER_RADIUS).abs() < 0.0001)
            .collect::<Vec<_>>();
        assert_eq!(inner.iter().filter(|wedge| wedge.id.is_some()).count(), 2);
        assert_eq!(inner.iter().filter(|wedge| wedge.id.is_none()).count(), 1);
        assert!(wedges
            .iter()
            .all(|wedge| (wedge.inner - INNER_RADIUS).abs() < 0.0001));
    }
}
