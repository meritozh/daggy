//! Official SF Symbols via AppKit `NSImage`, then GPUI `img`.
//!
//! GPUI has no first-party SF Symbols API. The community crate `gpui-symbols`
//! targets crates.io `gpui 0.2` and cannot be dropped onto this Zed-pinned
//! GPUI. Named symbols are loaded from the system catalog
//! (`imageWithSystemSymbolName:`), which is the licensed Apple path.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use gpui::{img, px, Image, ImageFormat, Img, Pixels, SharedString, Styled};

/// SF Symbol names used by the chrome. Names come from the SF Symbols catalog.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Glyph {
    Recents,
    Downloads,
    Applications,
    Photos,
    Movies,
    House,
    Desktop,
    Documents,
    Folder,
    File,
    Drive,
    ChevronLeft,
    ChevronRight,
    Refresh,
    Reveal,
    Search,
    Sidebar,
    Trash,
    Open,
}

impl Glyph {
    pub fn system_name(self) -> &'static str {
        match self {
            Self::Recents => "clock.arrow.circlepath",
            Self::Downloads => "arrow.down.circle.fill",
            Self::Applications => "square.grid.2x2.fill",
            Self::Photos => "photo.fill",
            Self::Movies => "film.fill",
            Self::House => "house.fill",
            Self::Desktop => "desktopcomputer",
            Self::Documents => "doc.text.fill",
            Self::Folder => "folder.fill",
            Self::File => "doc.fill",
            Self::Drive => "internaldrive.fill",
            Self::ChevronLeft => "chevron.left",
            Self::ChevronRight => "chevron.right",
            Self::Refresh => "arrow.clockwise",
            Self::Reveal => "folder",
            Self::Search => "magnifyingglass",
            Self::Sidebar => "sidebar.left",
            Self::Trash => "trash.fill",
            Self::Open => "plus",
        }
    }
}

impl From<Glyph> for SharedString {
    fn from(glyph: Glyph) -> Self {
        SharedString::from(glyph.system_name())
    }
}

pub fn glyph_for_location(id: &str) -> Glyph {
    match id {
        "recents" => Glyph::Recents,
        "downloads" => Glyph::Downloads,
        "applications" => Glyph::Applications,
        "photos" => Glyph::Photos,
        "movies" => Glyph::Movies,
        "home" => Glyph::House,
        "desktop" => Glyph::Desktop,
        "documents" => Glyph::Documents,
        "volume" => Glyph::Drive,
        _ => Glyph::Folder,
    }
}

/// Render a catalog symbol and wrap it as a GPUI image element.
pub fn symbol_img(glyph: Glyph, size: Pixels, color: u32) -> Option<Img> {
    let image = render_symbol(glyph.system_name(), size, color)?;
    Some(img(image).w(size).h(size))
}

fn cache() -> &'static Mutex<HashMap<(String, u32, u32), Arc<Image>>> {
    static CACHE: OnceLock<Mutex<HashMap<(String, u32, u32), Arc<Image>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn render_symbol(name: &str, size: Pixels, color: u32) -> Option<Arc<Image>> {
    let size_px = f32::from(size);
    let key = (name.to_string(), size_px.to_bits(), color);
    if let Some(hit) = cache()
        .lock()
        .ok()
        .and_then(|guard| guard.get(&key).cloned())
    {
        return Some(hit);
    }
    let png = render_png(name, size_px, color)?;
    if png.len() < 8 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let image = Arc::new(Image::from_bytes(ImageFormat::Png, png));
    if let Ok(mut guard) = cache().lock() {
        guard.insert(key, image.clone());
    }
    Some(image)
}

#[cfg(target_os = "macos")]
fn render_png(name: &str, point_size: f32, color: u32) -> Option<Vec<u8>> {
    macos::render_png(name, point_size, color)
}

#[cfg(not(target_os = "macos"))]
fn render_png(_name: &str, point_size: f32, color: u32) -> Option<Vec<u8>> {
    None
}

/// Empty-element fallback when AppKit cannot produce the symbol.
pub fn fallback_mark(size: Pixels, color: gpui::Hsla) -> gpui::Div {
    use gpui::{div, Styled};
    let edge = f32::from(size);
    div().w(size).h(px(edge * 0.8)).rounded(px(2.0)).bg(color)
}

#[cfg(target_os = "macos")]
mod macos {
    use objc::runtime::{Class, Object};
    use objc::{msg_send, sel, sel_impl};

