use super::theme;
use super::{OverlayMode, UiState};
use crate::controller::{ControllerCommand, LoginState, OverlayKind};
use crossbeam_channel::Sender;
use floem::{
    action::exec_after,
    event::EventListener,
    peniko::kurbo::{Rect, Size},
    peniko::{Brush, Color},
    prelude::*,
    reactive::{create_effect, RwSignal, SignalGet, SignalUpdate},
    style::{Style, Transition},
    unit::UnitExt,
    views::{dyn_container, h_stack_from_iter, svg, v_stack_from_iter},
    window::WindowId,
    WindowIdExt,
};
use otoa_input_core::OverlayPosition;
use otoa_input_platform::{primary_screen_size, primary_workarea};
use std::time::{Duration, Instant};
use tracing::debug;

const HIDDEN_SIZE: Size = Size::new(1.0, 1.0);
const CARD_WIDTH: f64 = 560.0;
const CARD_ONE_LINE_HEIGHT: f64 = 64.0;
const CARD_TWO_LINE_HEIGHT: f64 = 86.0;
const CARD_ERROR_HEIGHT: f64 = 108.0;
const TRANSPARENT_INSET: f64 = 24.0;
const SCREEN_EDGE_INSET: f64 = 48.0;
const ORB_SIZE: f64 = 40.0;
const EQ_WIDTH: f64 = 36.0;
const EQ_HEIGHT: f64 = 30.0;
const EQ_BASE_HEIGHTS: [f64; 5] = [8.0, 18.0, 26.0, 15.0, 7.0];
const EQ_PHASES: [f64; 5] = [0.0, 0.7, 1.4, 2.1, 2.8];
const EQ_GAP: f64 = 4.0;
const TRANSCRIPT_LINE_LIMIT: usize = 26;
const TRANSCRIPT_MAX_CHARS: usize = TRANSCRIPT_LINE_LIMIT * 2;
const MOTION_TICK: Duration = Duration::from_millis(33);
const WARMUP_TITLE: &str = "起動中";
const WARMUP_SUBTITLE: &str = "まだ話さないでください";
const WAITING_FOR_RESPONSE_TITLE: &str = "認識中";
const STARTING_SERVER_TITLE: &str = "サーバーを起動しています";
const STARTING_SERVER_SUBTITLE: &str = "しばらくお待ちください";

#[derive(Clone, Copy, PartialEq)]
struct MotionFrame {
    eq_heights: [f64; 5],
    ring_diameters: [f64; 2],
    ring_alphas: [f64; 2],
    caret_visible: bool,
    appear_scale: f64,
}

impl Default for MotionFrame {
    fn default() -> Self {
        Self {
            eq_heights: [0.0; 5],
            ring_diameters: [ORB_SIZE; 2],
            ring_alphas: [0.0; 2],
            caret_visible: true,
            appear_scale: 1.0,
        }
    }
}

pub(crate) fn initial_window_size(transparent: bool) -> Size {
    window_size(CARD_ONE_LINE_HEIGHT, transparent)
}

pub(crate) fn window_size(card_height: f64, transparent: bool) -> Size {
    let inset = if transparent {
        TRANSPARENT_INSET * 2.0
    } else {
        0.0
    };
    Size::new(CARD_WIDTH + inset, card_height + inset)
}

pub fn view(
    state: UiState,
    commands: Sender<ControllerCommand>,
    window_id: WindowId,
    transparent: bool,
) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let motion = RwSignal::new(MotionFrame::default());
    let ticker_running = RwSignal::new(false);
    let ticker_generation = RwSignal::new(0_u64);
    let appear_started = RwSignal::new(None::<Instant>);
    let motion_clock = Instant::now();

    create_effect(move |previous: Option<(OverlayMode, bool)>| {
        let mode = state.overlay_mode.get();
        let reduce_motion = state.settings.get().reduce_motion;
        if previous != Some((mode, reduce_motion)) {
            let appearing = matches!(mode, OverlayMode::Shown(_)) && !reduce_motion;
            let mut frame = motion.get_untracked();
            frame.appear_scale = if appearing { 0.98 } else { 1.0 };
            if motion.get_untracked() != frame {
                motion.set(frame);
            }
            appear_started.set(if appearing {
                Some(Instant::now())
            } else {
                None
            });
        }
        sync_motion_ticker(
            state,
            motion,
            ticker_running,
            ticker_generation,
            appear_started,
            motion_clock,
        );
        (mode, reduce_motion)
    });

    create_effect(move |previous: Option<OverlayWindowShape>| {
        let mode = state.overlay_mode.get();
        let position = state.settings.get().overlay_position();
        let shape = match mode {
            OverlayMode::Hidden => OverlayWindowShape::Hidden,
            OverlayMode::Splash => OverlayWindowShape::Shown {
                size: window_size(CARD_ONE_LINE_HEIGHT, transparent),
                position,
            },
            OverlayMode::Shown(_) => OverlayWindowShape::Shown {
                size: window_size(
                    card_height(
                        mode,
                        &state.overlay_committed.get(),
                        &state.overlay_partial.get(),
                        &state.overlay_error.get(),
                    ),
                    transparent,
                ),
                position,
            },
        };
        if previous.as_ref() != Some(&shape) {
            apply_overlay_window(&window_id, shape);
            debug!("overlay view changed: {}", overlay_summary(mode));
        }
        shape
    });

    let content = dyn_container(
        move || state.overlay_mode.get(),
        move |mode| match mode {
            OverlayMode::Splash => splash_content(state, transparent, motion).into_any(),
            OverlayMode::Hidden | OverlayMode::Shown(_) => {
                normal_content(state, hovered, transparent, motion).into_any()
            }
        },
    );

    container(content)
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
        .style(move |style| {
            let mut style = style
                .width_full()
                .height_full()
                .font_family(theme::font_family().to_string());
            if transparent {
                style = style.padding(TRANSPARENT_INSET);
            }
            style
        })
}

