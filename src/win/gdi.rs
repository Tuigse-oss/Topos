//! GDI helpers: fonts, colours, fills, text, and copying a painted
//! [`Frame`] into a bitmap.
//!
//! Painting is cosmetic, so a failed GDI call here never ends the program:
//! the fill helpers ignore failures (the next paint tries again), and only
//! resource creation reports errors to the caller.

use topos::view::{Frame, PixelRect, Rgb};
use winsafe::{
    self as w, co,
    guard::{DeleteDCGuard, DeleteObjectGuard},
};

/// A reusable offscreen bitmap and its drawing context. Selection guards
/// end before either resource can be dropped.
pub struct BackBuffer {
    dc: DeleteDCGuard,
    bitmap: DeleteObjectGuard<w::HBITMAP>,
    size: w::SIZE,
}

/// Paint a complete frame through a buffer reused at the same size. A size
/// change replaces it; an empty area releases it without calling `draw`.
pub fn paint_buffered(
    hdc: &w::HDC,
    size: w::SIZE,
    buffer: &mut Option<BackBuffer>,
    draw: impl FnOnce(&w::HDC),
) -> w::SysResult<()> {
    if size.cx <= 0 || size.cy <= 0 {
        *buffer = None;
        return Ok(());
    }
    if !buffer.as_ref().is_some_and(|back| back.size == size) {
        // Drop the previous size first, avoiding two large back buffers
        // coexisting while a window is resized.
        *buffer = None;
        *buffer = Some(BackBuffer {
            dc: hdc.CreateCompatibleDC()?,
            bitmap: hdc.CreateCompatibleBitmap(size.cx, size.cy)?,
            size,
        });
    }
    let back = buffer.as_ref().ok_or(co::ERROR::INVALID_PARAMETER)?;
    // Each paint sees the same DC state as a fresh buffer, even when the
    // previous paint changed text, brush or clipping settings.
    let saved = back.dc.SaveDC()?;
    let result = (|| {
        let _bitmap = back.dc.SelectObject(&*back.bitmap)?;
        draw(&back.dc);
        hdc.BitBlt(
            w::POINT::default(),
            size,
            &back.dc,
            w::POINT::default(),
            co::ROP::SRCCOPY,
        )
    })();
    let restored = back.dc.RestoreDC(saved);
    result.and(restored)
}

/// Fonts the window draws with, created once for the window's DPI.
pub struct Fonts {
    /// Toolbar, breadcrumb, tooltip body: the system UI face.
    pub ui: DeleteObjectGuard<w::HFONT>,
    /// Tooltip titles.
    pub ui_bold: DeleteObjectGuard<w::HFONT>,
    /// A hovered breadcrumb link.
    pub link: DeleteObjectGuard<w::HFONT>,
    /// Toolbar icons: a symbol font shipped with Windows 10 and 11.
    pub icons: DeleteObjectGuard<w::HFONT>,
    /// The drive field's drop-down chevron, from the same symbol font.
    pub chevron: DeleteObjectGuard<w::HFONT>,
    /// Map captions, classic: 8-point MS Sans Serif, the bitmap font of the
    /// classic Windows UI — every glyph pixel solid, nothing anti-aliased.
    pub map_classic: DeleteObjectGuard<w::HFONT>,
    /// Map captions, modern: 8-point Segoe UI with ClearType.
    pub map_modern: DeleteObjectGuard<w::HFONT>,
}

impl Fonts {
    pub fn new(dpi: u32) -> w::SysResult<Self> {
        let points = |pt: i32| -mul_div(pt, dpi, 72);
        let ui = |weight: co::FW, underline: bool| {
            font(
                points(9),
                "Segoe UI",
                weight,
                underline,
                co::QUALITY::CLEARTYPE,
            )
        };
        Ok(Self {
            ui: ui(co::FW::NORMAL, false)?,
            ui_bold: ui(co::FW::SEMIBOLD, false)?,
            link: ui(co::FW::NORMAL, true)?,
            icons: font(
                -mul_div(16, dpi, 96),
                "Segoe MDL2 Assets",
                co::FW::NORMAL,
                false,
                co::QUALITY::CLEARTYPE,
            )?,
            chevron: font(
                -mul_div(10, dpi, 96),
                "Segoe MDL2 Assets",
                co::FW::NORMAL,
                false,
                co::QUALITY::CLEARTYPE,
            )?,
            map_classic: font(
                points(8),
                "MS Sans Serif",
                co::FW::NORMAL,
                false,
                co::QUALITY::DEFAULT,
            )?,
            map_modern: font(
                points(8),
                "Segoe UI",
                co::FW::NORMAL,
                false,
                co::QUALITY::CLEARTYPE,
            )?,
        })
    }
}

