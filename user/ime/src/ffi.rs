// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The boundary to libchewing (C, `build.rs`): its functions, the hooks
//! the stand-in C library calls (`c/libc.c`: memory, abort) and a safe
//! [`Chewing`] around one context. Every `unsafe` block of the IME is in
//! this file (`CLAUDE.md`, the unsafe rule).
//!
//! The stand-in C library keeps its files in one global table, so there
//! is at most one [`Chewing`] at a time: creating a second fails while the
//! first exists.

#![allow(unsafe_code)]

use alloc::alloc::{Layout, alloc, dealloc};
use alloc::ffi::CString;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::sync::atomic::{AtomicBool, Ordering};

/// libchewing's opaque `ChewingContext`.
#[repr(C)]
struct Context {
    _private: [u8; 0],
}

type Logger = Option<unsafe extern "C" fn(*mut c_void, c_int, *const c_char, ...)>;

unsafe extern "C" {
    fn chewing_new2(
        syspath: *const c_char,
        userpath: *const c_char,
        logger: Logger,
        data: *mut c_void,
    ) -> *mut Context;
    fn chewing_delete(ctx: *mut Context);
    fn chewing_set_ChiEngMode(ctx: *mut Context, mode: c_int);
    fn chewing_get_ChiEngMode(ctx: *const Context) -> c_int;
    fn chewing_set_candPerPage(ctx: *mut Context, n: c_int);
    fn chewing_set_maxChiSymbolLen(ctx: *mut Context, n: c_int);
    fn chewing_set_spaceAsSelection(ctx: *mut Context, mode: c_int);
    fn chewing_set_escCleanAllBuf(ctx: *mut Context, mode: c_int);
    fn chewing_set_autoShiftCur(ctx: *mut Context, mode: c_int);
    fn chewing_set_phraseChoiceRearward(ctx: *mut Context, mode: c_int);

    fn chewing_handle_Default(ctx: *mut Context, key: c_int) -> c_int;
    fn chewing_handle_Space(ctx: *mut Context) -> c_int;
    fn chewing_handle_Enter(ctx: *mut Context) -> c_int;
    fn chewing_handle_Esc(ctx: *mut Context) -> c_int;
    fn chewing_handle_Backspace(ctx: *mut Context) -> c_int;
    fn chewing_handle_Del(ctx: *mut Context) -> c_int;
    fn chewing_handle_Left(ctx: *mut Context) -> c_int;
    fn chewing_handle_Right(ctx: *mut Context) -> c_int;
    fn chewing_handle_Up(ctx: *mut Context) -> c_int;
    fn chewing_handle_Down(ctx: *mut Context) -> c_int;
    fn chewing_handle_Home(ctx: *mut Context) -> c_int;
    fn chewing_handle_End(ctx: *mut Context) -> c_int;
    fn chewing_handle_PageUp(ctx: *mut Context) -> c_int;
    fn chewing_handle_PageDown(ctx: *mut Context) -> c_int;
    fn chewing_handle_Tab(ctx: *mut Context) -> c_int;
    fn chewing_handle_ShiftSpace(ctx: *mut Context) -> c_int;
    fn chewing_handle_CtrlNum(ctx: *mut Context, key: c_int) -> c_int;

    fn chewing_commit_Check(ctx: *const Context) -> c_int;
    fn chewing_commit_String_static(ctx: *const Context) -> *const c_char;
    fn chewing_buffer_String_static(ctx: *const Context) -> *const c_char;
    fn chewing_bopomofo_String_static(ctx: *const Context) -> *const c_char;
    fn chewing_cursor_Current(ctx: *const Context) -> c_int;
    fn chewing_keystroke_CheckIgnore(ctx: *const Context) -> c_int;
    fn chewing_cand_TotalChoice(ctx: *const Context) -> c_int;
    fn chewing_cand_ChoicePerPage(ctx: *const Context) -> c_int;
    fn chewing_cand_CurrentPage(ctx: *const Context) -> c_int;
    fn chewing_cand_TotalPage(ctx: *const Context) -> c_int;
    fn chewing_cand_string_by_index_static(ctx: *mut Context, index: c_int) -> *const c_char;

    fn ime_vfs_reset();
    fn ime_vfs_add(path: *const c_char, data: *const u8, len: usize) -> c_int;
    fn ime_vfs_get(path: *const c_char, len: *mut usize, dirty: *mut c_int) -> *const u8;
}

/// Bytes in front of each C allocation (`c/libc.c`, `HDR`): the alignment.
const C_ALIGN: usize = 16;

/// `malloc` of the stand-in C library.
#[unsafe(no_mangle)]
pub extern "C" fn ime_rust_alloc(size: usize) -> *mut u8 {
    match Layout::from_size_align(size.max(1), C_ALIGN) {
        // SAFETY: the layout has a nonzero size.
        Ok(layout) => unsafe { alloc(layout) },
        Err(_) => core::ptr::null_mut(),
    }
}