fn sync_motion_ticker(
    state: UiState,
    motion: RwSignal<MotionFrame>,
    ticker_running: RwSignal<bool>,
    ticker_generation: RwSignal<u64>,
    appear_started: RwSignal<Option<Instant>>,
    motion_clock: Instant,
) {
    let mode = state.overlay_mode.get_untracked();
    let reduce_motion = state.settings.get_untracked().reduce_motion;
    let needs_ticker = motion_needs_ticker(mode, reduce_motion, motion.get_untracked());
    if needs_ticker && !ticker_running.get_untracked() {
        ticker_running.set(true);
        ticker_generation.update(|generation| *generation += 1);
        let generation = ticker_generation.get_untracked();
        MotionTicker {
            state,
            motion,
            ticker_running,
            ticker_generation,
            appear_started,
            motion_clock,
            generation,
        }
        .schedule(Instant::now(), state.level.get_untracked().clamp(0.0, 1.0));
    } else if !needs_ticker && ticker_running.get_untracked() {
        ticker_running.set(false);
        ticker_generation.update(|generation| *generation += 1);
    }
}

fn motion_needs_ticker(mode: OverlayMode, reduce_motion: bool, frame: MotionFrame) -> bool {
    let state_moves = match mode {
        OverlayMode::Shown(OverlayKind::Recognizing) => true,
        OverlayMode::Shown(
            OverlayKind::WarmingUp
            | OverlayKind::Connecting
            | OverlayKind::Finalizing
            | OverlayKind::WaitingForResponse
            | OverlayKind::StartingServer,
        ) => !reduce_motion,
        _ => false,
    };
    state_moves || frame.appear_scale < 1.0
}

#[derive(Clone, Copy)]
struct MotionTicker {
    state: UiState,
    motion: RwSignal<MotionFrame>,
    ticker_running: RwSignal<bool>,
    ticker_generation: RwSignal<u64>,
    appear_started: RwSignal<Option<Instant>>,
    motion_clock: Instant,
    generation: u64,
}

impl MotionTicker {
    fn schedule(self, last_tick: Instant, smooth_level: f64) {
        exec_after(MOTION_TICK, move |_| {
            if !self.ticker_running.get_untracked()
                || self.ticker_generation.get_untracked() != self.generation
            {
                return;
            }

            let now = Instant::now();
            let delta = now
                .saturating_duration_since(last_tick)
                .as_secs_f64()
                .clamp(0.0, 0.2);
            let target = self.state.level.get_untracked().clamp(0.0, 1.0);
            let next_smooth_level = if target >= smooth_level {
                target
            } else {
                let decay = 1.0 - (-delta / 0.12).exp();
                smooth_level + (target - smooth_level) * decay
            };
            let elapsed = self.motion_clock.elapsed().as_secs_f64();
            let mode = self.state.overlay_mode.get_untracked();
            let reduce_motion = self.state.settings.get_untracked().reduce_motion;
            let mut next = self.motion.get_untracked();
            next.eq_heights = eq_heights(mode, reduce_motion, next_smooth_level, elapsed);
            let (first_diameter, first_alpha) = ring_values(mode, elapsed, 0.0);
            let (second_diameter, second_alpha) = ring_values(mode, elapsed, 1.3);
            next.ring_diameters = [first_diameter, second_diameter];
            next.ring_alphas = [first_alpha, second_alpha];
            next.caret_visible = ((elapsed / 0.55).floor() as u64).is_multiple_of(2);

            if let Some(started) = self.appear_started.get_untracked() {
                let progress = started.elapsed().as_secs_f64() / theme::motion::APPEAR;
                if progress >= 1.0 {
                    next.appear_scale = 1.0;
                    self.appear_started.set(None);
                } else {
                    let eased = 1.0 - (1.0 - progress).powi(2);
                    next.appear_scale = 0.98 + 0.02 * eased;
                }
            } else {
                next.appear_scale = 1.0;
            }

            if self.motion.get_untracked() != next {
                self.motion.set(next);
            }

            if motion_needs_ticker(mode, reduce_motion, next) {
                self.schedule(now, next_smooth_level);
            } else {
                self.ticker_running.set(false);
            }
        });
    }
}

#[derive(Clone, Copy, PartialEq)]
enum OverlayWindowShape {
    Hidden,
    Shown {
        size: Size,
        position: OverlayPosition,
    },
}

fn apply_overlay_window(window_id: &WindowId, shape: OverlayWindowShape) {
    match shape {
        OverlayWindowShape::Hidden => {
            window_id.set_content_size(HIDDEN_SIZE);
            if let Some((x, y, width, height)) = screen_bounds(window_id) {
                set_window_bounds(window_id, x + width - 1.0, y + height - 1.0, 1.0, 1.0);
            }
            debug!("overlay hidden bounds w=1 h=1");
        }
        OverlayWindowShape::Shown { size, position } => {
            window_id.set_content_size(size);
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                debug!(?position, "Wayland compositor chooses the overlay position");
                return;
            }
            let bounds = match position {
                OverlayPosition::Center => primary_screen_size()
                    .map(|(width, height)| (0.0, 0.0, width, height))
                    .or_else(|| screen_bounds(window_id)),
                OverlayPosition::Bottom | OverlayPosition::Top => screen_bounds(window_id),
            };
            if let Some((x, y, width, height)) = bounds {
                let (window_x, window_y) = match position {
                    OverlayPosition::Bottom => (
                        x + (width - size.width) / 2.0,
                        y + height - SCREEN_EDGE_INSET - size.height
                            + if size.width > CARD_WIDTH {
                                TRANSPARENT_INSET
                            } else {
                                0.0
                            },
                    ),
                    OverlayPosition::Top => (
                        x + (width - size.width) / 2.0,
                        y + SCREEN_EDGE_INSET
                            - if size.width > CARD_WIDTH {
                                TRANSPARENT_INSET
                            } else {
                                0.0
                            },
                    ),
                    OverlayPosition::Center => (
                        x + (width - size.width) / 2.0,
                        y + (height - size.height) / 2.0,
                    ),
                };
                set_window_bounds(window_id, window_x, window_y, size.width, size.height);
                // X11 のウィンドウマネージャー無し環境では、位置とサイズを同じ
                // イベント周回で変更すると surface の再構成だけ遅れることがある。
                // 次の周回でも content size を伝えて、108px のエラー面などを確実に
                // 再描画させる。
                let delayed_window_id = *window_id;
                exec_after(Duration::from_millis(50), move |_| {
                    delayed_window_id.set_content_size(size);
                    let _ = delayed_window_id.force_repaint();
                });
                debug!(
                    "overlay shown bounds x={window_x} y={window_y} w={} h={} position={position:?}",
                    size.width, size.height
                );
            }
        }
    }
}

