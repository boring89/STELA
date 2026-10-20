// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The firmware framebuffer as a [`Canvas`], the global framebuffer
//! console, and the panic screen.
//!
//! The kernel reaches the framebuffer through the HHDM with ordinary
//! write-back caching; user space (the window server) maps it
//! write-combining through [`device`]. Once it does, the console stops
//! drawing ([`hand_over`]; the serial port goes on) and only the panic
//! screen draws again. Pixels are 32 bits in the order the GOP reported
//! (`Rgb32` or `Bgr32`).

use super::font::{self, Font};
use super::text::{Canvas, Rgb, TextConsole, draw_char};
use crate::mm::device::DeviceRange;
use crate::sync::SpinLock;
use bootinfo::{BootInfo, Framebuffer, HHDM_BASE, PixelFormat};
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

/// The framebuffer's memory.
pub(crate) struct FbCanvas {
    base: *mut u8,
    width: usize,
    height: usize,
    pitch: usize,
    bgr: bool,
}

// SAFETY: the canvas only holds the address of device memory, which may be
// written from any processor.
unsafe impl Send for FbCanvas {}

impl FbCanvas {
    /// A canvas for the boot framebuffer, if there is one the HHDM covers.
    fn new(info: &BootInfo) -> Option<FbCanvas> {
        let fb: &Framebuffer = &info.framebuffer;
        let end = fb.phys_addr.checked_add(fb.size)?;
        let fits = fb.is_present()
            && end <= info.hhdm_size
            && u64::from(fb.pitch) * u64::from(fb.height) <= fb.size
            && fb.pitch >= fb.width * 4;
        fits.then(|| FbCanvas {
            base: (HHDM_BASE + fb.phys_addr) as *mut u8,
            width: fb.width as usize,
            height: fb.height as usize,
            pitch: fb.pitch as usize,
            bgr: fb.format() == PixelFormat::Bgr32,
        })
    }

    const fn encode(&self, Rgb(r, g, b): Rgb) -> u32 {
        let (r, g, b) = (r as u32, g as u32, b as u32);
        if self.bgr {
            // Byte 0 blue, byte 1 green, byte 2 red (little endian u32).
            (r << 16) | (g << 8) | b
        } else {
            (b << 16) | (g << 8) | r
        }
    }
}

impl Canvas for FbCanvas {
    fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    fn set_pixel(&mut self, x: usize, y: usize, color: Rgb) {
        if x >= self.width || y >= self.height {
            return;
        }
        let value = self.encode(color);
        // SAFETY: (x, y) is inside the visible area, so the 4 bytes at
        // y * pitch + x * 4 lie within the framebuffer (checked in `new`:
        // pitch * height <= size, pitch >= width * 4), which the HHDM maps.
        // The framebuffer is not Rust-owned memory; no references to it
        // exist. Offsets are multiples of 4 from a page-aligned base.
        unsafe {
            self.base
                .add(y * self.pitch + x * 4)
                .cast::<u32>()
                .write_volatile(value)
        };
    }

    fn fill(&mut self, x: usize, y: usize, width: usize, height: usize, color: Rgb) {
        let value = self.encode(color);
        let x_end = (x + width).min(self.width);
        for py in y..(y + height).min(self.height) {
            for px in x..x_end {
                // SAFETY: (px, py) is inside the visible area; see set_pixel.
                // A plain write lets the compiler turn the row into a block
                // store; the framebuffer has no read side effects.
                unsafe {
                    self.base
                        .add(py * self.pitch + px * 4)
                        .cast::<u32>()
                        .write(value)
                };
            }
        }
    }

    fn scroll_up(&mut self, rows: usize, color: Rgb) {
        let rows = rows.min(self.height);
        let keep = (self.height - rows) * self.pitch;
        // SAFETY: both ranges lie within the first pitch * height bytes of
        // the framebuffer; `copy` handles the overlap (memmove).
        unsafe { core::ptr::copy(self.base.add(rows * self.pitch), self.base, keep) };
        self.fill(0, self.height - rows, self.width, rows, color);
    }
}

/// Boot console colours: light grey on near black.
const CONSOLE_FG: Rgb = Rgb(0xD8, 0xD8, 0xD8);
const CONSOLE_BG: Rgb = Rgb(0x10, 0x10, 0x14);

/// Panic screen colours: parchment on a dark Pompeian red.
const PANIC_FG: Rgb = Rgb(0xF2, 0xE8, 0xD5);
const PANIC_BG: Rgb = Rgb(0x5C, 0x1A, 0x14);

struct FbConsole {
    canvas: FbCanvas,
    text: TextConsole,
}

static CONSOLE: SpinLock<Option<FbConsole>> = SpinLock::new(None);

/// Where the framebuffer is, kept for the panic screen (which must not
/// depend on the console lock).
static PANIC_INFO: SpinLock<Option<&'static BootInfo>> = SpinLock::new(None);

