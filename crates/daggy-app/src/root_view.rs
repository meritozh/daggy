//! Graphite instrument-panel shell for the sunburst and ranked contents.

use std::sync::Arc;

use daggy_core::{fmt, Kind, NodeView, TreeProjection};
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, relative, uniform_list, AnyElement, Context, Element, Entity, FocusHandle, FontWeight,
    InteractiveElement, IntoElement, ParentElement, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement, Styled, UniformListScrollHandle, Window,
};

use crate::app_state::{AppState, DisplaySource};
use crate::sunburst::Sunburst;

const CANVAS: u32 = 0x0D0F12;
const SURFACE: u32 = 0x14171C;
const ELEVATED: u32 = 0x1A1E24;
const HOVER: u32 = 0x20262E;
const SELECTED: u32 = 0x272E37;
const LINE: u32 = 0x292E36;
const TEXT: u32 = 0xE7E9EC;
const SECONDARY: u32 = 0x9299A3;
const MUTED: u32 = 0x626A75;
const ACCENT: u32 = 0xE7A65A;

pub struct RootView {
    state: Entity<AppState>,
    sunburst: Sunburst,
    focus_handle: FocusHandle,
    list_scroll: UniformListScrollHandle,
}

struct WorkspaceRender<'a> {
    projection: &'a TreeProjection,
    source: DisplaySource,
    hovered: Option<u32>,
    selected: Option<u32>,
    center_note: &'static str,
    finished_with_gaps: bool,
}