fn screen_bounds(window_id: &WindowId) -> Option<(f64, f64, f64, f64)> {
    primary_workarea()
        .or_else(|| primary_screen_size().map(|(width, height)| (0.0, 0.0, width, height)))
        .or_else(|| {
            window_id
                .monitor_bounds()
                .map(|bounds| (bounds.x0, bounds.y0, bounds.width(), bounds.height()))
        })
}

fn set_window_bounds(window_id: &WindowId, x: f64, y: f64, width: f64, height: f64) {
    window_id.set_window_outer_bounds(Rect::new(x, y, x + width, y + height));
}

fn card_height(mode: OverlayMode, committed: &str, partial: &str, error: &str) -> f64 {
    if matches!(
        mode,
        OverlayMode::Shown(OverlayKind::Error | OverlayKind::Notice)
    ) || !error.is_empty()
    {
        CARD_ERROR_HEIGHT
    } else if has_two_transcript_lines(committed, partial) {
        CARD_TWO_LINE_HEIGHT
    } else {
        CARD_ONE_LINE_HEIGHT
    }
}

fn normal_content(
    state: UiState,
    hovered: RwSignal<bool>,
    transparent: bool,
    motion: RwSignal<MotionFrame>,
) -> impl IntoView {
    let orb = orb_view(state, motion, transparent);
    let eq = eq_view(state, motion);
    let body = v_stack((
        status_line(state, hovered),
        transcript_content(state, motion),
    ))
    .style(|style| {
        style
            .flex_grow(1.0)
            .width_full()
            .height_full()
            .justify_center()
            .gap(2.0)
    });

    h_stack((orb, eq, body))
        .style(move |style| card_style(style, state, hovered, transparent, motion))
        .style(|style| style.gap(12.0))
        .on_event_cont(EventListener::PointerEnter, move |_| hovered.set(true))
        .on_event_cont(EventListener::PointerLeave, move |_| hovered.set(false))
}

fn splash_content(
    state: UiState,
    transparent: bool,
    motion: RwSignal<MotionFrame>,
) -> impl IntoView {
    // Floem の SVG は現在色で単色化するため、2 色のワードマークは生成元を
    // 分けて描く。アプリマークは元 SVG の背景に、白いマークと琥珀の点を重ねる。
    let mark = app_mark_view();
    let wordmark_otoa = svg(theme::WORDMARK_OTOA_SVG.to_string())
        .style(|style| style.absolute().size(92.0, 18.0).color(theme::color::NAVY));
    let wordmark_input = svg(theme::WORDMARK_INPUT_SVG.to_string()).style(|style| {
        style
            .absolute()
            .size(92.0, 18.0)
            .color(theme::color::BRAND_STRONG)
    });
    let wordmark = stack((wordmark_otoa, wordmark_input)).style(|style| style.size(92.0, 18.0));
    let tagline = label(move || splash_status(&state.login_state.get())).style(|style| {
        style
            .font_family(theme::font_family().to_string())
            .font_size(theme::text::CAPTION)
            .font_weight(theme::text::CAPTION_WEIGHT)
            .color(theme::color::INK_SOFT)
    });
    let brand =
        v_stack((wordmark, tagline)).style(|style| style.flex_grow(1.0).justify_center().gap(2.0));
    let chip = splash_route_chip(state);

    h_stack((mark, brand, chip))
        .style(move |style| card_style(style, state, RwSignal::new(false), transparent, motion))
        .style(|style| style.gap(12.0))
}

fn app_mark_view() -> impl IntoView {
    let base = svg(theme::APP_ICON_SVG.to_string())
        .style(|style| style.absolute().size(40.0, 40.0).color(theme::color::BRAND));
    let mark = svg(theme::MARK_SVG.to_string()).style(|style| {
        style
            .absolute()
            .size(40.0, 40.0)
            .color(theme::color::ON_BRAND)
    });
    let dot = empty().style(|style| {
        style
            .absolute()
            .inset_left(25.0)
            .inset_top(25.0)
            .size(6.0, 6.0)
            .border(1.0)
            .border_color(theme::color::ON_BRAND)
            .border_radius(theme::radius::PILL)
            .background(theme::color::AMBER)
    });
    stack((base, mark, dot)).style(|style| style.size(40.0, 40.0))
}

