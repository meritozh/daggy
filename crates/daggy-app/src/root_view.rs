//! The root view: toolbar + sunburst + list + status bar.
//!
//! Observes [`AppState`] and re-renders whenever scan progress arrives.

use std::path::PathBuf;

use daggy_core::{fmt, Kind, TreeSnapshot};
use gpui::{
    div, px, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window,
};

use crate::app_state::{AppEvent, AppState};
use crate::sunburst::Sunburst;

pub struct RootView {
    state: Entity<AppState>,
    sunburst: Sunburst,
    _sub: Option<Subscription>,
}

impl RootView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            state,
            sunburst: Sunburst::new(),
            _sub: None,
        };
        view.subscribe_state(cx);
        // Auto-open the folder picker on launch.
        view.pick_folder(cx);
        view
    }

    fn subscribe_state(&mut self, cx: &mut Context<Self>) {
        // Re-render whenever the app state notifies.
        cx.observe(&self.state, |_, _state, cx| {
            cx.notify();
        })
        .detach();

        // React to app events (persist cache when a scan finishes).
        cx.subscribe(&self.state, |this, _state, ev: &AppEvent, cx| match ev {
            AppEvent::ScanFinished { root_path } => {
                this.save_cache(root_path.clone(), cx);
            }
        })
        .detach();
    }

    /// Open the native folder picker and start scanning the selection.
    fn pick_folder(&self, cx: &mut Context<Self>) {
        use gpui::PathPromptOptions;
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose a folder to analyze".into()),
        });
        let state = self.state.clone();
        cx.spawn(async move |_this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(path) = paths.into_iter().next() {
                    let _ = state.update(cx, |s, cx| s.open_cached_or_scan(path, cx));
                }
            }
        })
        .detach();
    }

    /// Persist the cache on a background task (milestone 6 hardens this).
    fn save_cache(&self, root_path: PathBuf, cx: &mut Context<Self>) {
        let snap = self.state.read(cx).snapshot();
        let tree = snapshot_to_tree(&snap);
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = daggy_cache::save(&tree, &root_path) {
                    log::warn!("cache save failed: {e}");
                }
            })
            .detach();
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = self.state.read(cx).snapshot();
        let focus = self.state.read(cx).focus.0.or(snap.root);
        let status_text = self.state.read(cx).status_text();
        let has_data = snap.root.is_some();
        let root_path: SharedString = self
            .state
            .read(cx)
            .root_path
            .as_ref()
            .map(|p| p.to_string_lossy().into())
            .unwrap_or_else(|| "no folder".into());

        let breadcrumb = build_breadcrumb(&snap, focus);
        let rows = build_list_rows(&snap, focus);

        div()
            .id("root")
            .bg(gpui::rgb(0x11111b))
            .text_color(gpui::rgb(0xcdd6f4))
            .flex()
            .flex_col()
            .size_full()
            .focusable()
            .on_action(cx.listener(|this, _action: &crate::GoUp, _window, cx| {
                this.state.update(cx, |s, cx| s.go_up(cx));
            }))
            // ── Toolbar ────────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .items_center()
                    .px(px(16.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(gpui::rgb(0x313244))
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(gpui::rgb(0xa6adc8))
                            .child(breadcrumb),
                    )
                    .child(
                        div()
                            .id("pick")
                            .px(px(10.))
                            .py(px(4.))
                            .bg(gpui::rgb(0x313244))
                            .rounded(px(6.))
                            .text_sm()
                            .hover(|s| s.bg(gpui::rgb(0x45475a)))
                            .child("Open…")
                            .on_click(cx.listener(|this, _e, _w, cx| {
                                this.pick_folder(cx);
                            })),
                    ),
            )
            // ── Body: sunburst + list ─────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(self.sunburst.render(&snap, focus.unwrap_or(0))),
                    )
                    .child(
                        div()
                            .id("list")
                            .w(px(360.))
                            .border_l_1()
                            .border_color(gpui::rgb(0x313244))
                            .flex()
                            .flex_col()
                            .overflow_y_scroll()
                            .children(rows.into_iter().map(|row| {
                                list_row(row, self.state.downgrade())
                            })),
                    ),
            )
            // ── Status bar ────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .items_center()
                    .px(px(16.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(gpui::rgb(0x313244))
                    .bg(gpui::rgb(0x181825))
                    .text_xs()
                    .text_color(gpui::rgb(0xa6adc8))
                    .child(SharedString::from(if has_data {
                        status_text
                    } else {
                        format!("daggy · {root_path}")
                    })),
            )
    }
}

// ---------------------------------------------------------------------------
// Pure helpers.
// ---------------------------------------------------------------------------

