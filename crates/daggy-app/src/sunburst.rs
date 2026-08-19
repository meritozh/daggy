//! Three-ring interactive allocation map.

use std::path::Path;
use std::sync::{Arc, Mutex};

use daggy_core::{fmt, NodeId, TreeProjection};
use gpui::{
    canvas, div, px, App, Bounds, FontWeight, InteractiveElement, IntoElement, MouseButton,
    ParentElement, PathBuilder, Pixels, Point, SharedString, StatefulInteractiveElement, Styled,
    Window,
};

use crate::app_state::AppState;
use crate::theme::Theme;

const RINGS: usize = 3;
const INNER_RADIUS: f32 = 0.28;
const MIN_ANGLE: f32 = 0.7_f32.to_radians();

#[derive(Clone)]
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
        center_note: &'static str,
        theme: Theme,
        state: gpui::WeakEntity<AppState>,
    ) -> impl IntoElement {
        let wedges = Arc::new(build_wedges(projection, &theme));
        let highlighted = hovered.or(selected);
        let bounds_store = self.bounds.clone();
        let paint_wedges = wedges.clone();
        let highlight_amount = if theme.is_dark { 0.18 } else { 0.10 };
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
                let highlight = highlight_amount;
                let painted = paint_wedges
                    .iter()
                    .map(|wedge| PaintedWedge {
                        start: wedge.start,
                        end: wedge.end,
                        inner: radius * wedge.inner + px(1.2),
                        outer: radius * wedge.outer - px(1.2),
                        color: if wedge.id == highlighted {
                            lighten(wedge.color, highlight)
                        } else {
                            wedge.color
                        },
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
        let highlighted_node = highlighted.and_then(|id| {
            projection
                .chart_node(id)
                .map(|chart| &chart.node)
                .or_else(|| projection.rows.iter().find(|row| row.id == id))
        });
        let label = highlighted_node
            .or(focus)
            .map(|node| {
                if node.parent.is_none() {
                    Path::new(node.name.as_ref())
                        .file_name()
                        .unwrap_or(node.name.as_ref())
                        .to_string_lossy()
                        .into_owned()
                } else {
                    node.display_name()
                }
            })
            .unwrap_or_else(|| "daggy".into());
        let size = highlighted_node
            .or(focus)
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
                            .w(px(176.))
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(
                                div()
                                    .w_full()
                                    .truncate()
                                    .text_center()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text)
                                    .child(SharedString::from(label)),
                            )
                            .child(
                                div()
                                    .mt(px(4.))
                                    .text_size(px(20.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .child(SharedString::from(size)),
                            )
                            .child(
                                div()
                                    .mt(px(4.))
                                    .text_xs()
                                    .text_color(if center_note == "calculating" {
                                        theme.warning
                                    } else {
                                        theme.text_tertiary
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
    let children = parent
        .children
        .iter()
        .filter_map(|id| {
            projection
                .chart_node(*id)
                .map(|node| (*id, node.node.block_size))
        })
        .collect::<Vec<_>>();
    let total = children
        .iter()
        .map(|(_, size)| *size)
        .sum::<u64>()
        .saturating_add(parent.omitted_size);
    if total == 0 {
        return;
    }

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
        let color_index = if depth == 0 {
            index
        } else {
            branch_color
                .wrapping_mul(2)
                .wrapping_add(index)
                .wrapping_add(depth)
        };
        let wedge_end = cursor + span;
        wedges.push(Wedge {
            id: Some(id),
            start: cursor + 0.0015,
            end: wedge_end - 0.0015,
            inner,
            outer,
            color: depth_color(theme.sunburst[color_index % theme.sunburst.len()], depth),
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
            start: cursor + 0.0015,
            end: (cursor + span).min(end) - 0.0015,
            inner,
            outer,
            color: depth_color(theme.sunburst_grouped, depth),
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
    let normalized_radius = (dx * dx + dy * dy).sqrt() / (radius / px(1.0));
    if normalized_radius < INNER_RADIUS {
        return Some(None);
    }
    let mut angle = dy.atan2(dx) + std::f32::consts::FRAC_PI_2;
    if angle < 0.0 {
        angle += std::f32::consts::TAU;
    }
    for wedge in wedges.iter().rev() {
        if normalized_radius >= wedge.inner
            && normalized_radius <= wedge.outer
            && angle >= wedge.start
            && angle <= wedge.end
        {
            return Some(wedge.id);
        }
    }
    None
}

fn paint_wedge(window: &mut Window, center: Point<Pixels>, wedge: &PaintedWedge) {
    if wedge.end <= wedge.start || wedge.outer <= wedge.inner {
        return;
    }
    let steps = (((wedge.end - wedge.start) / std::f32::consts::TAU) * 112.0)
        .ceil()
        .max(3.0) as usize;
    let mut path = PathBuilder::fill();
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

fn depth_color(color: u32, depth: usize) -> u32 {
    if depth == 0 {
        color
    } else {
        lighten(color, 0.08 * depth as f32)
    }
}

fn lighten(color: u32, amount: f32) -> u32 {
    let red = ((color >> 16) & 0xff) as f32;
    let green = ((color >> 8) & 0xff) as f32;
    let blue = (color & 0xff) as f32;
    let blend = |channel: f32| (channel + (255.0 - channel) * amount).round() as u32;
    (blend(red) << 16) | (blend(green) << 8) | blend(blue)
}