fn card_style(
    style: Style,
    state: UiState,
    hovered: RwSignal<bool>,
    transparent: bool,
    motion: RwSignal<MotionFrame>,
) -> Style {
    let height = card_height(
        state.overlay_mode.get(),
        &state.overlay_committed.get(),
        &state.overlay_partial.get(),
        &state.overlay_error.get(),
    );
    let mut style = style
        .size(CARD_WIDTH, height)
        .padding_left(12.0)
        .padding_right(16.0)
        .padding_vert(12.0)
        .border(1.0)
        .border_color(if hovered.get() {
            theme::color::BRAND_2
        } else {
            theme::color::LINE_SOFT
        })
        .background(theme::color::SURFACE)
        .scale((motion.get().appear_scale * 100.0).pct())
        .transition_color(Transition::ease_in_out(Duration::from_secs_f64(
            theme::motion::HOVER,
        )));
    if transparent {
        style = style
            .border_radius(theme::radius::BAR)
            .box_shadow_blur(theme::shadow::E3.blur)
            .box_shadow_color(theme::shadow::E3.color)
            .box_shadow_spread(theme::shadow::E3.spread)
            .box_shadow_h_offset(theme::shadow::E3.h_offset)
            .box_shadow_v_offset(theme::shadow::E3.v_offset);
    }
    style
}

fn status_line(state: UiState, hovered: RwSignal<bool>) -> impl IntoView {
    h_stack((
        empty().style(move |style| {
            style
                .size(6.0, 6.0)
                .border_radius(theme::radius::PILL)
                .background(overlay_kind_color(state.overlay_mode.get()))
                .transition_background(Transition::ease_in_out(Duration::from_millis(200)))
        }),
        label(move || {
            let mode = state.overlay_mode.get();
            if hovered.get()
                && matches!(
                    mode,
                    OverlayMode::Shown(
                        OverlayKind::Recognizing
                            | OverlayKind::WarmingUp
                            | OverlayKind::Connecting
                            | OverlayKind::Finalizing
                            | OverlayKind::WaitingForResponse
                            | OverlayKind::StartingServer
                    )
                )
            {
                "クリックで待受を止める".to_string()
            } else if hovered.get() && matches!(mode, OverlayMode::Shown(OverlayKind::LoginNeeded))
            {
                "クリックでログイン".to_string()
            } else {
                overlay_status_text(mode, state.settings.get().auto_paste).to_string()
            }
        })
        .style(|style| {
            style
                .font_family(theme::font_family().to_string())
                .font_size(theme::text::MICRO)
                .font_weight(theme::text::MICRO_WEIGHT)
                .line_height(1.0)
                .color(theme::color::INK_SOFT)
        }),
        empty().style(|style| style.flex_grow(1.0)),
        route_chip(state),
    ))
    .style(|style| style.width_full().height(20.0).items_center().gap(6.0))
}

fn transcript_content(state: UiState, motion: RwSignal<MotionFrame>) -> impl IntoView {
    dyn_container(
        move || {
            (
                state.overlay_mode.get(),
                state.overlay_committed.get(),
                state.overlay_partial.get(),
                state.overlay_error.get(),
                state.settings.get().reduce_motion,
            )
        },
        move |(mode, committed, partial, error, reduce_motion)| match mode {
            // ダウンロードの進み具合は committed に載せて渡ってくる。
            OverlayMode::Shown(OverlayKind::Preparing) => text(committed.clone())
                .style(|style| {
                    text_style(
                        style,
                        theme::text::CAPTION,
                        theme::text::CAPTION_WEIGHT,
                        theme::color::INK,
                    )
                })
                .into_any(),
            OverlayMode::Shown(OverlayKind::Error | OverlayKind::Notice) => {
                text(display_error(&error))
                    .style(|style| {
                        text_style(
                            style,
                            theme::text::CAPTION,
                            theme::text::CAPTION_WEIGHT,
                            theme::color::INK,
                        )
                    })
                    .into_any()
            }
            OverlayMode::Shown(OverlayKind::LoginNeeded) => {
                text("クリックするとブラウザでログインします")
                    .style(|style| {
                        text_style(
                            style,
                            theme::text::CAPTION,
                            theme::text::CAPTION_WEIGHT,
                            theme::color::INK,
                        )
                    })
                    .into_any()
            }
            OverlayMode::Shown(OverlayKind::WarmingUp) => text(WARMUP_SUBTITLE)
                .style(|style| {
                    text_style(
                        style,
                        theme::text::CAPTION,
                        theme::text::CAPTION_WEIGHT,
                        theme::color::INK,
                    )
                })
                .into_any(),
            OverlayMode::Shown(OverlayKind::StartingServer) => text(STARTING_SERVER_SUBTITLE)
                .style(|style| {
                    text_style(
                        style,
                        theme::text::CAPTION,
                        theme::text::CAPTION_WEIGHT,
                        theme::color::INK,
                    )
                })
                .into_any(),
            OverlayMode::Shown(OverlayKind::Recognizing)
            | OverlayMode::Shown(OverlayKind::Finalizing)
            | OverlayMode::Shown(OverlayKind::Committed) => transcript_view(
                &committed,
                &partial,
                matches!(
                    mode,
                    OverlayMode::Shown(OverlayKind::Recognizing | OverlayKind::Finalizing)
                ),
                matches!(mode, OverlayMode::Shown(OverlayKind::Recognizing)),
                reduce_motion,
                motion,
            )
            .into_any(),
            _ => empty().into_any(),
        },
    )
}