/// `free` of the stand-in C library: `p` and `size` are what
/// [`ime_rust_alloc`] gave and was asked for.
///
/// # Safety
///
/// `p` must come from `ime_rust_alloc(size)` and not be freed yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ime_rust_free(p: *mut u8, size: usize) {
    if let Ok(layout) = Layout::from_size_align(size.max(1), C_ALIGN) {
        // SAFETY: the caller's contract: allocated with this layout.
        unsafe { dealloc(p, layout) };
    }
}

/// `exit`, `abort` and a failed `assert` in the C code.
///
/// # Safety
///
/// `msg` must be a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ime_rust_abort(msg: *const c_char) -> ! {
    // SAFETY: the caller passes a NUL-terminated string.
    let text = unsafe { CStr::from_ptr(msg) };
    panic!("libchewing: {}", text.to_str().unwrap_or("?"));
}

/// Whether a [`Chewing`] exists (the C side has one file table).
static IN_USE: AtomicBool = AtomicBool::new(false);

/// Where [`Chewing::new`] puts the dictionary files: libchewing's
/// "system path".
pub const SYS_DIR: &str = "/sys";
/// The user-phrase file in the C library's table.
pub const USER_FILE: &str = "/user/uhash.dat";

/// Why [`Chewing::new`] failed.
#[derive(Debug, PartialEq, Eq)]
pub enum InitError {
    /// Another [`Chewing`] exists.
    Busy,
    /// A file name is too long or the table is full.
    Files,
    /// libchewing did not start (a dictionary file missing or broken).
    Chewing,
}

/// A key as libchewing takes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A printable ASCII character (the keyboard layout maps it).
    Char(u8),
    /// Space.
    Space,
    /// Shift-Space (full and half shape).
    ShiftSpace,
    /// Enter.
    Enter,
    /// Esc.
    Esc,
    /// Backspace.
    Backspace,
    /// Delete.
    Delete,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Up arrow.
    Up,
    /// Down arrow (opens the candidate list).
    Down,
    /// Home.
    Home,
    /// End.
    End,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Tab.
    Tab,
    /// Ctrl + a digit (add a phrase to the user's dictionary).
    CtrlDigit(u8),
}

/// One libchewing context and the files its C library sees.
pub struct Chewing {
    ctx: *mut Context,
    /// The dictionary files: the C side points into them.
    _files: Vec<Vec<u8>>,
}

fn c_path(dir: &str, name: &str) -> Result<CString, InitError> {
    let mut s = String::from(dir);
    s.push('/');
    s.push_str(name);
    CString::new(s).map_err(|_| InitError::Files)
}

/// The string libchewing returns (NULL as empty).
///
/// # Safety
///
/// `p` is NULL or a NUL-terminated string.
unsafe fn string(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: the caller's contract.
    let s = unsafe { CStr::from_ptr(p) };
    String::from_utf8_lossy(s.to_bytes()).into_owned()
}

impl Chewing {
    /// Starts libchewing with the dictionary `files` (name, contents:
    /// `dictionary.dat`, `index_tree.dat`, `pinyin.tab`, `swkb.dat`,
    /// `symbols.dat`) and the user's phrases (`uhash.dat`, empty for a new
    /// user).
    pub fn new(files: Vec<(&str, Vec<u8>)>, user: &[u8]) -> Result<Chewing, InitError> {
        if IN_USE.swap(true, Ordering::AcqRel) {
            return Err(InitError::Busy);
        }
        let r = Self::start(files, user);
        if r.is_err() {
            IN_USE.store(false, Ordering::Release);
        }
        r
    }

    fn start(files: Vec<(&str, Vec<u8>)>, user: &[u8]) -> Result<Chewing, InitError> {
        // SAFETY: no context exists (IN_USE), so nothing points into
        // the table.
        unsafe { ime_vfs_reset() };
        let mut kept = Vec::new();
        for (name, data) in files {
            let path = c_path(SYS_DIR, name)?;
            // SAFETY: path is NUL-terminated; data stays alive (and does
            // not move: a Vec's buffer is on the heap) in `kept` as long
            // as the context, which is the only user of the table.
            if unsafe { ime_vfs_add(path.as_ptr(), data.as_ptr(), data.len()) } != 0 {
                return Err(InitError::Files);
            }
            kept.push(data);
        }
        let user_path = CString::new(USER_FILE).map_err(|_| InitError::Files)?;
        if !user.is_empty() {
            let copy = user.to_vec();
            // SAFETY: as above; the C side copies the bytes before it
            // writes to them (`own` in c/libc.c).
            if unsafe { ime_vfs_add(user_path.as_ptr(), copy.as_ptr(), copy.len()) } != 0 {
                return Err(InitError::Files);
            }
            kept.push(copy);
        }
        let sys = CString::new(SYS_DIR).map_err(|_| InitError::Files)?;
        // SAFETY: both paths are NUL-terminated and only read during the
        // call; no logger.
        let ctx = unsafe {
            chewing_new2(
                sys.as_ptr(),
                user_path.as_ptr(),
                None,
                core::ptr::null_mut(),
            )
        };
        if ctx.is_null() {
            return Err(InitError::Chewing);
        }
        // SAFETY: ctx is a live context (for all the setters below).
        unsafe {
            chewing_set_ChiEngMode(ctx, 1);
            chewing_set_candPerPage(ctx, 9);
            chewing_set_maxChiSymbolLen(ctx, 16);
            chewing_set_spaceAsSelection(ctx, 1);
            chewing_set_escCleanAllBuf(ctx, 1);
            chewing_set_autoShiftCur(ctx, 1);
            chewing_set_phraseChoiceRearward(ctx, 1);
        }
        Ok(Chewing { ctx, _files: kept })
    }

