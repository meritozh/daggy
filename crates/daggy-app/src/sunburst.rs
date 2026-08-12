//! The sunburst visualization: concentric ring wedges, DaisyDisk-style.
//!
//! The focused node sits at the center. Its children form the first ring, their
//! children the second, and so on, up to [`MAX_RINGS`] levels deep. Each ring
//! is split into angular wedges proportional to size. Wedges are drawn as fill
//! paths via the low-level canvas/paint API; colors are graded by size.

use daggy_core::{fmt, TreeSnapshot};
use gpui::{
    canvas, div, px, App, Bounds, IntoElement, ParentElement, PathBuilder, Pixels, Point,
    SharedString, Styled, Window,
};

/// How many concentric rings to draw around the focused node.
const MAX_RINGS: usize = 4;

/// A hue from the Catppuccin Mocha palette, picked by a size bucket so bigger
/// wedges read as warmer/darker.
fn wedge_color(depth: usize, index: usize) -> u32 {
    const PALETTE: [u32; 8] = [
        0xf38ba8, // red
        0xfab387, // peach
        0xf9e2af, // yellow
        0xa6e3a1, // green
        0x94e2d5, // teal
        0x89b4fa, // blue
        0xb4befe, // lavender
        0xf5c2e7, // pink
    ];
    PALETTE[(index + depth) % PALETTE.len()]
}

/// A precomputed wedge to draw. `inner`/`outer` are normalized radii in [0,1]
/// until paint, where they're scaled to pixels.
#[derive(Clone)]
struct Wedge {
    color: u32,
    start: f32,
    end: f32,
    inner: f32,
    outer: f32,
}

/// The sunburst view.
pub struct Sunburst {
    /// Cached wedges for the current focus, recomputed on each render from the
    /// snapshot (cheap — at most a few thousand simple structs).
    wedges: Vec<Wedge>,
    center_label: SharedString,
    center_size: SharedString,
}

impl Sunburst {
    pub fn new() -> Self {
        Self {
            wedges: Vec::new(),
            center_label: "daggy".into(),
            center_size: "".into(),
        }
    }

    /// Recompute wedge geometry for `focus` from the snapshot.
    fn recompute(&mut self, snap: &TreeSnapshot, focus: u32) {
        self.wedges.clear();
        let (label, total) = match snap.get(focus) {
            Some(n) => (n.name.clone(), n.size),
            None => return,
        };
        self.center_label = SharedString::from(label.to_string());
        self.center_size = SharedString::from(fmt::human_bytes(total));

        // BFS by depth, assigning angular spans. Each node owns an arc; its
        // children subdivide that arc proportional to their sizes.
        let mut queue: Vec<(u32, f32, f32)> = Vec::new(); // (id, start, end)
        if let Some(node) = snap.get(focus) {
            // First ring covers the full circle (0..2π).
            let total_child: u64 =
                node.children.iter().filter_map(|c| snap.get(*c)).map(|n| n.size).sum();
            let total = total_child.max(1) as f32;
            let mut cursor = 0.0_f32;
            let children: Vec<u32> = node.children.clone();
            for (i, &c) in children.iter().enumerate() {
                let span = snap.get(c).map(|n| (n.size as f32 / total) * std::f32::consts::TAU).unwrap_or(0.0);
                let _ = i;
                let start = cursor;
                let end = cursor + span;
                queue.push((c, start, end));
                cursor = end;
            }
        }

        let mut depth = 0usize;
        let mut ring_width = 0.0_f32;
        while !queue.is_empty() && depth < MAX_RINGS {
            let mut next: Vec<(u32, f32, f32)> = Vec::new();
            // ring_width is set on the first iteration from the canvas; we use
            // a placeholder here and scale at paint time via normalized radii.
            let _ = ring_width;
            for (idx, (id, start, end)) in queue.drain(..).enumerate() {
                let color = wedge_color(depth, idx);
                // Normalized inner/outer radii in [0,1]; scaled to pixels at paint.
                let inner = depth as f32 / MAX_RINGS as f32;
                let outer = (depth + 1) as f32 / MAX_RINGS as f32;
                self.wedges.push(Wedge { color, start, end, inner, outer });
                // Enqueue this node's children within [start, end].
                if let Some(node) = snap.get(id) {
                    let total_child: u64 =
                        node.children.iter().filter_map(|c| snap.get(*c)).map(|n| n.size).sum();
                    let total = total_child.max(1) as f32;
                    let mut cursor = start;
                    let children: Vec<u32> = node.children.clone();
                    for &c in &children {
                        let span = snap.get(c).map(|n| (n.size as f32 / total) * (end - start)).unwrap_or(0.0);
                        let s = cursor;
                        let e = cursor + span;
                        next.push((c, s, e));
                        cursor = e;
                    }
                }
            }
            queue = next;
            depth += 1;
            ring_width += 0.0;
        }
    }