fn transcript_view(
    committed: &str,
    partial: &str,
    show_caret: bool,
    animate_caret: bool,
    reduce_motion: bool,
    motion: RwSignal<MotionFrame>,
) -> impl IntoView {
    let lines = transcript_lines(committed, partial);
    if lines.is_empty() {
        return v_stack_from_iter(vec![empty().into_any()]).style(|style| style.gap(0.0));
    }

    let line_count = lines.len();
    let line_views = lines
        .into_iter()
        .enumerate()
        .map(|(line_index, pieces)| {
            let mut views = pieces
                .into_iter()
                .map(|piece| {
                    let color = if piece.partial {
                        theme::color::INK_SOFT
                    } else {
                        theme::color::INK
                    };
                    text(piece.text)
                        .style(move |style| {
                            text_style(
                                style,
                                theme::text::TRANSCRIPT,
                                theme::text::TRANSCRIPT_WEIGHT,
                                color,
                            )
                        })
                        .into_any()
                })
                .collect::<Vec<_>>();
            if show_caret && line_index + 1 == line_count {
                views.push(caret_view(motion, reduce_motion || !animate_caret).into_any());
            }
            h_stack_from_iter(views)
                .style(|style| style.text_ellipsis())
                .into_any()
        })
        .collect::<Vec<_>>();
    v_stack_from_iter(line_views).style(|style| style.gap(0.0))
}

fn caret_view(motion: RwSignal<MotionFrame>, static_on: bool) -> impl IntoView {
    empty().style(move |style| {
        let visible = static_on || motion.get().caret_visible;
        style
            .size(2.0, 18.0)
            .border_radius(1.0)
            .background(if visible {
                theme::color::INK
            } else {
                Color::rgba8(0x0f, 0x1a, 0x2e, 0x00)
            })
    })
}

fn route_chip(state: UiState) -> impl IntoView {
    route_chip_with_visibility(state, false)
}

fn splash_route_chip(state: UiState) -> impl IntoView {
    route_chip_with_visibility(state, true)
}

fn route_chip_with_visibility(state: UiState, splash: bool) -> impl IntoView {
    let icon = dyn_container(
        move || state.route_local.get(),
        move |local| match local {
            Some(local) => svg(if local { PC_ICON } else { SERVER_ICON }.to_string())
                .style(|style| style.size(12.0, 12.0))
                .into_any(),
            None => empty().into_any(),
        },
    );
    let caption = label(move || {
        if state.route_local.get().unwrap_or(false) {
            "この PC で認識"
        } else {
            "サーバーで認識"
        }
    });
    let contents = h_stack((icon, caption)).style(|style| {
        style
            .absolute()
            .size(112.0, 20.0)
            .items_center()
            .gap(5.0)
            .padding_horiz(10.0)
            .padding_vert(4.0)
            .font_family(theme::font_family().to_string())
            .font_size(theme::text::MICRO)
            .font_weight(theme::text::MICRO_WEIGHT)
            .line_height(1.0)
            .color(theme::color::BRAND_STRONG)
    });
    let background = svg(CHIP_BACKGROUND_SVG.to_string()).style(|style| {
        style
            .absolute()
            .size(112.0, 20.0)
            .color(theme::color::BRAND_TINT)
    });
    stack((background, contents)).style(move |style| {
        let visible = state.route_local.get().is_some()
            && if splash {
                matches!(state.overlay_mode.get(), OverlayMode::Splash)
            } else {
                matches!(
                    state.overlay_mode.get(),
                    OverlayMode::Shown(
                        OverlayKind::Connecting
                            | OverlayKind::WarmingUp
                            | OverlayKind::Recognizing
                            | OverlayKind::Finalizing
                            | OverlayKind::WaitingForResponse
                            | OverlayKind::StartingServer
                            | OverlayKind::Committed
                    )
                )
            };
        style
            .display(if visible {
                floem::style::Display::Flex
            } else {
                floem::style::Display::None
            })
            .size(112.0, 20.0)
            .align_self(Some(floem::style::AlignItems::Center))
    })
}

fn orb_view(state: UiState, motion: RwSignal<MotionFrame>, transparent: bool) -> impl IntoView {
    let rings = dyn_container(
        move || (state.overlay_mode.get(), state.settings.get().reduce_motion),
        move |(mode, reduce_motion)| {
            if matches!(
                mode,
                OverlayMode::Shown(
                    OverlayKind::Recognizing
                        | OverlayKind::WarmingUp
                        | OverlayKind::Connecting
                        | OverlayKind::WaitingForResponse
                        | OverlayKind::StartingServer
                )
            ) && !reduce_motion
            {
                stack((ring_view(mode, motion, 0), ring_view(mode, motion, 1)))
                    .style(|style| style.absolute().size(ORB_SIZE, ORB_SIZE))
                    .into_any()
            } else {
                empty().into_any()
            }
        },
    );
    let circle = empty().style(move |style| {
        let mode = state.overlay_mode.get();
        let brush = orb_brush(mode);
        let mut style = style
            .absolute()
            .size(ORB_SIZE, ORB_SIZE)
            .border_radius(theme::radius::PILL)
            .background(brush)
            .transition_background(Transition::ease_in_out(Duration::from_millis(200)));
        if matches!(
            mode,
            OverlayMode::Shown(
                OverlayKind::Recognizing
                    | OverlayKind::Finalizing
                    | OverlayKind::WaitingForResponse
                    | OverlayKind::StartingServer
                    | OverlayKind::Committed
            )
        ) {
            let shadow = if transparent {
                theme::shadow::BRAND_GLOW
            } else {
                theme::Shadow {
                    blur: 12.0,
                    h_offset: 0.0,
                    v_offset: 4.0,
                    spread: 0.0,
                    color: theme::shadow::BRAND_GLOW.color,
                }
            };
            style = style
                .box_shadow_blur(shadow.blur)
                .box_shadow_color(shadow.color)
                .box_shadow_spread(shadow.spread)
                .box_shadow_h_offset(shadow.h_offset)
                .box_shadow_v_offset(shadow.v_offset);
        }
        style
    });
    let icon = container(dyn_container(
        move || state.overlay_mode.get(),
        move |mode| {
            svg(orb_icon(mode).to_string())
                .style(|style| style.size(18.0, 18.0).color(theme::color::ON_BRAND))
                .into_any()
        },
    ))
    .style(|style| {
        style
            .absolute()
            .size(ORB_SIZE, ORB_SIZE)
            .items_center()
            .justify_center()
    });
    stack((rings, circle, icon)).style(|style| style.size(ORB_SIZE, ORB_SIZE))
}