impl RootView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let restoring_state = state.clone();
        cx.defer(move |cx| {
            restoring_state.update(cx, |state, cx| state.restore_last_root(cx));
        });
        Self {
            state,
            sunburst: Sunburst::new(),
            focus_handle: cx.focus_handle(),
            list_scroll: UniformListScrollHandle::new(),
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn pick_folder(&self, cx: &mut Context<Self>) {
        use gpui::PathPromptOptions;
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose a folder to analyze".into()),
        });
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                if let Some(path) = paths.into_iter().next() {
                    state.update(cx, |state, cx| state.open_path(path, cx));
                }
            }
        })
        .detach();
    }

    fn render_empty(&self, loading: bool, cx: &mut Context<Self>) -> AnyElement {
        let headline = if loading {
            "Opening folder"
        } else {
            "See what occupies your disk"
        };
        let detail = if loading {
            "Checking the last complete snapshot before scanning."
        } else {
            "Choose a folder. Daggy maps its largest branches first, then completes the count quietly in the background."
        };
        let mut panel = div()
            .w(px(480.))
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .size(px(112.))
                    .rounded_full()
                    .border_1()
                    .border_color(gpui::rgb(0x4B4034))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(px(72.))
                            .rounded_full()
                            .border_1()
                            .border_color(gpui::rgb(0x7B5E3D))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().size(px(30.)).rounded_full().bg(gpui::rgb(ACCENT))),
                    ),
            )
            .child(
                div()
                    .mt(px(28.))
                    .text_size(px(26.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(gpui::rgb(TEXT))
                    .child(headline),
            )
            .child(
                div()
                    .mt(px(10.))
                    .w(px(430.))
                    .text_center()
                    .text_sm()
                    .line_height(relative(1.55))
                    .text_color(gpui::rgb(SECONDARY))
                    .child(detail),
            );
        if !loading {
            panel = panel.child(
                div()
                    .id("empty-choose-folder")
                    .mt(px(24.))
                    .h(px(40.))
                    .px(px(18.))
                    .rounded(px(8.))
                    .bg(gpui::rgb(ACCENT))
                    .text_color(gpui::rgb(0x17120C))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|style| style.bg(gpui::rgb(0xF0B66F)))
                    .active(|style| style.opacity(0.78))
                    .on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx)))
                    .child("Choose folder"),
            );
        }
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgb(CANVAS))
            .child(panel)
            .into_any()
    }

    fn render_workspace(
        &mut self,
        workspace: WorkspaceRender<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let WorkspaceRender {
            projection,
            source,
            hovered,
            selected,
            center_note,
            finished_with_gaps,
        } = workspace;
        let focus = projection.focus_node.clone();
        let rows = Arc::new(projection.rows.clone());
        let row_count = rows.len();
        let total = focus.as_ref().map_or(0, |node| node.block_size).max(1);
        let row_state = self.state.downgrade();
        let scroll = self.list_scroll.clone();
        let list = uniform_list(
            "content-rows",
            row_count,
            cx.processor(move |_this, range: std::ops::Range<usize>, _window, _cx| {
                let mut elements = Vec::with_capacity(range.len());
                for index in range {
                    if let Some(row) = rows.get(index) {
                        elements.push(render_row(
                            row.clone(),
                            total,
                            selected == Some(row.id),
                            hovered == Some(row.id),
                            row_state.clone(),
                        ));
                    }
                }
                elements
            }),
        )
        .track_scroll(&scroll)
        .size_full();

        let focus_name = focus
            .as_ref()
            .map(NodeView::display_name)
            .unwrap_or_else(|| "Folder".into());
        let focus_size = fmt::human_bytes(focus.as_ref().map_or(0, |node| node.block_size));
        let focus_items = focus.as_ref().map_or(0, |node| node.child_count);
        let completeness = if source == DisplaySource::Cached {
            "Cached"
        } else if finished_with_gaps {
            "Incomplete"
        } else if focus.as_ref().is_some_and(|node| node.complete) {
            "Complete"
        } else {
            "Calculating"
        };

        div()
            .size_full()
            .flex()
            .min_h(px(0.))
            .bg(gpui::rgb(CANVAS))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .size_full()
                    .child(self.sunburst.render(
                        projection,
                        hovered,
                        selected,
                        center_note,
                        self.state.downgrade(),
                    )),
            )
            .child(
                div()
                    .w(px(408.))
                    .h_full()
                    .flex_shrink_0()
                    .border_l_1()
                    .border_color(gpui::rgb(LINE))
                    .bg(gpui::rgb(SURFACE))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px(px(20.))
                            .pt(px(20.))
                            .pb(px(16.))
                            .border_b_1()
                            .border_color(gpui::rgb(LINE))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .child(div().size(px(8.)).rounded_full().bg(gpui::rgb(
                                        if finished_with_gaps {
                                            0xB86F58
                                        } else if source == DisplaySource::Live {
                                            0x809A77
                                        } else {
                                            ACCENT
                                        },
                                    )))
                                    .child(
                                        div()
                                            .ml(px(8.))
                                            .text_xs()
                                            .text_color(gpui::rgb(SECONDARY))
                                            .child(completeness),
                                    ),
                            )
                            .child(
                                div()
                                    .mt(px(10.))
                                    .text_size(px(20.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(gpui::rgb(TEXT))
                                    .truncate()
                                    .child(focus_name),
                            )
                            .child(
                                div()
                                    .mt(px(8.))
                                    .flex()
                                    .items_end()
                                    .child(
                                        div()
                                            .font_family("SF Mono")
                                            .text_size(px(24.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(gpui::rgb(TEXT))
                                            .child(focus_size),
                                    )
                                    .child(
                                        div()
                                            .ml(px(10.))
                                            .mb(px(3.))
                                            .text_xs()
                                            .text_color(gpui::rgb(MUTED))
                                            .child(format!(
                                                "{} direct items",
                                                fmt::human_count(focus_items as u64)
                                            )),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .h(px(34.))
                            .px(px(16.))
                            .flex()
                            .items_center()
                            .border_b_1()
                            .border_color(gpui::rgb(LINE))
                            .text_xs()
                            .text_color(gpui::rgb(MUTED))
                            .child(div().flex_1().child("Contents"))
                            .child(div().w(px(82.)).text_right().child("Size"))
                            .child(div().w(px(48.)).text_right().child("Share")),
                    )
                    .child(div().flex_1().min_h(px(0.)).child(list))
                    .when(projection.omitted_rows > 0, |panel| {
                        panel.child(
                            div()
                                .h(px(32.))
                                .px(px(16.))
                                .border_t_1()
                                .border_color(gpui::rgb(LINE))
                                .flex()
                                .items_center()
                                .text_xs()
                                .text_color(gpui::rgb(MUTED))
                                .child(format!(
                                    "{} smaller items grouped",
                                    fmt::human_count(projection.omitted_rows as u64)
                                )),
                        )
                    }),
            )
            .into_any()
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (
            projection,
            source,
            hovered,
            selected,
            status,
            root_path,
            center_note,
            finished_with_gaps,
        ) = {
            let state = self.state.read(cx);
            let projection = state.projection();
            let status = state.status_line(&projection);
            let center_note = state.center_note(&projection);
            (
                projection,
                state.source,
                state.hovered,
                state.selected,
                status,
                state.root_path.clone(),
                center_note,
                !state.scanning && state.status.errors > 0,
            )
        };
        let breadcrumb = if projection.ancestors.is_empty() {
            root_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            projection
                .ancestors
                .iter()
                .map(NodeView::display_name)
                .collect::<Vec<_>>()
                .join("  /  ")
        };
        let loading = source == DisplaySource::Loading;
        let content = if projection.focus.is_some() {
            self.render_workspace(
                WorkspaceRender {
                    projection: &projection,
                    source,
                    hovered,
                    selected,
                    center_note,
                    finished_with_gaps,
                },
                cx,
            )
        } else {
            self.render_empty(loading, cx)
        };

        div()
            .id("daggy-root")
            .track_focus(&self.focus_handle)
            .focusable()
            .size_full()
            .min_w(px(940.))
            .min_h(px(620.))
            .font_family("SF Pro Text")
            .bg(gpui::rgb(CANVAS))
            .text_color(gpui::rgb(TEXT))
            .flex()
            .flex_col()
            .on_action(cx.listener(|this, _: &crate::GoUp, _, cx| {
                this.state.update(cx, |state, cx| state.go_up(cx));
            }))
            .on_action(cx.listener(|this, _: &crate::SelectPrevious, _, cx| {
                if let Some(index) = this
                    .state
                    .update(cx, |state, cx| state.move_selection(-1, cx))
                {
                    this.list_scroll
                        .scroll_to_item(index, ScrollStrategy::Nearest);
                }
            }))
            .on_action(cx.listener(|this, _: &crate::SelectNext, _, cx| {
                if let Some(index) = this
                    .state
                    .update(cx, |state, cx| state.move_selection(1, cx))
                {
                    this.list_scroll
                        .scroll_to_item(index, ScrollStrategy::Nearest);
                }
            }))
            .on_action(cx.listener(|this, _: &crate::ActivateSelection, _, cx| {
                this.state
                    .update(cx, |state, cx| state.activate_selection(cx));
            }))
            .on_action(cx.listener(|this, _: &crate::OpenFolder, _, cx| {
                this.pick_folder(cx);
            }))
            .child(
                div()
                    .h(px(56.))
                    .flex_shrink_0()
                    .pl(px(84.))
                    .pr(px(14.))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(gpui::rgb(LINE))
                    .bg(gpui::rgb(SURFACE))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(gpui::rgb(TEXT))
                            .child("daggy"),
                    )
                    .child(div().mx(px(14.)).h(px(18.)).w(px(1.)).bg(gpui::rgb(LINE)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_sm()
                            .text_color(gpui::rgb(SECONDARY))
                            .child(SharedString::from(breadcrumb)),
                    )
                    .child(
                        div()
                            .id("choose-folder")
                            .ml(px(14.))
                            .h(px(40.))
                            .px(px(14.))
                            .rounded(px(8.))
                            .bg(gpui::rgb(ELEVATED))
                            .text_color(gpui::rgb(TEXT))
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|style| style.bg(gpui::rgb(HOVER)))
                            .active(|style| style.opacity(0.72))
                            .on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx)))
                            .child("Choose folder"),
                    ),
            )
            .child(div().flex_1().min_h(px(0.)).child(content))
            .child(
                div()
                    .h(px(30.))
                    .flex_shrink_0()
                    .px(px(14.))
                    .border_t_1()
                    .border_color(gpui::rgb(LINE))
                    .bg(gpui::rgb(SURFACE))
                    .flex()
                    .items_center()
                    .text_xs()
                    .text_color(gpui::rgb(SECONDARY))
                    .child(div().flex_1().child(status))
                    .child(
                        div()
                            .font_family("SF Mono")
                            .text_color(gpui::rgb(MUTED))
                            .child("↑↓ select   return open   esc up   ⌘O folder"),
                    ),
            )
    }
}

