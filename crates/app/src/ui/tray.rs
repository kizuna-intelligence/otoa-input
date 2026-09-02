use crate::controller::{ControllerCommand, LoginState};
use anyhow::Result;
use crossbeam_channel::Sender;
use otoa_input_core::SessionState;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

#[cfg(not(target_os = "linux"))]
use floem::{
    ext_event::create_signal_from_channel,
    reactive::{create_effect, SignalGet},
};

#[derive(Debug, Clone, Copy)]
pub enum TrayAction {
    OpenSettings,
    Quit,
}

#[derive(Debug, Clone)]
pub enum TrayUpdate {
    Session(SessionState),
    Attention(bool),
    LoginState(LoginState),
}

const LOGIN_ID: &str = "otoa-login";
const LOGOUT_ID: &str = "otoa-logout";
const TOGGLE_ID: &str = "otoa-toggle";
const SETTINGS_ID: &str = "otoa-settings";
const QUIT_ID: &str = "otoa-quit";

#[allow(dead_code)]
const NORMAL_16_RGBA: &[u8] = include_bytes!("../../../../resources/icons/tray/otoa-tray-16.rgba");
#[allow(dead_code)]
const NORMAL_22_RGBA: &[u8] = include_bytes!("../../../../resources/icons/tray/otoa-tray-22.rgba");
#[allow(dead_code)]
const NORMAL_32_RGBA: &[u8] = include_bytes!("../../../../resources/icons/tray/otoa-tray-32.rgba");
#[allow(dead_code)]
const ATTENTION_16_RGBA: &[u8] =
    include_bytes!("../../../../resources/icons/tray/otoa-tray-attention-16.rgba");
#[allow(dead_code)]
const ATTENTION_22_RGBA: &[u8] =
    include_bytes!("../../../../resources/icons/tray/otoa-tray-attention-22.rgba");
#[allow(dead_code)]
const ATTENTION_32_RGBA: &[u8] =
    include_bytes!("../../../../resources/icons/tray/otoa-tray-attention-32.rgba");
#[allow(dead_code)]
const STOPPED_16_RGBA: &[u8] =
    include_bytes!("../../../../resources/icons/tray/otoa-tray-stopped-16.rgba");
#[allow(dead_code)]
const STOPPED_22_RGBA: &[u8] =
    include_bytes!("../../../../resources/icons/tray/otoa-tray-stopped-22.rgba");
#[allow(dead_code)]
const STOPPED_32_RGBA: &[u8] =
    include_bytes!("../../../../resources/icons/tray/otoa-tray-stopped-32.rgba");
#[allow(dead_code)]
const TEMPLATE_22_RGBA: &[u8] =
    include_bytes!("../../../../resources/icons/tray/otoa-tray-template-22.rgba");

struct TrayMenu {
    toggle: MenuItem,
    settings: MenuItem,
    login: Option<MenuItem>,
    logout: Option<MenuItem>,
    quit: MenuItem,
}

struct TrayVisual {
    session: SessionState,
    attention: bool,
}

struct TrayRuntime {
    tray: TrayIcon,
    menu: TrayMenu,
    visual: TrayVisual,
}

pub fn install(
    commands: Sender<ControllerCommand>,
    actions: Sender<TrayAction>,
    updates: crossbeam_channel::Receiver<TrayUpdate>,
    account_settings_available: bool,
) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("otoa-tray".to_string())
            .spawn(move || {
                if let Err(error) = run_linux(
                    commands,
                    actions,
                    updates,
                    account_settings_available,
                    ready_tx,
                ) {
                    tracing::warn!("tray loop stopped: {error:#}");
                }
            })?;
        let ready = ready_rx
            .recv()
            .map_err(|_| anyhow::anyhow!("tray thread exited before initialization"))?;
        ready.map_err(|error| anyhow::anyhow!(error))?;
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    {
        let runtime = build_tray(account_settings_available)?;
        TRAY.with(|slot| *slot.borrow_mut() = Some(runtime));
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            TRAY.with(|slot| {
                if let Some(runtime) = slot.borrow().as_ref() {
                    dispatch_event(&event.id, &runtime.menu, &commands, &actions);
                }
            });
        }));
        let update_signal = create_signal_from_channel(updates);
        create_effect(move |_| {
            if let Some(update) = update_signal.get() {
                TRAY.with(|slot| {
                    if let Some(runtime) = slot.borrow_mut().as_mut() {
                        apply_tray_update(runtime, update);
                    }
                });
            }
        });
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
thread_local! {
    static TRAY: std::cell::RefCell<Option<TrayRuntime>> = const { std::cell::RefCell::new(None) };
}

