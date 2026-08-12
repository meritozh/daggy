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

fn main() {
    // `gpui_platform::application()` constructs an `Application` with the
    // correct platform backend for the current OS (Metal on macOS).
    gpui_platform::application().run(|cx: &mut App| {
        // Bind Escape to the GoUp action app-wide.
        cx.bind_keys([KeyBinding::new("escape", GoUp, None)]);

        let bounds = Bounds::centered(None, size(gpui::px(1040.), gpui::px(680.)), cx);
        let app_state = cx.new(app_state::AppState::new);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_decorations: Some(WindowDecorations::Client),
                titlebar: Some(TitlebarOptions {
                    title: Some("daggy".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_window, cx| cx.new(|cx| root_view::RootView::new(app_state, cx)),
        )
        .expect("failed to open window");
    });
}