struct Row {
    id: u32,
    name: std::sync::Arc<str>,
    size: u64,
    is_dir: bool,
    pct: f32,
}

/// Convert a snapshot back into a Tree for the cache encoder. Nodes are `Clone`
/// (their `name` is an `Arc<str>`), so we just clone each and patch references.
fn snapshot_to_tree(snap: &TreeSnapshot) -> daggy_core::Tree {
    use daggy_core::{Node, NodeId, Tree};
    let mut tree = Tree::new();
    let root = match snap.root {
        Some(r) => r,
        None => return tree,
    };
    // DFS from the root, allocating fresh ids and recording old→new mapping.
    let mut id_of: Vec<Option<NodeId>> = vec![None; snap.cap() as usize];
    fn visit(
        snap: &TreeSnapshot,
        tree: &mut Tree,
        id_of: &mut Vec<Option<NodeId>>,
        old: NodeId,
    ) -> NodeId {
        if let Some(Some(new)) = id_of.get(old as usize) {
            return *new;
        }
        let mut node: Node = snap.get(old).expect("node exists").clone();
        node.id = 0;
        node.parent = None;
        node.children = Vec::new();
        let new_id = tree.alloc(node);
        if id_of.len() <= old as usize {
            id_of.resize((old as usize) + 1, None);
        }
        id_of[old as usize] = Some(new_id);

        let children: Vec<NodeId> = snap
            .get(old)
            .expect("node exists")
            .children
            .iter()
            .map(|&c| visit(snap, tree, id_of, c))
            .collect();
        for &c in &children {
            if let Some(cn) = tree.get_mut(c) {
                cn.parent = Some(new_id);
            }
        }
        let n = tree.get_mut(new_id).unwrap();
        n.children = children;
        new_id
    }

    let new_root = visit(&snap, &mut tree, &mut id_of, root);
    tree.root = Some(new_root);
    tree
}

fn build_breadcrumb(snap: &TreeSnapshot, focus: Option<u32>) -> SharedString {
    let mut parts: Vec<String> = Vec::new();
    let mut cursor = focus.and_then(|id| snap.get(id));
    while let Some(n) = cursor {
        parts.push(n.name.to_string());
        cursor = n.parent.and_then(|p| snap.get(p));
    }
    parts.reverse();
    SharedString::from(parts.join(" › "))
}

fn build_list_rows(snap: &TreeSnapshot, focus: Option<u32>) -> Vec<Row> {
    let parent_size = focus
        .and_then(|id| snap.get(id))
        .map(|n| n.size.max(1) as f32)
        .unwrap_or(1.0);
    let mut rows: Vec<Row> = focus
        .and_then(|id| snap.get(id))
        .map(|n| {
            n.children
                .iter()
                .filter_map(|&c| snap.get(c))
                .map(|c| Row {
                    id: c.id,
                    name: c.name.clone(),
                    size: c.size,
                    is_dir: c.kind.is_dir(),
                    pct: (c.size as f32 / parent_size) * 100.0,
                })
                .collect()
        })
        .unwrap_or_default();
    rows.sort_by(|a, b| b.size.cmp(&a.size));
    rows
}

/// Build a single list row element. Clicking drills into that node (focus it,
/// and lazily expand it if it's an unscanned directory).
fn list_row(row: Row, state: gpui::WeakEntity<AppState>) -> impl IntoElement {
    let id = row.id;
    let is_dir = row.is_dir;
    let icon = if is_dir { "📁" } else { "📄" };
    let pct = format!("{:.0}%", row.pct);
    let name = SharedString::from(row.name.as_ref().to_string());
    let size = SharedString::from(fmt::human_bytes(row.size));
    div()
        .id(gpui::ElementId::NamedInteger(
            SharedString::from("row"),
            id as u64,
        ))
        .flex()
        .items_center()
        .px(px(14.))
        .py(px(6.))
        .hover(|s| s.bg(gpui::rgb(0x313244)))
        .on_click(move |_ev, _window, cx| {
            let _ = state.update(cx, |s, cx| {
                s.set_focus(Some(id), cx);
                if matches!(s.focus_state(id), daggy_core::ScanState::Unscanned) {
                    s.expand(id, cx);
                }
            });
        })
        .child(div().w(px(20.)).text_sm().child(SharedString::from(icon)))
        .child(div().flex_1().text_sm().child(name))
        .child(
            div()
                .w(px(80.))
                .text_sm()
                .text_color(gpui::rgb(0xa6adc8))
                .text_right()
                .child(size),
        )
        .child(
            div()
                .w(px(56.))
                .text_xs()
                .text_color(gpui::rgb(0x6c7086))
                .text_right()
                .child(SharedString::from(pct)),
        )
}

#[allow(dead_code)]
fn _kind_unused(_k: Kind) {}
