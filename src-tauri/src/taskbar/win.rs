// Drawing the readout inside the Windows taskbar.
//
// The readout is a small native window made a child of the taskbar
// (`Shell_TrayWnd`) and parked just left of the notification area
// (`TrayNotifyWnd`), so it moves, hides and auto-hides with the taskbar itself
// and is never covered by it. Windows has no API for putting text on the
// taskbar; this is the approach monitoring tools such as TrafficMonitor use.
//
// It is plain Win32 rather than a webview: a second WebView2 would cost tens of
// megabytes to draw two lines of text, and could not blend onto the taskbar's
// translucent backdrop. Text is rendered with grayscale anti-aliasing into a
// 32-bit bitmap and handed to `UpdateLayeredWindow` with per-pixel alpha, which
// is what lets it sit on the acrylic taskbar like the clock does.
//
// Everything runs on one dedicated thread with its own message loop. Being a
// child of another process's window ties this thread's input queue to
// Explorer's, so nothing on it may ever block: it only draws and forwards
// clicks.
//
// The readout is two buttons in one window. Hovering it shows a refresh icon on
// its left, which asks for fresh numbers; the numbers themselves open the
// dashboard. Each part has its own hover highlight. The icon's space is kept
// while it is hidden, so showing it never shifts the numbers.
//
// Nothing polls. The readout is redrawn when its text changes and when Windows
// says something moved: a location change on the taskbar or the tray (window
// event hook), a theme, scaling or display change (broadcasts to the hidden
// host window), and Explorer restarting (`TaskbarCreated`), which destroys the
// old taskbar and every child in it.

use std::cell::RefCell;
use std::sync::{Mutex, OnceLock};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontIndirectW, DeleteDC, DeleteObject,
    GetTextExtentPoint32W, GetTextFaceW, GetTextMetricsW, ScreenToClient, SelectObject, SetBkMode,
    SetTextColor, TextOutW, AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DEFAULT_CHARSET, DIB_RGB_COLORS, HDC, HFONT, LOGFONTW,
    TEXTMETRICW, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowExW, FindWindowW,
    GetMessageW, GetWindowRect, GetWindowThreadProcessId, IsWindow, LoadCursorW, PostMessageW,
    RegisterClassExW, RegisterWindowMessageW, SetWindowPos, ShowWindow, TranslateMessage,
    UpdateLayeredWindow, EVENT_OBJECT_LOCATIONCHANGE, HWND_TOP, IDC_ARROW, MSG, OBJID_WINDOW,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, ULW_ALPHA, WINEVENT_OUTOFCONTEXT, WM_APP, WM_DESTROY,
    WM_DISPLAYCHANGE, WM_DPICHANGED, WM_LBUTTONUP, WM_MOUSEMOVE, WM_SETTINGCHANGE, WNDCLASSEXW,
    WS_CHILD, WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};

/// Sent once the pointer leaves after `TrackMouseEvent`. Declared here because
/// the `windows` crate files it under the common controls.
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Posted to the host window to re-lay-out and redraw.
const WM_REDRAW: u32 = WM_APP + 1;

/// The text to draw, and the host window to poke when it changes. Shared with
/// the rest of the app; everything else lives on the readout's own thread.
struct Shared {
    lines: Vec<String>,
    /// Whether a refresh is under way; the icon dims and ignores clicks.
    busy: bool,
    /// The host window, once created. Stored as an integer because `HWND` is a
    /// raw pointer and not `Send`; it is only ever used to post a message.
    host: Option<isize>,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    lines: Vec::new(),
    busy: false,
    host: None,
});

/// What a click does, by where it lands.
struct Actions {
    open: Box<dyn Fn() + Send + Sync>,
    refresh: Box<dyn Fn() + Send + Sync>,
}

static ACTIONS: OnceLock<Actions> = OnceLock::new();

/// The part of the readout under the pointer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Zone {
    #[default]
    None,
    /// The refresh icon.
    Refresh,
    /// The numbers, which open the dashboard.
    Numbers,
}

/// State owned by the readout thread.
#[derive(Default)]
struct Ui {
    /// The window inside the taskbar, while one exists.
    readout: Option<HWND>,
    /// Which part the pointer is over, for the icon and the hover highlight.
    hover: Zone,
    /// Where the refresh icon's zone ends, from the last paint.
    refresh_w: i32,
    /// The location-change hook on Explorer, re-made with the taskbar.
    hook: Option<HWINEVENTHOOK>,
    /// The taskbar and tray the readout is laid out against.
    taskbar: Option<HWND>,
    tray: Option<HWND>,
}

