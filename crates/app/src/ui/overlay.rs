use super::theme;
use super::{OverlayMode, UiState};
use crate::controller::LoginState;
use crate::controller::{ControllerCommand, OverlayKind};
use crossbeam_channel::Sender;
use floem::{
    peniko::kurbo::{Rect, Size},
    prelude::*,
    reactive::{create_effect, SignalGet},
    views::dyn_container,
    window::WindowId,
    WindowIdExt,
};
use otoa_input_platform::primary_screen_size;
use tracing::debug;

const HIDDEN_SIZE: Size = Size::new(1.0, 1.0);
pub(crate) const SPLASH_SIZE: Size = Size::new(360.0, 200.0);
const NORMAL_SIZE: (f64, f64) = (640.0, 80.0);
const TEXT_SIZE: (f64, f64) = (640.0, 100.0);
const ERROR_SIZE: (f64, f64) = (640.0, 128.0);

pub fn view(
    state: UiState,
    commands: Sender<ControllerCommand>,
    window_id: WindowId,
) -> impl IntoView {
    create_effect(move |prev: Option<OverlayWindowShape>| {
        let mode = state.overlay_mode.get();
        let shape = match mode {
            OverlayMode::Hidden => OverlayWindowShape::Hidden,
            OverlayMode::Splash => OverlayWindowShape::Splash,
            OverlayMode::Shown(_) => OverlayWindowShape::Shown(overlay_size(
                !state.overlay_committed.get().is_empty(),
                !state.overlay_partial.get().is_empty(),
                !state.overlay_error.get().is_empty(),
            )),
        };
        if prev.as_ref() != Some(&shape) {
            apply_overlay_window(&window_id, shape);
            debug!("overlay view changed: {}", overlay_summary(mode));
        }
        shape
    });

    container(dyn_container(
        move || state.overlay_mode.get(),
        move |mode| match mode {
            OverlayMode::Splash => splash_content(state).into_any(),
            OverlayMode::Hidden | OverlayMode::Shown(_) => normal_content(state).into_any(),
        },
    ))
    .on_click_stop(move |_| {
        let command = if matches!(
            state.login_state.get_untracked(),
            LoginState::LoggedOut | LoginState::Failed { .. }
        ) {
            ControllerCommand::StartLogin
        } else {
            ControllerCommand::StartStop
        };
        let _ = commands.send(command);
    })
    .style(|style| {
        style
            .width_full()
            .height_full()
            .font_family(theme::font_family().to_string())
            .padding_horiz(theme::space::LG)
            .padding_vert(theme::space::MD)
            .border(1.0)
            .border_color(theme::color::BORDER)
            .background(theme::color::SURFACE)
    })
}

#[derive(Clone, Copy, PartialEq)]
enum OverlayWindowShape {
    Hidden,
    Splash,
    Shown((f64, f64)),
}

fn apply_overlay_window(window_id: &WindowId, shape: OverlayWindowShape) {
    match shape {
        OverlayWindowShape::Hidden => {
            window_id.set_content_size(HIDDEN_SIZE);
            if let Some((screen_width, screen_height)) = primary_screen_size() {
                set_window_bounds(window_id, screen_width - 1.0, screen_height - 1.0, 1.0, 1.0);
            } else if let Some(monitor) = window_id.monitor_bounds() {
                set_window_bounds(window_id, monitor.x1 - 1.0, monitor.y1 - 1.0, 1.0, 1.0);
            }
            debug!("overlay hidden bounds w=1 h=1");
        }
        OverlayWindowShape::Splash => {
            window_id.set_content_size(SPLASH_SIZE);
            if let Some((screen_width, screen_height)) = primary_screen_size() {
                set_window_bounds(
                    window_id,
                    (screen_width - SPLASH_SIZE.width) / 2.0,
                    (screen_height - SPLASH_SIZE.height) / 2.0,
                    SPLASH_SIZE.width,
                    SPLASH_SIZE.height,
                );
            } else if let Some(monitor) = window_id.monitor_bounds() {
                set_window_bounds(
                    window_id,
                    monitor.x0 + (monitor.width() - SPLASH_SIZE.width) / 2.0,
                    monitor.y0 + (monitor.height() - SPLASH_SIZE.height) / 2.0,
                    SPLASH_SIZE.width,
                    SPLASH_SIZE.height,
                );
            }
            debug!("overlay splash bounds w=360 h=200");
        }
        OverlayWindowShape::Shown((width, height)) => {
            window_id.set_content_size(Size::new(width, height));
            if let Some((screen_width, screen_height)) = primary_screen_size() {
                set_window_bounds(
                    window_id,
                    (screen_width - width) / 2.0,
                    (screen_height - height) / 2.0,
                    width,
                    height,
                );
            } else if let Some(monitor) = window_id.monitor_bounds() {
                set_window_bounds(
                    window_id,
                    monitor.x0 + (monitor.width() - width) / 2.0,
                    monitor.y0 + (monitor.height() - height) / 2.0,
                    width,
                    height,
                );
            }
            debug!("overlay shown bounds w={width} h={height}");
        }
    }
}

fn set_window_bounds(window_id: &WindowId, x: f64, y: f64, width: f64, height: f64) {
    window_id.set_window_outer_bounds(Rect::new(x, y, x + width, y + height));
}

