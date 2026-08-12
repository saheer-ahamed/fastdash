//! Widget mode: the same window, shrunk.
//!
//! Toggling the widget does not open a second window. It takes the main one and
//! turns it into a small, borderless, always-on-top, non-resizable panel, then
//! puts it back exactly as it was. One window means one webview, so the widget
//! shares the React state - and therefore the already-fetched snapshots - with
//! the dashboard, instead of standing up a second copy of the app that would
//! fetch everything again.
//!
//! The geometry it displaces is remembered here rather than recomputed, so
//! coming back lands on the user's own size and position (and re-maximizes if
//! that is what they had) instead of the config defaults.

use std::sync::Mutex;

use tauri::{LogicalSize, PhysicalPosition, PhysicalSize, WebviewWindow};

/// Widget size, in logical pixels. Tall enough for the header, the taller of
/// the two views (Claude's pair of meters, 120px) and the timestamp footer, and
/// no taller: the point is that it can sit over other work without covering any
/// of it.
const PIP_WIDTH: f64 = 300.0;
const PIP_HEIGHT: f64 = 216.0;
/// Gap left between the widget and the corner of the screen.
const PIP_MARGIN: f64 = 24.0;

/// The dashboard's minimum size, mirroring `tauri.conf.json`. It has to be
/// lifted before the window can shrink, and put back when it grows again -
/// otherwise the widget size is silently clamped to 880x560.
const MIN_WIDTH: f64 = 880.0;
const MIN_HEIGHT: f64 = 560.0;

/// What the window looked like before it became a widget.
///
/// The size is the *inner* one, because `set_size` sets the inner size: saving
/// the outer size and handing it back grows the window by its frame on every
/// round trip, which on this machine was +18x47 physical pixels a toggle.
struct Restore {
    inner_size: PhysicalSize<u32>,
    position: PhysicalPosition<i32>,
    maximized: bool,
}

static RESTORE: Mutex<Option<Restore>> = Mutex::new(None);

/// Shrink the window into the widget, or restore it. Idempotent: entering while
/// already in widget mode keeps the saved geometry rather than overwriting it
/// with the widget's own.
pub fn set_mode(window: &WebviewWindow, enabled: bool) -> tauri::Result<()> {
    if enabled {
        enter(window)
    } else {
        leave(window)
    }
}

fn enter(window: &WebviewWindow) -> tauri::Result<()> {
    let mut saved = RESTORE.lock().unwrap();
    if saved.is_none() {
        let maximized = window.is_maximized()?;
        // A maximized window's outer size is the screen, which is not what the
        // user would get back. Un-maximizing first restores the size it had
        // before, and that is what is worth remembering.
        if maximized {
            window.unmaximize()?;
        }
        *saved = Some(Restore {
            inner_size: window.inner_size()?,
            position: window.outer_position()?,
            maximized,
        });
    }

    // Order matters. The minimum has to go before the resize or the new size is
    // clamped to it, and dropping the decorations changes the outer size, so the
    // resize comes after them.
    window.set_min_size(None::<LogicalSize<f64>>)?;
    window.set_resizable(false)?;
    window.set_decorations(false)?;
    window.set_size(LogicalSize::new(PIP_WIDTH, PIP_HEIGHT))?;
    window.set_always_on_top(true)?;
    park_bottom_right(window)?;
    Ok(())
}

fn leave(window: &WebviewWindow) -> tauri::Result<()> {
    window.set_always_on_top(false)?;
    window.set_decorations(true)?;
    window.set_resizable(true)?;
    window.set_min_size(Some(LogicalSize::new(MIN_WIDTH, MIN_HEIGHT)))?;

    if let Some(saved) = RESTORE.lock().unwrap().take() {
        window.set_size(saved.inner_size)?;
        window.set_position(saved.position)?;
        if saved.maximized {
            window.maximize()?;
        }
    }
    Ok(())
}

/// Park the widget in the bottom-right corner of whichever monitor it is on.
/// Shrinking in place would leave it wherever the dashboard's top-left corner
/// happened to be - often the middle of the screen, which is the one place a
/// permanently-on-top window is most in the way. A monitor we cannot identify
/// is not an error: the widget just stays where it is.
fn park_bottom_right(window: &WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = window.current_monitor()? else {
        return Ok(());
    };
    let scale = monitor.scale_factor();
    let margin = (PIP_MARGIN * scale).round() as i32;
    let size = window.outer_size()?;
    let area = monitor.size();
    let origin = monitor.position();

    let x = origin.x + area.width as i32 - size.width as i32 - margin;
    let y = origin.y + area.height as i32 - size.height as i32 - margin;
    window.set_position(PhysicalPosition::new(x, y))
}
