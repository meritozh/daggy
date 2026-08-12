//! Daggy — a DaisyDisk-inspired disk usage analyzer built with GPUI.
//!
//! This is the application binary. It wires the scanning engine
//! (`daggy-core`) and the cache (`daggy-cache`) to the GPUI frontend.

mod app_state;
mod root_view;
mod sunburst;

use gpui::{
    size, App, AppContext, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowDecorations,
    WindowOptions,
};

/// Action: navigate up one level (Escape).
#[derive(Clone, PartialEq, Eq, Default, Debug, gpui::Action)]
#[action(namespace = daggy)]
pub struct GoUp;

#[derive(Clone, PartialEq, Eq, Default, Debug, gpui::Action)]
#[action(namespace = daggy)]
pub struct SelectPrevious;

#[derive(Clone, PartialEq, Eq, Default, Debug, gpui::Action)]
#[action(namespace = daggy)]
pub struct SelectNext;

#[derive(Clone, PartialEq, Eq, Default, Debug, gpui::Action)]
#[action(namespace = daggy)]
pub struct ActivateSelection;

#[derive(Clone, PartialEq, Eq, Default, Debug, gpui::Action)]
#[action(namespace = daggy)]
pub struct OpenFolder;

fn main() {
    // `gpui_platform::application()` constructs an `Application` with the
    // correct platform backend for the current OS (Metal on macOS).
    gpui_platform::application().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("escape", GoUp, None),
            KeyBinding::new("up", SelectPrevious, None),
            KeyBinding::new("down", SelectNext, None),
            KeyBinding::new("enter", ActivateSelection, None),
            KeyBinding::new("cmd-o", OpenFolder, None),
        ]);

        let bounds = Bounds::centered(None, size(gpui::px(1180.), gpui::px(760.)), cx);
        let app_state = cx.new(app_state::AppState::new);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(gpui::px(940.), gpui::px(620.))),
                window_decorations: Some(WindowDecorations::Client),
                titlebar: Some(TitlebarOptions {
                    title: Some("daggy".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| root_view::RootView::new(app_state, cx));
                let focus = view.read(cx).focus_handle();
                focus.focus(window, cx);
                view
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