/// `value * numerator / denominator`, rounded, for DPI scaling.
#[must_use]
pub fn mul_div(value: i32, numerator: u32, denominator: u32) -> i32 {
    let scaled = i64::from(value) * i64::from(numerator);
    let denominator = i64::from(denominator.max(1));
    let rounded = (scaled + denominator / 2).div_euclid(denominator);
    i32::try_from(rounded).unwrap_or(i32::MAX)
}

fn font(
    height: i32,
    face: &str,
    weight: co::FW,
    underline: bool,
    quality: co::QUALITY,
) -> w::SysResult<DeleteObjectGuard<w::HFONT>> {
    let mut lf = w::LOGFONT::new_face(height, face);
    lf.lfWeight = weight;
    lf.lfUnderline = u8::from(underline);
    lf.lfQuality = quality;
    w::HFONT::CreateFontIndirect(&lf)
}

#[must_use]
pub fn colorref(color: Rgb) -> w::COLORREF {
    w::COLORREF::from_rgb(color.r, color.g, color.b)
}

#[must_use]
pub fn rect(r: PixelRect) -> w::RECT {
    w::RECT {
        left: r.x,
        top: r.y,
        right: r.right(),
        bottom: r.bottom(),
    }
}

/// Fill a rectangle with a solid colour, through the DC brush (no brush is
/// created per fill).
pub fn fill(hdc: &w::HDC, rc: w::RECT, color: Rgb) {
    if let Ok(brush) = w::HBRUSH::GetStockObject(co::STOCK_BRUSH::DC)
        && hdc.SetDCBrushColor(colorref(color)).is_ok()
    {
        let _ = hdc.FillRect(rc, &brush);
    }
}

/// Outline a rectangle on its inside, `thickness` pixels wide.
pub fn outline(hdc: &w::HDC, rc: w::RECT, thickness: i32, color: Rgb) {
    let t = thickness
        .min((rc.right - rc.left) / 2)
        .min((rc.bottom - rc.top) / 2);
    if t <= 0 {
        return;
    }
    let edge = |left, top, right, bottom| w::RECT {
        left,
        top,
        right,
        bottom,
    };
    fill(hdc, edge(rc.left, rc.top, rc.right, rc.top + t), color);
    fill(
        hdc,
        edge(rc.left, rc.bottom - t, rc.right, rc.bottom),
        color,
    );
    fill(
        hdc,
        edge(rc.left, rc.top + t, rc.left + t, rc.bottom - t),
        color,
    );
    fill(
        hdc,
        edge(rc.right - t, rc.top + t, rc.right, rc.bottom - t),
        color,
    );
}

/// Draw `text` in `rc` with `DrawText` flags, clipped to `rc`.
pub fn text(hdc: &w::HDC, text: &str, rc: w::RECT, flags: co::DT) {
    let mut bounds = rc;
    let _ = hdc.DrawText(text, &mut bounds, flags | co::DT::NOPREFIX);
}

/// The size `text` takes with `DrawText` (lines split at `\n`).
#[must_use]
pub fn measure(hdc: &w::HDC, text: &str) -> w::SIZE {
    let mut bounds = w::RECT::default();
    match hdc.DrawText(
        text,
        &mut bounds,
        co::DT::CALCRECT | co::DT::NOPREFIX | co::DT::NOCLIP,
    ) {
        Ok(_) => w::SIZE::with(bounds.right - bounds.left, bounds.bottom - bounds.top),
        Err(_) => w::SIZE::default(),
    }
}