thread_local! {
    static UI: RefCell<Ui> = RefCell::new(Ui::default());
}

/// Start the readout thread. Called once, at startup. `open` runs when the
/// numbers are clicked, `refresh` when the refresh icon is.
pub fn start(open: Box<dyn Fn() + Send + Sync>, refresh: Box<dyn Fn() + Send + Sync>) {
    if ACTIONS.set(Actions { open, refresh }).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("taskbar-readout".into())
        .spawn(run);
    if let Err(e) = spawned {
        eprintln!("[taskbar] could not start: {e}");
    }
}

/// Replace the readout's text; an empty list hides it.
pub fn set_lines(lines: Vec<String>) {
    update(|shared| std::mem::replace(&mut shared.lines, lines.clone()) != lines);
}

/// Mark a refresh as started or finished.
pub fn set_busy(busy: bool) {
    update(|shared| std::mem::replace(&mut shared.busy, busy) != busy);
}

/// Apply `change` and, if it reports a difference, ask the readout to redraw.
fn update(change: impl FnOnce(&mut Shared) -> bool) {
    let mut shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    if !change(&mut shared) {
        return;
    }
    if let Some(host) = shared.host {
        // SAFETY: posting to a window handle is sound even if it has since
        // been destroyed; the call just fails.
        unsafe {
            let _ = PostMessageW(Some(HWND(host as *mut _)), WM_REDRAW, WPARAM(0), LPARAM(0));
        }
    }
}

fn run() {
    // SAFETY: plain Win32 window setup and message loop, all on this thread.
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else {
            return;
        };
        let instance = module.into();
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();

        let host_class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(host_proc),
            hInstance: instance,
            lpszClassName: w!("fastdash.taskbar.host"),
            ..Default::default()
        };
        let readout_class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(readout_proc),
            hInstance: instance,
            hCursor: cursor,
            lpszClassName: w!("fastdash.taskbar.readout"),
            ..Default::default()
        };
        if RegisterClassExW(&host_class) == 0 || RegisterClassExW(&readout_class) == 0 {
            eprintln!("[taskbar] could not register window classes");
            return;
        }

        // A hidden top-level window, because only top-level windows receive
        // the broadcasts the readout has to follow: theme and scaling changes,
        // display changes, and Explorer announcing a new taskbar.
        let host = match CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("fastdash.taskbar.host"),
            w!("fastdash taskbar"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[taskbar] could not create the host window: {e}");
                return;
            }
        };
        SHARED.lock().unwrap_or_else(|e| e.into_inner()).host = Some(host.0 as isize);

        embed();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Explorer's broadcast when it has (re)created the taskbar.
fn taskbar_created() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    // SAFETY: registering a message name has no preconditions.
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

/// Put the readout into the current taskbar, replacing any earlier one.
unsafe fn embed() {
    unembed();
    let Ok(taskbar) = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) else {
        // No taskbar yet (Explorer starting or restarting). `TaskbarCreated`
        // brings us back here once there is one.
        return;
    };
    let tray = FindWindowExW(Some(taskbar), None, w!("TrayNotifyWnd"), PCWSTR::null()).ok();
    let Ok(module) = GetModuleHandleW(None) else {
        return;
    };

    // Created hidden; the first layout sizes and shows it. Layered, for
    // per-pixel alpha; a child, so it lives inside the taskbar rather than
    // fighting it for the top of the z-order.
    let readout = match CreateWindowExW(
        WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
        w!("fastdash.taskbar.readout"),
        w!("fastdash"),
        WS_CHILD | WS_CLIPSIBLINGS,
        0,
        0,
        0,
        0,
        Some(taskbar),
        None,
        Some(module.into()),
        None,
    ) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("[taskbar] could not create the readout: {e}");
            return;
        }
    };

    // Follow the taskbar and the tray as they move and resize: the tray grows
    // and shrinks as icons come and go, and the readout sits against it.
    let mut explorer = 0u32;
    GetWindowThreadProcessId(taskbar, Some(&mut explorer));
    let hook = SetWinEventHook(
        EVENT_OBJECT_LOCATIONCHANGE,
        EVENT_OBJECT_LOCATIONCHANGE,
        None,
        Some(on_location_change),
        explorer,
        0,
        WINEVENT_OUTOFCONTEXT,
    );

    UI.with_borrow_mut(|ui| {
        ui.readout = Some(readout);
        ui.taskbar = Some(taskbar);
        ui.tray = tray;
        ui.hook = (!hook.is_invalid()).then_some(hook);
        ui.hover = Zone::None;
    });
    layout();
}