fn render_row(
    row: NodeView,
    total: u64,
    selected: bool,
    hovered: bool,
    state: gpui::WeakEntity<AppState>,
) -> AnyElement {
    let id = row.id;
    let share = row.block_size as f32 / total as f32;
    let percentage = if share > 0.0 && share < 0.01 {
        "<1%".to_string()
    } else {
        format!("{:.0}%", share * 100.0)
    };
    let size = format!(
        "{}{}",
        if row.complete { "" } else { "~" },
        fmt::human_bytes(row.block_size)
    );
    let name = SharedString::from(row.display_name());
    let hover_state = state.clone();
    let click_state = state.clone();
    div()
        .id(gpui::ElementId::NamedInteger(
            "content-row".into(),
            id as u64,
        ))
        .relative()
        .h(px(44.))
        .w_full()
        .flex()
        .items_center()
        .px(px(16.))
        .border_b_1()
        .border_color(gpui::rgb(0x20242A))
        .bg(gpui::rgb(if selected {
            SELECTED
        } else if hovered {
            HOVER
        } else {
            SURFACE
        }))
        .cursor_pointer()
        .hover(|style| style.bg(gpui::rgb(HOVER)))
        .active(|style| style.opacity(0.76))
        .on_hover(move |is_hovered, _, cx| {
            let _ = hover_state.update(cx, |state, cx| {
                state.set_hovered((*is_hovered).then_some(id), cx)
            });
        })
        .on_click(move |_, _, cx| {
            let _ = click_state.update(cx, |state, cx| state.activate_node(id, cx));
        })
        .child(
            div()
                .size(px(10.))
                .mr(px(10.))
                .rounded(px(if row.kind == Kind::Dir { 3. } else { 5. }))
                .border_1()
                .border_color(gpui::rgb(if row.kind == Kind::Dir {
                    ACCENT
                } else {
                    0x697786
                }))
                .bg(gpui::rgb(if row.kind == Kind::Dir {
                    0x35291D
                } else {
                    0x28313A
                })),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_sm()
                .text_color(gpui::rgb(TEXT))
                .child(name),
        )
        .child(
            div()
                .w(px(82.))
                .text_right()
                .font_family("SF Mono")
                .text_xs()
                .text_color(gpui::rgb(SECONDARY))
                .child(size),
        )
        .child(
            div()
                .w(px(48.))
                .text_right()
                .font_family("SF Mono")
                .text_xs()
                .text_color(gpui::rgb(MUTED))
                .child(percentage),
        )
        .child(
            div()
                .absolute()
                .bottom(px(0.))
                .left(px(0.))
                .h(px(1.))
                .w(relative(share.clamp(0.0, 1.0)))
                .bg(gpui::rgb(0x8B6842)),
        )
        .into_any()
}
