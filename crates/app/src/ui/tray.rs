use super::theme;
use crate::controller::ControllerCommand;
use crate::controller::LoginState;
use anyhow::Result;
use crossbeam_channel::Sender;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

#[derive(Debug, Clone, Copy)]
pub enum TrayAction {
    OpenSettings,
    Quit,
}

#[derive(Debug, Clone)]
pub enum TrayUpdate {
    /// **Linux 以外では今のところ使われない。** 理由は `apply_login_state` を参照。
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    LoginState(LoginState),
}

const LOGIN_ID: &str = "otoa-login";
const LOGOUT_ID: &str = "otoa-logout";
const TOGGLE_ID: &str = "otoa-toggle";
const SETTINGS_ID: &str = "otoa-settings";
const QUIT_ID: &str = "otoa-quit";

pub fn install(
    commands: Sender<ControllerCommand>,
    actions: Sender<TrayAction>,
    updates: crossbeam_channel::Receiver<TrayUpdate>,
) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("otoa-tray".to_string())
            .spawn(move || {
                if let Err(error) = run_linux(commands, actions, updates, ready_tx) {
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
        let (tray, ids, _login, _logout) = build_tray()?;
        drop(updates);
        TRAY.with(|slot| *slot.borrow_mut() = Some(tray));
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            dispatch_event(&event.id, &ids, &commands, &actions);
        }));
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
thread_local! {
    static TRAY: std::cell::RefCell<Option<TrayIcon>> = const { std::cell::RefCell::new(None) };
}

fn build_tray() -> Result<(TrayIcon, [MenuId; 5], MenuItem, MenuItem)> {
    let login = MenuItem::with_id(LOGIN_ID, "ログイン", true, None);
    let logout = MenuItem::with_id(LOGOUT_ID, "ログアウト", false, None);
    let toggle = MenuItem::with_id(TOGGLE_ID, "待受オン/オフ", true, None);
    let settings = MenuItem::with_id(SETTINGS_ID, "設定", true, None);
    let quit = MenuItem::with_id(QUIT_ID, "終了", true, None);
    let ids = [
        login.id().clone(),
        logout.id().clone(),
        toggle.id().clone(),
        settings.id().clone(),
        quit.id().clone(),
    ];
    let menu = Menu::new();
    menu.append_items(&[&login, &logout, &toggle, &settings, &quit])?;
    let tray = TrayIconBuilder::new()
        .with_tooltip("Otoa Input")
        .with_menu(Box::new(menu))
        .with_icon(build_icon()?)
        .build()?;
    Ok((tray, ids, login, logout))
}

fn build_icon() -> Result<Icon> {
    const SIZE: u32 = 16;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as i32 - 7;
            let dy = y as i32 - 7;
            let alpha = if dx * dx + dy * dy <= 49 { 255 } else { 0 };
            let brand = theme::color::BRAND;
            let mut pixel = [brand.r, brand.g, brand.b, brand.a];
            pixel[3] = alpha;
            rgba.extend_from_slice(&pixel);
        }
    }
    Ok(Icon::from_rgba(rgba, SIZE, SIZE)?)
}

fn dispatch_event(
    id: &MenuId,
    ids: &[MenuId; 5],
    commands: &Sender<ControllerCommand>,
    actions: &Sender<TrayAction>,
) {
    if id == &ids[0] {
        let _ = commands.send(ControllerCommand::StartLogin);
    } else if id == &ids[1] {
        let _ = commands.send(ControllerCommand::Logout);
    } else if id == &ids[2] {
        let _ = commands.send(ControllerCommand::StartStop);
    } else if id == &ids[3] {
        let _ = actions.send(TrayAction::OpenSettings);
    } else if id == &ids[4] {
        let _ = commands.send(ControllerCommand::Shutdown);
        let _ = actions.send(TrayAction::Quit);
    }
}

#[cfg(target_os = "linux")]
fn run_linux(
    commands: Sender<ControllerCommand>,
    actions: Sender<TrayAction>,
    updates: crossbeam_channel::Receiver<TrayUpdate>,
    ready: Sender<std::result::Result<(), String>>,
) -> Result<()> {
    if let Err(error) =
        gtk::init().map_err(|error| anyhow::anyhow!("failed to initialize GTK: {error}"))
    {
        let _ = ready.send(Err(error.to_string()));
        return Err(error);
    }
    let (tray, ids, login, logout) = match build_tray() {
        Ok(tray) => tray,
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return Err(error);
        }
    };
    let _ = ready.send(Ok(()));

    loop {
        while let Ok(update) = updates.try_recv() {
            match update {
                TrayUpdate::LoginState(state) => apply_login_state(&login, &logout, &state),
            }
        }
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            dispatch_event(&event.id, &ids, &commands, &actions);
            if event.id == ids[4] {
                drop(tray);
                return Ok(());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
}

/// トレイのログイン項目の見た目を状態に合わせる。
///
/// **今のところ Linux でしか呼ばれない。** Linux は自前のイベントループを
/// 回しているので、そこから更新できる。Windows と macOS は tray-icon の
/// ハンドラに任せていてループが無く、更新を差し込む先が無い。
/// そのため両 OS では、ログイン状態が変わってもトレイの文言が変わらない。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn apply_login_state(login: &MenuItem, logout: &MenuItem, state: &LoginState) {
    match state {
        LoginState::LoggedOut | LoginState::Failed { .. } | LoginState::NotRequired => {
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
    }
}
