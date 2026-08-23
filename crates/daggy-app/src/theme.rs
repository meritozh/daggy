//! Pencil design tokens for the Daggy window.
//!
//! Surfaces step by background lightness. The allocation map is the only
//! saturated area. Light appearance keeps the same roles at inverted values.

use gpui::{
    hsla, point, px, rgb, rgba, size, App, Bounds, Hsla, Pixels, Point, Size, TitlebarOptions,
    WindowAppearance, WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions,
};

pub const SIDEBAR_WIDTH: f32 = 232.0;
pub const INSPECTOR_WIDTH: f32 = 320.0;
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
    pub inspector: Hsla,
    pub overlay: Hsla,
    pub overlay_strong: Hsla,
    pub border: Hsla,
    pub sidebar_border: Hsla,
    pub inspector_border: Hsla,
    pub text: Hsla,
    pub text_secondary: Hsla,
    pub text_tertiary: Hsla,
    pub text_ghost: Hsla,
    pub accent: Hsla,
    pub on_selection: Hsla,
    pub folder: Hsla,
    pub icon_rgb: u32,
    pub toolbar_icon_rgb: u32,
    pub warning: Hsla,
    pub danger: Hsla,
    pub sunburst: [u32; 7],
    pub sunburst_grouped: u32,
    pub track: Hsla,
    pub hint: Hsla,
    pub volume_selected: Hsla,
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
            canvas: rgb(0x1E1E20).into(),
            sidebar: rgb(0x232326).into(),
            surface: rgb(0x1E1E20).into(),
            inspector: rgb(0x28282B).into(),
            overlay: hsla(240.0 / 360.0, 0.02, 1.0, 0.06),
            overlay_strong: hsla(240.0 / 360.0, 0.02, 1.0, 0.10),
            border: rgba(0xFFFFFF14).into(),
            sidebar_border: rgba(0xFFFFFF14).into(),
            inspector_border: rgba(0xFFFFFF0F).into(),
            text: rgb(0xF2F2F5).into(),
            text_secondary: rgb(0x98989F).into(),
            text_tertiary: rgb(0x98989F).into(),
            text_ghost: rgb(0x6E6E76).into(),
            accent: rgb(0x3E8EF7).into(),
            on_selection: rgb(0xFFFFFF).into(),
            folder: rgb(0x3EC6BD).into(),
            icon_rgb: 0x3EC6BD,
            toolbar_icon_rgb: 0x98989F,
            warning: rgb(0xF2913E).into(),
            danger: rgb(0xF2568C).into(),
            sunburst: [
                0x3E8EF7, 0xF2568C, 0x3EC6BD, 0x6E6E76, 0x9A6BF2, 0xF2913E, 0xD89A2A,
            ],
            sunburst_grouped: 0x5C5C64,
            track: rgb(0x3A3A40).into(),
            hint: rgba(0x00000040).into(),
            volume_selected: rgba(0x3E8EF733).into(),
        }
    }

    pub fn light() -> Self {
        Self {
            is_dark: false,
            canvas: rgb(0xF4F4F6).into(),
            sidebar: rgb(0xE8E8EC).into(),
            surface: rgb(0xF4F4F6).into(),
            inspector: rgb(0xFFFFFF).into(),
            overlay: hsla(240.0 / 360.0, 0.02, 0.08, 0.06),
            overlay_strong: hsla(240.0 / 360.0, 0.02, 0.08, 0.10),
            border: rgba(0x00000014).into(),
            sidebar_border: rgba(0x00000014).into(),
            inspector_border: rgba(0x0000000F).into(),
            text: rgb(0x1D1D1F).into(),
            text_secondary: rgb(0x6E6E73).into(),
            text_tertiary: rgb(0x6E6E73).into(),
            text_ghost: rgb(0xAEAEB2).into(),
            accent: rgb(0x3E8EF7).into(),
            on_selection: rgb(0xFFFFFF).into(),
            folder: rgb(0x2AA99F).into(),
            icon_rgb: 0x2AA99F,
            toolbar_icon_rgb: 0x6E6E73,
            warning: rgb(0xC47220).into(),
            danger: rgb(0xD94A78).into(),
            sunburst: [
                0x2E7AE6, 0xD94A78, 0x2AA99F, 0x5C5C64, 0x8A5BE0, 0xD67D2E, 0xB88620,
            ],
            sunburst_grouped: 0x7A7A82,
            track: rgb(0xD8D8DE).into(),
            hint: rgba(0x0000000A).into(),
            volume_selected: rgba(0x3E8EF722).into(),
        }
    }

    pub fn sunburst_rgb(&self, index: usize) -> u32 {
        self.sunburst[index % self.sunburst.len()]
    }

    pub fn sunburst_color(&self, index: usize) -> Hsla {
        rgb(self.sunburst_rgb(index)).into()
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
    pub inspector_width: Pixels,
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
        inspector_width: px(INSPECTOR_WIDTH),
        titlebar_height: px(TITLEBAR_HEIGHT),
        default_size: size(px(1440.0), px(900.0)),
        min_size: size(px(980.0), px(640.0)),
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
    fn system_appearance_picks_pencil_palettes() {
        let dark = Theme::from_appearance(WindowAppearance::Dark);
        let vibrant_dark = Theme::from_appearance(WindowAppearance::VibrantDark);
        let light = Theme::from_appearance(WindowAppearance::Light);
        let vibrant_light = Theme::from_appearance(WindowAppearance::VibrantLight);

        assert!(dark.is_dark);
        assert!(vibrant_dark.is_dark);
        assert!(!light.is_dark);
        assert!(!vibrant_light.is_dark);
        assert_eq!(dark.accent, rgb(0x3E8EF7).into());
        assert_eq!(light.accent, rgb(0x3E8EF7).into());
        assert_eq!(dark.canvas, rgb(0x1E1E20).into());
        assert_eq!(dark.inspector, rgb(0x28282B).into());
        assert_eq!(dark.sunburst[0], 0x3E8EF7);
        assert_eq!(dark.sunburst[1], 0xF2568C);
        assert_eq!(dark.sunburst[2], 0x3EC6BD);
        assert_eq!(dark.sunburst[6], 0xD89A2A);
        for &color in dark.sunburst.iter().chain(light.sunburst.iter()) {
            let red = ((color >> 16) & 0xff) as u32;
            let green = ((color >> 8) & 0xff) as u32;
            let blue = color & 0xff;
            assert!(
                red.min(green).min(blue) < 200,
                "{color:06x} is too close to white"
            );
        }
        assert_eq!(dark.danger, rgb(0xF2568C).into());
        assert_eq!(dark.text_tertiary, dark.text_secondary);
        assert!(dark.overlay.a > 0.0 && dark.overlay.a < 0.1);
        assert!(light.overlay.a > 0.0 && light.overlay.a < 0.1);
    }

    #[test]
    fn native_chrome_matches_pencil_window() {
        let chrome = native_chrome();
        assert_eq!(chrome.background, WindowBackgroundAppearance::Opaque);
        assert!(chrome.transparent_titlebar);
        assert!(chrome.client_decorations);
        assert_eq!(chrome.traffic_light, point(px(16.0), px(20.0)));
        assert_eq!(chrome.sidebar_width, px(232.0));
        assert_eq!(chrome.inspector_width, px(320.0));
        assert_eq!(chrome.titlebar_height, px(52.0));
        assert_eq!(chrome.default_size, size(px(1440.0), px(900.0)));
        assert_eq!(chrome.min_size, size(px(980.0), px(640.0)));
    }

    #[test]
    fn sidebar_is_a_solid_step_off_the_canvas() {
        assert!(!Theme::dark().sidebar.is_transparent());
        assert!(!Theme::light().sidebar.is_transparent());
        assert_ne!(Theme::light().sidebar, Theme::light().canvas);
        assert_ne!(Theme::dark().sidebar, Theme::dark().canvas);
        assert_ne!(Theme::dark().inspector, Theme::dark().canvas);
    }
}
