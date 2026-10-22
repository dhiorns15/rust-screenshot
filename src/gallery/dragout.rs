//! Dragging screenshots out of the library into Explorer, chat apps, email and so on.

#![allow(non_snake_case)] // COM method names

use std::path::PathBuf;

use windows::core::{implement, HRESULT, HSTRING};
use windows::Win32::Foundation::{BOOL, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, S_OK};
use windows::Win32::System::Com::{CoTaskMemFree, IDataObject};
use windows::Win32::System::Ole::{DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize, DROPEFFECT, DROPEFFECT_COPY};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{BHID_DataObject, IShellItemArray, SHCreateShellItemArrayFromIDLists, SHParseDisplayName};

/// Tells OLE when the drag ends: on release of the left button, or Esc.
#[implement(IDropSource)]
struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape_pressed: BOOL, key_state: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape_pressed.as_bool() {
            DRAGDROP_S_CANCEL
        } else if (key_state & MK_LBUTTON).0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Runs a copy drag of `paths` until the user drops or cancels it. Blocks meanwhile (OLE runs its
/// own message loop), so call it from the UI thread when the mouse starts dragging.
pub fn drag_files(paths: &[PathBuf]) -> Result<(), String> {
    unsafe {
        // Usually already done by the windowing library. Never undone: the shell's data object can
        // leave work behind that calls back into this thread's OLE later.
        let _ = OleInitialize(None);
        let result = drag(paths);
        release_mouse();
        result.map_err(|e| e.message())
    }
}

/// OLE's drag loop swallows the mouse release, so the window (and egui) would think the button is
/// still held; hand it a release where the mouse is now.
unsafe fn release_mouse() {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, PostMessageW, WM_LBUTTONUP};

    let hwnd = GetActiveWindow();
    let mut p = POINT { x: 0, y: 0 };
    if hwnd.is_null() || GetCursorPos(&mut p) == 0 || ScreenToClient(hwnd, &mut p) == 0 {
        return;
    }
    let lparam = (p.x as u16 as u32 | (p.y as u16 as u32) << 16) as isize;
    PostMessageW(hwnd, WM_LBUTTONUP, 0, lparam);
}

unsafe fn drag(paths: &[PathBuf]) -> windows::core::Result<()> {
    let data = data_object(paths)?;
    let source: IDropSource = DropSource.into();
    let mut effect = DROPEFFECT::default();
    DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut effect).ok()
}

/// The shell's own data object for the files, as Explorer would drag them.
unsafe fn data_object(paths: &[PathBuf]) -> windows::core::Result<IDataObject> {
    let mut pidls = Vec::new();
    for path in paths {
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        if SHParseDisplayName(&HSTRING::from(path.as_os_str()), None, &mut pidl, 0, None).is_ok() {
            pidls.push(pidl);
        }
    }
    let ids: Vec<*const ITEMIDLIST> = pidls.iter().map(|&p| p as *const _).collect();
    let data = SHCreateShellItemArrayFromIDLists(&ids).and_then(|items: IShellItemArray| items.BindToHandler(None, &BHID_DataObject));
    for pidl in pidls {
        CoTaskMemFree(Some(pidl as *const _));
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};

    #[test]
    fn offers_files_as_a_file_drop() {
        let path = std::env::temp_dir().join(format!("rsnip-drag-{}.png", std::process::id()));
        std::fs::write(&path, b"png").unwrap();
        // Like the app's UI thread, the OLE thread lives as long as the process: the shell's data
        // object can leave background work behind that calls back into it, and test threads exit.
        let (tx, rx) = std::sync::mpsc::channel();
        let file = path.clone();
        std::thread::spawn(move || unsafe {
            let _ = OleInitialize(None);
            let data = data_object(std::slice::from_ref(&file)).expect("data object");
            let format = FORMATETC { cfFormat: 15, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
            let _ = tx.send(data.QueryGetData(&format) == S_OK);
            loop {
                std::thread::park();
            }
        });
        let has_hdrop = rx.recv().unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(has_hdrop, "CF_HDROP offered");
    }
}
