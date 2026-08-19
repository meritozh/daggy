//! Native macOS shell: sidebar locations, unified titlebar, map, inspector.

use std::path::PathBuf;
use std::sync::Arc;

use daggy_core::{fmt, Kind, NodeView, TreeProjection};
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, relative, uniform_list, AnyElement, Context, CursorStyle, Element, Entity, FocusHandle,
    FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement, Render,
    ScrollStrategy, SharedString, StatefulInteractiveElement, Styled, UniformListScrollHandle,
    Window, WindowControlArea,
};

use crate::app_state::{AppState, DisplaySource};
use crate::locations::{
    display_name, location_is_active, sidebar_locations, window_title, Location,
};
use crate::sf_symbol::{self, Glyph};
use crate::sunburst::Sunburst;
use crate::theme::{native_chrome, Theme};

pub struct RootView {
    state: Entity<AppState>,
    sunburst: Sunburst,
    theme: Theme,
    focus_handle: FocusHandle,
    list_scroll: UniformListScrollHandle,
    _appearance: gpui::Subscription,
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
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let appearance = cx.observe_window_appearance(window, |this, window, cx| {
            this.theme = Theme::from_appearance(window.appearance());
            cx.notify();
        });
        let startup = state.clone();
        cx.defer(move |cx| {
            if let Some(path) = std::env::args().nth(1).map(PathBuf::from) {
                if path.is_dir() {
                    startup.update(cx, |state, cx| state.open_path(path, cx));
                    return;
                }
            }
            startup.update(cx, |state, cx| state.restore_last_root(cx));
        });
        Self {
            state,
            sunburst: Sunburst::new(),
            theme: Theme::from_appearance(window.appearance()),
            focus_handle: cx.focus_handle(),
            list_scroll: UniformListScrollHandle::new(),
            _appearance: appearance,
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

    fn render_sidebar(&self, root_path: Option<&PathBuf>) -> impl IntoElement {
        let theme = self.theme;
        let chrome = native_chrome();
        let locations = sidebar_locations(root_path.map(PathBuf::as_path));
        div()
            .w(chrome.sidebar_width)
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(drag_strip("sidebar-drag", theme.sidebar).h(chrome.titlebar_height))
            .child(
                div()
                    .px(px(16.0))
                    .pt(px(2.0))
                    .pb(px(6.0))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_tertiary)
                    .child("Favorites"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .px(px(8.0))
                    .children(locations.into_iter().map(|location| {
                        sidebar_row(location, root_path, theme, self.state.downgrade())
                    })),
            )
            .child(div().flex_1())
    }

    fn render_empty(&self, loading: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let headline = if loading {
            "Opening Folder"
        } else {
            "No Folder Selected"
        };
        let detail = if loading {
            "Checking the last complete snapshot."
        } else {
            "Choose a favorite in the sidebar or open a folder to measure allocated space."
        };
        let mut panel = div()
            .w(px(360.0))
            .flex()
            .flex_col()
            .child(
                div()
                    .text_size(px(17.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(headline),
            )
            .child(
                div()
                    .mt(px(6.0))
                    .text_size(px(13.0))
                    .line_height(relative(1.4))
                    .text_color(theme.text_secondary)
                    .child(detail),
            );
        if !loading {
            panel = panel.child(
                div()
                    .id("empty-choose-folder")
                    .mt(px(16.0))
                    .h(px(21.0))
                    .px(px(14.0))
                    .rounded(px(5.0))
                    .bg(theme.accent)
                    .text_color(theme.on_selection)
                    .font_weight(FontWeight::MEDIUM)
                    .text_size(px(13.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor(CursorStyle::Arrow)
                    .hover(|style| style.opacity(0.92))
                    .active(|style| style.opacity(0.78))
                    .on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx)))
                    .child("Open…"),
            );
        }
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.canvas)
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
        let theme = self.theme;
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
                            theme,
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
            .map(node_label)
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
        let status_color = if finished_with_gaps {
            theme.danger
        } else if source == DisplaySource::Live && focus.as_ref().is_some_and(|node| node.complete) {
            theme.success
        } else {
            theme.warning
        };

        div()
            .size_full()
            .flex()
            .min_h(px(0.0))
            .bg(theme.canvas)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .size_full()
                    .child(self.sunburst.render(
                        projection,
                        hovered,
                        selected,
                        center_note,
                        theme,
                        self.state.downgrade(),
                    )),
            )
            .child(
                div()
                    .w(px(332.0))
                    .h_full()
                    .flex_shrink_0()
                    .border_l_1()
                    .border_color(theme.border)
                    .bg(theme.surface)
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px(px(16.0))
                            .pt(px(16.0))
                            .pb(px(12.0))
                            .border_b_1()
                            .border_color(theme.border)
                            .bg(theme.raised)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .child(
                                        div()
                                            .size(px(6.0))
                                            .rounded_full()
                                            .bg(status_color),
                                    )
                                    .child(
                                        div()
                                            .ml(px(7.0))
                                            .text_xs()
                                            .text_color(theme.text_tertiary)
                                            .child(completeness),
                                    ),
                            )
                            .child(
                                div()
                                    .mt(px(8.0))
                                    .text_size(px(15.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .truncate()
                                    .child(focus_name),
                            )
                            .child(
                                div()
                                    .mt(px(4.0))
                                    .flex()
                                    .items_end()
                                    .child(
                                        div()
                                            .text_size(px(20.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.text)
                                            .child(focus_size),
                                    )
                                    .child(
                                        div()
                                            .ml(px(8.0))
                                            .mb(px(3.0))
                                            .text_xs()
                                            .text_color(theme.text_tertiary)
                                            .child(format!(
                                                "{} items",
                                                fmt::human_count(focus_items as u64)
                                            )),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .h(px(22.0))
                            .px(px(16.0))
                            .flex()
                            .items_center()
                            .border_b_1()
                            .border_color(theme.border)
                            .bg(theme.inset)
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_tertiary)
                            .child(div().flex_1().child("Name"))
                            .child(div().w(px(48.0)))
                            .child(div().w(px(72.0)).text_right().child("Size"))
                            .child(div().w(px(40.0)).text_right().child("%")),
                    )
                    .child(div().flex_1().min_h(px(0.0)).bg(theme.surface).child(list))
                    .when(projection.omitted_rows > 0, |panel| {
                        panel.child(
                            div()
                                .h(px(26.0))
                                .px(px(16.0))
                                .border_t_1()
                                .border_color(theme.border)
                                .flex()
                                .items_center()
                                .text_xs()
                                .text_color(theme.text_tertiary)
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let chrome = native_chrome();
        let (
            projection,
            source,
            hovered,
            selected,
            status,
            root_path,
            center_note,
            finished_with_gaps,
            can_go_up,
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
                state.can_go_up(),
            )
        };
        let title = projection
            .focus_node
            .as_ref()
            .map(node_label)
            .unwrap_or_else(|| window_title(root_path.as_deref()));
        window.set_window_title(&title);
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
            .min_w(chrome.min_size.width)
            .min_h(chrome.min_size.height)
            .font_family("SF Pro Text")
            .text_color(theme.text)
            .flex()
            .flex_row()
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
            .on_action(cx.listener(|_this, _: &crate::About, window, cx| {
                let _ = window.prompt(
                    gpui::PromptLevel::Info,
                    "Daggy",
                    Some("A local disk usage analyzer."),
                    &["OK"],
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &crate::CopySelection, _, cx| {
                if let Some(text) = this.state.read(cx).copy_label() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                }
            }))
            .on_action(cx.listener(|this, _: &crate::RevealInFinder, _, cx| {
                this.state.read(cx).reveal_in_finder();
            }))
            .on_action(cx.listener(|_, _: &crate::MinimizeWindow, window, _| {
                window.minimize_window();
            }))
            .on_action(cx.listener(|_, _: &crate::ZoomWindow, window, _| {
                window.zoom_window();
            }))
            .on_action(cx.listener(|_, _: &crate::CloseWindow, window, _| {
                window.remove_window();
            }))
            .child(self.render_sidebar(root_path.as_ref()))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(theme.canvas)
                    .child(
                        drag_strip("content-drag", theme.surface)
                            .h(chrome.titlebar_height)
                            .px(px(12.0))
                            .border_b_1()
                            .border_color(theme.border)
                            .child(
                                div()
                                    .id("go-up")
                                    .w(px(28.0))
                                    .h(px(22.0))
                                    .mr(px(8.0))
                                    .rounded(px(5.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(16.0))
                                    .text_color(if can_go_up {
                                        theme.text
                                    } else {
                                        theme.text_ghost
                                    })
                                    .cursor(CursorStyle::Arrow)
                                    .when(can_go_up, |button| {
                                        button
                                            .hover(|style| style.bg(theme.overlay))
                                            .active(|style| style.opacity(0.78))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.state.update(cx, |state, cx| state.go_up(cx));
                                            }))
                                    })
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .child(symbol_or_mark(
                                        Glyph::ChevronLeft,
                                        11.0,
                                        if can_go_up {
                                            theme.icon_rgb
                                        } else {
                                            0xAEAEB2
                                        },
                                        theme.folder,
                                    )),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .child(SharedString::from(title)),
                            )
                            .child(
                                div()
                                    .id("choose-folder")
                                    .ml(px(12.0))
                                    .h(px(22.0))
                                    .px(px(8.0))
                                    .rounded(px(5.0))
                                    .text_color(theme.accent)
                                    .text_size(px(13.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::Arrow)
                                    .hover(|style| style.bg(theme.overlay))
                                    .active(|style| style.opacity(0.78))
                                    .on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx)))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .gap(px(4.0))
                                    .child(symbol_or_mark(
                                        Glyph::Open,
                                        11.0,
                                        theme.icon_rgb,
                                        theme.folder,
                                    ))
                                    .child("Open…"),
                            ),
                    )
                    .child(div().flex_1().min_h(px(0.0)).child(content))
                    .child(
                        div()
                            .h(px(22.0))
                            .flex_shrink_0()
                            .px(px(12.0))
                            .border_t_1()
                            .border_color(theme.border)
                            .bg(theme.surface)
                            .flex()
                            .items_center()
                            .text_xs()
                            .text_color(theme.text_tertiary)
                            .child(status),
                    ),
            )
    }
}

fn node_label(node: &NodeView) -> String {
    display_name(std::path::Path::new(node.name.as_ref()))
}

fn symbol_or_mark(
    glyph: Glyph,
    size: f32,
    color: u32,
    fallback: gpui::Hsla,
) -> AnyElement {
    if let Some(icon) = sf_symbol::symbol_img(glyph, px(size), color) {
        icon.into_any()
    } else {
        sf_symbol::fallback_mark(px(size), fallback).into_any()
    }
}

fn drag_strip(id: &'static str, background: Hsla) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .window_control_area(WindowControlArea::Drag)
        .flex()
        .items_center()
        .flex_shrink_0()
        .cursor(CursorStyle::Arrow)
        .on_mouse_down(MouseButton::Left, |_, window, _| {
            window.start_window_move();
        })
        .on_click(|event, window, _| {
            if event.click_count() == 2 {
                window.titlebar_double_click();
            }
        })
        .bg(background)
}

fn sidebar_row(
    location: Location,
    current: Option<&PathBuf>,
    theme: Theme,
    state: gpui::WeakEntity<AppState>,
) -> impl IntoElement {
    let active = location_is_active(&location.path, current.map(PathBuf::as_path));
    let path = location.path.clone();
    let name = SharedString::from(location.name);
    div()
        .id(SharedString::from(format!("loc-{}", location.id)))
        .h(px(28.0))
        .px(px(8.0))
        .mb(px(1.0))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .bg(if active {
            theme.overlay_strong
        } else {
            gpui::transparent_black()
        })
        .cursor(CursorStyle::Arrow)
        .hover(|style| style.bg(theme.overlay))
        .active(|style| style.opacity(0.78))
        .on_click(move |_, _, cx| {
            let path = path.clone();
            let _ = state.update(cx, |state, cx| state.open_path(path, cx));
        })
        .child(
            div()
                .w(px(16.0))
                .mr(px(8.0))
                .flex()
                .items_center()
                .justify_center()
                .child(symbol_or_mark(
                    sf_symbol::glyph_for_location(location.id),
                    13.0,
                    theme.icon_rgb,
                    theme.folder,
                )),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(13.0))
                .text_color(theme.text)
                .child(name),
        )
}

