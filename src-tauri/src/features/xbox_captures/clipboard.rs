//! Puts things on the Windows clipboard the way Win+PrtScn and Explorer do, so they paste into
//! Discord, Paint and Explorer. Knows nothing about Xbox.
//!
//! The clipboard is opened with a window of our own, not `OpenClipboard(NULL)`: with no owner,
//! `EmptyClipboard` leaves `SetClipboardData` unreliable. The window is message-only, and lives
//! only for the call, on the calling thread.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, bail};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICBitmapSource, IWICImagingFactory, IWICPalette,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IStream,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
};
use windows::core::{Interface, w};

const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;
const DROPEFFECT_COPY: u32 = 1;
const OPEN_ATTEMPTS: u32 = 10;
const OPEN_RETRY: Duration = Duration::from_millis(40);
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

pub fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(PNG_SIGNATURE)
}

/// Copies an encoded image (PNG, JPEG, whatever Windows can decode). PNG files also go on as
/// "PNG", which Chromium apps such as Discord prefer: it keeps the file as it was.
pub fn copy_image(file: &[u8]) -> anyhow::Result<()> {
    let dib = decode_dib(file).context("couldn't decode the image")?;
    let png = is_png(file).then_some(file);
    with_clipboard(|| {
        if let Some(png) = png {
            let format = unsafe { RegisterClipboardFormatW(w!("PNG")) };
            set_data(format, png)?;
        }
        set_data(CF_DIB, &dib)
    })
}

/// Copies files, as Explorer's Ctrl+C does.
pub fn copy_files(paths: &[&Path]) -> anyhow::Result<()> {
    let drop = drop_files(paths);
    with_clipboard(|| {
        set_data(CF_HDROP, &drop)?;
        let effect = unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) };
        set_data(effect, &DROPEFFECT_COPY.to_le_bytes())
    })
}

/// A `DROPFILES` header (wide characters) followed by the paths, each ending in a null, and one
/// more null at the end.
fn drop_files(paths: &[&Path]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend(20u32.to_le_bytes()); // offset of the list
    bytes.extend([0u8; 8]); // drop point
    bytes.extend(0u32.to_le_bytes()); // not in the non-client area
    bytes.extend(1u32.to_le_bytes()); // wide characters
    for path in paths {
        for unit in path.as_os_str().to_string_lossy().encode_utf16() {
            bytes.extend(unit.to_le_bytes());
        }
        bytes.extend(0u16.to_le_bytes());
    }
    bytes.extend(0u16.to_le_bytes());
    bytes
}

/// A bottom-up 32-bit `BITMAPINFOHEADER` and pixels, which every program that reads `CF_DIB`
/// understands.
fn dib_from_bgra(width: u32, height: u32, bgra: &[u8]) -> Vec<u8> {
    let stride = width as usize * 4;
    let mut out = Vec::with_capacity(40 + bgra.len());
    out.extend(40u32.to_le_bytes()); // biSize
    out.extend((width as i32).to_le_bytes());
    out.extend((height as i32).to_le_bytes()); // positive: bottom-up
    out.extend(1u16.to_le_bytes()); // planes
    out.extend(32u16.to_le_bytes()); // bit count
    out.extend(0u32.to_le_bytes()); // BI_RGB
    out.extend((bgra.len() as u32).to_le_bytes());
    out.extend([0u8; 16]); // resolution, colours used and important
    for row in bgra.chunks_exact(stride).rev() {
        out.extend(row);
    }
    out
}

/// Decodes with the Windows Imaging Component.
fn decode_dib(file: &[u8]) -> anyhow::Result<Vec<u8>> {
    unsafe {
        // Fails harmlessly if the thread already chose a different model
        let init = CoInitializeEx(None, COINIT_MULTITHREADED);
        let result = (|| -> windows::core::Result<Vec<u8>> {
            let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let stream = factory.CreateStream()?;
            stream.InitializeFromMemory(file)?;
            let decoder = factory.CreateDecoderFromStream(
                &stream.cast::<IStream>()?,
                std::ptr::null(),
                WICDecodeMetadataCacheOnDemand,
            )?;
            let frame = decoder.GetFrame(0)?;
            let converter = factory.CreateFormatConverter()?;
            converter.Initialize(
                &frame.cast::<IWICBitmapSource>()?,
                &GUID_WICPixelFormat32bppBGRA,
                WICBitmapDitherTypeNone,
                None::<&IWICPalette>,
                0.0,
                WICBitmapPaletteTypeCustom,
            )?;
            let (mut width, mut height) = (0, 0);
            converter.GetSize(&mut width, &mut height)?;
            let mut pixels = vec![0u8; width as usize * height as usize * 4];
            converter.CopyPixels(std::ptr::null(), width * 4, &mut pixels)?;
            Ok(dib_from_bgra(width, height, &pixels))
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        Ok(result?)
    }
}

/// The window that owns the clipboard while it's open.
struct Owner(HWND);

impl Owner {
    fn new() -> windows::core::Result<Self> {
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!(""),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
            .map(Self)
        }
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

/// Opens the clipboard (another program may be holding it for a moment), empties it, runs `fill`
/// and closes it again.
fn with_clipboard(fill: impl FnOnce() -> anyhow::Result<()>) -> anyhow::Result<()> {
    let owner = Owner::new().context("couldn't create a window to own the clipboard")?;
    let mut opened = false;
    for attempt in 0..OPEN_ATTEMPTS {
        if unsafe { OpenClipboard(Some(owner.0)) }.is_ok() {
            opened = true;
            break;
        }
        if attempt + 1 < OPEN_ATTEMPTS {
            std::thread::sleep(OPEN_RETRY);
        }
    }
    if !opened {
        bail!("another program is using the clipboard");
    }
    let result = unsafe { EmptyClipboard() }.context("couldn't empty the clipboard").and_then(|()| fill());
    let _ = unsafe { CloseClipboard() };
    result
}

/// Hands a copy of `bytes` to the clipboard, which owns it from then on.
fn set_data(format: u32, bytes: &[u8]) -> anyhow::Result<()> {
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
        let ptr = GlobalLock(mem);
        if ptr.is_null() {
            let _ = GlobalFree(Some(mem));
            bail!("couldn't allocate clipboard memory");
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
        let _ = GlobalUnlock(mem);
        // On failure the memory is still ours to free
        if let Err(e) = SetClipboardData(format, Some(HANDLE(mem.0))) {
            let _ = GlobalFree(Some(HGLOBAL(mem.0)));
            return Err(e).context("couldn't put data on the clipboard");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_files_are_recognised() {
        assert!(is_png(b"\x89PNG\r\n\x1a\n....."));
        assert!(!is_png(b"\xff\xd8\xff\xe0"));
        assert!(!is_png(b""));
    }

    #[test]
    fn drop_lists_are_wide_and_double_terminated() {
        let bytes = drop_files(&[Path::new("C:\\a.png")]);
        assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 20);
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 1);
        // "C:\a.png" is 8 characters, then two nulls
        assert_eq!(bytes.len(), 20 + (8 + 2) * 2);
        assert_eq!(&bytes[bytes.len() - 4..], &[0, 0, 0, 0]);
    }

    #[test]
    fn dibs_are_flipped_to_bottom_up() {
        // 1 pixel wide, 2 tall: top red, bottom blue (BGRA)
        let dib = dib_from_bgra(1, 2, &[0, 0, 255, 255, 255, 0, 0, 255]);
        assert_eq!(dib.len(), 40 + 8);
        assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), 2);
        assert_eq!(&dib[40..44], &[255, 0, 0, 255]);
    }
}