fn build_tray(account_settings_available: bool) -> Result<TrayRuntime> {
    let toggle = MenuItem::with_id(TOGGLE_ID, toggle_label(SessionState::Disabled), true, None);
    let settings = MenuItem::with_id(SETTINGS_ID, "設定…", true, None);
    let login =
        account_settings_available.then(|| MenuItem::with_id(LOGIN_ID, "ログイン", true, None));
    let logout =
        account_settings_available.then(|| MenuItem::with_id(LOGOUT_ID, "ログアウト", false, None));
    let quit = MenuItem::with_id(QUIT_ID, "終了", true, None);
    let version = MenuItem::new(
        format!("Otoa Input v{}", env!("CARGO_PKG_VERSION")),
        false,
        None,
    );
    let first_separator = PredefinedMenuItem::separator();
    let second_separator = PredefinedMenuItem::separator();

    let menu = Menu::new();
    menu.append(&toggle)?;
    menu.append(&settings)?;
    menu.append(&first_separator)?;
    if let Some(login) = &login {
        menu.append(login)?;
    }
    if let Some(logout) = &logout {
        menu.append(logout)?;
    }
    if account_settings_available {
        menu.append(&second_separator)?;
    }
    menu.append(&version)?;
    menu.append(&quit)?;

    let icon = icon_for(SessionState::Disabled, false)?;
    let builder = TrayIconBuilder::new()
        .with_tooltip(tooltip(SessionState::Disabled))
        .with_menu(Box::new(menu))
        .with_icon(icon);
    #[cfg(target_os = "macos")]
    let builder = builder.with_icon_as_template(true);
    let tray = builder.build()?;

    Ok(TrayRuntime {
        tray,
        menu: TrayMenu {
            toggle,
            settings,
            login,
            logout,
            quit,
        },
        visual: TrayVisual {
            session: SessionState::Disabled,
            attention: false,
        },
    })
}

fn icon_from_rgba(bytes: &[u8], size: u32) -> Result<Icon> {
    Ok(Icon::from_rgba(bytes.to_vec(), size, size)?)
}

fn icon_for(session: SessionState, attention: bool) -> Result<Icon> {
    #[cfg(target_os = "macos")]
    {
        let _ = session;
        let _ = attention;
        return icon_from_rgba(TEMPLATE_22_RGBA, 22);
    }

    #[cfg(target_os = "windows")]
    {
        let (bytes, size) = if attention {
            (ATTENTION_32_RGBA, 32)
        } else if !session.listening_enabled() {
            (STOPPED_32_RGBA, 32)
        } else {
            (NORMAL_32_RGBA, 32)
        };
        return icon_from_rgba(bytes, size);
    }

    #[cfg(target_os = "linux")]
    {
        let (bytes, size) = if attention {
            (ATTENTION_22_RGBA, 22)
        } else if !session.listening_enabled() {
            (STOPPED_22_RGBA, 22)
        } else {
            (NORMAL_22_RGBA, 22)
        };
        icon_from_rgba(bytes, size)
    }
}

fn dispatch_event(
    id: &MenuId,
    menu: &TrayMenu,
    commands: &Sender<ControllerCommand>,
    actions: &Sender<TrayAction>,
) {
    if id == menu.toggle.id() {
        let _ = commands.send(ControllerCommand::StartStop);
    } else if id == menu.settings.id() {
        let _ = actions.send(TrayAction::OpenSettings);
    } else if menu.login.as_ref().is_some_and(|item| id == item.id()) {
        let _ = commands.send(ControllerCommand::StartLogin);
    } else if menu.logout.as_ref().is_some_and(|item| id == item.id()) {
        let _ = commands.send(ControllerCommand::Logout);
    } else if id == menu.quit.id() {
        let _ = commands.send(ControllerCommand::Shutdown);
        let _ = actions.send(TrayAction::Quit);
    }
}

