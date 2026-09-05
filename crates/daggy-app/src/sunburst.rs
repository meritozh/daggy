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
/// Minimum painted arc length at a ring's midline. Filelight hides anything
/// under ~3px and QDirStat uses a 3px tile floor; 6px leaves a clickable
/// fill after the 1.5px slice gaps on both sides.
const MIN_ARC_PX: f32 = 6.0;
const RING_GAP: f32 = 1.5;
const SLICE_GAP: f32 = 1.5;
const FALLBACK_RADIUS: f32 = 200.0;

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

fn chart_radius(bounds: Option<Bounds<Pixels>>) -> Pixels {
    bounds
        .map(|bounds| bounds.size.width.min(bounds.size.height) * 0.455)
        .unwrap_or(px(FALLBACK_RADIUS))
}

fn ring_min_angle(radius: Pixels, depth: usize) -> f32 {
    let ring_width = (1.0 - INNER_RADIUS) / RINGS as f32;
    let inner = INNER_RADIUS + ring_width * depth as f32;
    let outer = inner + ring_width;
    let (inner, outer) = ring_band(radius, inner, outer);
    let mid = ((inner / px(1.0)) + (outer / px(1.0))) * 0.5;
    MIN_ARC_PX / mid.max(1.0)
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
        let radius = chart_radius(self.bounds.lock().ok().and_then(|stored| *stored));
        let wedges = Arc::new(build_wedges(projection, &theme, radius));
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

fn build_wedges(projection: &TreeProjection, theme: &Theme, radius: Pixels) -> Vec<Wedge> {
    let mut wedges = Vec::new();
    if let Some(focus) = projection.focus {
        append_children(
            projection,
            focus,
            0.0,
            std::f32::consts::TAU,
            0,
            0,
            radius,
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
    radius: Pixels,
    theme: &Theme,
    wedges: &mut Vec<Wedge>,
) {
    if depth >= RINGS {
        return;
    }
    let min_angle = ring_min_angle(radius, depth);
    if end - start < min_angle {
        return;
    }
    let Some(parent) = projection.chart_node(parent) else {
        return;
    };
    if !parent.node.kind.is_dir() {
        return;
    }
    let total = parent.node.block_size;
    if total == 0 {
        return;
    }
    let children = parent
        .children
        .iter()
        .filter_map(|id| {
            let node = projection.chart_node(*id)?;
            node.node
                .kind
                .is_dir()
                .then_some((*id, node.node.block_size))
        })
        .collect::<Vec<_>>();

    let ring_width = (1.0 - INNER_RADIUS) / RINGS as f32;
    let inner = INNER_RADIUS + ring_width * depth as f32;
    let outer = inner + ring_width;
    let parent_span = end - start;
    let mut cursor = start;
    let mut grouped = parent.omitted_size;
    let mut painted = 0usize;
    for (id, size) in children {
        let span = child_span(parent_span, size, total);
        if span < min_angle {
            grouped = grouped.saturating_add(size);
            continue;
        }
        let Some(wedge_end) = clipped_end(cursor, span, end) else {
            break;
        };
        let color_index = if depth == 0 { painted } else { branch_color };
        wedges.push(Wedge {
            id: Some(id),
            start: cursor,
            end: wedge_end,
            inner,
            outer,
            color: ring_shade(
                theme.sunburst[color_index % theme.sunburst.len()],
                depth,
                painted,
            ),
        });
        append_children(
            projection,
            id,
            cursor,
            wedge_end,
            depth + 1,
            color_index,
            radius,
            theme,
            wedges,
        );
        cursor = wedge_end;
        painted += 1;
    }
    if grouped > 0 {
        let span = child_span(parent_span, grouped, total);
        if span >= min_angle {
            if let Some(wedge_end) = clipped_end(cursor, span, end) {
                wedges.push(Wedge {
                    id: None,
                    start: cursor,
                    end: wedge_end,
                    inner,
                    outer,
                    color: ring_shade(theme.sunburst_grouped, depth, 0),
                });
            }
        }
    }
}

fn child_span(parent_span: f32, size: u64, parent_size: u64) -> f32 {
    if size == 0 || parent_size == 0 {
        return 0.0;
    }
    parent_span * (size as f64 / parent_size as f64) as f32
}

fn clipped_end(start: f32, span: f32, limit: f32) -> Option<f32> {
    let end = (start + span).min(limit);
    (end > start).then_some(end)
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

    fn alloc_dir(tree: &mut daggy_core::Tree, parent: Option<NodeId>, name: &str) -> NodeId {
        use std::ffi::OsStr;
        use std::sync::Arc;

        use daggy_core::Node;

        let id = tree.alloc(Node::unscanned_dir(
            0,
            parent,
            Arc::from(OsStr::new(name)),
        ));
        match parent {
            Some(parent) => tree.get_mut(parent).unwrap().children.push(id),
            None => tree.root = Some(id),
        }
        id
    }

    fn alloc_file(
        tree: &mut daggy_core::Tree,
        parent: NodeId,
        name: &str,
        size: u64,
    ) -> NodeId {
        use std::ffi::OsStr;
        use std::sync::Arc;

        use daggy_core::{Kind, Node};

        let id = tree.alloc(Node::unscanned_dir(
            0,
            Some(parent),
            Arc::from(OsStr::new(name)),
        ));
        if let Some(node) = tree.get_mut(id) {
            node.kind = Kind::File;
            node.size = size;
            node.block_size = size;
        }
        tree.get_mut(parent).unwrap().children.push(id);
        id
    }

    fn settle(tree: &mut daggy_core::Tree) {
        let files: Vec<NodeId> = tree
            .iter()
            .filter(|node| !node.kind.is_dir())
            .map(|node| node.id)
            .collect();
        for id in files {
            tree.recompute_upward(id);
        }
    }

    fn file_tree(entries: &[(&str, u64)]) -> daggy_core::Tree {
        use daggy_core::Tree;

        let mut tree = Tree::new();
        let root = alloc_dir(&mut tree, None, "/");
        for (name, size) in entries {
            alloc_file(&mut tree, root, name, *size);
        }
        settle(&mut tree);
        tree
    }

    fn assert_near(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.0001,
            "expected {expected}, got {actual}"
        );
    }

    fn map_wedges(tree: &daggy_core::Tree, branch_limit: usize) -> Vec<Wedge> {
        let projection = tree.projection(tree.root, 3, branch_limit, 128, 32);
        build_wedges(&projection, &Theme::light(), px(FALLBACK_RADIUS))
    }

    #[test]
    fn files_leave_the_ring_empty() {
        let tree = file_tree(&[("movie.mp4", 63), ("game", 105)]);
        let wedges = map_wedges(&tree, 32);
        assert!(
            wedges.is_empty(),
            "a folder of files has no directory arcs, got {wedges:?}"
        );
    }

    #[test]
    fn folder_arc_uses_parent_size_and_does_not_close_over_files() {
        use daggy_core::Tree;

        let mut tree = Tree::new();
        let root = alloc_dir(&mut tree, None, "/");
        let apps = alloc_dir(&mut tree, Some(root), "Apps");
        alloc_file(&mut tree, apps, "App.app", 40);
        alloc_file(&mut tree, root, "movie.mp4", 60);
        settle(&mut tree);

        let wedges = map_wedges(&tree, 32);
        assert_eq!(wedges.len(), 1);
        assert_near(wedges[0].start, 0.0);
        assert_near(wedges[0].end, std::f32::consts::TAU * 0.4);
        assert_near(wedges[0].inner, INNER_RADIUS);
    }

    #[test]
    fn nested_folder_arc_is_relative_to_its_parent_folder() {
        use daggy_core::Tree;

        let mut tree = Tree::new();
        let root = alloc_dir(&mut tree, None, "/");
        let apps = alloc_dir(&mut tree, Some(root), "Apps");
        let editor = alloc_dir(&mut tree, Some(apps), "Editor");
        alloc_file(&mut tree, editor, "bin", 10);
        alloc_file(&mut tree, apps, "cache.dat", 30);
        alloc_file(&mut tree, root, "movie.mp4", 60);
        settle(&mut tree);

        let wedges = map_wedges(&tree, 32);
        assert_eq!(wedges.len(), 2);

        let ring_width = (1.0 - INNER_RADIUS) / RINGS as f32;
        let inner = wedges
            .iter()
            .find(|wedge| (wedge.inner - INNER_RADIUS).abs() < 0.0001)
            .expect("parent folder arc");
        let nested = wedges
            .iter()
            .find(|wedge| (wedge.inner - (INNER_RADIUS + ring_width)).abs() < 0.0001)
            .expect("nested folder arc");

        assert_near(inner.start, 0.0);
        assert_near(inner.end, std::f32::consts::TAU * 0.4);
        assert_near(nested.start, 0.0);
        assert_near(nested.end, std::f32::consts::TAU * 0.1);
    }

    #[test]
    fn omitted_directories_share_one_grouped_slice_on_the_same_ring() {
        use daggy_core::Tree;

        let mut tree = Tree::new();
        let root = alloc_dir(&mut tree, None, "/");
        for (name, size) in [("a", 40), ("b", 30), ("c", 20), ("d", 10)] {
            let dir = alloc_dir(&mut tree, Some(root), name);
            alloc_file(&mut tree, dir, "keep", size);
        }
        settle(&mut tree);

        let wedges = map_wedges(&tree, 2);
        let inner = wedges
            .iter()
            .filter(|wedge| (wedge.inner - INNER_RADIUS).abs() < 0.0001)
            .collect::<Vec<_>>();
        assert_eq!(inner.iter().filter(|wedge| wedge.id.is_some()).count(), 2);
        assert_eq!(inner.iter().filter(|wedge| wedge.id.is_none()).count(), 1);
        let grouped = inner.iter().find(|wedge| wedge.id.is_none()).unwrap();
        assert_near(grouped.end - grouped.start, std::f32::consts::TAU * 0.3);
    }

    #[test]
    fn hairline_directories_collapse_into_one_grouped_slice() {
        use daggy_core::Tree;

        let mut tree = Tree::new();
        let root = alloc_dir(&mut tree, None, "/");
        let large = alloc_dir(&mut tree, Some(root), "large");
        alloc_file(&mut tree, large, "keep", 92);
        for index in 0..8 {
            let dir = alloc_dir(&mut tree, Some(root), &format!("tiny-{index}"));
            alloc_file(&mut tree, dir, "keep", 1);
        }
        settle(&mut tree);

        let min_angle = ring_min_angle(px(FALLBACK_RADIUS), 0);
        assert!(std::f32::consts::TAU * 0.01 < min_angle);

        let wedges = map_wedges(&tree, 32);
        let inner = wedges
            .iter()
            .filter(|wedge| (wedge.inner - INNER_RADIUS).abs() < 0.0001)
            .collect::<Vec<_>>();
        assert_eq!(inner.iter().filter(|wedge| wedge.id.is_some()).count(), 1);
        assert_eq!(inner.iter().filter(|wedge| wedge.id.is_none()).count(), 1);
        let grouped = inner.iter().find(|wedge| wedge.id.is_none()).unwrap();
        assert_near(grouped.end - grouped.start, std::f32::consts::TAU * 0.08);
    }
}
