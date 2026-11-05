//! Glyph-to-icon rasterization (port of `UI/Tray/TrayIconRasterizer.cs` + `TrayIconRenderer.cs`
//! drawing + `AppIconLoader.cs`): icon-font glyph layers drawn centered into a square image with
//! DirectWrite + Direct2D into a WIC bitmap, then converted to an HICON by the tray icon.

use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::ManuallyDrop;

use tadn_common::theme::Argb;
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::{
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
    D2D1CreateFactory, ID2D1Factory,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_METRICS, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_GLYPH_METRICS, DWRITE_GLYPH_RUN, DWRITE_MEASURING_MODE_NATURAL,
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteFontFace,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, GetDC,
    GetDIBits, HBITMAP, HGDIOBJ, ReleaseDC,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnLoad, WICRect,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, GetIconInfo, HICON, ICONINFO, IMAGE_ICON, LR_DEFAULTCOLOR, LR_LOADFROMFILE,
    LoadImageW,
};
use windows_core::{BOOL, PCWSTR};
use windows_numerics::Vector2;

use crate::util::{ComScope, wide};

/// Straight-alpha RGBA8 pixels, row-major, `width * height * 4` bytes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IconImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// One glyph drawn into the icon; layers are drawn in order (first = bottom).
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphLayer {
    /// The glyph (one code point of the icon font).
    pub text: String,
    /// Font family, e.g. `Palette::icon_font`.
    pub font_family: String,
    pub color: Argb,
    /// Extra layer opacity (0..1), e.g. 0.21 for the faint full-volume backdrop.
    pub opacity: f32,
}

/// Icon fonts tried after a layer's own family is missing (`TrayIconRenderOptions.IconFontFamilies`).
const ICON_FONT_FALLBACKS: [&str; 2] = ["Segoe Fluent Icons", "Segoe MDL2 Assets"];

/// Renders the layers at `size_px` x `size_px` with the glyph em box filling the icon and optical
/// centering identical to the C# rasterizer (DirectWrite/Direct2D, grayscale antialiasing).
///
/// Placement (`TrayGlyphLayout.Center`): font size = icon size; each glyph's advance box is
/// centred horizontally and its line box (ascent + descent) vertically. Every later layer shares
/// the first layer's left edge, so partial glyphs line up with the full backdrop glyph. A layer's
/// alpha is `(byte)(color alpha * opacity)` like the C# backdrop paint.
pub fn render_glyph_icon(layers: &[GlyphLayer], size_px: u32) -> windows_core::Result<IconImage> {
    if size_px == 0 {
        return Err(windows_core::Error::new(E_INVALIDARG, "icon size must be at least 1 pixel"));
    }
    RASTER_CONTEXT.with(|context| {
        let mut context = context.borrow_mut();
        if context.is_none() {
            *context = Some(RasterContext::new()?);
        }
        match context.as_mut() {
            Some(context) => context.render(layers, size_px),
            None => Err(windows_core::Error::new(E_FAIL, "raster context unavailable")),
        }
    })
}

thread_local! {
    /// Factories and font faces of the rendering thread (renders may run on a worker thread).
    static RASTER_CONTEXT: RefCell<Option<RasterContext>> = const { RefCell::new(None) };
}

struct RasterContext {
    d2d: ID2D1Factory,
    wic: IWICImagingFactory,
    fonts: IDWriteFontCollection,
    faces: HashMap<String, Option<IDWriteFontFace>>,
    /// Keeps COM initialized on this thread for the cached WIC factory.
    _com: ComScope,
}

struct PlacedGlyphs {
    face: IDWriteFontFace,
    indices: Vec<u16>,
    advances: Vec<f32>,
    origin_x: f32,
    baseline_y: f32,
}

