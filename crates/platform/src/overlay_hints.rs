use anyhow::{Context, Result};
use x11rb::{
    atom_manager,
    connection::Connection,
    properties::WmHints,
    protocol::xproto::{
        AtomEnum, ClientMessageEvent, ConnectionExt as XprotoConnectionExt, EventMask, PropMode,
        Window,
    },
    wrapper::ConnectionExt as WrapperConnectionExt,
};

atom_manager! {
    OverlayAtoms: OverlayAtomsCookie {
        _NET_CLIENT_LIST,
        _NET_WM_PID,
        _NET_WM_NAME,
        UTF8_STRING,
        _NET_WM_STATE,
        _NET_WM_STATE_STICKY,
        _NET_WM_STATE_ABOVE,
        _NET_WM_STATE_SKIP_TASKBAR,
        _NET_WM_STATE_SKIP_PAGER,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_UTILITY,
        _NET_WORKAREA,
        _NET_WM_CM_S0,
    }
}

const OVERLAY_TITLE: &[u8] = b"Otoa Input";

/// プライマリスクリーンの論理サイズ (width, height)。取得できなければ None。
pub fn primary_screen_size() -> Option<(f64, f64)> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        tracing::warn!("cannot get primary screen size from X11 while running on Wayland");
        return None;
    }
    if std::env::var_os("DISPLAY").is_none() {
        tracing::warn!("cannot get primary screen size: DISPLAY is not set");
        return None;
    }

    let (conn, screen_num) = match x11rb::connect(None) {
        Ok(connection) => connection,
        Err(error) => {
            tracing::warn!("failed to connect to X11 for primary screen size: {error}");
            return None;
        }
    };
    let Some(screen) = conn.setup().roots.get(screen_num) else {
        tracing::warn!("failed to get X11 primary screen: screen number is out of range");
        return None;
    };

    Some((
        f64::from(screen.width_in_pixels),
        f64::from(screen.height_in_pixels),
    ))
}

/// Linux/X11 で透過したウィンドウを正しく合成できるかを返す。
/// Wayland と X11 以外のプラットフォームでは呼び出し側が true を返す。
pub fn compositor_available() -> bool {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return true;
    }
    let Ok((conn, screen_num)) = x11rb::connect(None) else {
        return false;
    };
    let Some(atoms) = OverlayAtoms::new(&conn)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
    else {
        return false;
    };
    match conn
        .get_selection_owner(atoms._NET_WM_CM_S0)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
    {
        Some(reply) => reply.owner != 0,
        None => {
            tracing::debug!(screen_num, "could not query X11 compositor selection owner");
            false
        }
    }
}

/// X11 の `_NET_WORKAREA`。パネルを除いた (x, y, width, height) を返す。
pub fn primary_workarea() -> Option<(f64, f64, f64, f64)> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_none() {
        return None;
    }
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let screen = conn.setup().roots.get(screen_num)?;
    let atoms = OverlayAtoms::new(&conn).ok()?.reply().ok()?;
    let reply = conn
        .get_property(
            false,
            screen.root,
            atoms._NET_WORKAREA,
            AtomEnum::CARDINAL,
            0,
            4,
        )
        .ok()?
        .reply()
        .ok()?;
    let values = reply.value32()?.take(4).collect::<Vec<_>>();
    if values.len() != 4 {
        return None;
    }
    Some((
        f64::from(values[0]),
        f64::from(values[1]),
        f64::from(values[2]),
        f64::from(values[3]),
    ))
}

/// Reapply the X11 hints to the overlay window owned by `pid`.
pub fn apply_overlay_hints(pid: u32) -> Result<bool> {
    let (conn, screen_num) = x11rb::connect(None).context("connect to X11")?;
    let screen = &conn.setup().roots[screen_num];
    let atoms = OverlayAtoms::new(&conn)
        .context("request X11 overlay atoms")?
        .reply()
        .context("read X11 overlay atoms")?;

    let client_list = conn
        .get_property(
            false,
            screen.root,
            atoms._NET_CLIENT_LIST,
            AtomEnum::WINDOW,
            0,
            u32::MAX,
        )
        .context("request X11 client list")?
        .reply()
        .context("read X11 client list")?;

    let windows = client_list
        .value32()
        .into_iter()
        .flatten()
        .filter_map(|window| window_for_overlay(&conn, &atoms, window, pid).transpose())
        .collect::<Result<Vec<_>>>()?;

    let found = !windows.is_empty();
    for window in windows {
        apply_hints_to_window(&conn, screen.root, &atoms, window)?;
    }

    Ok(found)
}

fn window_for_overlay(
    conn: &impl XprotoConnectionExt,
    atoms: &OverlayAtoms,
    window: Window,
    pid: u32,
) -> Result<Option<Window>> {
    let pid_reply = conn
        .get_property(false, window, atoms._NET_WM_PID, AtomEnum::CARDINAL, 0, 1)
        .context("request X11 window PID")?
        .reply()
        .context("read X11 window PID")?;
    let Some(window_pid) = pid_reply.value32().and_then(|mut values| values.next()) else {
        return Ok(None);
    };
    if window_pid != pid {
        return Ok(None);
    }

    let title_reply = conn
        .get_property(false, window, atoms._NET_WM_NAME, atoms.UTF8_STRING, 0, 256)
        .context("request X11 window title")?
        .reply()
        .context("read X11 window title")?;
    Ok((title_reply.value == OVERLAY_TITLE).then_some(window))
}

fn apply_hints_to_window(
    conn: &impl XprotoConnectionExt,
    root: Window,
    atoms: &OverlayAtoms,
    window: Window,
) -> Result<()> {
    let state_atoms = [
        atoms._NET_WM_STATE_STICKY,
        atoms._NET_WM_STATE_ABOVE,
        atoms._NET_WM_STATE_SKIP_TASKBAR,
        atoms._NET_WM_STATE_SKIP_PAGER,
    ];

    conn.change_property32(
        PropMode::REPLACE,
        window,
        atoms._NET_WM_STATE,
        AtomEnum::ATOM,
        &state_atoms,
    )
    .context("set X11 overlay state")?;
    conn.change_property32(
        PropMode::REPLACE,
        window,
        atoms._NET_WM_WINDOW_TYPE,
        AtomEnum::ATOM,
        &[atoms._NET_WM_WINDOW_TYPE_UTILITY],
    )
    .context("set X11 overlay window type")?;

    let mut wm_hints = WmHints::new();
    wm_hints.input = Some(false);
    wm_hints
        .set(conn, window)
        .context("set X11 overlay WM_HINTS")?;

    for state_atom in state_atoms {
        let event =
            ClientMessageEvent::new(32, window, atoms._NET_WM_STATE, [1, state_atom, 0, 0, 0]);
        conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )
        .context("request X11 overlay state")?;
    }

    conn.sync().context("flush X11 overlay hints")?;
    Ok(())
}
