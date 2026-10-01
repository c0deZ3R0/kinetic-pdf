//! Driver capabilities, validated DEVMODE settings, and cancellable spooling.
use super::*;
use std::{
    mem::size_of,
    ptr::{null, null_mut},
    sync::atomic::Ordering,
};
use windows_sys::Win32::{
    Foundation::POINT,
    Graphics::{Gdi::*, Printing::*},
    Storage::Xps::*,
    UI::Input::KeyboardAndMouse::GetActiveWindow,
};
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe fn string(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut n = 0;
    while *ptr.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(ptr, n))
}
struct Handle(PRINTER_HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            ClosePrinter(self.0);
        }
    }
}
fn open(name: &[u16]) -> Result<Handle, String> {
    let mut handle = PRINTER_HANDLE::default();
    unsafe {
        if OpenPrinterW(name.as_ptr(), &mut handle, null()) == 0 {
            return Err("Could not open this printer".into());
        }
    }
    Ok(Handle(handle))
}
struct Dc(HDC, bool);
impl Drop for Dc {
    fn drop(&mut self) {
        unsafe {
            if self.1 {
                AbortDoc(self.0);
            }
            DeleteDC(self.0);
        }
    }
}
thread_local! { static ABORT_FLAG: std::cell::RefCell<Option<Arc<AtomicBool>>> = const { std::cell::RefCell::new(None) }; }
struct AbortFlag;
impl Drop for AbortFlag {
    fn drop(&mut self) {
        ABORT_FLAG.with(|flag| *flag.borrow_mut() = None);
    }
}
unsafe extern "system" fn abort_proc(_: HDC, _: i32) -> i32 {
    ABORT_FLAG.with(|flag| i32::from(!flag.borrow().as_ref().is_some_and(|flag| flag.load(Ordering::Relaxed))))
}
fn dc(printer: &Printer) -> Result<Dc, String> {
    let driver = wide("WINSPOOL");
    let name = wide(&printer.name);
    let handle = unsafe { CreateDCW(driver.as_ptr(), name.as_ptr(), null(), printer.mode.as_ptr().cast()) };
    if handle.is_null() {
        return Err("The printer could not create a drawing context".into());
    }
    Ok(Dc(handle, false))
}
fn validate_mode(mode: &[u32]) -> Result<(), String> {
    if mode.len() * 4 < size_of::<DEVMODEW>() {
        return Err("The printer returned incomplete settings".into());
    }
    let dm = unsafe { &*(mode.as_ptr().cast::<DEVMODEW>()) };
    if usize::from(dm.dmSize) < size_of::<DEVMODEW>()
        || usize::from(dm.dmSize) + usize::from(dm.dmDriverExtra) > mode.len() * 4
    {
        return Err("The printer returned invalid settings".into());
    }
    Ok(())
}
pub fn printers() -> Result<(Vec<String>, String), String> {
    unsafe {
        let mut needed = 0;
        let mut count = 0;
        EnumPrintersW(PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS, null(), 4, null_mut(), 0, &mut needed, &mut count);
        if needed == 0 {
            return Ok((Vec::new(), String::new()));
        }
        // Pointer-aligned storage for PRINTER_INFO_4W and its trailing strings.
        let mut storage = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
        if EnumPrintersW(
            PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS,
            null(),
            4,
            storage.as_mut_ptr().cast(),
            needed,
            &mut needed,
            &mut count,
        ) == 0
        {
            return Err("Could not list installed printers".into());
        }
        if count as usize * size_of::<PRINTER_INFO_4W>() > storage.len() * size_of::<usize>() {
            return Err("Invalid printer list".into());
        }
        let infos = std::slice::from_raw_parts(storage.as_ptr().cast::<PRINTER_INFO_4W>(), count as usize);
        let mut names: Vec<_> = infos.iter().map(|i| string(i.pPrinterName)).collect();
        names.sort();
        names.dedup();
        let mut length = 0;
        GetDefaultPrinterW(null_mut(), &mut length);
        let mut default = vec![0u16; length as usize];
        let default = if length > 0 && GetDefaultPrinterW(default.as_mut_ptr(), &mut length) != 0 {
            string(default.as_ptr())
        } else {
            String::new()
        };
        Ok((names, default))
    }
}
pub fn load_printer(name: &str) -> Result<Printer, String> {
    unsafe {
        let namew = wide(name);
        let handle = open(&namew)?;
        let length = DocumentPropertiesW(null_mut(), handle.0, namew.as_ptr(), null_mut(), null(), 0);
        if length < size_of::<DEVMODEW>() as i32 {
            return Err("The printer did not return usable settings".into());
        }
        let mut mode = vec![0u32; (length as usize).div_ceil(4)];
        if DocumentPropertiesW(null_mut(), handle.0, namew.as_ptr(), mode.as_mut_ptr().cast(), null(), DM_OUT_BUFFER)
            != 1
        {
            return Err("Could not read printer settings".into());
        }
        validate_mode(&mode)?;
        let dm = &*(mode.as_ptr().cast::<DEVMODEW>());
        let n = DeviceCapabilitiesW(namew.as_ptr(), null(), DC_PAPERS, null_mut(), dm);
        if n <= 0 || n > 10000 {
            return Err("The printer did not report supported paper sizes".into());
        }
        let mut ids = vec![0u16; n as usize];
        let mut sizes = vec![POINT::default(); n as usize];
        let mut names = vec![0u16; n as usize * 64];
        if DeviceCapabilitiesW(namew.as_ptr(), null(), DC_PAPERS, ids.as_mut_ptr(), dm) != n
            || DeviceCapabilitiesW(namew.as_ptr(), null(), DC_PAPERSIZE, sizes.as_mut_ptr().cast(), dm) != n
            || DeviceCapabilitiesW(namew.as_ptr(), null(), DC_PAPERNAMES, names.as_mut_ptr(), dm) != n
        {
            return Err("Could not read supported paper sizes".into());
        }
        let papers = ids
            .iter()
            .enumerate()
            .filter(|(i, _)| sizes[*i].x > 0 && sizes[*i].y > 0)
            .map(|(i, id)| {
                let slice = &names[i * 64..(i + 1) * 64];
                let end = slice.iter().position(|&v| v == 0).unwrap_or(64);
                Paper {
                    id: *id as i16,
                    name: String::from_utf16_lossy(&slice[..end]),
                    mm: [sizes[i].x as f32 / 10., sizes[i].y as f32 / 10.],
                }
            })
            .collect();
        let default_paper = dm.Anonymous1.Anonymous1.dmPaperSize;
        let landscape = dm.Anonymous1.Anonymous1.dmOrientation == DMORIENT_LANDSCAPE as i16;
        let colour = DeviceCapabilitiesW(namew.as_ptr(), null(), DC_COLORDEVICE, null_mut(), dm) == 1;
        let duplex = DeviceCapabilitiesW(namew.as_ptr(), null(), DC_DUPLEX, null_mut(), dm) == 1;
        let default_colour = if dm.dmColor == DMCOLOR_MONOCHROME { Colour::Grayscale } else { Colour::Colour };
        let default_duplex = match dm.dmDuplex {
            DMDUP_VERTICAL => Duplex::LongEdge,
            DMDUP_HORIZONTAL => Duplex::ShortEdge,
            _ => Duplex::Single,
        };
        Ok(Printer {
            name: name.into(),
            mode,
            papers,
            default_paper,
            landscape,
            colour,
            duplex,
            default_colour,
            default_duplex,
        })
    }
}
pub fn properties(printer: &Printer) -> Result<Option<Printer>, String> {
    unsafe {
        let name = wide(&printer.name);
        let handle = open(&name)?;
        let mut result = printer.clone();
        let code = DocumentPropertiesW(
            GetActiveWindow(),
            handle.0,
            name.as_ptr(),
            result.mode.as_mut_ptr().cast(),
            printer.mode.as_ptr().cast(),
            DM_IN_BUFFER | DM_OUT_BUFFER | DM_IN_PROMPT,
        );
        if code == 2 {
            return Ok(None);
        }
        if code != 1 {
            return Err("Could not open Printer Properties".into());
        }
        validate_mode(&result.mode)?;
        let dm = &*(result.mode.as_ptr().cast::<DEVMODEW>());
        result.default_paper = dm.Anonymous1.Anonymous1.dmPaperSize;
        result.landscape = dm.Anonymous1.Anonymous1.dmOrientation == DMORIENT_LANDSCAPE as i16;
        result.default_colour = if dm.dmColor == DMCOLOR_MONOCHROME { Colour::Grayscale } else { Colour::Colour };
        result.default_duplex = match dm.dmDuplex {
            DMDUP_VERTICAL => Duplex::LongEdge,
            DMDUP_HORIZONTAL => Duplex::ShortEdge,
            _ => Duplex::Single,
        };
        Ok(Some(result))
    }
}
pub fn configure(printer: &Printer, options: &Options, source: [f32; 2]) -> Result<Configured, String> {
    unsafe {
        let (paper, landscape) = paper_for(printer, options, source)?;
        let mut result = printer.clone();
        validate_mode(&result.mode)?;
        let dm = &mut *(result.mode.as_mut_ptr().cast::<DEVMODEW>());
        dm.dmFields |= DM_PAPERSIZE | DM_ORIENTATION | DM_SCALE | DM_COPIES | DM_COLLATE;
        dm.dmFields &= !(DM_PAPERWIDTH | DM_PAPERLENGTH | DM_FORMNAME);
        dm.Anonymous1.Anonymous1.dmPaperSize = paper;
        dm.Anonymous1.Anonymous1.dmOrientation =
            if landscape { DMORIENT_LANDSCAPE as i16 } else { DMORIENT_PORTRAIT as i16 };
        dm.Anonymous1.Anonymous1.dmScale = 100;
        dm.Anonymous1.Anonymous1.dmCopies = 1;
        dm.dmCollate = 0;
        if printer.colour {
            dm.dmFields |= DM_COLOR;
            dm.dmColor = if options.colour == Colour::Colour { DMCOLOR_COLOR } else { DMCOLOR_MONOCHROME };
        }
        if printer.duplex {
            dm.dmFields |= DM_DUPLEX;
            dm.dmDuplex = match options.duplex {
                Duplex::Single => DMDUP_SIMPLEX,
                Duplex::LongEdge => DMDUP_VERTICAL,
                Duplex::ShortEdge => DMDUP_HORIZONTAL,
            };
        } else if options.duplex != Duplex::Single {
            return Err("This printer does not support double-sided printing".into());
        }
        let input = result.mode.clone();
        let name = wide(&printer.name);
        let handle = open(&name)?;
        if DocumentPropertiesW(
            null_mut(),
            handle.0,
            name.as_ptr(),
            result.mode.as_mut_ptr().cast(),
            input.as_ptr().cast(),
            DM_IN_BUFFER | DM_OUT_BUFFER,
        ) != 1
        {
            return Err("The printer rejected these settings".into());
        }
        validate_mode(&result.mode)?;
        let normalized = &*(result.mode.as_ptr().cast::<DEVMODEW>());
        if normalized.Anonymous1.Anonymous1.dmPaperSize != paper
            || normalized.Anonymous1.Anonymous1.dmOrientation != if landscape { 2 } else { 1 }
        {
            return Err("The printer changed the requested paper or orientation. Check Printer Properties.".into());
        }
        let device = dc(&result)?;
        let dpi = [GetDeviceCaps(device.0, LOGPIXELSX as i32), GetDeviceCaps(device.0, LOGPIXELSY as i32)];
        if dpi.iter().any(|&d| d <= 0) {
            return Err("Invalid printer resolution".into());
        }
        let pt = |index: u32, axis: usize| GetDeviceCaps(device.0, index as i32) as f32 * 72. / dpi[axis] as f32;
        let metrics = Metrics {
            paper: [pt(PHYSICALWIDTH, 0), pt(PHYSICALHEIGHT, 1)],
            printable: [pt(PHYSICALOFFSETX, 0), pt(PHYSICALOFFSETY, 1), pt(HORZRES, 0), pt(VERTRES, 1)],
            dpi,
        };
        if metrics.paper.iter().any(|&v| v <= 0.) || metrics.printable[2] <= 0. || metrics.printable[3] <= 0. {
            return Err("Invalid printer dimensions".into());
        }
        Ok(Configured { printer: result, metrics, options: options.clone() })
    }
}

