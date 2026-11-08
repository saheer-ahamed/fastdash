//! The notification-area icon: the way back to a hidden dashboard when the
//! taskbar readout is off, and the only way to quit now that closing the
//! window only hides it.
//!
//! `tray-icon` delivers its events on its own callback; they are forwarded as
//! [`Command`]s to the GPUI task that owns the window.

use futures::channel::mpsc::UnboundedSender;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::i18n::t;
use crate::Command;

/// Build the icon. It lives as long as the returned handle.
pub fn build(commands: UnboundedSender<Command>) -> anyhow::Result<TrayIcon> {
    let open = MenuItem::new(t("tray.open").as_ref(), true, None);
    let quit = MenuItem::new(t("tray.quit").as_ref(), true, None);
    let menu = Menu::new();
    menu.append_items(&[&open, &quit])?;
    let (open_id, quit_id) = (open.id().clone(), quit.id().clone());

    let menu_commands = commands.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let command = if event.id == open_id {
            Command::Show
        } else if event.id == quit_id {
            Command::Quit
        } else {
            return;
        };
        let _ = menu_commands.unbounded_send(command);
    }));
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            let _ = commands.unbounded_send(Command::Show);
        }
    }));

    Ok(TrayIconBuilder::new()
        .with_tooltip("fastdash")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_icon(icon()?)
        .build()?)
}

fn icon() -> anyhow::Result<tray_icon::Icon> {
    let png = include_bytes!("../resources/icons/32x32.png");
    let image = image::load_from_memory(png)?.into_rgba8();
    let (width, height) = image.dimensions();
    Ok(tray_icon::Icon::from_rgba(image.into_raw(), width, height)?)
}