fn overlay_size(has_committed: bool, has_partial: bool, has_error: bool) -> (f64, f64) {
    if has_error {
        ERROR_SIZE
    } else if has_committed || has_partial {
        TEXT_SIZE
    } else {
        NORMAL_SIZE
    }
}

fn overlay_kind_color(mode: OverlayMode) -> floem::peniko::Color {
    let OverlayMode::Shown(kind) = mode else {
        return theme::color::IDLE;
    };
    match kind {
        OverlayKind::Recognizing => theme::color::BRAND,
        OverlayKind::Finalizing => theme::color::CYAN,
        OverlayKind::Committed => theme::color::ACTIVE,
        OverlayKind::Connecting => theme::color::AMBER,
        OverlayKind::Error => theme::color::ERROR,
        OverlayKind::LoginNeeded => theme::color::IDLE,
    }
}

fn splash_content(state: UiState) -> impl IntoView {
    let status = label(move || splash_status(&state.login_state.get())).style(|style| {
        style
            .font_family(theme::font_family().to_string())
            .font_size(theme::text::CAPTION)
            .color(theme::color::TEXT_MUTED)
    });
    v_stack((
        img(|| theme::LOGO_PNG.to_vec()).style(|style| style.size(96.0, 96.0)),
        label(|| "Otoa Input").style(|style| {
            style
                .font_family(theme::font_family().to_string())
                .font_size(theme::text::TITLE)
                .color(theme::color::TEXT)
        }),
        status,
    ))
    .style(|style| {
        style
            .width_full()
            .height_full()
            .items_center()
            .justify_center()
            .gap(theme::space::XS)
    })
}

fn normal_content(state: UiState) -> impl IntoView {
    v_stack((
        h_stack((
            empty().style(move |style| {
                style
                    .size(12.0, 12.0)
                    .border_radius(999.0)
                    .background(overlay_kind_color(state.overlay_mode.get()))
            }),
            label(move || overlay_status_text(state.overlay_mode.get()).to_string()).style(
                |style| {
                    style
                        .font_family(theme::font_family().to_string())
                        .font_size(theme::text::CAPTION)
                        .color(theme::color::TEXT_MUTED)
                },
            ),
            empty().style(|style| style.flex_grow(1.0)),
        ))
        .style(|style| style.width_full().items_center().gap(8.0)),
        label(move || display_text(&state.overlay_committed.get())).style(|style| {
            style
                .width_full()
                .font_family(theme::font_family().to_string())
                .font_size(theme::text::BODY)
                .line_height(1.4)
                .color(theme::color::TEXT)
                .height(40.0)
                .text_ellipsis()
        }),
        label(move || display_text(&state.overlay_partial.get())).style(|style| {
            style
                .width_full()
                .font_family(theme::font_family().to_string())
                .font_size(theme::text::BODY)
                .line_height(1.4)
                .color(theme::color::TEXT_MUTED)
                .height(40.0)
                .text_ellipsis()
        }),
        label(move || state.overlay_error.get()).style(|style| {
            style
                .width_full()
                .font_family(theme::font_family().to_string())
                .font_size(theme::text::CAPTION)
                .color(theme::color::ERROR)
        }),
    ))
    .style(|style| style.width_full().height_full().gap(6.0))
}

fn splash_status(login_state: &LoginState) -> &'static str {
    match login_state {
        LoginState::LoggedIn { .. } | LoginState::NotRequired => "待機中",
        LoginState::LoggedOut | LoginState::InProgress | LoginState::Failed { .. } => {
            "ログインが必要です"
        }
    }
}

fn display_text(text: &str) -> String {
    const LINE_LIMIT: usize = 40;
    const MAX_CHARS: usize = LINE_LIMIT * 2;
    let mut chars = text.chars().take(MAX_CHARS).collect::<Vec<_>>();
    if text.chars().count() > MAX_CHARS {
        if let Some(last) = chars.last_mut() {
            *last = '…';
        }
    }
    if chars.len() > LINE_LIMIT && !chars[..LINE_LIMIT].contains(&'\n') {
        chars.insert(LINE_LIMIT, '\n');
    }
    chars.into_iter().collect()
}

/// オーバーレイに出す状態名。色だけでは何が起きているか分からない。
fn overlay_status_text(mode: OverlayMode) -> &'static str {
    let OverlayMode::Shown(kind) = mode else {
        return "";
    };
    match kind {
        OverlayKind::Connecting => "接続中",
        OverlayKind::Recognizing => "音声入力中",
        OverlayKind::Finalizing => "認識中",
        OverlayKind::Committed => "確定",
        OverlayKind::Error => "エラー",
        OverlayKind::LoginNeeded => "ログインが必要です",
    }
}

fn overlay_summary(mode: OverlayMode) -> &'static str {
    match mode {
        OverlayMode::Hidden => "hidden",
        OverlayMode::Splash => "splash",
        OverlayMode::Shown(kind) => match kind {
            OverlayKind::Connecting => "connecting",
            OverlayKind::Recognizing => "recognizing",
            OverlayKind::Finalizing => "finalizing",
            OverlayKind::Committed => "committed",
            OverlayKind::Error => "error",
            OverlayKind::LoginNeeded => "login-needed",
        },
    }
}