/// Each raster band uses at most 8 MB, independent of the sheet's height.
/// Render into a small bitmap using the full page's origin and dimensions;
/// Pdfium clips the draw, including form fields, to that bitmap.
fn raster(
    page: &PdfPage<'_>,
    device: HDC,
    pos: &Placement,
    metrics: &Metrics,
    options: &Options,
    cancelled: &AtomicBool,
) -> Result<bool, String> {
    let dpi = f32::from(options.dpi);
    let [x, y, w, h] = pos.rect;
    let [cx, cy, cw, ch] = pos.clip;
    let left = x.max(cx);
    let top = y.max(cy);
    let right = (x + w).min(cx + cw);
    let bottom = (y + h).min(cy + ch);
    if right <= left || bottom <= top {
        return Ok(true);
    }
    let bw = ((right - left) * dpi / 72.).ceil().max(1.) as i32;
    let bh = ((bottom - top) * dpi / 72.).ceil().max(1.) as i32;
    if bw > 100000 || bh > 100000 {
        return Err("The requested raster size is too large. Use a lower image DPI.".into());
    }
    let rows = (8 * 1024 * 1024 / (bw as usize * 4)).clamp(1, 256) as i32;
    let full = [(w * dpi / 72.).round().max(1.) as i32, (h * dpi / 72.).round().max(1.) as i32];
    let origin = [((x - left) * dpi / 72.).round() as i32, ((y - top) * dpi / 72.).round() as i32];
    let to_device =
        |p: f32, axis: usize| ((p - metrics.printable[axis]) * metrics.dpi[axis] as f32 / 72.).round() as i32;
    let dx = to_device(left, 0);
    let dy = to_device(top, 1);
    let dw = ((right - left) * metrics.dpi[0] as f32 / 72.).round() as i32;
    let dh = ((bottom - top) * metrics.dpi[1] as f32 / 72.).round() as i32;
    let mut row = 0;
    while row < bh {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let height = rows.min(bh - row);
        let mut bitmap = PdfBitmap::empty(bw, height, PdfBitmapFormat::BGRA).map_err(|e| e.to_string())?;
        let mut config = PdfRenderConfig::new()
            .set_fixed_size(full[0], full[1])
            .set_origin(origin[0], origin[1] - row)
            .set_clear_color(PdfColor::WHITE)
            .use_print_quality(true)
            .use_grayscale_rendering(options.colour != Colour::Colour);
        if pos.rotate {
            config = config.rotate(PdfPageRenderRotation::Degrees90, false);
        }
        page.render_into_bitmap_with_config(&mut bitmap, &config).map_err(|e| e.to_string())?;
        let source = bitmap.as_raw_bytes();
        let (bits, pixels) = if options.colour == Colour::Colour {
            (32, source)
        } else {
            raster_data(&source, bw as usize, height as usize, options.colour)
        };
        #[repr(C)]
        struct Info {
            header: BITMAPINFOHEADER,
            palette: [RGBQUAD; 256],
        }
        let mut info = Info {
            header: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: bw,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: bits,
                biCompression: BI_RGB,
                biSizeImage: pixels.len() as u32,
                biClrUsed: if bits == 1 {
                    2
                } else if bits == 8 {
                    256
                } else {
                    0
                },
                ..Default::default()
            },
            palette: [RGBQUAD::default(); 256],
        };
        for (i, entry) in info.palette.iter_mut().enumerate() {
            let value = if bits == 1 {
                if i == 0 {
                    0
                } else {
                    255
                }
            } else {
                i as u8
            };
            *entry = RGBQUAD { rgbBlue: value, rgbGreen: value, rgbRed: value, rgbReserved: 0 };
        }
        let y0 = (i64::from(row) * i64::from(dh) / i64::from(bh)) as i32;
        let y1 = (i64::from(row + height) * i64::from(dh) / i64::from(bh)) as i32;
        unsafe {
            let result = StretchDIBits(
                device,
                dx,
                dy + y0,
                dw,
                y1 - y0,
                0,
                0,
                bw,
                height,
                pixels.as_ptr().cast(),
                (&info as *const Info).cast(),
                DIB_RGB_COLORS,
                SRCCOPY,
            );
            if result == 0 || result == -1 {
                return Err("The printer could not draw an image band".into());
            }
        }
        row += height;
    }
    Ok(true)
}