    pub fn render(&mut self, snap: &TreeSnapshot, focus: u32) -> impl IntoElement {
        self.recompute(snap, focus);
        let label = self.center_label.clone();
        let size = self.center_size.clone();
        let wedges = self.wedges.clone();

        let painter = canvas(
            move |bounds: Bounds<Pixels>, _window, _cx: &mut App| {
                // Precompute pixel geometry for the paint closure.
                let center = Point {
                    x: bounds.origin.x + bounds.size.width * 0.5,
                    y: bounds.origin.y + bounds.size.height * 0.5,
                };
                let max_radius = bounds.size.width.min(bounds.size.height) * 0.5 * 0.95;
                // Convert normalized radii to pixels up front.
                let painted: Vec<PaintedWedge> = wedges
                    .iter()
                    .map(|w| PaintedWedge {
                        color: w.color,
                        start: w.start,
                        end: w.end,
                        inner: max_radius * w.inner,
                        outer: max_radius * w.outer,
                    })
                    .collect();
                (center, painted)
            },
            move |_, (center, painted), window, _cx: &mut App| {
                for wedge in &painted {
                    paint_wedge(window, center, wedge);
                }
            },
        )
        .size_full();

        // Overlay the center label/size on top of the canvas.
        div()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .size_full()
            .bg(gpui::rgb(0x181825))
            .child(painter)
            .child(
                div()
                    .absolute()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .text_color(gpui::rgb(0xcdd6f4))
                            .text_lg()
                            .child(label),
                    )
                    .child(
                        div()
                            .text_color(gpui::rgb(0xa6adc8))
                            .text_sm()
                            .child(size),
                    ),
            )
    }
}

/// A wedge with pixel-space radii, ready to paint.
struct PaintedWedge {
    color: u32,
    start: f32,
    end: f32,
    inner: Pixels,
    outer: Pixels,
}

/// Draw one ring wedge as a filled annular sector. We approximate the arc with
/// line segments around the circle.
fn paint_wedge(window: &mut Window, center: Point<Pixels>, wedge: &PaintedWedge) {
    if wedge.end <= wedge.start {
        return;
    }
    let steps = ((wedge.end - wedge.start) / std::f32::consts::TAU * 64.0).ceil() as usize;
    let steps = steps.max(2);
    let mut path = PathBuilder::fill();
    // Outer arc, start -> end.
    for i in 0..=steps {
        let t = wedge.start + (wedge.end - wedge.start) * (i as f32 / steps as f32);
        let p = polar(center, wedge.outer, t);
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    // Inner arc, end -> start (reverse so the sector closes correctly).
    for i in 0..=steps {
        let t = wedge.end - (wedge.end - wedge.start) * (i as f32 / steps as f32);
        let p = polar(center, wedge.inner, t);
        path.line_to(p);
    }
    path.close();
    if let Ok(built) = path.build() {
        window.paint_path(built, gpui::rgb(wedge.color));
    }
}

/// Convert polar (angle in radians) to a pixel point around `center`.
fn polar(center: Point<Pixels>, radius: Pixels, angle: f32) -> Point<Pixels> {
    // Start at 12 o'clock (-π/2) and sweep clockwise. Extract the f32 value by
    // dividing by 1px (Pixels / Pixels -> f32).
    let r = radius / px(1.0);
    let a = angle - std::f32::consts::FRAC_PI_2;
    Point {
        x: center.x + px(a.cos() * r),
        y: center.y + px(a.sin() * r),
    }
}

impl Default for Sunburst {
    fn default() -> Self {
        Self::new()
    }
}
