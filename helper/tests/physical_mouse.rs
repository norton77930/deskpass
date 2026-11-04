//! Physical mouse input seen while a test runs (ticket 138).
//!
//! The interactive desktop is shared with a person. A check that the cursor did not move cannot
//! tell a background action from a hand on the mouse, so a test records, with a low-level mouse
//! hook, every mouse event that was not injected (`LLMHF_INJECTED` clear: it came from a device,
//! not from SendInput) and names them when the check fails. Injected events are left out: the
//! helper's input is injected, and it must never move the cursor in background delivery.

#![allow(dead_code)]

use std::sync::Mutex;

use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, MSG, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_QUIT,
};

const LLMHF_INJECTED: u32 = 0x1;

static PHYSICAL: Mutex<Vec<String>> = Mutex::new(Vec::new());

unsafe extern "system" fn record(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let event = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        if event.flags & LLMHF_INJECTED == 0 {
            if let Ok(mut physical) = PHYSICAL.lock() {
                physical.push(format!("message {:#x} at {},{}", wparam.0, event.pt.x, event.pt.y));
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// Records physical mouse events from `start` until dropped.
pub struct PhysicalMouse {
    thread: u32,
}

impl PhysicalMouse {
    pub fn start() -> PhysicalMouse {
        PHYSICAL.lock().map(|mut physical| physical.clear()).ok();
        let (ready, hooked) = std::sync::mpsc::channel();
        std::thread::spawn(move || unsafe {
            let module = GetModuleHandleW(None).ok();
            let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(record), module.map(|module| HINSTANCE(module.0)), 0);
            let _ = ready.send(GetCurrentThreadId());
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {}
            if let Ok(hook) = hook {
                let _ = UnhookWindowsHookEx(hook);
            }
        });
        PhysicalMouse { thread: hooked.recv().expect("mouse hook thread") }
    }

    /// The physical mouse events so far, or `none` (worded for an assertion message).
    pub fn seen(&self) -> String {
        let physical = PHYSICAL.lock().map(|physical| physical.clone()).unwrap_or_default();
        if physical.is_empty() {
            return "none".to_string();
        }
        format!("{} ({})", physical.len(), physical.iter().take(6).cloned().collect::<Vec<_>>().join("; "))
    }
}

impl Drop for PhysicalMouse {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}
