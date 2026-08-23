//! Pencil shell: unified titlebar, favorites sidebar, map, inspector.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use daggy_core::{fmt, NodeId, NodeView, TreeProjection};
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, relative, uniform_list, AnyElement, Context, CursorStyle, Element, Entity,
    FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, Render, ScrollStrategy, SharedString, StatefulInteractiveElement, Styled,
    UniformListScrollHandle, Window, WindowControlArea,
};

use crate::app_state::{AppState, DisplaySource};
use crate::locations::{
    display_name, location_is_active, sidebar_locations, window_title, Location,
};
use crate::sf_symbol::{self, Glyph};
use crate::sunburst::Sunburst;
use crate::theme::{native_chrome, Theme};
use crate::volume::{volume_info, VolumeInfo};

pub struct RootView {
    state: Entity<AppState>,
    sunburst: Sunburst,
    theme: Theme,
    focus_handle: FocusHandle,
    list_scroll: UniformListScrollHandle,
    sidebar_open: bool,
    filter: Option<String>,
    filter_matches: RefCell<Option<(Arc<Vec<NodeId>>, String, Arc<Vec<usize>>)>>,
    _appearance: gpui::Subscription,
}

struct WorkspaceRender<'a> {
    projection: &'a TreeProjection,
    source: DisplaySource,
    hovered: Option<u32>,
    selected: Option<u32>,
    center_note: SharedString,
    files: u64,
    trash: Option<u64>,
    status: String,
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
            sidebar_open: true,
            filter: None,
            filter_matches: RefCell::new(None),
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

    fn render_titlebar(
        &self,
        title: SharedString,
        has_root: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let chrome = native_chrome();
        drag_strip("titlebar-drag", theme.surface)
            .h(chrome.titlebar_height)
            .px(px(16.0))
            .gap(px(12.0))
            .border_b_1()
            .border_color(theme.border)
            .child(div().w(px(52.0)).h(px(12.0)).flex_shrink_0())
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(title),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(toolbar_button(
                        "rescan",
                        Glyph::Refresh,
                        has_root,
                        theme,
                        cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |state, cx| state.rescan(cx));
                        }),
                    ))
                    .child(toolbar_button(
                        "reveal",
                        Glyph::Reveal,
                        has_root,
                        theme,
                        cx.listener(|this, _, _, cx| {
                            this.state.read(cx).reveal_in_finder();
                        }),
                    ))
                    .child(toolbar_button(
                        "search",
                        Glyph::Search,
                        has_root,
                        theme,
                        cx.listener(|this, _, _, cx| this.toggle_filter(cx)),
                    ))
                    .child(toolbar_button(
                        "sidebar",
                        Glyph::Sidebar,
                        true,
                        theme,
                        cx.listener(|this, _, _, cx| {
                            this.sidebar_open = !this.sidebar_open;
                            cx.notify();
                        }),
                    )),
            )
    }

    fn render_sidebar(
        &self,
        root_path: Option<&PathBuf>,
        volume: Option<&VolumeInfo>,
    ) -> impl IntoElement {
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
            .border_color(theme.sidebar_border)
            .px(px(12.0))
            .pt(px(16.0))
            .pb(px(16.0))
            .gap(px(4.0))
            .child(section_label("FAVORITES", theme))
            .children(
                locations.into_iter().map(|location| {
                    sidebar_row(location, root_path, theme, self.state.downgrade())
                }),
            )
            .child(div().flex_1())
            .when_some(volume.cloned(), |sidebar, volume| {
                sidebar
                    .child(section_label("LOCATIONS", theme))
                    .child(volume_card(
                        volume,
                        root_path,
                        theme,
                        self.state.downgrade(),
                    ))
            })
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
            "Choose a favorite, a volume, or open a folder to measure allocated space."
        };
        let mut panel = div()
            .w(px(360.0))
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .text_size(px(26.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(headline),
            )
            .child(
                div()
                    .mt(px(8.0))
                    .text_size(px(13.0))
                    .line_height(relative(1.4))
                    .text_color(theme.text_secondary)
                    .text_center()
                    .child(detail),
            );
        if !loading {
            panel = panel.child(
                div()
                    .id("empty-choose-folder")
                    .mt(px(20.0))
                    .h(px(32.0))
                    .px(px(14.0))
                    .rounded(px(8.0))
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
            .flex_1()
            .min_w(px(0.0))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.canvas)
            .child(panel)
            .into_any()
    }

    fn render_inspector(
        &self,
        workspace: &WorkspaceRender<'_>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let chrome = native_chrome();
        let projection = workspace.projection;
        let subject_id = workspace
            .hovered
            .or(workspace.selected)
            .or(projection.focus);
        let subject = subject_id.and_then(|id| node_view(projection, id));
        let color = subject
            .as_ref()
            .and_then(|node| branch_color(projection, node.id, theme))
            .unwrap_or(theme.folder);
        let path = subject_id.and_then(|id| self.state.read(cx).path_of(id));
        let crumbs = path.as_deref().map(breadcrumb_parts).unwrap_or_default();
        let size = subject
            .as_ref()
            .map(|node| fmt::human_bytes(node.block_size))
            .unwrap_or_else(|| "0 B".into());
        let files = workspace.files;
        let trash = workspace.trash;
        let query = self
            .filter
            .as_ref()
            .map(|value| value.to_ascii_lowercase())
            .filter(|value| !value.is_empty());
        let ids = self.state.read(cx).ranked_child_ids();
        let matches = query
            .as_ref()
            .map(|query| self.filtered_indices(&ids, query, cx));
        let row_count = matches.as_ref().map_or(ids.len(), |indices| indices.len());
        let selected = workspace.selected;
        let hovered = workspace.hovered;
        let scroll = self.list_scroll.clone();
        let list_ids = ids.clone();
        let list_matches = matches.clone();
        let list = uniform_list(
            "content-rows",
            row_count,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let state = this.state.read(cx);
                let mut elements = Vec::with_capacity(range.len());
                for listed in range {
                    let rank = list_matches
                        .as_ref()
                        .and_then(|indices| indices.get(listed).copied())
                        .unwrap_or(listed);
                    let Some(id) = list_ids.get(rank).copied() else {
                        continue;
                    };
                    let Some(row) = state.node_view(id) else {
                        continue;
                    };
                    elements.push(render_row(
                        row,
                        rank,
                        selected == Some(id),
                        hovered == Some(id),
                        theme,
                        this.state.downgrade(),
                    ));
                }
                elements
            }),
        )
        .track_scroll(&scroll)
        .h_full()
        .w_full();

        let name = subject
            .as_ref()
            .map(|node| SharedString::from(node_label(node)))
            .unwrap_or_else(|| SharedString::from("No Folder"));
        let hint = inspector_hint(workspace.source, trash, &workspace.status, theme);

        div()
            .w(chrome.inspector_width)
            .h_full()
            .flex_shrink_0()
            .border_l_1()
            .border_color(theme.inspector_border)
            .bg(theme.inspector)
            .flex()
            .flex_col()
            .px(px(20.0))
            .pt(px(20.0))
            .pb(px(20.0))
            .gap(px(20.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(symbol_or_mark(Glyph::Folder, 22.0, rgb_of(color), color))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(17.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child(name),
                    ),
            )
            .child(breadcrumb(crumbs, theme, self.state.downgrade()))
            .child(
                div()
                    .w_full()
                    .flex()
                    .child(stat_block("SIZE", size, color, theme))
                    .child(stat_block(
                        "FILES",
                        fmt::human_count(files),
                        theme.text,
                        theme,
                    ))
                    .child(stat_block(
                        "TRASH",
                        trash.map(fmt::human_bytes).unwrap_or_else(|| "—".into()),
                        theme.text_secondary,
                        theme,
                    )),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child("Largest Items"),
                    )
                    .child(div().text_size(px(11.0)).text_color(theme.accent).child(
                        match self.filter.as_deref() {
                            Some("") => "Type to filter".to_string(),
                            Some(query) => format!("Filter: {query}"),
                            None => "Sort by size".to_string(),
                        },
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_hidden()
                    .child(list),
            )
            .when_some(hint, |panel, hint| panel.child(hint))
    }

    fn filtered_indices(
        &self,
        ids: &Arc<Vec<NodeId>>,
        query: &str,
        cx: &Context<Self>,
    ) -> Arc<Vec<usize>> {
        if let Some((cached_ids, cached_query, matches)) = self.filter_matches.borrow().as_ref() {
            if Arc::ptr_eq(cached_ids, ids) && cached_query == query {
                return matches.clone();
            }
        }
        let state = self.state.read(cx);
        let matches = Arc::new(
            ids.iter()
                .enumerate()
                .filter_map(|(index, id)| {
                    state
                        .node_view(*id)
                        .filter(|row| row.display_name().to_ascii_lowercase().contains(query))
                        .map(|_| index)
                })
                .collect::<Vec<_>>(),
        );
        *self.filter_matches.borrow_mut() = Some((ids.clone(), query.to_string(), matches.clone()));
        matches
    }

    fn toggle_filter(&mut self, cx: &mut Context<Self>) {
        if self.filter.is_some() {
            self.filter = None;
        } else {
            self.filter = Some(String::new());
        }
        cx.notify();
    }

    fn handle_filter_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(query) = self.filter.as_mut() else {
            return false;
        };
        if event.keystroke.modifiers.control
            || event.keystroke.modifiers.platform
            || event.keystroke.modifiers.alt
        {
            return false;
        }
        match event.keystroke.key.as_str() {
            "backspace" => {
                query.pop();
                cx.notify();
                true
            }
            "escape" => {
                self.filter = None;
                cx.notify();
                true
            }
            "up" | "down" | "enter" | "tab" => false,
            _ => {
                if let Some(character) = event.keystroke.key_char.as_ref() {
                    if !character.chars().any(|unit| unit.is_control()) && character != "\r" {
                        query.push_str(character);
                        cx.notify();
                        return true;
                    }
                }
                false
            }
        }
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let chrome = native_chrome();
        let (projection, source, hovered, selected, root_path, center_note, files, trash, status) = {
            let state = self.state.read(cx);
            let projection = state.projection();
            let subject = state.hovered.or(state.selected).or(projection.focus);
            let files = subject
                .map(|id| state.files_for(id, &projection))
                .unwrap_or(0);
            let note = state.center_note(&projection);
            let status = state.status_line(&projection);
            (
                projection,
                state.source,
                state.hovered,
                state.selected,
                state.root_path.clone(),
                note,
                files,
                state.trash_size(),
                status,
            )
        };
        let volume = volume_info(root_path.as_deref().unwrap_or(Path::new("/")));
        let os_title = projection
            .focus_node
            .as_ref()
            .map(node_label)
            .unwrap_or_else(|| window_title(root_path.as_deref()));
        window.set_window_title(&os_title);
        let bar_title = if let Some(volume) = volume.as_ref() {
            volume.title()
        } else if source == DisplaySource::Loading || source == DisplaySource::Cached {
            format!("{os_title} — scanning")
        } else {
            os_title
        };
        let focus_is_volume = volume
            .as_ref()
            .is_some_and(|volume| volume.is_mount(root_path.as_deref()));
        let map_note = if focus_is_volume {
            if let Some(volume) = volume.as_ref() {
                format!("of {} used", fmt::human_bytes(volume.total))
            } else {
                center_note.to_string()
            }
        } else {
            center_note.to_string()
        };
        let loading = source == DisplaySource::Loading;
        let has_map = projection.focus.is_some();
        let workspace = WorkspaceRender {
            projection: &projection,
            source,
            hovered,
            selected,
            center_note: SharedString::from(map_note),
            files,
            trash,
            status,
        };
        let map = if has_map {
            div()
                .flex_1()
                .min_w(px(0.0))
                .h_full()
                .child(self.sunburst.render(
                    &projection,
                    hovered,
                    selected,
                    workspace.center_note.clone(),
                    theme,
                    self.state.downgrade(),
                ))
                .into_any()
        } else {
            self.render_empty(loading, cx)
        };
        let content = div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .bg(theme.canvas)
            .child(map)
            .child(self.render_inspector(&workspace, cx))
            .into_any();

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
            .flex_col()
            .bg(theme.canvas)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                this.handle_filter_key(event, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::GoUp, _, cx| {
                if this.filter.take().is_some() {
                    cx.notify();
                    return;
                }
                if !this.state.read(cx).can_go_up() {
                    return;
                }
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
            .on_action(cx.listener(|this, _: &crate::Rescan, _, cx| {
                this.state.update(cx, |state, cx| state.rescan(cx));
            }))
            .on_action(cx.listener(|this, _: &crate::ToggleSidebar, _, cx| {
                this.sidebar_open = !this.sidebar_open;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &crate::FocusSearch, _, cx| {
                this.toggle_filter(cx);
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
            .child(self.render_titlebar(SharedString::from(bar_title), root_path.is_some(), cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .flex()
                    .flex_row()
                    .when(self.sidebar_open, |row| {
                        row.child(self.render_sidebar(root_path.as_ref(), volume.as_ref()))
                    })
                    .child(content),
            )
    }
}

fn node_label(node: &NodeView) -> String {
    display_name(Path::new(node.name.as_ref()))
}

fn node_view(projection: &TreeProjection, id: NodeId) -> Option<NodeView> {
    if let Some(node) = projection.focus_node.as_ref().filter(|node| node.id == id) {
        return Some(node.clone());
    }
    if let Some(row) = projection.rows.iter().find(|row| row.id == id) {
        return Some(row.clone());
    }
    projection.chart_node(id).map(|chart| chart.node.clone())
}

fn branch_color(projection: &TreeProjection, id: NodeId, theme: Theme) -> Option<Hsla> {
    let focus = projection.focus?;
    if id == focus {
        return None;
    }
    let mut cursor = Some(id);
    while let Some(current) = cursor {
        if let Some(index) = projection
            .chart_node(focus)
            .and_then(|chart| chart.children.iter().position(|child| *child == current))
        {
            return Some(theme.sunburst_color(index));
        }
        if let Some(index) = projection.rows.iter().position(|row| row.id == current) {
            return Some(theme.sunburst_color(index));
        }
        cursor = node_view(projection, current).and_then(|node| node.parent);
        if cursor == Some(focus) {
            break;
        }
    }
    None
}

fn rgb_of(color: Hsla) -> u32 {
    let rgba = color.to_rgb();
    let red = (rgba.r * 255.0).round() as u32;
    let green = (rgba.g * 255.0).round() as u32;
    let blue = (rgba.b * 255.0).round() as u32;
    (red << 16) | (green << 8) | blue
}

fn breadcrumb_parts(path: &Path) -> Vec<(String, PathBuf)> {
    let mut parts = Vec::new();
    let mut accumulated = PathBuf::new();
    for component in path.components() {
        accumulated.push(component.as_os_str());
        parts.push((display_name(&accumulated), accumulated.clone()));
    }
    if parts.len() > 3 {
        parts = parts.split_off(parts.len() - 3);
    }
    parts
}

fn section_label(label: &'static str, theme: Theme) -> impl IntoElement {
    div()
        .px(px(8.0))
        .pb(px(2.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text_tertiary)
        .child(label)
}

fn toolbar_button(
    id: &'static str,
    glyph: Glyph,
    enabled: bool,
    theme: Theme,
    click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let color = if enabled {
        theme.toolbar_icon_rgb
    } else {
        rgb_of(theme.text_ghost)
    };
    div()
        .id(id)
        .w(px(32.0))
        .h(px(26.0))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::Arrow)
        .when(enabled, |button| {
            button
                .hover(|style| style.bg(theme.overlay))
                .active(|style| style.opacity(0.78))
                .on_click(click)
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(symbol_or_mark(glyph, 15.0, color, theme.text_secondary))
}

fn symbol_or_mark(glyph: Glyph, size: f32, color: u32, fallback: Hsla) -> AnyElement {
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
        .rounded(px(6.0))
        .flex()
        .items_center()
        .gap(px(8.0))
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
        .child(symbol_or_mark(
            sf_symbol::glyph_for_location(location.id),
            15.0,
            theme.icon_rgb,
            theme.folder,
        ))
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

fn volume_card(
    volume: VolumeInfo,
    current: Option<&PathBuf>,
    theme: Theme,
    state: gpui::WeakEntity<AppState>,
) -> impl IntoElement {
    let active = volume.is_mount(current.map(PathBuf::as_path));
    let path = volume.mount.clone();
    let share = volume.share().clamp(0.0, 1.0);
    div()
        .id("volume-card")
        .w_full()
        .rounded(px(8.0))
        .px(px(10.0))
        .py(px(8.0))
        .bg(if active {
            theme.volume_selected
        } else {
            gpui::transparent_black()
        })
        .when(active, |card| card.border_1().border_color(theme.accent))
        .cursor(CursorStyle::Arrow)
        .hover(|style| style.bg(theme.volume_selected))
        .active(|style| style.opacity(0.78))
        .on_click(move |_, _, cx| {
            let path = path.clone();
            let _ = state.update(cx, |state, cx| state.open_path(path, cx));
        })
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(symbol_or_mark(Glyph::Drive, 15.0, 0x3E8EF7, theme.accent))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                                .child(SharedString::from(volume.name.clone())),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(5.0))
                        .rounded(px(3.0))
                        .bg(theme.track)
                        .child(
                            div()
                                .h_full()
                                .w(relative(share))
                                .rounded(px(3.0))
                                .bg(theme.accent),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.accent)
                                .child(format!("{} used", fmt::human_bytes(volume.used()))),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.text_secondary)
                                .child(format!("{} free", fmt::human_bytes(volume.free))),
                        ),
                ),
        )
}

fn breadcrumb(
    parts: Vec<(String, PathBuf)>,
    theme: Theme,
    state: gpui::WeakEntity<AppState>,
) -> impl IntoElement {
    let last = parts.len().saturating_sub(1);
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .children(
            parts
                .into_iter()
                .enumerate()
                .flat_map(move |(index, (label, path))| {
                    let state = state.clone();
                    let is_last = index == last;
                    let mut items = Vec::new();
                    if index > 0 {
                        items.push(symbol_or_mark(
                            Glyph::ChevronRight,
                            11.0,
                            theme.toolbar_icon_rgb,
                            theme.text_secondary,
                        ));
                    }
                    items.push(
                        div()
                            .id(SharedString::from(format!("crumb-{index}")))
                            .text_size(px(12.0))
                            .text_color(if is_last {
                                theme.text
                            } else {
                                theme.text_secondary
                            })
                            .cursor(CursorStyle::Arrow)
                            .hover(|style| style.opacity(0.8))
                            .active(|style| style.opacity(0.7))
                            .on_click(move |_, _, cx| {
                                let path = path.clone();
                                let _ = state.update(cx, |state, cx| state.open_or_focus(path, cx));
                            })
                            .child(label)
                            .into_any(),
                    );
                    items
                }),
        )
}

fn stat_block(
    label: &'static str,
    value: String,
    value_color: Hsla,
    theme: Theme,
) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .text_size(px(18.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(value_color)
                .child(value),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(theme.text_secondary)
                .child(label),
        )
}

fn inspector_hint(
    source: DisplaySource,
    trash: Option<u64>,
    status: &str,
    theme: Theme,
) -> Option<AnyElement> {
    let trash_hint = trash
        .filter(|bytes| *bytes > 0)
        .filter(|_| source == DisplaySource::Live);
    let (glyph, color, rgb, text) = if let Some(trash) = trash_hint {
        (
            Glyph::Trash,
            theme.warning,
            rgb_of(theme.warning),
            format!("{} can be freed from Trash", fmt::human_bytes(trash)),
        )
    } else if source == DisplaySource::Cached || source == DisplaySource::Loading {
        (
            Glyph::Refresh,
            theme.warning,
            rgb_of(theme.warning),
            status.to_string(),
        )
    } else if status.contains("unreadable") {
        (
            Glyph::Refresh,
            theme.danger,
            rgb_of(theme.danger),
            status.to_string(),
        )
    } else {
        return None;
    };
    Some(
        div()
            .w_full()
            .rounded(px(8.0))
            .px(px(12.0))
            .py(px(10.0))
            .bg(theme.hint)
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(symbol_or_mark(glyph, 15.0, rgb, color))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .text_color(theme.text_secondary)
                    .child(text),
            )
            .into_any(),
    )
}

fn render_row(
    row: NodeView,
    color_index: usize,
    selected: bool,
    hovered: bool,
    theme: Theme,
    state: gpui::WeakEntity<AppState>,
) -> AnyElement {
    let id = row.id;
    let size = format!(
        "{}{}",
        if row.complete { "" } else { "~" },
        fmt::human_bytes(row.block_size)
    );
    let name = SharedString::from(row.display_name());
    let color = theme.sunburst_color(color_index);
    let hover_state = state.clone();
    let click_state = state;
    div()
        .id(gpui::ElementId::NamedInteger(
            "content-row".into(),
            id as u64,
        ))
        .h(px(32.0))
        .w_full()
        .flex()
        .items_center()
        .gap(px(10.0))
        .px(px(10.0))
        .rounded(px(6.0))
        .bg(if selected {
            theme.overlay_strong
        } else if hovered {
            theme.overlay
        } else {
            gpui::transparent_black()
        })
        .cursor(CursorStyle::Arrow)
        .hover(|style| style.bg(theme.overlay))
        .active(|style| style.opacity(0.78))
        .on_hover(move |is_hovered, _, cx| {
            let _ = hover_state.update(cx, |state, cx| {
                state.set_hovered((*is_hovered).then_some(id), cx)
            });
        })
        .on_click(move |_, _, cx| {
            let _ = click_state.update(cx, |state, cx| state.activate_node(id, cx));
        })
        .child(div().size(px(8.0)).rounded_full().bg(color).flex_shrink_0())
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(13.0))
                .text_color(theme.text)
                .child(name),
        )
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_secondary)
                .child(size),
        )
        .into_any()
}
