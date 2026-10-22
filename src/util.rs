use std::ffi::{OsStr, OsString};
use std::iter::once;
use std::os::windows::ffi::{OsStrExt, OsStringExt};

use windows_sys::Win32::Foundation::RECT;

/// Null-terminated UTF-16 string for passing to Win32.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(once(0)).collect()
}

pub fn wide_os(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(once(0)).collect()
}

/// Copies `s` into a fixed-size UTF-16 buffer, truncating and null-terminating.
pub fn fill(dst: &mut [u16], s: &str) {
    let max = dst.len() - 1;
    let mut len = 0;
    for (d, c) in dst.iter_mut().zip(s.encode_utf16().take(max)) {
        *d = c;
        len += 1;
    }
    dst[len] = 0;
}

pub unsafe fn from_wide_ptr(p: *const u16) -> OsString {
    let mut len = 0;
    while *p.add(len) != 0 {
        len += 1;
    }
    OsString::from_wide(std::slice::from_raw_parts(p, len))
}

pub const fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
    RECT { left, top, right, bottom }
}

pub fn is_empty(r: &RECT) -> bool {
    r.right <= r.left || r.bottom <= r.top
}

pub fn same(a: &RECT, b: &RECT) -> bool {
    a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
}

pub fn contains(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

pub fn intersect(a: &RECT, b: &RECT) -> RECT {
    rect(a.left.max(b.left), a.top.max(b.top), a.right.min(b.right), a.bottom.min(b.bottom))
}

pub fn union(a: &RECT, b: &RECT) -> RECT {
    if is_empty(a) {
        return *b;
    }
    if is_empty(b) {
        return *a;
    }
    rect(a.left.min(b.left), a.top.min(b.top), a.right.max(b.right), a.bottom.max(b.bottom))
}

pub fn inflate(r: &RECT, d: i32) -> RECT {
    rect(r.left - d, r.top - d, r.right + d, r.bottom + d)
}

pub fn offset(r: &RECT, dx: i32, dy: i32) -> RECT {
    rect(r.left + dx, r.top + dy, r.right + dx, r.bottom + dy)
}