    const PNG_FILE_TYPE: u64 = 4;
    const FONT_WEIGHT_MEDIUM: f64 = 0.23;
    const SOURCE_OVER: u64 = 2;
    const RETINA: f64 = 2.0;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSSize {
        width: f64,
        height: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSRect {
        origin: NSPoint,
        size: NSSize,
    }

    pub fn render_png(name: &str, point_size: f32, color: u32) -> Option<Vec<u8>> {
        unsafe {
            let ns_string = Class::get("NSString")?;
            let ns_image = Class::get("NSImage")?;
            let ns_color = Class::get("NSColor")?;
            let ns_config = Class::get("NSImageSymbolConfiguration")?;
            let ns_bitmap = Class::get("NSBitmapImageRep")?;
            let ns_dict = Class::get("NSDictionary")?;
            let ns_context = Class::get("NSGraphicsContext")?;

            let c_name = std::ffi::CString::new(name).ok()?;
            let symbol_name: *mut Object =
                msg_send![ns_string, stringWithUTF8String: c_name.as_ptr()];
            if symbol_name.is_null() {
                return None;
            }

            let image: *mut Object = msg_send![
                ns_image,
                imageWithSystemSymbolName: symbol_name
                accessibilityDescription: std::ptr::null::<Object>()
            ];
            if image.is_null() {
                return None;
            }

            let red = ((color >> 16) & 0xff) as f64 / 255.0;
            let green = ((color >> 8) & 0xff) as f64 / 255.0;
            let blue = (color & 0xff) as f64 / 255.0;
            let tint: *mut Object = msg_send![
                ns_color,
                colorWithRed: red
                green: green
                blue: blue
                alpha: 1.0f64
            ];

            let size_config: *mut Object = msg_send![
                ns_config,
                configurationWithPointSize: point_size as f64
                weight: FONT_WEIGHT_MEDIUM
            ];
            let color_config: *mut Object =
                msg_send![ns_config, configurationWithHierarchicalColor: tint];
            let combined: *mut Object =
                msg_send![size_config, configurationByApplyingConfiguration: color_config];
            let configured: *mut Object = msg_send![image, imageWithSymbolConfiguration: combined];
            let image = if configured.is_null() {
                image
            } else {
                configured
            };

            let pixels = ((point_size as f64) * RETINA).ceil() as isize;
            let logical = NSSize {
                width: point_size as f64,
                height: point_size as f64,
            };
            let space_name = std::ffi::CString::new("NSDeviceRGBColorSpace").ok()?;
            let space: *mut Object =
                msg_send![ns_string, stringWithUTF8String: space_name.as_ptr()];
            let allocated: *mut Object = msg_send![ns_bitmap, alloc];
            let rep: *mut Object = msg_send![
                allocated,
                initWithBitmapDataPlanes: std::ptr::null::<*mut u8>()
                pixelsWide: pixels
                pixelsHigh: pixels
                bitsPerSample: 8isize
                samplesPerPixel: 4isize
                hasAlpha: 1u8
                isPlanar: 0u8
                colorSpaceName: space
                bytesPerRow: 0isize
                bitsPerPixel: 0isize
            ];
            if rep.is_null() {
                return None;
            }
            let _: () = msg_send![rep, setSize: logical];

            let ctx: *mut Object = msg_send![ns_context, graphicsContextWithBitmapImageRep: rep];
            if ctx.is_null() {
                return None;
            }
            let _: () = msg_send![ns_context, saveGraphicsState];
            let _: () = msg_send![ns_context, setCurrentContext: ctx];
            let dest = NSRect {
                origin: NSPoint { x: 0.0, y: 0.0 },
                size: logical,
            };
            let src = NSRect {
                origin: NSPoint { x: 0.0, y: 0.0 },
                size: NSSize {
                    width: 0.0,
                    height: 0.0,
                },
            };
            let _: () = msg_send![
                image,
                drawInRect: dest
                fromRect: src
                operation: SOURCE_OVER
                fraction: 1.0f64
            ];
            let _: () = msg_send![ctx, flushGraphics];
            let _: () = msg_send![ns_context, restoreGraphicsState];

            let empty: *mut Object = msg_send![ns_dict, dictionary];
            let png: *mut Object =
                msg_send![rep, representationUsingType: PNG_FILE_TYPE properties: empty];
            if png.is_null() {
                return None;
            }
            let bytes: *const u8 = msg_send![png, bytes];
            let length: usize = msg_send![png, length];
            if bytes.is_null() || length == 0 {
                return None;
            }
            Some(std::slice::from_raw_parts(bytes, length).to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_names_match_sf_symbols() {
        assert_eq!(Glyph::House.system_name(), "house.fill");
        assert_eq!(Glyph::Folder.system_name(), "folder.fill");
        assert_eq!(Glyph::ChevronLeft.system_name(), "chevron.left");
        assert_eq!(Glyph::Refresh.system_name(), "arrow.clockwise");
        assert_eq!(Glyph::Drive.system_name(), "internaldrive.fill");
        assert_eq!(glyph_for_location("downloads"), Glyph::Downloads);
        assert_eq!(glyph_for_location("photos"), Glyph::Photos);
        assert_eq!(glyph_for_location("recents"), Glyph::Recents);
        assert_eq!(glyph_for_location("current"), Glyph::Folder);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn system_catalog_renders_folder_png() {
        let png = render_png("folder.fill", 16.0, 0x3E8EF7).expect("folder.fill should render");
        assert!(
            png.starts_with(b"\x89PNG\r\n\x1a\n"),
            "AppKit must return PNG"
        );
        assert!(png.len() > 64);
        let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
        assert!(
            width >= 28,
            "Retina raster must be at least 2x 15pt, got {width}px"
        );
    }
}