impl RasterContext {
    fn new() -> windows_core::Result<Self> {
        let com = ComScope::enter();
        // SAFETY: factory creation with valid arguments; COM is initialized above.
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let mut fonts = None;
            dwrite.GetSystemFontCollection(&mut fonts, false)?;
            let fonts = fonts.ok_or_else(|| windows_core::Error::new(E_FAIL, "no system font collection"))?;
            Ok(Self { d2d, wic, fonts, faces: HashMap::new(), _com: com })
        }
    }

    /// The face of `family`, or of the first installed icon-font fallback (cached either way).
    fn face(&mut self, family: &str) -> Option<IDWriteFontFace> {
        if let Some(face) = self.faces.get(family) {
            return face.clone();
        }
        let face = std::iter::once(family).chain(ICON_FONT_FALLBACKS).find_map(|candidate| self.find_face(candidate));
        if face.is_none() {
            log::warn!("raster: none of '{family}', {ICON_FONT_FALLBACKS:?} is installed");
        }
        self.faces.insert(family.to_owned(), face.clone());
        face
    }

    fn find_face(&self, family: &str) -> Option<IDWriteFontFace> {
        let name = wide(family);
        let (mut index, mut exists) = (0u32, BOOL(0));
        // SAFETY: valid NUL-terminated name and out-pointers to locals.
        unsafe {
            self.fonts.FindFamilyName(PCWSTR(name.as_ptr()), &mut index, &mut exists).ok()?;
            if !exists.as_bool() {
                return None;
            }
            let font = self
                .fonts
                .GetFontFamily(index)
                .and_then(|f| {
                    f.GetFirstMatchingFont(
                        DWRITE_FONT_WEIGHT_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        DWRITE_FONT_STYLE_NORMAL,
                    )
                })
                .ok()?;
            font.CreateFontFace().ok()
        }
    }

    fn place(&mut self, layer: &GlyphLayer, size: f32) -> windows_core::Result<PlacedGlyphs> {
        let face = self.face(&layer.font_family).ok_or_else(|| {
            windows_core::Error::new(E_FAIL, format!("icon font '{}' is not installed", layer.font_family))
        })?;
        let code_points: Vec<u32> = layer.text.chars().map(u32::from).collect();
        let mut indices = vec![0u16; code_points.len()];
        let mut glyph_metrics = vec![DWRITE_GLYPH_METRICS::default(); code_points.len()];
        let mut font_metrics = DWRITE_FONT_METRICS::default();
        // SAFETY: every buffer holds `code_points.len()` elements.
        unsafe {
            face.GetMetrics(&mut font_metrics);
            if !code_points.is_empty() {
                face.GetGlyphIndices(code_points.as_ptr(), code_points.len() as u32, indices.as_mut_ptr())?;
                face.GetDesignGlyphMetrics(indices.as_ptr(), indices.len() as u32, glyph_metrics.as_mut_ptr(), false)?;
            }
        }
        let scale = size / f32::from(font_metrics.designUnitsPerEm.max(1));
        let advances: Vec<f32> = glyph_metrics.iter().map(|m| m.advanceWidth as f32 * scale).collect();
        let advance_width: f32 = advances.iter().sum();
        // Skia's ascent is negative: y = (S - (descent - ascent)) / 2 - ascent.
        let ascent = f32::from(font_metrics.ascent) * scale;
        let descent = f32::from(font_metrics.descent) * scale;
        Ok(PlacedGlyphs {
            face,
            indices,
            advances,
            origin_x: (size - advance_width) / 2.0,
            baseline_y: (size - (descent + ascent)) / 2.0 + ascent,
        })
    }

    fn render(&mut self, layers: &[GlyphLayer], size_px: u32) -> windows_core::Result<IconImage> {
        let size = size_px as f32;
        let mut placed = Vec::with_capacity(layers.len());
        for layer in layers {
            placed.push(self.place(layer, size)?);
        }

        // SAFETY: COM objects created from live factories; every pointer passed refers to data
        // that outlives the call that receives it.
        unsafe {
            let bitmap =
                self.wic.CreateBitmap(size_px, size_px, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad)?;
            let properties = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let target = self.d2d.CreateWicBitmapRenderTarget(&bitmap, &properties)?;
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            target.BeginDraw();
            target.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));

            let shared_left = placed.first().map_or(0.0, |first| first.origin_x);
            for (layer, glyphs) in layers.iter().zip(&placed) {
                if glyphs.indices.is_empty() {
                    continue;
                }
                let brush = target.CreateSolidColorBrush(&layer_color(layer.color, layer.opacity), None)?;
                let run = DWRITE_GLYPH_RUN {
                    fontFace: ManuallyDrop::new(Some(glyphs.face.clone())),
                    fontEmSize: size,
                    glyphCount: glyphs.indices.len() as u32,
                    glyphIndices: glyphs.indices.as_ptr(),
                    glyphAdvances: glyphs.advances.as_ptr(),
                    glyphOffsets: std::ptr::null(),
                    isSideways: BOOL(0),
                    bidiLevel: 0,
                };
                target.DrawGlyphRun(
                    Vector2 { X: shared_left, Y: glyphs.baseline_y },
                    &run,
                    &brush,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
                drop(ManuallyDrop::into_inner(run.fontFace));
            }
            target.EndDraw(None, None)?;

            let stride = size_px * 4;
            let mut pixels = vec![0u8; (stride * size_px) as usize];
            let rect = WICRect { X: 0, Y: 0, Width: size_px as i32, Height: size_px as i32 };
            bitmap.CopyPixels(&rect, stride, &mut pixels)?;
            premultiplied_bgra_to_straight_rgba(&mut pixels);
            Ok(IconImage { width: size_px, height: size_px, rgba: pixels })
        }
    }
}