/// Clears the screen and starts the framebuffer console.
pub(crate) fn init(info: &'static BootInfo) {
    let Some(mut canvas) = FbCanvas::new(info) else {
        return;
    };
    let font = font::for_width(canvas.width);
    let mut text = TextConsole::new(font, canvas.width, canvas.height, CONSOLE_FG, CONSOLE_BG);
    text.clear(&mut canvas);
    *CONSOLE.lock() = Some(FbConsole { canvas, text });
    *PANIC_INFO.lock() = Some(info);
}

/// Set once user space owns the screen.
static HANDED_OVER: AtomicBool = AtomicBool::new(false);

/// The boot framebuffer as user space sees it.
#[derive(Clone, Debug)]
pub struct FramebufferDevice {
    /// Its pages.
    pub range: DeviceRange,
    /// Visible width in pixels.
    pub width: u32,
    /// Visible height in pixels.
    pub height: u32,
    /// Bytes per row.
    pub pitch: u32,
    /// Byte 0 of a pixel is blue (else red).
    pub bgr: bool,
}

/// The boot framebuffer, if there is a usable one (the checks of the
/// console: inside the direct map, the rows fit).
pub fn device() -> Option<FramebufferDevice> {
    let info = (*PANIC_INFO.lock())?;
    FbCanvas::new(info)?;
    let fb = &info.framebuffer;
    Some(FramebufferDevice {
        range: DeviceRange::new(fb.phys_addr, u64::from(fb.pitch) * u64::from(fb.height)),
        width: fb.width,
        height: fb.height,
        pitch: fb.pitch,
        bgr: fb.format() == PixelFormat::Bgr32,
    })
}

/// User space took the screen: the console stops drawing (it keeps going
/// to the serial port). The panic screen still draws.
pub fn hand_over() {
    HANDED_OVER.store(true, Ordering::Release);
}

/// Whether [`hand_over`] happened.
pub fn handed_over() -> bool {
    HANDED_OVER.load(Ordering::Acquire)
}

/// Writes to the framebuffer console, if it exists, is not busy (a panic
/// while printing must not deadlock) and still owns the screen.
pub(crate) fn write_str(s: &str) {
    if handed_over() {
        return;
    }
    if let Some(mut console) = CONSOLE.try_lock()
        && let Some(FbConsole { canvas, text }) = console.as_mut()
    {
        text.write_str(canvas, s);
    }
}

/// Collects panic text without allocating.
struct PanicText {
    bytes: [u8; 4096],
    len: usize,
}

impl Write for PanicText {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // Keep what fits; cut at a character boundary.
        let mut take = s.len().min(self.bytes.len() - self.len);
        while !s.is_char_boundary(take) {
            take -= 1;
        }
        self.bytes[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

/// Draws `s` centred on row `y`.
fn draw_centered(canvas: &mut FbCanvas, font: &Font, y: usize, s: &str, fg: Rgb, bg: Rgb) {
    let width = s.chars().count() * font.width;
    let mut x = canvas.width.saturating_sub(width) / 2;
    for c in s.chars() {
        draw_char(canvas, font, x, y, c, fg, bg);
        x += font.width;
    }
}

/// Paints the panic screen: the whole framebuffer in the panic colour,
/// "STELA" and "kernel panic" centred, and the message as a left-aligned
/// block centred on the screen. Uses its own canvas, so it works even if
/// the panic happened while the console was locked.
pub(crate) fn panic_screen(message: fmt::Arguments) {
    let Some(info) = PANIC_INFO.try_lock().and_then(|i| *i) else {
        return;
    };
    let Some(mut canvas) = FbCanvas::new(info) else {
        return;
    };
    // The console must not draw over the panic screen afterwards.
    if let Some(mut console) = CONSOLE.try_lock() {
        *console = None;
    }
    let mut text = PanicText {
        bytes: [0; 4096],
        len: 0,
    };
    let _ = text.write_fmt(message);
    let message = core::str::from_utf8(&text.bytes[..text.len]).unwrap_or("(unprintable message)");

    let font = font::for_width(canvas.width);
    let (w, h) = canvas.size();
    canvas.fill(0, 0, w, h, PANIC_BG);

    let lines = message.lines().count();
    let block_width = message
        .lines()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
        * font.width;
    // Title, blank line, subtitle, two blank lines, message.
    let total_rows = 5 + lines;
    let mut y = h.saturating_sub(total_rows * font.height) / 2;
    draw_centered(&mut canvas, font, y, "S T E L A", PANIC_FG, PANIC_BG);
    y += 2 * font.height;
    draw_centered(&mut canvas, font, y, "kernel panic", PANIC_FG, PANIC_BG);
    y += 3 * font.height;
    let left = w.saturating_sub(block_width) / 2;
    for line in message.lines() {
        let mut x = left;
        for c in line.chars() {
            draw_char(&mut canvas, font, x, y, c, PANIC_FG, PANIC_BG);
            x += font.width;
        }
        y += font.height;
    }
}