fn ring_view(mode: OverlayMode, motion: RwSignal<MotionFrame>, ring_index: usize) -> impl IntoView {
    let base_alpha = if matches!(
        mode,
        OverlayMode::Shown(
            OverlayKind::WarmingUp | OverlayKind::Connecting | OverlayKind::StartingServer
        )
    ) {
        0.35
    } else {
        0.55
    };
    empty().style(move |style| {
        let frame = motion.get();
        let diameter = frame.ring_diameters[ring_index];
        let alpha = frame.ring_alphas[ring_index] * base_alpha;
        style
            .absolute()
            .size(diameter, diameter)
            .inset_left((ORB_SIZE - diameter) / 2.0)
            .inset_top((ORB_SIZE - diameter) / 2.0)
            .border(2.0)
            .border_radius(theme::radius::PILL)
            .border_color(theme::color::BRAND.multiply_alpha(alpha as f32))
    })
}

fn eq_view(state: UiState, motion: RwSignal<MotionFrame>) -> impl IntoView {
    dyn_container(
        move || state.overlay_mode.get(),
        move |mode| {
            if matches!(
                mode,
                OverlayMode::Shown(
                    OverlayKind::Error | OverlayKind::Notice | OverlayKind::LoginNeeded
                )
            ) {
                empty().style(|style| style.size(0.0, 0.0)).into_any()
            } else {
                let bars = EQ_BASE_HEIGHTS
                    .into_iter()
                    .enumerate()
                    .map(|(index, base)| eq_bar(state, motion, index, base).into_any())
                    .collect::<Vec<_>>();
                h_stack_from_iter(bars)
                    .style(|style| {
                        style
                            .size(EQ_WIDTH, EQ_HEIGHT)
                            .items_center()
                            .justify_center()
                            .gap(EQ_GAP)
                    })
                    .into_any()
            }
        },
    )
}

fn eq_bar(state: UiState, motion: RwSignal<MotionFrame>, index: usize, base: f64) -> impl IntoView {
    empty().style(move |style| {
        let mode = state.overlay_mode.get();
        let reduce_motion = state.settings.get().reduce_motion;
        let height = match mode {
            OverlayMode::Shown(OverlayKind::Recognizing) => motion.get().eq_heights[index],
            OverlayMode::Shown(OverlayKind::Committed) => base * 0.35,
            OverlayMode::Shown(
                OverlayKind::Preparing
                | OverlayKind::Error
                | OverlayKind::Notice
                | OverlayKind::LoginNeeded,
            )
            | OverlayMode::Hidden
            | OverlayMode::Splash => 0.0,
            OverlayMode::Shown(
                OverlayKind::WarmingUp
                | OverlayKind::Connecting
                | OverlayKind::Finalizing
                | OverlayKind::WaitingForResponse
                | OverlayKind::StartingServer,
            ) => {
                if reduce_motion {
                    base
                } else {
                    motion.get().eq_heights[index]
                }
            }
        };
        style
            .width(4.0)
            .height(height.round().clamp(1.0, EQ_HEIGHT))
            .border_radius(2.0)
            .background(theme::color::BRAND)
    })
}

fn eq_heights(mode: OverlayMode, reduce_motion: bool, smooth_level: f64, elapsed: f64) -> [f64; 5] {
    EQ_BASE_HEIGHTS
        .into_iter()
        .enumerate()
        .map(|(index, base)| match mode {
            OverlayMode::Shown(OverlayKind::Recognizing) => {
                let level = (smooth_level / 0.5).clamp(0.0, 1.0);
                let modulation = if reduce_motion {
                    1.0
                } else {
                    let omega = std::f64::consts::TAU / theme::motion::EQ_IDLE;
                    0.85 + 0.15 * (omega * elapsed + EQ_PHASES[index]).sin()
                };
                (base * (0.35 + 0.65 * level) * modulation).round()
            }
            OverlayMode::Shown(
                OverlayKind::WarmingUp
                | OverlayKind::Connecting
                | OverlayKind::Finalizing
                | OverlayKind::WaitingForResponse
                | OverlayKind::StartingServer,
            ) => {
                if reduce_motion {
                    base
                } else {
                    let omega = std::f64::consts::TAU / theme::motion::EQ_IDLE;
                    (base * (0.775 + 0.225 * (omega * elapsed).sin())).round()
                }
            }
            OverlayMode::Shown(OverlayKind::Committed) => (base * 0.35).round(),
            OverlayMode::Shown(
                OverlayKind::Preparing
                | OverlayKind::Error
                | OverlayKind::Notice
                | OverlayKind::LoginNeeded,
            )
            | OverlayMode::Hidden
            | OverlayMode::Splash => 0.0,
        })
        .collect::<Vec<_>>()
        .try_into()
        .expect("EQ_BASE_HEIGHTS always has five entries")
}

fn ring_values(mode: OverlayMode, elapsed: f64, delay: f64) -> (f64, f64) {
    let in_ring_state = matches!(
        mode,
        OverlayMode::Shown(
            OverlayKind::Recognizing
                | OverlayKind::WarmingUp
                | OverlayKind::Connecting
                | OverlayKind::WaitingForResponse
                | OverlayKind::StartingServer
        )
    );
    if !in_ring_state || elapsed < delay {
        return (ORB_SIZE, 0.0);
    }
    let progress = ((elapsed - delay) % theme::motion::RING_PULSE) / theme::motion::RING_PULSE;
    let eased = 1.0 - (1.0 - progress).powi(3);
    (ORB_SIZE + 36.0 * eased, 1.0 - progress)
}

