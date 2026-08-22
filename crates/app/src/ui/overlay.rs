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
        label(move || display_error(&state.overlay_error.get())).style(|style| {
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
    let text_chars = text.chars().collect::<Vec<_>>();
    let mut chars = if text_chars.len() > MAX_CHARS {
        let mut tail = Vec::with_capacity(MAX_CHARS);
        tail.push('…');
        tail.extend_from_slice(&text_chars[text_chars.len() - (MAX_CHARS - 1)..]);
        tail
    } else {
        text_chars
    };
    if chars.len() > LINE_LIMIT && !chars[..LINE_LIMIT].contains(&'\n') {
        chars.insert(LINE_LIMIT, '\n');
    }
    chars.into_iter().collect()
}

/// エラー文を折り返す。
///
/// **転記テキストと違い、大事なのは先頭である**（何が起きたか）。
/// だから末尾ではなく先頭を残し、`LINE_LIMIT` ごとに折り返す。
/// 折り返さないと 1 行のまま溢れて、右端で切れて読めなくなる（実際に起きた）。
fn display_error(text: &str) -> String {
    const LINE_LIMIT: usize = 34;
    const MAX_LINES: usize = 3;
    let chars = text.chars().collect::<Vec<_>>();
    let limit = LINE_LIMIT * MAX_LINES;
    let mut lines = Vec::new();
    for chunk in chars.chunks(LINE_LIMIT).take(MAX_LINES) {
        lines.push(chunk.iter().collect::<String>());
    }
    if chars.len() > limit {
        if let Some(last) = lines.last_mut() {
            last.pop();
            last.push('…');
        }
    }
    lines.join("\n")
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

#[cfg(test)]
mod tests {
    use super::display_text;

    #[test]
    fn display_text_keeps_exactly_eighty_characters() {
        let text = "a".repeat(80);
        assert_eq!(
            display_text(&text),
            format!("{}\n{}", "a".repeat(40), "a".repeat(40))
        );
    }

    #[test]
    fn display_text_keeps_the_tail_after_eighty_characters() {
        let text = format!("discard{}", "z".repeat(79));
        let displayed = display_text(&text);

        assert_eq!(
            displayed,
            format!("…{}\n{}", "z".repeat(39), "z".repeat(40))
        );
        assert!(!displayed.contains("discard"));
    }

    #[test]
    fn display_text_truncates_japanese_at_character_boundaries() {
        let text = format!("捨てる{}末尾", "語".repeat(80));
        let displayed = display_text(&text);

        assert!(displayed.starts_with('…'));
        assert!(displayed.ends_with("末尾"));
        assert_eq!(
            displayed
                .chars()
                .filter(|character| *character != '\n')
                .count(),
            80
        );
    }
}

#[cfg(test)]
mod error_tests {
    use super::display_error;

    #[test]
    fn long_error_wraps_instead_of_overflowing_one_line() {
        let text = "認識モデル kodama-ja-streaming-small が見つかりません。設定から認識エンジンを選び直すか、README の手順でモデルを置いてください";
        let displayed = display_error(text);
        assert!(displayed.contains('\n'), "折り返されていない: {displayed}");
        for line in displayed.lines() {
            assert!(line.chars().count() <= 34, "行が長すぎる: {line}");
        }
        assert!(displayed.starts_with("認識モデル"), "先頭が残っていない");
    }

    #[test]
    fn short_error_is_unchanged() {
        assert_eq!(display_error("接続できません"), "接続できません");
    }
}
