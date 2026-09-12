//! The HUD overlay window: transparent, undecorated, always on top, and
//! click-through by default so it can sit over the game without stealing
//! input. A global hotkey (Ctrl+Shift+O) makes it interactive for moving
//! and resizing, and the overlay page draws a frame while it is.
//!
//! Decision 3 in `docs/PLAN.md`. Requires Elite in borderless mode; a
//! fullscreen-exclusive game paints over every other window.

use crate::state::AppState;
use anyhow::{Context, Result};
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

pub fn setup(app: &AppHandle) -> Result<()> {
    if let Some(w) = app.get_webview_window("overlay") {
        w.set_ignore_cursor_events(true)
            .context("overlay click-through")?;
        if w.outer_position()
            .ok()
            .is_some_and(|p| p.x == 24 && p.y == 24)
        {
            if let (Ok(Some(monitor)), Ok(size)) = (w.primary_monitor(), w.outer_size()) {
                let area = monitor.size();
                let x = ((area.width as i32 - size.width as i32) / 2).max(0);
                let _ = w.set_position(tauri::PhysicalPosition::new(x, 20));
            }
        }

        // tauri-plugin-window-state only writes its file on a clean exit,
        // which a dev rebuild or a crash never gives it. Persist on every
        // move/resize instead so the HUD position survives anything.
        let handle = app.clone();
        w.on_window_event(move |e| {
            if matches!(
                e,
                tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_)
            ) {
                use tauri_plugin_window_state::{AppHandleExt, StateFlags};
                if let Err(err) = handle.save_window_state(StateFlags::all()) {
                    tracing::warn!(error = %err, "could not save window state");
                }
            }
        });
    }

    // Closing the main window saves its state and ends the session on
    // every platform (the tray that once kept EDDA alive behind a closed
    // window went with the local data, 2026-09-09). The explicit exit
    // keeps an overlay-only process from lingering with no way back.
    if let Some(main) = app.get_webview_window("main") {
        let handle = app.clone();
        main.on_window_event(move |e| {
            if matches!(e, tauri::WindowEvent::CloseRequested { .. }) {
                use tauri_plugin_window_state::{AppHandleExt, StateFlags};
                let _ = handle.save_window_state(StateFlags::all());
                handle.exit(0);
            }
        });
    }

    let hide = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyH);
    app.plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(move |app, shortcut, event| {
                // Push-to-talk needs press and release; everything else only press.
                if crate::listen::hotkey_event(app, shortcut, event.state()) {
                    return;
                }
                if event.state() != ShortcutState::Pressed {
                    return;
                }
                if shortcut == &hide {
                    if let Some(w) = app.get_webview_window("overlay") {
                        let visible = w.is_visible().unwrap_or(true);
                        let _ = if visible { w.hide() } else { w.show() };
                    }
                }
            })
            .build(),
    )
    .context("registering global shortcut plugin")?;
    app.global_shortcut()
        .register(hide)
        .context("registering Ctrl+Shift+H")?;
    tracing::info!("overlay ready: Ctrl+Shift+H to hide or show; unlock it from Settings");
    Ok(())
}

/// Flip the overlay between click-through and interactive.
pub fn set_interactive(app: &AppHandle, state: &AppState, interactive: bool) -> Result<bool> {
    let Some(w) = app.get_webview_window("overlay") else {
        anyhow::bail!("no overlay window");
    };
    w.set_ignore_cursor_events(!interactive)?;
    if interactive {
        let _ = w.set_focus();
    }
    state
        .overlay_interactive
        .store(interactive, Ordering::Relaxed);
    tracing::info!(interactive, "overlay interaction toggled");
    let _ = app.emit(crate::events::OVERLAY_INTERACTIVE, interactive);
    Ok(interactive)
}
