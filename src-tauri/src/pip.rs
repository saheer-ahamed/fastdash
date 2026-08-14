//! Widget mode: the same window, shrunk.
//!
//! Toggling the widget does not open a second window. It takes the main one and
//! turns it into a small, borderless, always-on-top, non-resizable panel, then
//! puts it back exactly as it was. One window means one webview, so the widget
//! shares the React state - and therefore the already-fetched snapshots - with
//! the dashboard, instead of standing up a second copy of the app that would
//! fetch everything again.
//!
//! The same window has a third, smaller shape: the tiny mode the widget
//! minimizes into, a square clinging to whichever side of the screen it is
//! nearest. It is the "out of the way but not gone" state - a widget that has
//! not been looked at for a few seconds folds itself into it, and a click
//! unfolds it again.
//!
//! The geometry each shape displaces is remembered here rather than recomputed,
//! so coming back lands on the user's own size and position (and re-maximizes if
//! that is what they had) instead of the config defaults.

use std::sync::Mutex;

use serde::Deserialize;
use tauri::{LogicalSize, PhysicalPosition, PhysicalSize, WebviewWindow};

/// Widget size, in logical pixels. Tall enough for the header, the taller of
/// the two views (Claude's pair of meters, 120px) and the timestamp footer, and
/// no taller: the point is that it can sit over other work without covering any
/// of it.
const PIP_WIDTH: f64 = 300.0;
const PIP_HEIGHT: f64 = 216.0;
/// Gap left between the widget and the corner of the screen.
const PIP_MARGIN: f64 = 24.0;

/// The minimized square, in logical pixels. Small enough to read as a handle
/// rather than a window, big enough to stay a comfortable click target.
const TINY_SIZE: f64 = 34.0;

/// Which shape the one window currently has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// The full app.
    Dashboard,
    /// The small always-on-top panel.
    Widget,
    /// The square docked to a side of the screen.
    Tiny,
}

/// A screen to place a window on: where it starts, how big it is, and its scale
/// factor - the three things every placement here needs together.
type Screen = (PhysicalPosition<i32>, PhysicalSize<u32>, f64);

/// Which side of a screen the tiny square is clinging to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Left,
    Right,
}

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

/// Where the widget was when it was last put away.
///
/// Without this the widget goes back to the corner the app picked every time,
/// which quietly undoes the one bit of arranging the user did: they drag it to
/// the spot that does not cover their work, glance at the dashboard, and it is
/// back over the thing they moved it off. Remembered for the session only - the
/// dashboard's own geometry is not persisted across launches either, and a
/// widget that reappears on a monitor that is no longer there would be worse
/// than one that starts in the corner.
static PIP_SPOT: Mutex<Option<PhysicalPosition<i32>>> = Mutex::new(None);

/// The shape the window has right now. Kept here because the transitions are
/// not symmetric - what shrinking has to save and restore depends on where it
/// is coming from - and the frontend, which can be reloaded, is not a place to
/// keep the truth about the window.
static MODE: Mutex<Mode> = Mutex::new(Mode::Dashboard);

/// Give the window the requested shape. Idempotent: asking for the shape it
/// already has does nothing rather than re-deriving its geometry.
pub fn set_mode(window: &WebviewWindow, target: Mode) -> tauri::Result<()> {
    let mut current = MODE.lock().unwrap();
    if *current == target {
        return Ok(());
    }

    match target {
        Mode::Dashboard => leave(window, *current)?,
        Mode::Widget => {
            if *current == Mode::Dashboard {
                enter(window)?;
                size_widget(window)?;
                match remembered_spot(window)? {
                    Some(spot) => window.set_position(spot)?,
                    None => park_bottom_right(window)?,
                }
            } else {
                unfold(window)?;
            }
        }
        Mode::Tiny => {
            if *current == Mode::Dashboard {
                enter(window)?;
            } else {
                // Where the widget stood, so leaving tiny mode for the
                // dashboard and coming back does not lose the spot the user
                // arranged before they minimized.
                *PIP_SPOT.lock().unwrap() = window.outer_position().ok();
            }
            fold(window)?;
        }
    }

    *current = target;
    Ok(())
}