unsafe fn unembed() {
    let (readout, hook) = UI.with_borrow_mut(|ui| {
        ui.taskbar = None;
        ui.tray = None;
        (ui.readout.take(), ui.hook.take())
    });
    if let Some(hook) = hook {
        let _ = UnhookWinEvent(hook);
    }
    if let Some(readout) = readout {
        if IsWindow(Some(readout)).as_bool() {
            let _ = DestroyWindow(readout);
        }
    }
}

unsafe extern "system" fn on_location_change(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if id_object != OBJID_WINDOW.0 {
        return;
    }
    let watched = UI.with_borrow(|ui| ui.taskbar == Some(hwnd) || ui.tray == Some(hwnd));
    if watched {
        layout();
    }
}

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_REDRAW | WM_SETTINGCHANGE | WM_DISPLAYCHANGE | WM_DPICHANGED => {
            let alive =
                UI.with_borrow(|ui| ui.readout.is_some_and(|r| IsWindow(Some(r)).as_bool()));
            if alive {
                layout();
            } else {
                embed();
            }
            LRESULT(0)
        }
        m if m == taskbar_created() => {
            embed();
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Which part of the readout a client-area x coordinate falls in.
fn zone_at(lp: LPARAM) -> Zone {
    // The low word of `lp` is the signed x coordinate.
    let x = (lp.0 & 0xFFFF) as u16 as i16 as i32;
    let refresh_w = UI.with_borrow(|ui| ui.refresh_w);
    if x < refresh_w {
        Zone::Refresh
    } else {
        Zone::Numbers
    }
}

unsafe extern "system" fn readout_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_LBUTTONUP => {
            if let Some(actions) = ACTIONS.get() {
                match zone_at(lp) {
                    Zone::Refresh => {
                        let busy = SHARED.lock().unwrap_or_else(|e| e.into_inner()).busy;
                        if !busy {
                            (actions.refresh)();
                        }
                    }
                    _ => (actions.open)(),
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let zone = zone_at(lp);
            let before = UI.with_borrow_mut(|ui| std::mem::replace(&mut ui.hover, zone));
            if before == Zone::None {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut track);
            }
            if before != zone {
                layout();
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            UI.with_borrow_mut(|ui| ui.hover = Zone::None);
            layout();
            LRESULT(0)
        }
        WM_DESTROY => {
            // Explorer went away and took its children with it. Forget the
            // handle; `TaskbarCreated` embeds a fresh one.
            UI.with_borrow_mut(|ui| {
                if ui.readout == Some(hwnd) {
                    ui.readout = None;
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Whether the taskbar is drawn light, so the text has to be dark.
fn light_taskbar() -> bool {
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: the out pointers are valid for the sizes given.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    status.is_ok() && value == 1
}

/// Size, place and paint the readout from the current text and taskbar.
unsafe fn layout() {
    let Some((readout, taskbar, tray, hover)) =
        UI.with_borrow(|ui| Some((ui.readout?, ui.taskbar?, ui.tray, ui.hover)))
    else {
        return;
    };
    let (lines, busy) = {
        let shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        (shared.lines.clone(), shared.busy)
    };
    if lines.is_empty() {
        let _ = ShowWindow(readout, SW_HIDE);
        return;
    }

    let mut bar = RECT::default();
    if GetWindowRect(taskbar, &mut bar).is_err() {
        return;
    }
    let bar_w = bar.right - bar.left;
    let bar_h = bar.bottom - bar.top;
    // Anchor against the tray's left edge, in taskbar coordinates. Without a
    // tray, fall back to the taskbar's right edge.
    let mut anchor = POINT {
        x: bar.right,
        y: bar.top,
    };
    if let Some(tray) = tray {
        let mut r = RECT::default();
        if GetWindowRect(tray, &mut r).is_ok() {
            anchor.x = r.left;
        }
    }
    let _ = ScreenToClient(taskbar, &mut anchor);

    let dpi = match GetDpiForWindow(taskbar) {
        0 => 96,
        d => d,
    };
    let px = |v: i32| (v * dpi as i32 + 48) / 96;

    let light = light_taskbar();
    let Some(bitmap) = render(&lines, dpi, bar_h, hover, busy, light) else {
        return;
    };
    UI.with_borrow_mut(|ui| ui.refresh_w = bitmap.refresh_w);
    let width = bitmap.width;
    let x = (anchor.x - px(2) - width).max(0).min(bar_w - width);

    let _ = SetWindowPos(
        readout,
        Some(HWND_TOP),
        x,
        0,
        width,
        bar_h,
        SWP_NOACTIVATE | SWP_SHOWWINDOW,
    );
    bitmap.present(readout);
}

/// A painted readout, ready to hand to `UpdateLayeredWindow`.
struct Bitmap {
    dc: HDC,
    bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
    old: windows::Win32::Graphics::Gdi::HGDIOBJ,
    width: i32,
    height: i32,
    /// Where the refresh icon's zone ends, for hit-testing clicks.
    refresh_w: i32,
}

impl Bitmap {
    unsafe fn present(&self, hwnd: HWND) {
        let size = SIZE {
            cx: self.width,
            cy: self.height,
        };
        let origin = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        if let Err(e) = UpdateLayeredWindow(
            hwnd,
            None,
            None,
            Some(&size),
            Some(self.dc),
            Some(&origin),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        ) {
            eprintln!("[taskbar] could not update the readout: {e}");
        }
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        // SAFETY: these are the objects `render` created, released once.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// The first of `faces` this machine has, `px` pixels tall at 100% scaling.
/// The last face is taken unconditionally.
unsafe fn font(dc: HDC, faces: &[&str], px: i32, dpi: u32) -> HFONT {
    let height = -((px * dpi as i32 + 48) / 96);
    for (i, face) in faces.iter().enumerate() {
        let mut lf = LOGFONTW {
            lfHeight: height,
            lfWeight: 400,
            lfCharSet: DEFAULT_CHARSET,
            lfQuality: ANTIALIASED_QUALITY,
            ..Default::default()
        };
        for (dst, src) in lf.lfFaceName.iter_mut().zip(face.encode_utf16()) {
            *dst = src;
        }
        let f = CreateFontIndirectW(&lf);
        // GDI silently substitutes a missing face, so check what it picked.
        let old = SelectObject(dc, f.into());
        let mut got = [0u16; 64];
        let n = GetTextFaceW(dc, Some(&mut got)) as usize;
        SelectObject(dc, old);
        let picked = String::from_utf16_lossy(&got[..n.saturating_sub(1).min(got.len())]);
        if picked.eq_ignore_ascii_case(face) || i == faces.len() - 1 {
            return f;
        }
        let _ = DeleteObject(f.into());
    }
    unreachable!("the loop returns on its last face")
}

/// The clock's typeface where Windows 11 has it, Segoe UI elsewhere. The clock
/// is 12px at 100% scaling.
const TEXT_FACES: &[&str] = &["Segoe UI Variable Text", "Segoe UI"];
const TEXT_PX: i32 = 12;
/// The system icon font: Fluent on Windows 11, MDL2 on Windows 10. Both have
/// the Refresh glyph at the same code point.
const ICON_FACES: &[&str] = &["Segoe Fluent Icons", "Segoe MDL2 Assets"];
const ICON_PX: i32 = 13;
const REFRESH_GLYPH: u16 = 0xE72C;

/// Paint `lines` (right-aligned, vertically centred) and, while the readout is
/// hovered or refreshing, the refresh icon left of them. Everything is drawn
/// as white-on-black coverage first, then converted into premultiplied colour
/// and alpha: GDI cannot draw text with alpha itself, so the grayscale
/// anti-aliasing it produces is used as the alpha channel.
unsafe fn render(
    lines: &[String],
    dpi: u32,
    height: i32,
    hover: Zone,
    busy: bool,
    light: bool,
) -> Option<Bitmap> {
    let px = |v: i32| (v * dpi as i32 + 48) / 96;
    let dc = CreateCompatibleDC(None);

    let icon_font = font(dc, ICON_FACES, ICON_PX, dpi);
    let old_font = SelectObject(dc, icon_font.into());
    let glyph = [REFRESH_GLYPH];
    let mut icon = SIZE::default();
    let _ = GetTextExtentPoint32W(dc, &glyph, &mut icon);

    let f = font(dc, TEXT_FACES, TEXT_PX, dpi);
    SelectObject(dc, f.into());

    let wide: Vec<Vec<u16>> = lines.iter().map(|l| l.encode_utf16().collect()).collect();
    let mut text_w = 0;
    for l in &wide {
        let mut s = SIZE::default();
        let _ = GetTextExtentPoint32W(dc, l, &mut s);
        text_w = text_w.max(s.cx);
    }
    let mut tm = TEXTMETRICW::default();
    let _ = GetTextMetricsW(dc, &mut tm);
    let line_h = tm.tmHeight;

    let pad = px(8);
    // The icon's zone is the glyph with padding either side; the numbers' zone
    // is the rest.
    let refresh_w = icon.cx + 2 * pad;
    let width = refresh_w + text_w + pad;

    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        // Negative: top-down rows, so pixel (x, y) is at y * width + x.
        biHeight: -height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let info = BITMAPINFO {
        bmiHeader: header,
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
        Ok(b) if !bits.is_null() => b,
        _ => {
            SelectObject(dc, old_font);
            let _ = DeleteObject(f.into());
            let _ = DeleteObject(icon_font.into());
            let _ = DeleteDC(dc);
            return None;
        }
    };
    let old_bitmap = SelectObject(dc, bitmap.into());

    SetBkMode(dc, TRANSPARENT);
    SetTextColor(dc, COLORREF(0x00FF_FFFF));
    let block = line_h * wide.len() as i32;
    let top = (height - block) / 2;
    for (i, l) in wide.iter().enumerate() {
        let mut s = SIZE::default();
        let _ = GetTextExtentPoint32W(dc, l, &mut s);
        let _ = TextOutW(dc, width - pad - s.cx, top + i as i32 * line_h, l);
    }
    // The icon only while it can be used or is working: hidden at rest, so
    // the readout reads like the clock.
    if hover != Zone::None || busy {
        SelectObject(dc, icon_font.into());
        let _ = TextOutW(dc, pad, (height - icon.cy) / 2, &glyph);
    }
    SelectObject(dc, old_font);
    let _ = DeleteObject(f.into());
    let _ = DeleteObject(icon_font.into());

    let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, (width * height) as usize);
    let (text_rgb, glow_rgb, glow_alpha) = if light {
        (0x1Bu32, 0x00u32, 0.06f32)
    } else {
        (0xFFu32, 0xFFu32, 0.08f32)
    };
    // The hover highlight: a rounded rectangle inset from the taskbar's edges,
    // as the clock and the task buttons draw theirs, over whichever part is
    // under the pointer.
    let inset = px(4);
    let radius = px(4) as f32;
    let gap = px(1);
    let glow_span = match hover {
        Zone::None => None,
        Zone::Refresh => Some((0, refresh_w - gap)),
        Zone::Numbers => Some((refresh_w + gap, width)),
    };
    // A refresh in flight dims the icon, which is also why clicks on it are
    // ignored until it finishes.
    let icon_alpha = if busy { 0.35 } else { 1.0 };
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) as usize;
            let mut coverage = (pixels[i] & 0xFF) as f32 / 255.0;
            if x < refresh_w {
                coverage *= icon_alpha;
            }
            let glow = glow_span.map_or(0.0, |(left, right)| {
                glow_alpha * rounded_rect_coverage(x, y, left, right, height, inset, radius)
            });
            // Never fully transparent: a zero-alpha pixel lets clicks fall
            // through to the taskbar, and the whole readout should be a button.
            let bg_a = glow.max(1.0 / 255.0);
            let a = coverage + bg_a * (1.0 - coverage);
            let channel = |text: u32, bg: u32| {
                let c = text as f32 * coverage + bg as f32 * glow * (1.0 - coverage);
                c.round().clamp(0.0, 255.0) as u32
            };
            let c = channel(text_rgb, glow_rgb);
            let a = (a * 255.0).round().clamp(0.0, 255.0) as u32;
            pixels[i] = (a << 24) | (c << 16) | (c << 8) | c;
        }
    }

    Some(Bitmap {
        dc,
        bitmap,
        old: old_bitmap,
        width,
        height,
        refresh_w,
    })
}

/// How much of pixel (x, y) a rounded rectangle spanning `left..right`, inset
/// by `inset` from the top and bottom of `h`, covers, anti-aliased across one
/// pixel at the corners.
fn rounded_rect_coverage(x: i32, y: i32, left: i32, right: i32, h: i32, inset: i32, r: f32) -> f32 {
    let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
    let (left, right) = (left as f32, right as f32);
    let (top, bottom) = (inset as f32, (h - inset) as f32);
    if cx < left || cx > right || cy < top || cy > bottom {
        return 0.0;
    }
    let dx = (left + r - cx).max(cx - (right - r)).max(0.0);
    let dy = (top + r - cy).max(cy - (bottom - r)).max(0.0);
    let d = (dx * dx + dy * dy).sqrt();
    (r - d + 0.5).clamp(0.0, 1.0)
}