fn apply_tray_update(runtime: &mut TrayRuntime, update: TrayUpdate) {
    match update {
        TrayUpdate::Session(session) => {
            runtime.visual.session = session;
            runtime.menu.toggle.set_text(toggle_label(session));
            refresh_tray_visual(runtime);
        }
        TrayUpdate::Attention(attention) => {
            runtime.visual.attention = attention;
            refresh_tray_visual(runtime);
        }
        TrayUpdate::LoginState(state) => {
            apply_login_state(
                runtime.menu.login.as_ref(),
                runtime.menu.logout.as_ref(),
                &state,
            );
        }
    }
}

fn refresh_tray_visual(runtime: &TrayRuntime) {
    match icon_for(runtime.visual.session, runtime.visual.attention) {
        Ok(icon) => {
            if let Err(error) = runtime.tray.set_icon(Some(icon)) {
                tracing::debug!(%error, "failed to update tray icon");
            }
        }
        Err(error) => tracing::warn!("failed to create tray icon: {error:#}"),
    }
    if let Err(error) = runtime
        .tray
        .set_tooltip(Some(tooltip(runtime.visual.session)))
    {
        tracing::debug!(%error, "failed to update tray tooltip");
    }
}

/// 待受の状態に応じたメニュー文言。
pub fn toggle_label(state: SessionState) -> &'static str {
    if !state.listening_enabled() {
        "待受を始める"
    } else {
        "待受を止める"
    }
}

/// 待受の状態に応じたツールチップ。
pub fn tooltip(state: SessionState) -> String {
    format!(
        "Otoa Input ・ {}",
        if !state.listening_enabled() {
            "停止中"
        } else {
            "待受中"
        }
    )
}

#[cfg(target_os = "linux")]
fn run_linux(
    commands: Sender<ControllerCommand>,
    actions: Sender<TrayAction>,
    updates: crossbeam_channel::Receiver<TrayUpdate>,
    account_settings_available: bool,
    ready: Sender<std::result::Result<(), String>>,
) -> Result<()> {
    if let Err(error) =
        gtk::init().map_err(|error| anyhow::anyhow!("failed to initialize GTK: {error}"))
    {
        let _ = ready.send(Err(error.to_string()));
        return Err(error);
    }
    let mut runtime = match build_tray(account_settings_available) {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return Err(error);
        }
    };
    let _ = ready.send(Ok(()));

    loop {
        while let Ok(update) = updates.try_recv() {
            apply_tray_update(&mut runtime, update);
        }
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            dispatch_event(&event.id, &runtime.menu, &commands, &actions);
            if event.id == *runtime.menu.quit.id() {
                drop(runtime);
                return Ok(());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
}

/// トレイのログイン項目の見た目を状態に合わせる。
fn apply_login_state(login: Option<&MenuItem>, logout: Option<&MenuItem>, state: &LoginState) {
    let (Some(login), Some(logout)) = (login, logout) else {
        return;
    };
    match state {
        LoginState::LoggedOut | LoginState::Failed { .. } => {
            login.set_text("ログイン");
            login.set_enabled(true);
            logout.set_enabled(false);
        }
        LoginState::InProgress => {
            login.set_text("ログイン処理中…");
            login.set_enabled(false);
            logout.set_enabled(false);
        }
        LoginState::LoggedIn { .. } => {
            login.set_text("ログイン");
            login.set_enabled(false);
            logout.set_enabled(true);
        }
        LoginState::NotRequired => {
            login.set_text("ログイン");
            login.set_enabled(false);
            logout.set_enabled(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{toggle_label, tooltip};
    use otoa_input_core::SessionState;

    #[test]
    fn toggle_label_follows_session_state() {
        assert_eq!(toggle_label(SessionState::Disabled), "待受を始める");
        assert_eq!(toggle_label(SessionState::Stopping), "待受を始める");
        assert_eq!(toggle_label(SessionState::Listening), "待受を止める");
        assert_eq!(toggle_label(SessionState::Failed), "待受を止める");
    }

    #[test]
    fn tooltip_follows_session_state() {
        assert_eq!(tooltip(SessionState::Disabled), "Otoa Input ・ 停止中");
        assert_eq!(tooltip(SessionState::Stopping), "Otoa Input ・ 停止中");
        assert_eq!(tooltip(SessionState::Listening), "Otoa Input ・ 待受中");
    }
}