/// The layer's straight color with alpha `(byte)(a * opacity)`.
fn layer_color(color: Argb, opacity: f32) -> D2D1_COLOR_F {
    let alpha = (f32::from(color.alpha()) * opacity.clamp(0.0, 1.0)) as u8;
    let channel = |shift: u32| ((color.0 >> shift) & 0xFF) as f32 / 255.0;
    D2D1_COLOR_F { r: channel(16), g: channel(8), b: channel(0), a: f32::from(alpha) / 255.0 }
}

/// In place: premultiplied B,G,R,A → straight R,G,B,A.
fn premultiplied_bgra_to_straight_rgba(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        let a = u32::from(pixel[3]);
        let unpremultiply = |c: u8| (u32::from(c) * 255 + a / 2).checked_div(a).map_or(0, |v| v.min(255) as u8);
        let (b, g, r) = (unpremultiply(pixel[0]), unpremultiply(pixel[1]), unpremultiply(pixel[2]));
        pixel[0] = r;
        pixel[1] = g;
        pixel[2] = b;
    }
}

/// Converts straight RGBA pixels to a 32-bit HICON (caller owns it; `DestroyIcon` when done).
pub fn create_hicon(image: &IconImage) -> windows_core::Result<HICON> {
    let (width, height) = (image.width as i32, image.height as i32);
    if width <= 0 || height <= 0 || image.rgba.len() != (image.width * image.height * 4) as usize {
        return Err(windows_core::Error::new(E_INVALIDARG, "IconImage size does not match its pixels"));
    }
    let info = bitmap_info(width, height);
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: `info` describes a top-down 32-bit DIB; `bits` receives its pixel memory, which holds
    // exactly width * height * 4 bytes and is written before the icon is created. The GDI objects
    // are deleted after CreateIconIndirect copied them.
    unsafe {
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let pixels = std::slice::from_raw_parts_mut(bits.cast::<u8>(), image.rgba.len());
        for (dst, src) in pixels.chunks_exact_mut(4).zip(image.rgba.chunks_exact(4)) {
            dst.copy_from_slice(&[src[2], src[1], src[0], src[3]]);
        }
        // All-zero AND mask: the alpha channel decides transparency. Rows are WORD aligned.
        let mask_stride = (image.width as usize).div_ceil(16) * 2;
        let mask_bits = vec![0u8; mask_stride * image.height as usize];
        let mask = CreateBitmap(width, height, 1, 1, Some(mask_bits.as_ptr().cast()));
        let icon_info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&icon_info);
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        icon
    }
}

