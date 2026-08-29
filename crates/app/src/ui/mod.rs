pub mod overlay;
pub mod settings_view;
pub mod theme;
pub mod tray;

use crate::controller::LoginState;
use crate::controller::{ControllerCommand, LevelStatus, OverlayKind, OverlayView, UiUpdate};
use crate::settings::Settings;
use crate::wiring::{Runtime, SettingsPage};
use crossbeam_channel::{Receiver, Sender};
use floem::{
    ext_event::create_signal_from_channel,
    new_window, quit_app,
    reactive::{create_effect, RwSignal, SignalGet, SignalUpdate},
    window::{WindowConfig, WindowLevel},
    AppEvent, Application,
};
use otoa_input_core::{OverlayTransparency, SessionState};
#[cfg(target_os = "linux")]
use otoa_input_platform::apply_overlay_hints;
use otoa_input_platform::compositor_available;
#[cfg(target_os = "linux")]
use std::{thread, time::Duration};
#[cfg(target_os = "linux")]
use tracing::warn;

#[derive(Clone, Copy)]
/// 画面が読む状態。
///
/// 設定画面を自分で描く配布（[`crate::Deps::settings_view`]）がここを読むので、
/// 中身は公開してある。書き換えてよいのは `settings` と
/// `settings_window_open` だけで、残りは本体が更新する。
pub struct UiState {
    pub overlay_mode: RwSignal<OverlayMode>,
    pub overlay_committed: RwSignal<String>,
    pub overlay_partial: RwSignal<String>,
    pub overlay_error: RwSignal<String>,
    pub session_state: RwSignal<SessionState>,
    pub level: RwSignal<f64>,
    pub level_status: RwSignal<LevelStatus>,
    pub route_local: RwSignal<Option<bool>>,
    pub account_email: RwSignal<String>,
    pub login_state: RwSignal<LoginState>,
    pub settings: RwSignal<Settings>,
    pub settings_window_open: RwSignal<bool>,
    pub account_settings_available: bool,
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
    settings_view_override: Option<crate::SettingsView>,
    extra_settings_page: Option<crate::ExtraSettingsPage>,
    settings_extensions: Vec<crate::SettingsExtension>,
) -> anyhow::Result<()> {
    let open_settings_on_start = runtime.is_settings_preview();
    let settings_preview_page = runtime
        .settings_preview_page()
        .unwrap_or(SettingsPage::General);
    let account_settings_available = runtime.account_settings_available();
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
        session_state: RwSignal::new(SessionState::Disabled),
        level: RwSignal::new(0.0),
        level_status: RwSignal::new(LevelStatus::Normal),
        route_local: RwSignal::new(None),
        account_email: RwSignal::new(String::new()),
        login_state: RwSignal::new(LoginState::LoggedOut),
        settings: RwSignal::new(settings.clone()),
        settings_window_open: RwSignal::new(false),
        account_settings_available,
    };
    let ui_signal = create_signal_from_channel(ui_updates);
    let (tray_actions, tray_events) = crossbeam_channel::unbounded();
    let (tray_updates, tray_update_rx) = crossbeam_channel::unbounded();

    // 二度目の起動が置いた「設定画面を開け」の合図を拾い、トレイの「設定…」と
    // 同じ道へ流す。開き方をここで二重に持たないためである。
    // 起動前からの残骸は捨てる。前回の合図で、起動するなり設定画面が
    // 開くのを防ぐ。
    let _ = otoa_input_platform::activation::take_open_settings_request();
    let activation_actions = tray_actions.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if otoa_input_platform::activation::take_open_settings_request() {
            let _ = activation_actions.send(tray::TrayAction::OpenSettings);
        }
    });
    let tray_updates_for_ui = tray_updates.clone();
    create_effect(move |_| {
        if let Some(update) = ui_signal.get() {
            apply_ui_update(state, update, &tray_updates_for_ui);
        }
    });

    let tray_signal = create_signal_from_channel(tray_events);
    let settings_commands = runtime.commands.clone();
    let settings_view_for_tray = settings_view_override.clone();
    let extra_for_tray = extra_settings_page.clone();
    let extensions_for_tray = settings_extensions.clone();
    create_effect(move |_| {
        let Some(action) = tray_signal.get() else {
            return;
        };
        match action {
            tray::TrayAction::OpenSettings => {
                open_settings_window(
                    state,
                    settings_commands.clone(),
                    SettingsPage::General,
                    settings_view_for_tray.clone(),
                    extra_for_tray.clone(),
                    extensions_for_tray.clone(),
                );
            }
            tray::TrayAction::Quit => quit_app(),
        }
    });

    let overlay_commands = runtime.commands.clone();
    let command_for_termination = runtime.commands.clone();
    let reopen_actions = tray_actions.clone();
    let app = Application::new().on_event(move |event| match event {
        AppEvent::WillTerminate => {
            let _ = command_for_termination.send(ControllerCommand::Shutdown);
        }
        // macOS で Dock のアイコンから開き直したとき。トレイに手が届かない
        // ときの設定導線として、トレイの「設定…」と同じ道へ流す。
        AppEvent::Reopen { .. } => {
            let _ = reopen_actions.send(tray::TrayAction::OpenSettings);
        }
    });
    // トレイの生成は **イベントループを作った後** に行う。macOS/Windows では
    // `TrayIconBuilder::build()` が NSApplication を初期化するため、winit が
    // イベントループ（＝principal class）を握る前に呼ぶと
    // 「requires control over the principal class」で落ちる。Linux は別スレッドの
    // 独自ループで動くのでこの制約はないが、順序を分けないでここへ寄せておく。
    if let Err(error) = tray::install(
        runtime.commands.clone(),
        tray_actions,
        tray_update_rx,
        account_settings_available,
    ) {
        tracing::warn!("failed to initialize tray; continuing without tray: {error:#}");
    }
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
        open_settings_window(
            state,
            runtime.commands.clone(),
            settings_preview_page,
            settings_view_override.clone(),
            extra_settings_page.clone(),
            settings_extensions.clone(),
        );
    }
    app.run();

    runtime.shutdown();
    Ok(())
}

