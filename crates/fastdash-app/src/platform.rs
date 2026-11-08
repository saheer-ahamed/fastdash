//! The few things GPUI does not do for a desktop app, done natively: hiding
//! and restoring the window, keeping to one instance, and a last-resort
//! message box.

use gpui::{App, Window};

/// Hide the window. Closing the dashboard hides it instead of quitting: the
/// taskbar readout and the tray icon keep the app one click away.
pub fn hide(window: &Window, cx: &mut App) {
    #[cfg(windows)]
    {
        let _ = cx;
        if let Some(hwnd) = win::hwnd(window) {
            win::show_window(hwnd, false);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = window;
        cx.hide();
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = cx;
        window.minimize_window();
    }
}

/// Bring the window back: unhide, unminimise, raise and focus it.
pub fn show(window: &Window, cx: &mut App) {
    #[cfg(windows)]
    if let Some(hwnd) = win::hwnd(window) {
        win::show_window(hwnd, true);
    }
    window.activate_window();
    cx.activate(true);
}

/// Claim the single-instance slot. `None` means another fastdash is already
/// running - it has been asked to show itself, and this process should exit.
/// Otherwise `on_show` runs (on a background thread) whenever a later launch
/// asks this instance to come forward.
pub fn single_instance(on_show: impl Fn() + Send + 'static) -> Option<InstanceGuard> {
    #[cfg(windows)]
    {
        win::single_instance(on_show)
    }
    #[cfg(not(windows))]
    {
        let _ = on_show;
        Some(InstanceGuard)
    }
}

/// Holds the single-instance slot for the life of the process.
pub struct InstanceGuard;

/// A blocking native message box, for when the UI itself cannot be trusted.
pub fn message_box(title: &str, body: &str) {
    #[cfg(windows)]
    win::message_box(title, body);
    #[cfg(not(windows))]
    eprintln!("{title}\n\n{body}");
}

#[cfg(windows)]
mod win {
    use std::os::windows::ffi::OsStrExt;

    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HWND, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
        INFINITE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, ShowWindow, MB_ICONERROR, MB_OK, SW_HIDE, SW_RESTORE,
    };

    use super::InstanceGuard;

    /// Names are per-session (`Local\`), so two users on one machine each get
    /// their own instance.
    const MUTEX: &str = "Local\\co.zro.fastdash.instance";
    const SHOW_EVENT: &str = "Local\\co.zro.fastdash.show";

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn hwnd(window: &Window) -> Option<HWND> {
        // `Window` has an inherent `window_handle()` that shadows the trait one.
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut _)),
            _ => None,
        }
    }

    pub fn show_window(hwnd: HWND, show: bool) {
        unsafe {
            let _ = ShowWindow(hwnd, if show { SW_RESTORE } else { SW_HIDE });
        }
    }

    pub fn single_instance(on_show: impl Fn() + Send + 'static) -> Option<InstanceGuard> {
        let mutex_name = wide(MUTEX);
        let event_name = wide(SHOW_EVENT);
        unsafe {
            // The mutex is never released: the OS frees it when the process
            // ends, however it ends.
            let mutex = CreateMutexW(None, true, PCWSTR(mutex_name.as_ptr()));
            let already_running = windows::Win32::Foundation::GetLastError()
                == windows::Win32::Foundation::ERROR_ALREADY_EXISTS;
            if mutex.is_err() || already_running {
                if let Ok(event) =
                    OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(event_name.as_ptr()))
                {
                    let _ = SetEvent(event);
                }
                return None;
            }
            let event = CreateEventW(None, false, false, PCWSTR(event_name.as_ptr())).ok()?;
            // Raw handles are not Send; the value is, and it stays valid for
            // the life of the process.
            let event_raw = event.0 as usize;
            std::thread::Builder::new()
                .name("fastdash-instance".into())
                .spawn(move || {
                    let event = windows::Win32::Foundation::HANDLE(event_raw as *mut _);
                    while WaitForSingleObject(event, INFINITE) == WAIT_OBJECT_0 {
                        on_show();
                    }
                })
                .ok()?;
        }
        Some(InstanceGuard)
    }

    pub fn message_box(title: &str, body: &str) {
        let title = wide(title);
        let body = wide(body);
        unsafe {
            MessageBoxW(
                None,
                PCWSTR(body.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}