pub fn spool(pdfium: &Pdfium, bytes: &[u8], job: &Job, progress: impl Fn(usize, usize)) -> Result<bool, String> {
    if ![1, 2, 4, 6, 9, 16].contains(&job.options.pages_per_sheet) {
        return Err("Choose a supported number of pages per sheet".into());
    }
    if !job.options.percent.is_finite() || job.options.percent <= 0. || job.options.percent > 1000. {
        return Err("Choose a scale between 1% and 1000%".into());
    }
    if ![150, 300, 600].contains(&job.options.dpi) {
        return Err("Choose 150, 300 or 600 DPI".into());
    }
    if job.pages.is_empty() {
        return Err("Choose at least one page to print".into());
    }
    if job.options.copies == 0 || job.options.copies > 999 {
        return Err("Copies must be between 1 and 999".into());
    }
    let doc = pdfium.load_pdf_from_byte_vec(bytes.to_vec(), None).map_err(|e| e.to_string())?;
    let groups: Vec<_> = job.pages.chunks(job.options.pages_per_sheet).collect();
    // Validate every selected sheet before submitting any page to the spooler.
    let mut configurations: Vec<Configured> = Vec::new();
    let mut cache: std::collections::HashMap<(i16, bool), Configured> = std::collections::HashMap::new();
    for (sheet, pages) in groups.iter().enumerate() {
        if job.cancelled.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let page = doc.pages().get(pages[0] as i32).map_err(|e| e.to_string())?;
        let source = [page.width().value, page.height().value];
        let mut effective = job.options.clone();
        if effective.duplex != Duplex::Single && effective.orientation == Orientation::Auto && sheet % 2 == 1 {
            let front = &configurations[sheet - 1];
            effective.orientation = if front.metrics.paper[0] > front.metrics.paper[1] {
                Orientation::Landscape
            } else {
                Orientation::Portrait
            };
        }
        let key = paper_for(&job.printer, &effective, source)?;
        let configuration = if let Some(config) = cache.get(&key) {
            config.clone()
        } else {
            let config = configure(&job.printer, &effective, source)?;
            cache.insert(key, config.clone());
            config
        };
        configurations.push(configuration);
    }
    if job.options.duplex != Duplex::Single
        && configurations.chunks(2).any(|pair| pair.len() == 2 && pair[0].printer.mode != pair[1].printer.mode)
    {
        return Err("The front and back of a sheet need the same paper and orientation. Choose a fixed orientation and paper size, or single-sided printing.".into());
    }
    let mut device = dc(&configurations[0].printer)?;
    ABORT_FLAG.with(|flag| *flag.borrow_mut() = Some(job.cancelled.clone()));
    let _abort_flag = AbortFlag;
    unsafe {
        SetAbortProc(device.0, Some(abort_proc));
    }
    let title = wide("Kinetic PDF");
    let output = job.output.as_ref().map(|p| wide(&p.to_string_lossy()));
    let info = DOCINFOW {
        cbSize: size_of::<DOCINFOW>() as i32,
        lpszDocName: title.as_ptr(),
        lpszOutput: output.as_ref().map_or(null(), |s| s.as_ptr()),
        ..Default::default()
    };
    unsafe {
        if StartDocW(device.0, &info) <= 0 {
            return Err("Could not start the print job".into());
        }
    }
    device.1 = true;
    // Keep software copies at one in DEVMODE, preventing driver duplication.
    // Collation is deterministic across single-sided and duplex jobs.
    let order = copy_order(groups.len(), &job.options);
    let mut previous = 0;
    for (at, &sheet) in order.iter().enumerate() {
        let Some(sheet) = sheet else {
            if job.cancelled.load(Ordering::Relaxed) {
                return Ok(false);
            }
            unsafe {
                if StartPage(device.0) <= 0 || EndPage(device.0) <= 0 {
                    return Err("Could not finish the blank back of a copy".into());
                }
            }
            progress(at + 1, order.len());
            continue;
        };
        if job.cancelled.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let configured = &configurations[sheet];
        if configurations[previous].printer.mode != configured.printer.mode {
            unsafe {
                let result = ResetDCW(device.0, configured.printer.mode.as_ptr().cast());
                if result.is_null() {
                    return Err("Could not change paper size for this sheet".into());
                }
                device.0 = result;
            }
        }
        previous = sheet;
        unsafe {
            if StartPage(device.0) <= 0 {
                return Err("Could not start a printed page".into());
            }
        }
        let sizes: Vec<_> = groups[sheet]
            .iter()
            .map(|&p| doc.pages().get(p as i32).map(|p| [p.width().value, p.height().value]).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        let positions = placements(&sizes, &configured.metrics, &job.options);
        for (&index, pos) in groups[sheet].iter().zip(positions) {
            let page = doc.pages().get(index as i32).map_err(|e| e.to_string())?;
            if job.cancelled.load(Ordering::Relaxed) {
                return Ok(false);
            }
            // Form overlays require the bitmap renderer. Pure B&W also needs
            // raster thresholding; normal vector output remains the default.
            if job.options.as_image || job.options.colour == Colour::BlackWhite || doc.form().is_some() {
                if !raster(&page, device.0, &pos, &configured.metrics, &job.options, &job.cancelled)? {
                    return Ok(false);
                }
            } else {
                unsafe {
                    let metrics = &configured.metrics;
                    let pixel = |v: f32, axis: usize| (v * metrics.dpi[axis] as f32 / 72.).round() as i32;
                    let [x, y, w, h] = pos.rect;
                    let [cx, cy, cw, ch] = pos.clip;
                    let saved = SaveDC(device.0);
                    if saved == 0 {
                        return Err("Could not save printer drawing state".into());
                    }
                    IntersectClipRect(
                        device.0,
                        pixel(cx - metrics.printable[0], 0),
                        pixel(cy - metrics.printable[1], 1),
                        pixel(cx + cw - metrics.printable[0], 0),
                        pixel(cy + ch - metrics.printable[1], 1),
                    );
                    let flags = 1 | 2048 | 512 | if job.options.colour == Colour::Grayscale { 8 } else { 0 };
                    page.render_to_printer_dc(
                        std::mem::transmute(device.0),
                        [
                            pixel(x - metrics.printable[0], 0),
                            pixel(y - metrics.printable[1], 1),
                            pixel(w, 0),
                            pixel(h, 1),
                        ],
                        i32::from(pos.rotate),
                        flags,
                    );
                    RestoreDC(device.0, saved);
                }
            }
        }
        unsafe {
            if EndPage(device.0) <= 0 {
                if job.cancelled.load(Ordering::Relaxed) {
                    return Ok(false);
                }
                return Err("Could not finish a printed page".into());
            }
        }
        progress(at + 1, order.len());
    }
    if job.cancelled.load(Ordering::Relaxed) {
        return Ok(false);
    }
    unsafe {
        if EndDoc(device.0) <= 0 {
            return Err("Could not finish the print job".into());
        }
    }
    device.1 = false;
    Ok(true)
}