pub(crate) fn open_settings_window(
    state: UiState,
    commands: Sender<ControllerCommand>,
    initial_page: SettingsPage,
    settings_view_override: Option<crate::SettingsView>,
    extra_settings_page: Option<crate::ExtraSettingsPage>,
    settings_extensions: Vec<crate::SettingsExtension>,
) {
    if state.settings_window_open.get_untracked() {
        return;
    }
    state.settings_window_open.set(true);
    let settings = state.settings.get_untracked();
    new_window(
        move |window_id| {
            let view = match settings_view_override {
                // 差し替えがあればそちらへ。公開版の画面は settings_view::view
                // として公開してあるので、差し替え側がそれを土台に使える。
                Some(build) => build(settings, state, commands, window_id),
                None => floem::IntoView::into_any(settings_view::view(
                    settings,
                    state,
                    commands,
                    window_id,
                    initial_page,
                    extra_settings_page,
                    settings_extensions,
                )),
            };
            // 開いたことを覚えるのはここなので、**戻すのもここでやる。**
            //
            // `on_cleanup` は使えない。あれは「view がツリーから外れたとき」に
            // 呼ばれるもので、ウィンドウを閉じても呼ばれない。実際にログで
            // 確認した(閉じたあと後始末が一度も走らず、次に開こうとすると
            // 「既に開いている」と誤判定して二度と開けなくなっていた)。
            //
            // floem は窓を壊す直前に `Event::WindowClosed` を投げるので、
            // そちらを拾う。差し替え画面でも同じように効く。
            floem::views::Decorators::on_event_stop(
                view,
                floem::event::EventListener::WindowClosed,
                move |_| {
                    state.settings_window_open.set(false);
                },
            )
        },
        Some(
            WindowConfig::default()
                // ここは開いた瞬間の大きさにすぎない。中身が描かれた時点で
                // settings_view が窓を中身に合わせて伸縮させる。
                .size((900.0, 720.0))
                .title("Otoa Input の設定")
                .resizable(true),
        ),
    );
}

fn apply_ui_update(state: UiState, update: UiUpdate, tray_updates: &Sender<tray::TrayUpdate>) {
    match update {
        UiUpdate::State(session_state) => {
            state.session_state.set(session_state);
            let _ = tray_updates.send(tray::TrayUpdate::Session(session_state));
            let _ = tray_updates.send(tray::TrayUpdate::Attention(tray_needs_attention(state)));
            tracing::trace!(?session_state, "session state update");
        }
        UiUpdate::Overlay(view) => {
            let overlay_attention = matches!(
                view,
                OverlayView::Shown {
                    kind: OverlayKind::Error | OverlayKind::LoginNeeded,
                    ..
                }
            );
            apply_overlay_update(state, view);
            let _ = tray_updates.send(tray::TrayUpdate::Attention(
                overlay_attention || tray_needs_attention(state),
            ));
        }
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
            let _ = tray_updates.send(tray::TrayUpdate::LoginState(login_state.clone()));
            let _ = tray_updates.send(tray::TrayUpdate::Attention(tray_needs_attention(state)));
        }
    }
}

fn tray_needs_attention(state: UiState) -> bool {
    state.session_state.get_untracked() == SessionState::Failed
        || matches!(
            state.overlay_mode.get_untracked(),
            OverlayMode::Shown(OverlayKind::Error | OverlayKind::LoginNeeded)
        )
        || login_needs_attention(state.login_state.get_untracked())
}

fn login_needs_attention(state: LoginState) -> bool {
    matches!(state, LoginState::LoggedOut | LoginState::Failed { .. })
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