/// Slide the tiny square along its edge by `dy` physical pixels, staying on the
/// screen it is docked to. The drag is deliberately one-dimensional: the square
/// belongs to an edge, and letting it be dropped in the middle of the screen
/// would just make a second, worse widget.
pub fn nudge_tiny(window: &WebviewWindow, dy: i32) -> tauri::Result<()> {
    if *MODE.lock().unwrap() != Mode::Tiny {
        return Ok(());
    }
    let Some((origin, area, _)) = home_screen(window)? else {
        return Ok(());
    };
    let size = window.outer_size()?;
    let at = window.outer_position()?;
    let y = clamp_y(at.y + dy, origin, area, size.height);
    window.set_position(PhysicalPosition::new(at.x, y))
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

    // Order matters. The minimum has to go before any resize or the new size is
    // clamped to it, and dropping the decorations changes the outer size, so
    // sizing is left to the caller, which does it after this.
    window.set_min_size(None::<LogicalSize<f64>>)?;
    window.set_resizable(false)?;
    window.set_decorations(false)?;
    window.set_always_on_top(true)?;
    Ok(())
}

fn size_widget(window: &WebviewWindow) -> tauri::Result<()> {
    window.set_size(LogicalSize::new(PIP_WIDTH, PIP_HEIGHT))
}

/// Shrink to the square and stick it to the nearer side of the screen, level
/// with where the widget was: minimizing should feel like the widget slid
/// sideways out of the way, not like it jumped somewhere else.
fn fold(window: &WebviewWindow) -> tauri::Result<()> {
    let before = window.outer_position()?;
    let was = window.outer_size()?;
    // Which screen the window is on is read *before* it is resized. Resizing
    // moves nothing, but it changes which monitor holds most of the window, and
    // "the screen it is on" would then answer for the neighbour it grew into -
    // which is how the widget ends up docked to a monitor the user was not
    // looking at.
    let screen = home_screen(window)?;
    window.set_size(LogicalSize::new(TINY_SIZE, TINY_SIZE))?;

    let Some((origin, area, _)) = screen else {
        return Ok(());
    };
    let now = window.outer_size()?;
    // A window with no decorations still has a frame on Windows - an invisible
    // resize border a handful of pixels wide, which `outer_size` counts and the
    // user cannot see. Docking by the outer rect would leave the square sitting
    // that far off the edge, looking like a near miss rather than a dock, so
    // the border is subtracted and the invisible part hangs off the screen.
    let border = (now.width.saturating_sub(window.inner_size()?.width) / 2) as i32;
    let edge = nearest_edge(before.x, was.width, origin, area);
    let x = match edge {
        Edge::Left => origin.x - border,
        Edge::Right => origin.x + area.width as i32 - now.width as i32 + border,
    };
    // Level with the middle of what was there, so the square lands where the
    // eye already is rather than at the top of the screen.
    let y = before.y + was.height as i32 / 2 - now.height as i32 / 2;
    window.set_position(PhysicalPosition::new(
        x,
        clamp_y(y, origin, area, now.height),
    ))
}

/// Grow the square back into the widget, hung off the edge it was docked to and
/// centred on it vertically - the reverse of `fold`.
fn unfold(window: &WebviewWindow) -> tauri::Result<()> {
    let before = window.outer_position()?;
    let was = window.outer_size()?;
    // Read the screen before growing, for the reason `fold` gives: a widget
    // unfolded from a square on the right edge overlaps the next monitor along,
    // and would otherwise be placed on that one.
    let screen = home_screen(window)?;
    size_widget(window)?;

    let Some((origin, area, scale)) = screen else {
        return Ok(());
    };
    let now = window.outer_size()?;
    let margin = (PIP_MARGIN * scale).round() as i32;
    let x = match nearest_edge(before.x, was.width, origin, area) {
        Edge::Left => origin.x + margin,
        Edge::Right => origin.x + area.width as i32 - now.width as i32 - margin,
    };
    let y = before.y + was.height as i32 / 2 - now.height as i32 / 2;
    window.set_position(PhysicalPosition::new(
        x,
        clamp_y(y, origin, area, now.height),
    ))
}

