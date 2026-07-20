use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui::Context;
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Show,
    Exit,
}

/// Creates the native tray icon and forwards native callbacks to the UI thread.
pub fn create(ctx: &Context) -> (Option<TrayIcon>, Receiver<TrayCommand>) {
    let (sender, receiver) = mpsc::channel();
    let menu = Menu::new();
    let show_item = MenuItem::new("显示 SoundCargo", true, None);
    let exit_item = MenuItem::new("退出 SoundCargo", true, None);
    let show_id = show_item.id().clone();
    let exit_id = exit_item.id().clone();
    let _ = menu.append(&show_item);
    let _ = menu.append(&exit_item);

    let icon = match Icon::from_rgba(icon_pixels(), 16, 16) {
        Ok(icon) => icon,
        Err(_) => return (None, receiver),
    };
    let tray = match TrayIconBuilder::new()
        .with_tooltip("SoundCargo 音乐播放器")
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .build()
    {
        Ok(tray) => tray,
        Err(_) => return (None, receiver),
    };

    let menu_sender: Sender<TrayCommand> = sender.clone();
    let menu_ctx = ctx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let command = if event.id == show_id {
            Some(TrayCommand::Show)
        } else if event.id == exit_id {
            Some(TrayCommand::Exit)
        } else {
            None
        };
        if let Some(command) = command {
            let _ = menu_sender.send(command);
            menu_ctx.request_repaint();
        }
    }));

    let tray_sender = sender;
    let tray_ctx = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            let _ = tray_sender.send(TrayCommand::Show);
            tray_ctx.request_repaint();
        }
    }));

    (Some(tray), receiver)
}

fn icon_pixels() -> Vec<u8> {
    let mut pixels = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16 {
        for x in 0..16 {
            let dx = x - 7;
            let dy = y - 7;
            let distance = dx * dx + dy * dy;
            if distance <= 49 {
                pixels.extend_from_slice(&[14, 179, 197, 255]);
            } else {
                pixels.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    pixels
}