fn text_style(style: Style, size: f32, weight: floem::text::Weight, color: Color) -> Style {
    style
        .font_family(theme::font_family().to_string())
        .font_size(size)
        .font_weight(weight)
        .line_height(1.4)
        .color(color)
}

fn orb_brush(mode: OverlayMode) -> Brush {
    match mode {
        OverlayMode::Shown(
            OverlayKind::Preparing
            | OverlayKind::WarmingUp
            | OverlayKind::Connecting
            | OverlayKind::StartingServer,
        ) => theme::color::BRAND_2.into(),
        OverlayMode::Shown(OverlayKind::Error) => theme::color::ERROR.into(),
        OverlayMode::Shown(OverlayKind::Notice) => theme::color::AMBER.into(),
        OverlayMode::Shown(OverlayKind::LoginNeeded) => theme::color::NAVY_SOFT.into(),
        OverlayMode::Shown(
            OverlayKind::Recognizing
            | OverlayKind::Finalizing
            | OverlayKind::WaitingForResponse
            | OverlayKind::Committed,
        ) => theme::grad_brand(),
        OverlayMode::Splash | OverlayMode::Hidden => theme::color::NAVY_SOFT.into(),
    }
}

fn overlay_kind_color(mode: OverlayMode) -> Color {
    let OverlayMode::Shown(kind) = mode else {
        return theme::color::NAVY_SOFT;
    };
    match kind {
        OverlayKind::Preparing
        | OverlayKind::WarmingUp
        | OverlayKind::Connecting
        | OverlayKind::StartingServer => theme::color::AMBER,
        OverlayKind::Recognizing => theme::color::BRAND,
        OverlayKind::Finalizing | OverlayKind::WaitingForResponse => theme::color::CYAN,
        OverlayKind::Committed => theme::color::BRAND,
        OverlayKind::Notice => theme::color::AMBER,
        OverlayKind::Error => theme::color::ERROR,
        OverlayKind::LoginNeeded => theme::color::NAVY_SOFT,
    }
}

fn orb_icon(mode: OverlayMode) -> &'static str {
    match mode {
        OverlayMode::Shown(OverlayKind::Error) => ERROR_ICON,
        OverlayMode::Shown(OverlayKind::Committed) => CHECK_ICON,
        OverlayMode::Shown(OverlayKind::LoginNeeded) => LOCK_ICON,
        _ => MIC_ICON,
    }
}

fn splash_status(login_state: &LoginState) -> &'static str {
    match login_state {
        LoginState::LoggedIn { .. } | LoginState::NotRequired => {
            "話すと、カーソル位置に貼り付きます"
        }
        LoginState::LoggedOut | LoginState::Failed { .. } => "クリックしてログインしてください",
        LoginState::InProgress => "ブラウザでログインを続けてください",
    }
}

fn overlay_status_text(mode: OverlayMode, auto_paste: bool) -> &'static str {
    let OverlayMode::Shown(kind) = mode else {
        return "";
    };
    match kind {
        OverlayKind::Preparing => "準備しています",
        OverlayKind::WarmingUp => WARMUP_TITLE,
        OverlayKind::Connecting => "つないでいます",
        OverlayKind::Recognizing => "聞いています",
        OverlayKind::Finalizing => "文字にしています",
        OverlayKind::WaitingForResponse => WAITING_FOR_RESPONSE_TITLE,
        OverlayKind::StartingServer => STARTING_SERVER_TITLE,
        OverlayKind::Committed => {
            if auto_paste {
                "貼り付けました"
            } else {
                "コピーしました"
            }
        }
        OverlayKind::Notice => "お知らせ",
        OverlayKind::Error => "うまくいきませんでした",
        OverlayKind::LoginNeeded => "ログインが必要です",
    }
}

fn overlay_summary(mode: OverlayMode) -> &'static str {
    match mode {
        OverlayMode::Hidden => "hidden",
        OverlayMode::Splash => "splash",
        OverlayMode::Shown(kind) => match kind {
            OverlayKind::Preparing => "preparing",
            OverlayKind::WarmingUp => "warming-up",
            OverlayKind::Connecting => "connecting",
            OverlayKind::Recognizing => "recognizing",
            OverlayKind::Finalizing => "finalizing",
            OverlayKind::WaitingForResponse => "waiting-for-response",
            OverlayKind::StartingServer => "starting-server",
            OverlayKind::Committed => "committed",
            OverlayKind::Notice => "notice",
            OverlayKind::Error => "error",
            OverlayKind::LoginNeeded => "login-needed",
        },
    }
}

#[derive(Clone)]
struct TextPiece {
    text: String,
    partial: bool,
}

fn transcript_lines(committed: &str, partial: &str) -> Vec<Vec<TextPiece>> {
    let mut chars = committed
        .chars()
        .map(|character| (character, false))
        .chain(partial.chars().map(|character| (character, true)))
        .collect::<Vec<_>>();
    if chars.len() > TRANSCRIPT_MAX_CHARS {
        let mut tail = Vec::with_capacity(TRANSCRIPT_MAX_CHARS);
        tail.push(('…', false));
        tail.extend_from_slice(&chars[chars.len() - (TRANSCRIPT_MAX_CHARS - 1)..]);
        chars = tail;
    }
    if chars.len() > TRANSCRIPT_LINE_LIMIT
        && !chars[..TRANSCRIPT_LINE_LIMIT]
            .iter()
            .any(|(character, _)| *character == '\n')
    {
        chars.insert(TRANSCRIPT_LINE_LIMIT, ('\n', false));
    }

    let mut lines: Vec<Vec<(char, bool)>> = vec![Vec::new()];
    for (character, partial) in chars {
        if character == '\n' {
            if lines.len() == 2 {
                break;
            }
            lines.push(Vec::new());
        } else if lines.len() <= 2 {
            lines.last_mut().unwrap().push((character, partial));
        }
    }
    lines
        .into_iter()
        .map(|line| {
            let mut pieces = Vec::<TextPiece>::new();
            for (character, partial) in line {
                if let Some(last) = pieces.last_mut() {
                    if last.partial == partial {
                        last.text.push(character);
                        continue;
                    }
                }
                pieces.push(TextPiece {
                    text: character.to_string(),
                    partial,
                });
            }
            pieces
        })
        .collect()
}

