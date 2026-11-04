//! One interactive-desktop test at a time.
//!
//! `cargo test` starts every integration-test binary together, and each binary
//! runs its tests on several threads. Those tests all foreground the fixture,
//! so a check that counts successful activations fails even when activation
//! itself works. This mutex is test-only; the helper does not take it.

use std::time::{Duration, Instant};

use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

pub struct InteractiveLock(HANDLE);

pub fn acquire() -> InteractiveLock {
    unsafe {
        let handle = CreateMutexW(None, false, w!("Local\\CUMCP_INTERACTIVE_TESTS")).expect("interactive test mutex");
        let started = Instant::now();
        loop {
            let wait = WaitForSingleObject(handle, 1_000);
            if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
                return InteractiveLock(handle);
            }
            if started.elapsed() > Duration::from_secs(300) {
                panic!("timed out waiting for the interactive-desktop test lock ({wait:?})");
            }
        }
    }
}

impl Drop for InteractiveLock {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}
