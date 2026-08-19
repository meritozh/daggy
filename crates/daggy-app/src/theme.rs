//! macOS Human Interface Guideline tokens.
//!
//! Surfaces follow system window, sidebar, and content backgrounds. Interactive
//! accent and row selection use system blue. The sunburst keeps a separate
//! categorical palette so allocation rings stay distinguishable.

use gpui::{
    hsla, point, px, rgb, size, App, Bounds, Hsla, Pixels, Point, Size,
    TitlebarOptions, WindowAppearance, WindowBackgroundAppearance, WindowBounds, WindowDecorations,
    WindowOptions,
};

pub const SIDEBAR_WIDTH: f32 = 220.0;
pub const TITLEBAR_HEIGHT: f32 = 52.0;

fn traffic_light() -> Point<Pixels> {
    point(px(16.0), px(20.0))
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub is_dark: bool,
    pub canvas: Hsla,
    pub sidebar: Hsla,
    pub surface: Hsla,
    pub raised: Hsla,
    pub inset: Hsla,
    pub overlay: Hsla,
    pub overlay_strong: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub text: Hsla,
    pub text_secondary: Hsla,
    pub text_tertiary: Hsla,
    pub text_ghost: Hsla,
    pub accent: Hsla,
    pub selection: Hsla,
    pub on_selection: Hsla,
    pub folder: Hsla,
    pub icon_rgb: u32,
    pub warning: Hsla,
    pub success: Hsla,
    pub danger: Hsla,
    pub sunburst: [u32; 7],
    pub sunburst_grouped: u32,
}

impl Theme {
    pub fn from_appearance(appearance: WindowAppearance) -> Self {
        if matches!(
            appearance,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ) {
            Self::dark()
        } else {
            Self::light()
        }
    }

    pub fn dark() -> Self {
        Self {
            is_dark: true,
            canvas: rgb(0x1C1C1E).into(),
            sidebar: rgb(0x2C2C2E).into(),
            surface: rgb(0x1C1C1E).into(),
            raised: rgb(0x2C2C2E).into(),
            inset: rgb(0x1C1C1E).into(),
            overlay: hsla(240.0 / 360.0, 0.02, 1.0, 0.06),
            overlay_strong: hsla(240.0 / 360.0, 0.02, 1.0, 0.12),
            border: hsla(240.0 / 360.0, 0.02, 1.0, 0.10),
            border_strong: hsla(240.0 / 360.0, 0.02, 1.0, 0.16),
            text: rgb(0xF5F5F7).into(),
            text_secondary: rgb(0x98989D).into(),
            text_tertiary: rgb(0x68686E).into(),
            text_ghost: rgb(0x48484A).into(),
            accent: rgb(0x0A84FF).into(),
            selection: rgb(0x0A84FF).into(),
            on_selection: rgb(0xFFFFFF).into(),
            folder: rgb(0x0A84FF).into(),
            icon_rgb: 0x0A84FF,
            warning: rgb(0xE0B36A).into(),
            success: rgb(0x62C987).into(),
            danger: rgb(0xE2726A).into(),
            sunburst: [
                0xE2795B, 0x6F8DA3, 0x7A9E7E, 0xC4A36A, 0x8B7CA8, 0x5E9BA8, 0xB57A6A,
            ],
            sunburst_grouped: 0x555555,
        }
    }

    pub fn light() -> Self {
        Self {
            is_dark: false,
            canvas: rgb(0xFFFFFF).into(),
            sidebar: rgb(0xEBEBEB).into(),
            surface: rgb(0xFFFFFF).into(),
            raised: rgb(0xF5F5F5).into(),
            inset: rgb(0xFFFFFF).into(),
            overlay: hsla(240.0 / 360.0, 0.02, 0.08, 0.05),
            overlay_strong: hsla(240.0 / 360.0, 0.02, 0.08, 0.10),
            border: hsla(240.0 / 360.0, 0.02, 0.08, 0.10),
            border_strong: hsla(240.0 / 360.0, 0.02, 0.08, 0.16),
            text: rgb(0x1D1D1F).into(),
            text_secondary: rgb(0x6E6E73).into(),
            text_tertiary: rgb(0x8E8E93).into(),
            text_ghost: rgb(0xAEAEB2).into(),
            accent: rgb(0x007AFF).into(),
            selection: rgb(0x007AFF).into(),
            on_selection: rgb(0xFFFFFF).into(),
            folder: rgb(0x007AFF).into(),
            icon_rgb: 0x007AFF,
            warning: rgb(0xA66B20).into(),
            success: rgb(0x2F8F52).into(),
            danger: rgb(0xC64A42).into(),
            sunburst: [
                0xC85F44, 0x4F7390, 0x4F7A55, 0xA07C32, 0x6B5C8A, 0x3D7884, 0x9A5A4E,
            ],
            sunburst_grouped: 0xB0B0B0,
        }
    }
}