fn bitmap_info(width: i32, height: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The app's own icon at `size_px`, for window icons and as the tray fallback before the first
/// glyph render: `app.ico` beside the exe wins over the embedded icon (resource id 1), like the
/// C# `AppIconLoader`; Windows picks the nearest frame and scales it to `size_px`.
pub fn app_icon_image(size_px: u32) -> Option<IconImage> {
    if size_px == 0 {
        return None;
    }
    let size = size_px as i32;
    let beside_exe =
        std::env::current_exe().ok().map(|exe| exe.with_file_name("app.ico")).filter(|path| path.is_file());
    // SAFETY: LoadImageW with a NUL-terminated path or this module's resource id; the returned
    // icon is destroyed below.
    let icon = unsafe {
        let loaded = match &beside_exe {
            Some(path) => {
                let path = wide(&path.to_string_lossy());
                LoadImageW(None, PCWSTR(path.as_ptr()), IMAGE_ICON, size, size, LR_LOADFROMFILE)
            }
            None => {
                let module = GetModuleHandleW(None).ok()?;
                // MAKEINTRESOURCE(1)
                let resource = PCWSTR(std::ptr::without_provenance(1));
                LoadImageW(Some(module.into()), resource, IMAGE_ICON, size, size, LR_DEFAULTCOLOR)
            }
        };
        HICON(loaded.ok()?.0)
    };
    let image = icon_pixels(icon, size_px);
    // SAFETY: the icon was loaded above and is owned here.
    let _ = unsafe { DestroyIcon(icon) };
    image
}

/// Reads an icon's pixels as straight RGBA (`GetIconInfo` + `GetDIBits`). Icons without alpha
/// take their transparency from the AND mask.
fn icon_pixels(icon: HICON, size_px: u32) -> Option<IconImage> {
    let size = size_px as i32;
    let mut info = ICONINFO::default();
    // SAFETY: `icon` is a valid icon; the bitmaps it returns are deleted below; GetDIBits writes
    // at most size * size 32-bit pixels into buffers of that size.
    unsafe {
        GetIconInfo(icon, &mut info).ok()?;
        let dc = GetDC(None);
        let read = |bitmap: HBITMAP| -> Option<Vec<u8>> {
            if bitmap.is_invalid() {
                return None;
            }
            let mut header = bitmap_info(size, size);
            let mut pixels = vec![0u8; (size_px * size_px * 4) as usize];
            let lines =
                GetDIBits(dc, bitmap, 0, size_px, Some(pixels.as_mut_ptr().cast()), &mut header, DIB_RGB_COLORS);
            (lines == size).then_some(pixels)
        };
        let color = read(info.hbmColor);
        let mask = read(info.hbmMask);
        ReleaseDC(None, dc);
        let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
        let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));

        let mut pixels = color?;
        let has_alpha = pixels.chunks_exact(4).any(|p| p[3] != 0);
        for (index, pixel) in pixels.chunks_exact_mut(4).enumerate() {
            pixel.swap(0, 2);
            if !has_alpha {
                // Mask white (set) = transparent.
                let transparent = mask.as_ref().is_some_and(|m| m[index * 4] != 0);
                pixel[3] = if transparent { 0 } else { 255 };
            }
        }
        Some(IconImage { width: size_px, height: size_px, rgba: pixels })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplied_pixels_unpremultiply_to_straight_rgba() {
        // Half-transparent pure red (premultiplied B,G,R,A = 0,0,128,128) and a transparent pixel.
        let mut pixels = vec![0, 0, 128, 128, 10, 20, 30, 0];
        premultiplied_bgra_to_straight_rgba(&mut pixels);
        assert_eq!(pixels, vec![255, 0, 0, 128, 0, 0, 0, 0]);
    }

    #[test]
    fn layer_alpha_truncates_like_the_csharp_byte_cast() {
        // 255 * 0.21 = 53.55 → 53.
        let color = layer_color(Argb(0xFF10_2030), 0.21);
        assert_eq!((color.a * 255.0).round() as u8, 53);
        assert!((color.r - 16.0 / 255.0).abs() < 1e-6);
    }
}