fn render_row(
    row: NodeView,
    total: u64,
    selected: bool,
    hovered: bool,
    theme: Theme,
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
    let click_state = state;
    div()
        .id(gpui::ElementId::NamedInteger(
            "content-row".into(),
            id as u64,
        ))
        .relative()
        .h(px(24.0))
        .w_full()
        .flex()
        .items_center()
        .px(px(16.0))
        .bg(if selected {
            theme.selection
        } else if hovered {
            theme.overlay
        } else {
            gpui::transparent_black()
        })
        .cursor(CursorStyle::Arrow)
        .hover(|style| {
            if selected {
                style
            } else {
                style.bg(theme.overlay)
            }
        })
        .active(|style| style.opacity(0.78))
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
                .w(px(16.0))
                .mr(px(8.0))
                .flex()
                .items_center()
                .justify_center()
                .child(symbol_or_mark(
                    if row.kind == Kind::Dir {
                        Glyph::Folder
                    } else {
                        Glyph::File
                    },
                    13.0,
                    if selected {
                        0xFFFFFF
                    } else if row.kind == Kind::Dir {
                        theme.icon_rgb
                    } else {
                        0x8E8E93
                    },
                    if selected {
                        theme.on_selection
                    } else if row.kind == Kind::Dir {
                        theme.folder
                    } else {
                        theme.text_ghost
                    },
                )),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(13.0))
                .text_color(if selected {
                    theme.on_selection
                } else {
                    theme.text
                })
                .child(name),
        )
        .child(
            div()
                .w(px(48.0))
                .h(px(4.0))
                .rounded(px(2.0))
                .bg(if selected {
                    theme.on_selection.opacity(0.28)
                } else {
                    theme.border_strong
                })
                .child(
                    div()
                        .h_full()
                        .w(relative(share.clamp(0.0, 1.0)))
                        .rounded(px(2.0))
                        .bg(if selected {
                            theme.on_selection
                        } else {
                            theme.accent
                        }),
                ),
        )
        .child(
            div()
                .w(px(72.0))
                .text_right()
                .text_xs()
                .text_color(if selected {
                    theme.on_selection
                } else {
                    theme.text_secondary
                })
                .child(size),
        )
        .child(
            div()
                .w(px(40.0))
                .text_right()
                .text_xs()
                .text_color(if selected {
                    theme.on_selection.opacity(0.85)
                } else {
                    theme.text_tertiary
                })
                .child(percentage),
        )
        .into_any()
}