/// Chrome knobs consumed by the window opener. Kept as a plain struct so tests
/// can assert the native look without constructing a GPUI `App`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeChrome {
    pub background: WindowBackgroundAppearance,
    pub transparent_titlebar: bool,
    pub client_decorations: bool,
    pub traffic_light: Point<Pixels>,
    pub sidebar_width: Pixels,
    pub titlebar_height: Pixels,
    pub default_size: Size<Pixels>,
    pub min_size: Size<Pixels>,
}

pub fn native_chrome() -> NativeChrome {
    NativeChrome {
        background: WindowBackgroundAppearance::Opaque,
        transparent_titlebar: true,
        client_decorations: true,
        traffic_light: traffic_light(),
        sidebar_width: px(SIDEBAR_WIDTH),
        titlebar_height: px(TITLEBAR_HEIGHT),
        default_size: size(px(1180.0), px(760.0)),
        min_size: size(px(880.0), px(560.0)),
    }
}

pub fn window_options(cx: &App) -> WindowOptions {
    let chrome = native_chrome();
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            chrome.default_size,
            cx,
        ))),
        window_min_size: Some(chrome.min_size),
        window_background: chrome.background,
        window_decorations: chrome
            .client_decorations
            .then_some(WindowDecorations::Client),
        titlebar: Some(TitlebarOptions {
            title: Some("Daggy".into()),
            appears_transparent: chrome.transparent_titlebar,
            traffic_light_position: Some(chrome.traffic_light),
        }),
        app_owns_titlebar_drag: true,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_appearance_picks_graphite_palettes() {
        let dark = Theme::from_appearance(WindowAppearance::Dark);
        let vibrant_dark = Theme::from_appearance(WindowAppearance::VibrantDark);
        let light = Theme::from_appearance(WindowAppearance::Light);
        let vibrant_light = Theme::from_appearance(WindowAppearance::VibrantLight);

        assert!(dark.is_dark);
        assert!(vibrant_dark.is_dark);
        assert!(!light.is_dark);
        assert!(!vibrant_light.is_dark);
        assert_eq!(dark.accent, rgb(0x0A84FF).into());
        assert_eq!(light.accent, rgb(0x007AFF).into());
        assert_eq!(dark.selection, rgb(0x0A84FF).into());
        assert_eq!(light.selection, rgb(0x007AFF).into());
        assert!(dark.overlay.a > 0.0 && dark.overlay.a < 0.1);
        assert!(light.overlay.a > 0.0 && light.overlay.a < 0.1);
    }

    #[test]
    fn native_chrome_uses_opaque_transparent_titlebar() {
        let chrome = native_chrome();
        assert_eq!(chrome.background, WindowBackgroundAppearance::Opaque);
        assert!(chrome.transparent_titlebar);
        assert!(chrome.client_decorations);
        assert_eq!(chrome.traffic_light, point(px(16.0), px(20.0)));
        assert_eq!(chrome.sidebar_width, px(220.0));
        assert_eq!(chrome.titlebar_height, px(52.0));
    }

    #[test]
    fn sidebar_is_a_solid_graphite_step() {
        assert!(!Theme::dark().sidebar.is_transparent());
        assert!(!Theme::light().sidebar.is_transparent());
        assert_ne!(Theme::light().sidebar, Theme::light().canvas);
        assert_ne!(Theme::dark().sidebar, Theme::dark().canvas);
    }
}