    /// Gives libchewing a key; false when it ignored it (the key belongs
    /// to the application: nothing being composed, or English mode).
    pub fn key(&mut self, key: Key) -> bool {
        let ctx = self.ctx;
        // SAFETY: ctx is live; the handlers take nothing else.
        unsafe {
            match key {
                Key::Char(c) => chewing_handle_Default(ctx, c_int::from(c)),
                Key::Space => chewing_handle_Space(ctx),
                Key::ShiftSpace => chewing_handle_ShiftSpace(ctx),
                Key::Enter => chewing_handle_Enter(ctx),
                Key::Esc => chewing_handle_Esc(ctx),
                Key::Backspace => chewing_handle_Backspace(ctx),
                Key::Delete => chewing_handle_Del(ctx),
                Key::Left => chewing_handle_Left(ctx),
                Key::Right => chewing_handle_Right(ctx),
                Key::Up => chewing_handle_Up(ctx),
                Key::Down => chewing_handle_Down(ctx),
                Key::Home => chewing_handle_Home(ctx),
                Key::End => chewing_handle_End(ctx),
                Key::PageUp => chewing_handle_PageUp(ctx),
                Key::PageDown => chewing_handle_PageDown(ctx),
                Key::Tab => chewing_handle_Tab(ctx),
                Key::CtrlDigit(d) => chewing_handle_CtrlNum(ctx, c_int::from(b'0' + d % 10)),
            };
            chewing_keystroke_CheckIgnore(ctx) == 0
        }
    }

    /// Chinese (true) or English mode.
    pub fn chinese(&self) -> bool {
        // SAFETY: ctx is live.
        unsafe { chewing_get_ChiEngMode(self.ctx) == 1 }
    }

    /// Switches between Chinese and English mode.
    pub fn set_chinese(&mut self, on: bool) {
        // SAFETY: ctx is live.
        unsafe { chewing_set_ChiEngMode(self.ctx, c_int::from(on)) };
    }

    /// The text the last key committed (empty if none).
    pub fn commit(&self) -> String {
        // SAFETY: ctx is live; the strings are libchewing's own buffers,
        // copied before the next call.
        unsafe {
            if chewing_commit_Check(self.ctx) == 0 {
                return String::new();
            }
            string(chewing_commit_String_static(self.ctx))
        }
    }

    /// The text being composed (`buffer`), the Zhuyin symbols typed for
    /// the current syllable, and the cursor in characters of `buffer`.
    pub fn preedit(&self) -> (String, String, usize) {
        // SAFETY: as in commit.
        unsafe {
            let buf = string(chewing_buffer_String_static(self.ctx));
            let zhuyin = string(chewing_bopomofo_String_static(self.ctx));
            let cursor = usize::try_from(chewing_cursor_Current(self.ctx)).unwrap_or(0);
            (buf, zhuyin, cursor)
        }
    }

    /// The candidates of the open list's current page (empty when the
    /// list is closed), the page and the number of pages.
    pub fn candidates(&mut self) -> (Vec<String>, usize, usize) {
        let mut list = Vec::new();
        // SAFETY: ctx is live; as in commit.
        unsafe {
            let total = chewing_cand_TotalChoice(self.ctx);
            if total <= 0 {
                return (list, 0, 0);
            }
            let per = chewing_cand_ChoicePerPage(self.ctx).max(1);
            let page = chewing_cand_CurrentPage(self.ctx).max(0);
            let pages = chewing_cand_TotalPage(self.ctx).max(1);
            let first = page * per;
            for i in first..(first + per).min(total) {
                list.push(string(chewing_cand_string_by_index_static(self.ctx, i)));
            }
            (
                list,
                usize::try_from(page).unwrap_or(0),
                usize::try_from(pages).unwrap_or(1),
            )
        }
    }

    /// The user-phrase file when libchewing changed it since the last
    /// call (to be saved).
    pub fn changed_user_data(&mut self) -> Option<Vec<u8>> {
        let path = CString::new(USER_FILE).ok()?;
        let mut len = 0usize;
        let mut dirty: c_int = 0;
        // SAFETY: path is NUL-terminated; the result points to `len`
        // bytes of the table, copied before the next C call.
        unsafe {
            let p = ime_vfs_get(path.as_ptr(), &mut len, &mut dirty);
            if p.is_null() || dirty == 0 {
                return None;
            }
            Some(core::slice::from_raw_parts(p, len).to_vec())
        }
    }
}

impl Drop for Chewing {
    fn drop(&mut self) {
        // SAFETY: ctx is live and not used after this.
        unsafe {
            chewing_delete(self.ctx);
            // The table points into `_files`, about to be freed.
            ime_vfs_reset();
        }
        IN_USE.store(false, Ordering::Release);
    }
}
