#![expect(
    unsafe_code,
    reason = "primary-monitor capture uses GDI BitBlt and GetDIBits handles"
)]

use super::capability::fail;
use serde_json::{Value, json};
use std::io::Cursor;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HGDIOBJ, ReleaseDC, SRCCOPY,
    SelectObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

pub(crate) fn capture_png() -> Result<Vec<u8>, String> {
    let (width, height, pixels) = capture_bgra()?;
    encode_png(width, height, &pixels)
}

pub(crate) fn list_monitors() -> Result<Vec<Value>, String> {
    let width = unsafe {
        // SAFETY: GetSystemMetrics is a documented user32 query with no pointer args.
        GetSystemMetrics(SM_CXSCREEN)
    };
    let height = unsafe {
        // SAFETY: same as width; primary screen size in pixels.
        GetSystemMetrics(SM_CYSCREEN)
    };
    if width <= 0 || height <= 0 {
        return Err(fail(
            "unavailable",
            "gdi",
            "GetSystemMetrics returned an empty screen",
        ));
    }
    Ok(vec![json!({
        "id": "primary",
        "width": width,
        "height": height,
        "scale": 1.0,
        "primary": true,
    })])
}

fn capture_bgra() -> Result<(u32, u32, Vec<u8>), String> {
    unsafe {
        // SAFETY: GDI screen capture of the primary monitor. All handles are
        // released on this path; no aliasing of the selected bitmap after
        // SelectObject restores `old`.
        let width = GetSystemMetrics(SM_CXSCREEN);
        let height = GetSystemMetrics(SM_CYSCREEN);
        if width <= 0 || height <= 0 {
            return Err(fail("unavailable", "gdi", "empty screen metrics"));
        }
        let hwnd: HWND = std::ptr::null_mut();
        let hdc = GetDC(hwnd);
        if hdc.is_null() {
            return Err(fail("unavailable", "gdi", "GetDC failed"));
        }
        let mem = CreateCompatibleDC(hdc);
        let bitmap = CreateCompatibleBitmap(hdc, width, height);
        let old = SelectObject(mem, bitmap as HGDIOBJ);
        let copied = BitBlt(mem, 0, 0, width, height, hdc, 0, 0, SRCCOPY);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: u32::try_from(std::mem::size_of::<BITMAPINFOHEADER>()).unwrap_or(40),
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [Default::default()],
        };
        let stride = usize::try_from(width).unwrap_or(0).saturating_mul(4);
        let mut pixels = vec![0u8; stride.saturating_mul(usize::try_from(height).unwrap_or(0))];
        let got = GetDIBits(
            mem,
            bitmap,
            0,
            height.cast_unsigned(),
            pixels.as_mut_ptr().cast(),
            std::ptr::from_mut(&mut info),
            DIB_RGB_COLORS,
        );
        SelectObject(mem, old);
        drop(DeleteObject(bitmap as HGDIOBJ));
        drop(DeleteDC(mem));
        drop(ReleaseDC(hwnd, hdc));
        if copied == 0 || got == 0 {
            return Err(fail("unavailable", "gdi", "BitBlt/GetDIBits failed"));
        }
        Ok((width.cast_unsigned(), height.cast_unsigned(), pixels))
    }
}

fn encode_png(width: u32, height: u32, bgra: &[u8]) -> Result<Vec<u8>, String> {
    let mut rgba = Vec::with_capacity(bgra.len());
    for chunk in bgra.chunks_exact(4) {
        rgba.extend_from_slice(&[chunk[2], chunk[1], chunk[0], chunk[3]]);
    }
    let img = image::RgbaImage::from_raw(width, height, rgba).ok_or_else(|| {
        fail(
            "unavailable",
            "gdi",
            "BGRA buffer does not match width*height",
        )
    })?;
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .map_err(|err| fail("unavailable", "gdi", err.to_string()))?;
    Ok(out.into_inner())
}
