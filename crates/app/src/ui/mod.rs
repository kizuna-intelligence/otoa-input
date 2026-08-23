pub mod overlay;
pub mod settings_view;
pub mod theme;
pub mod tray;

use crate::controller::LoginState;
use crate::controller::{ControllerCommand, LevelStatus, OverlayKind, OverlayView, UiUpdate};
use crate::settings::Settings;
use crate::wiring::Runtime;
use crossbeam_channel::{Receiver, Sender};
use floem::{
    ext_event::create_signal_from_channel,
    new_window, quit_app,
    reactive::{create_effect, RwSignal, SignalGet, SignalUpdate},
    window::{WindowConfig, WindowLevel},
    AppEvent, Application,
};
use otoa_input_core::OverlayTransparency;
#[cfg(target_os = "linux")]
use otoa_input_platform::apply_overlay_hints;
use otoa_input_platform::{compositor_available, primary_screen_size};
#[cfg(target_os = "linux")]
use std::{thread, time::Duration};
#[cfg(target_os = "linux")]
use tracing::warn;

#[derive(Clone, Copy)]
pub struct UiState {
    pub(crate) overlay_mode: RwSignal<OverlayMode>,
    pub(crate) overlay_committed: RwSignal<String>,
    pub(crate) overlay_partial: RwSignal<String>,
    pub(crate) overlay_error: RwSignal<String>,
    pub(crate) level: RwSignal<f64>,
    pub(crate) level_status: RwSignal<LevelStatus>,
    pub(crate) route_local: RwSignal<Option<bool>>,
    pub(crate) account_email: RwSignal<String>,
    pub(crate) login_state: RwSignal<LoginState>,
    pub(crate) settings: RwSignal<Settings>,
    pub(crate) settings_window_open: RwSignal<bool>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum OverlayMode {
    Splash,
    Hidden,
    Shown(OverlayKind),
}

pub fn run(
    settings: Settings,
    ui_updates: Receiver<UiUpdate>,
    runtime: Runtime,
) -> anyhow::Result<()> {
    let open_settings_on_start = runtime.is_settings_preview();
    let overlay_transparent = resolve_overlay_transparency(&settings);
    let state = UiState {
        overlay_mode: RwSignal::new(if open_settings_on_start {
            OverlayMode::Hidden
        } else {
            OverlayMode::Splash
        }),
        overlay_committed: RwSignal::new(String::new()),
        overlay_partial: RwSignal::new(String::new()),
        overlay_error: RwSignal::new(String::new()),
        level: RwSignal::new(0.0),
        level_status: RwSignal::new(LevelStatus::Normal),
        route_local: RwSignal::new(None),
        account_email: RwSignal::new(String::new()),
        login_state: RwSignal::new(LoginState::LoggedOut),
        settings: RwSignal::new(settings.clone()),
        settings_window_open: RwSignal::new(false),
    };
    let ui_signal = create_signal_from_channel(ui_updates);
    let (tray_actions, tray_events) = crossbeam_channel::unbounded();
    let (tray_updates, tray_update_rx) = crossbeam_channel::unbounded();
    if let Err(error) = tray::install(runtime.commands.clone(), tray_actions, tray_update_rx) {
        tracing::warn!("failed to initialize tray; continuing without tray: {error:#}");
    }
    let tray_updates_for_ui = tray_updates.clone();
    create_effect(move |_| {
        if let Some(update) = ui_signal.get() {
            apply_ui_update(state, update, &tray_updates_for_ui);
        }
    });

    let tray_signal = create_signal_from_channel(tray_events);
    let settings_commands = runtime.commands.clone();
    create_effect(move |_| {
        let Some(action) = tray_signal.get() else {
            return;
        };
        match action {
            tray::TrayAction::OpenSettings => {
                open_settings_window(state, settings_commands.clone());
            }
            tray::TrayAction::Quit => quit_app(),
        }
    });

    let overlay_commands = runtime.commands.clone();
    let command_for_termination = runtime.commands.clone();
    let app = Application::new().on_event(move |event| {
        if matches!(event, AppEvent::WillTerminate) {
            let _ = command_for_termination.send(ControllerCommand::Shutdown);
        }
    });
    #[cfg(target_os = "linux")]
    schedule_overlay_hints();
    let window_state = state;
    let app = app.window(
        move |window_id| {
            overlay::view(
                window_state,
                overlay_commands.clone(),
                window_id,
                overlay_transparent,
            )
        },
        Some(overlay_window_config(&settings, overlay_transparent)),
    );
    if open_settings_on_start {
        open_settings_window(state, runtime.commands.clone());
    }
    app.run();

    runtime.shutdown();
    Ok(())
}

pub(crate) fn open_settings_window(state: UiState, commands: Sender<ControllerCommand>) {
    if state.settings_window_open.get_untracked() {
        return;
    }
    state.settings_window_open.set(true);
    let settings = state.settings.get_untracked();
    new_window(
        move |window_id| settings_view::view(settings, state, commands, window_id),
        Some(
            WindowConfig::default()
                .size((560.0, settings_window_initial_height()))
                .title("Otoa Input 設定"),
        ),
    );
}

fn settings_window_initial_height() -> f64 {
    primary_screen_size()
        .map(|(_, height)| (height * 0.9).min(720.0))
        .unwrap_or(720.0)
}

fn apply_ui_update(state: UiState, update: UiUpdate, _tray_updates: &Sender<tray::TrayUpdate>) {
    match update {
        UiUpdate::State(session_state) => {
            tracing::trace!(?session_state, "session state update");
        }
        UiUpdate::Overlay(view) => apply_overlay_update(state, view),
        UiUpdate::Level { peak, status } => {
            state
                .level
                .set((f64::from(peak.unsigned_abs()) / f64::from(i16::MAX)).clamp(0.0, 1.0));
            state.level_status.set(status);
        }
        UiUpdate::Route { local } => state.route_local.set(Some(local)),
        UiUpdate::Account { email } => state.account_email.set(email.unwrap_or_default()),
        UiUpdate::LoginState(login_state) => {
            state.login_state.set(login_state.clone());
            let _ = _tray_updates.send(tray::TrayUpdate::LoginState(login_state));
        }
    }
}

fn apply_overlay_update(state: UiState, view: OverlayView) {
    let (mode, committed, partial, error) = match view {
        OverlayView::Splash => (
            OverlayMode::Splash,
            String::new(),
            String::new(),
            String::new(),
        ),
        OverlayView::Hidden => (
            OverlayMode::Hidden,
            String::new(),
            String::new(),
            String::new(),
        ),
        OverlayView::Shown {
            kind,
            committed,
            partial,
            error,
        } => (OverlayMode::Shown(kind), committed, partial, error),
    };

    if state.overlay_mode.get_untracked() != mode {
        state.overlay_mode.set(mode);
    }
    if state.overlay_committed.get_untracked() != committed {
        state.overlay_committed.set(committed);
    }
    if state.overlay_partial.get_untracked() != partial {
        state.overlay_partial.set(partial);
    }
    if state.overlay_error.get_untracked() != error {
        state.overlay_error.set(error);
    }
}

fn resolve_overlay_transparency(settings: &Settings) -> bool {
    let compositor = compositor_available();
    let requested = settings.overlay_transparency();
    let transparent = match requested {
        OverlayTransparency::On => true,
        OverlayTransparency::Off => false,
        OverlayTransparency::Auto => compositor,
    };
    tracing::info!(
        requested = ?requested,
        compositor_available = compositor,
        transparent,
        "overlay transparency resolved at startup"
    );
    transparent
}

fn overlay_window_config(settings: &Settings, transparent: bool) -> WindowConfig {
    tracing::debug!(position = ?settings.overlay_position(), transparent, "overlay initial window config");
    WindowConfig::default()
        .size(overlay::initial_window_size(transparent))
        .title("Otoa Input")
        .undecorated(true)
        .show_titlebar(false)
        .resizable(false)
        .with_transparent(transparent)
        .undecorated_shadow(false)
        .window_level(WindowLevel::AlwaysOnTop)
        .apply_default_theme(false)
}

#[cfg(target_os = "linux")]
fn schedule_overlay_hints() {
    let pid = std::process::id();
    let result = thread::Builder::new()
        .name("otoa-overlay-x11-hints".to_string())
        .spawn(move || {
            for attempt in 1..=10 {
                match apply_overlay_hints(pid) {
                    Ok(true) => return,
                    Ok(false) => tracing::debug!(attempt, "overlay X11 window not registered yet"),
                    Err(error) => warn!(attempt, "failed to apply overlay X11 hints: {error:#}"),
                }
                thread::sleep(Duration::from_millis(500));
            }
            warn!("overlay X11 hint retries exhausted");
        });

    if let Err(error) = result {
        warn!("failed to schedule overlay X11 hints: {error}");
    }
}