fn has_two_transcript_lines(committed: &str, partial: &str) -> bool {
    transcript_lines(committed, partial).len() > 1
}

#[allow(dead_code)]
fn display_text(text: &str) -> String {
    let text_chars = text.chars().collect::<Vec<_>>();
    let mut chars = if text_chars.len() > TRANSCRIPT_MAX_CHARS {
        let mut tail = Vec::with_capacity(TRANSCRIPT_MAX_CHARS);
        tail.push('…');
        tail.extend_from_slice(&text_chars[text_chars.len() - (TRANSCRIPT_MAX_CHARS - 1)..]);
        tail
    } else {
        text_chars
    };
    if chars.len() > TRANSCRIPT_LINE_LIMIT && !chars[..TRANSCRIPT_LINE_LIMIT].contains(&'\n') {
        chars.insert(TRANSCRIPT_LINE_LIMIT, '\n');
    }
    chars.into_iter().collect()
}

/// エラー文を折り返す。転記テキストと違い、大事なのは先頭であるため、
/// 末尾ではなく先頭を残して 34 文字ごとに折り返す。
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

const MIC_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M12 3a3 3 0 0 0-3 3v5a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3Z" fill="none" stroke="#fff" stroke-width="1.8" stroke-linecap="round"/><path d="M6.5 10.5a5.5 5.5 0 0 0 11 0M12 16v4M9 20h6" fill="none" stroke="#fff" stroke-width="1.8" stroke-linecap="round"/></svg>"##;
const CHECK_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="m5 12 4.5 4.5L19 7" fill="none" stroke="#fff" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const ERROR_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M12 5v8M12 17v1" fill="none" stroke="#fff" stroke-width="2.2" stroke-linecap="round"/></svg>"##;
const LOCK_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect x="5" y="10" width="14" height="10" rx="2" fill="none" stroke="#fff" stroke-width="1.8"/><path d="M8 10V7a4 4 0 0 1 8 0v3" fill="none" stroke="#fff" stroke-width="1.8" stroke-linecap="round"/></svg>"##;
const PC_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="12" rx="1.5" fill="none" stroke="#1c5cbb" stroke-width="1.8"/><path d="M8 20h8M12 16v4" fill="none" stroke="#1c5cbb" stroke-width="1.8" stroke-linecap="round"/></svg>"##;
const SERVER_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect x="4" y="3" width="16" height="18" rx="2" fill="none" stroke="#1c5cbb" stroke-width="1.8"/><path d="M8 7h8M8 12h8M8 17h5" fill="none" stroke="#1c5cbb" stroke-width="1.8" stroke-linecap="round"/></svg>"##;
const CHIP_BACKGROUND_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 112 20"><rect x="0" y="0" width="112" height="20" rx="10" fill="#ffffff"/></svg>"##;

#[cfg(test)]
mod tests {
    use super::{
        display_text, overlay_status_text, OverlayMode, STARTING_SERVER_SUBTITLE,
        STARTING_SERVER_TITLE, WAITING_FOR_RESPONSE_TITLE, WARMUP_SUBTITLE, WARMUP_TITLE,
    };
    use crate::controller::OverlayKind;

    #[test]
    fn warming_overlay_uses_the_required_title_and_instruction() {
        assert_eq!(WARMUP_TITLE, "起動中");
        assert_eq!(WARMUP_SUBTITLE, "まだ話さないでください");
        assert_eq!(
            overlay_status_text(OverlayMode::Shown(OverlayKind::WarmingUp), true),
            WARMUP_TITLE
        );
    }

    #[test]
    fn waiting_overlays_use_the_required_text() {
        assert_eq!(WAITING_FOR_RESPONSE_TITLE, "認識中");
        assert_eq!(STARTING_SERVER_TITLE, "サーバーを起動しています");
        assert_eq!(STARTING_SERVER_SUBTITLE, "しばらくお待ちください");
        assert_eq!(
            overlay_status_text(OverlayMode::Shown(OverlayKind::WaitingForResponse), true),
            WAITING_FOR_RESPONSE_TITLE
        );
        assert_eq!(
            overlay_status_text(OverlayMode::Shown(OverlayKind::StartingServer), true),
            STARTING_SERVER_TITLE
        );
    }

    #[test]
    fn display_text_keeps_exactly_fifty_two_characters() {
        let text = "a".repeat(52);
        assert_eq!(
            display_text(&text),
            format!("{}\n{}", "a".repeat(26), "a".repeat(26))
        );
    }

    #[test]
    fn display_text_keeps_the_tail_after_fifty_two_characters() {
        let text = format!("discard{}", "z".repeat(51));
        let displayed = display_text(&text);

        assert_eq!(
            displayed,
            format!("…{}\n{}", "z".repeat(25), "z".repeat(26))
        );
        assert!(!displayed.contains("discard"));
    }

    #[test]
    fn display_text_truncates_japanese_at_character_boundaries() {
        let text = format!("捨てる{}末尾", "語".repeat(52));
        let displayed = display_text(&text);

        assert!(displayed.starts_with('…'));
        assert!(displayed.ends_with("末尾"));
        assert_eq!(
            displayed
                .chars()
                .filter(|character| *character != '\n')
                .count(),
            52
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