fn leave(window: &WebviewWindow, from: Mode) -> tauri::Result<()> {
    // Read before anything else: restoring the decorations and the size both
    // move the window, so a position read afterwards is the dashboard's, not
    // the widget's. Skipped in tiny mode, where the position is the square's -
    // the widget's own was saved on the way in.
    if from == Mode::Widget {
        *PIP_SPOT.lock().unwrap() = window.outer_position().ok();
    }

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

/// The spot the widget was last put away from, if it is still somewhere the
/// user could reach it.
///
/// A monitor can go away between one toggle and the next - a laptop undocked,
/// a display unplugged, a resolution changed - and a window restored onto
/// coordinates nothing covers any more is invisible and, being undecorated,
/// undraggable. So the point the user would grab is checked against the
/// monitors that exist right now, and anything off them falls back to the
/// corner rather than stranding the widget.
fn remembered_spot(window: &WebviewWindow) -> tauri::Result<Option<PhysicalPosition<i32>>> {
    let Some(spot) = *PIP_SPOT.lock().unwrap() else {
        return Ok(None);
    };
    let screens: Vec<(PhysicalPosition<i32>, PhysicalSize<u32>)> = window
        .available_monitors()?
        .iter()
        .map(|m| (*m.position(), *m.size()))
        .collect();
    Ok(reachable(spot, &screens).then_some(spot))
}

/// Whether the widget placed at `spot` would land somewhere the user can grab
/// it. Split from the monitor lookup so the geometry is testable without a
/// window, which is the half that has to be right on a machine whose displays
/// come and go.
fn reachable(
    spot: PhysicalPosition<i32>,
    screens: &[(PhysicalPosition<i32>, PhysicalSize<u32>)],
) -> bool {
    // A point a little inside the widget's header, which is its drag handle.
    // Its top-left corner alone would be too strict: a window nudged one pixel
    // off the left edge is still perfectly usable.
    let x = spot.x + 40;
    let y = spot.y + 10;

    screens.iter().any(|(origin, size)| {
        x >= origin.x
            && x < origin.x + size.width as i32
            && y >= origin.y
            && y < origin.y + size.height as i32
    })
}

/// The screen the window is on: its origin, its size and its scale. `None` when
/// the window sits on no monitor we can identify, in which case every caller
/// leaves the geometry alone rather than guessing.
fn home_screen(window: &WebviewWindow) -> tauri::Result<Option<Screen>> {
    Ok(window
        .current_monitor()?
        .map(|m| (*m.position(), *m.size(), m.scale_factor())))
}

/// Which side of the screen a window of `width` at `x` is nearer to. Measured
/// from its centre, so the answer does not flip as the thing being docked
/// changes size under the same cursor.
fn nearest_edge(
    x: i32,
    width: u32,
    origin: PhysicalPosition<i32>,
    area: PhysicalSize<u32>,
) -> Edge {
    let centre = x + width as i32 / 2;
    let middle = origin.x + area.width as i32 / 2;
    if centre < middle {
        Edge::Left
    } else {
        Edge::Right
    }
}

/// Keep a window of `height` fully on its screen vertically. A square dragged
/// past the bottom of the screen would be as good as gone: nothing but the
/// dashboard could bring it back.
fn clamp_y(y: i32, origin: PhysicalPosition<i32>, area: PhysicalSize<u32>, height: u32) -> i32 {
    let lowest = origin.y + (area.height as i32 - height as i32).max(0);
    y.clamp(origin.y, lowest)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: i32, y: i32) -> PhysicalPosition<i32> {
        PhysicalPosition::new(x, y)
    }

    /// A laptop screen with a second monitor to its left, which is where
    /// negative coordinates come from on Windows.
    fn two_screens() -> Vec<(PhysicalPosition<i32>, PhysicalSize<u32>)> {
        vec![
            (at(0, 0), PhysicalSize::new(1920, 1080)),
            (at(-1920, -200), PhysicalSize::new(1920, 1200)),
        ]
    }

    #[test]
    fn a_spot_on_any_attached_screen_is_kept() {
        assert!(reachable(at(1500, 800), &two_screens()), "primary");
        assert!(reachable(at(-1800, 40), &two_screens()), "second screen");
        // Hanging off the left edge, header still on screen: usable, so kept.
        assert!(reachable(at(-30, 500), &two_screens()));
    }

    /// The case the check exists for: the monitor the widget was parked on is
    /// gone, so those coordinates now point at nothing. An undecorated window
    /// there cannot be seen or dragged back, so the corner is the safer answer.
    #[test]
    fn a_spot_on_a_detached_screen_is_dropped() {
        let only_primary = vec![(at(0, 0), PhysicalSize::new(1920, 1080))];
        assert!(!reachable(at(-1800, 40), &only_primary));
        // Below a screen that shrank, and past the right edge of every screen.
        assert!(!reachable(at(500, 1200), &only_primary));
        assert!(!reachable(at(4000, 100), &two_screens()));
    }

    #[test]
    fn with_no_screens_at_all_nothing_is_reachable() {
        assert!(!reachable(at(0, 0), &[]));
    }

    /// The square docks to whichever side its own middle is nearer, on the
    /// screen it is on - including the one at negative coordinates, where
    /// "left" is a long way below zero.
    #[test]
    fn the_square_docks_to_the_side_it_is_nearest() {
        let primary = (at(0, 0), PhysicalSize::new(1920, 1080));
        assert_eq!(nearest_edge(100, 300, primary.0, primary.1), Edge::Left);
        assert_eq!(nearest_edge(1500, 300, primary.0, primary.1), Edge::Right);
        // Straddling the middle: the half it leans into wins.
        assert_eq!(nearest_edge(800, 300, primary.0, primary.1), Edge::Left);
        assert_eq!(nearest_edge(820, 300, primary.0, primary.1), Edge::Right);

        let second = (at(-1920, -200), PhysicalSize::new(1920, 1200));
        assert_eq!(nearest_edge(-1800, 300, second.0, second.1), Edge::Left);
        assert_eq!(nearest_edge(-200, 300, second.0, second.1), Edge::Right);
    }

    /// Dragging the square along its edge must not be able to push it off the
    /// screen, in either direction.
    #[test]
    fn a_dragged_square_stays_on_its_screen() {
        let (origin, area) = (at(0, 0), PhysicalSize::new(1920, 1080));
        assert_eq!(clamp_y(500, origin, area, 40), 500);
        assert_eq!(clamp_y(-80, origin, area, 40), 0);
        assert_eq!(clamp_y(5000, origin, area, 40), 1040);

        // A screen hung above and to the left, where both bounds are negative.
        let (origin, area) = (at(-1920, -200), PhysicalSize::new(1920, 1200));
        assert_eq!(clamp_y(-500, origin, area, 40), -200);
        assert_eq!(clamp_y(5000, origin, area, 40), 960);
    }

    /// A window taller than the screen it is on has no legal range at all; the
    /// clamp has to pick a bound rather than panic on an inverted one.
    #[test]
    fn a_window_taller_than_its_screen_is_pinned_to_the_top() {
        let (origin, area) = (at(0, 0), PhysicalSize::new(1920, 400));
        assert_eq!(clamp_y(300, origin, area, 600), 0);
    }
}