/// A bitmap compatible with `hdc` holding `frame`'s pixels.
///
/// `SetDIBits` reads `width * height * 4` bytes from the buffer it is
/// handed and does not know the slice's length, so the length is checked
/// against the header here, before the call; a mismatch is an error, never
/// an out-of-bounds read.
pub fn frame_bitmap(hdc: &w::HDC, frame: &Frame) -> w::SysResult<DeleteObjectGuard<w::HBITMAP>> {
    let invalid = || co::ERROR::INVALID_PARAMETER;
    let width = i32::try_from(frame.width()).map_err(|_| invalid())?;
    let height = i32::try_from(frame.height()).map_err(|_| invalid())?;
    let rows = u32::try_from(frame.height()).map_err(|_| invalid())?;
    let expected = frame
        .width()
        .checked_mul(frame.height())
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(invalid)?;
    if width == 0 || height == 0 || frame.bgra().len() != expected {
        return Err(invalid());
    }
    let bitmap = hdc.CreateCompatibleBitmap(width, height)?;
    let mut bmi = w::BITMAPINFO::default();
    bmi.bmiHeader.biWidth = width;
    // Negative: rows run top to bottom, as in the frame.
    bmi.bmiHeader.biHeight = -height;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = co::BI::RGB;
    hdc.SetDIBits(&bitmap, 0, rows, frame.bgra(), &bmi, co::DIB::RGB_COLORS)?;
    Ok(bitmap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use winsafe::prelude::Handle;

    #[test]
    fn a_reused_buffer_restores_drawing_state_and_deselects_its_bitmap() {
        let screen = w::HWND::NULL.GetDC().unwrap();
        let target = screen.CreateCompatibleDC().unwrap();
        let target_bitmap = screen.CreateCompatibleBitmap(32, 32).unwrap();
        let _target = target.SelectObject(&*target_bitmap).unwrap();
        let mut buffer = None;
        let mut initial_state = None;
        let size = w::SIZE::with(16, 12);
        paint_buffered(&target, size, &mut buffer, |dc| {
            initial_state = Some((dc.GetBkMode().unwrap(), dc.GetTextColor().unwrap()));
            dc.SetBkMode(co::BKMODE::TRANSPARENT).unwrap();
            dc.SetTextColor(colorref(Rgb::new(200, 40, 60))).unwrap();
            fill(
                dc,
                w::RECT {
                    left: 0,
                    top: 0,
                    right: 16,
                    bottom: 12,
                },
                Rgb::gray(100),
            );
        })
        .unwrap();
        let back = buffer.as_ref().unwrap();
        let bitmap_handle = back.bitmap.to_string();
        // Windows refuses to select a bitmap into a second DC if the
        // first paint left it selected in the buffer's DC.
        let other = screen.CreateCompatibleDC().unwrap();
        {
            let _selected = other.SelectObject(&*back.bitmap).unwrap();
        }
        paint_buffered(&target, size, &mut buffer, |dc| {
            assert!(
                Some((dc.GetBkMode().unwrap(), dc.GetTextColor().unwrap())) == initial_state,
                "the previous paint must not change the next paint's drawing state"
            );
        })
        .unwrap();
        assert_eq!(buffer.as_ref().unwrap().bitmap.to_string(), bitmap_handle);
    }

    #[test]
    fn resizing_replaces_the_bitmap_and_empty_areas_release_it() {
        let screen = w::HWND::NULL.GetDC().unwrap();
        let target = screen.CreateCompatibleDC().unwrap();
        let target_bitmap = screen.CreateCompatibleBitmap(32, 32).unwrap();
        let _target = target.SelectObject(&*target_bitmap).unwrap();
        let mut buffer = None;
        for (width, height) in [(24, 16), (8, 6), (30, 20)] {
            paint_buffered(&target, w::SIZE::with(width, height), &mut buffer, |dc| {
                fill(
                    dc,
                    w::RECT {
                        left: 0,
                        top: 0,
                        right: width,
                        bottom: height,
                    },
                    Rgb::gray(80),
                );
            })
            .unwrap();
            let bitmap = buffer.as_ref().unwrap().bitmap.GetObject().unwrap();
            assert_eq!((bitmap.bmWidth, bitmap.bmHeight), (width, height));
        }
        paint_buffered(&target, w::SIZE::with(0, 20), &mut buffer, |_| {
            panic!("an empty area must not draw");
        })
        .unwrap();
        assert!(buffer.is_none());
    }

    #[test]
    fn dpi_scaling_rounds() {
        assert_eq!(mul_div(8, 96, 72), 11);
        assert_eq!(mul_div(9, 96, 72), 12);
        assert_eq!(mul_div(8, 144, 72), 16);
        assert_eq!(mul_div(16, 120, 96), 20);
        assert_eq!(mul_div(5, 7, 0), 35);
    }
}
