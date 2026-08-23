use super::{theme, UiState};
use crate::controller::{ControllerCommand, LevelStatus, LoginState};
use crate::settings::Settings;
use crate::wiring::SettingsPage;
use crossbeam_channel::Sender;
use floem::{
    action::exec_after,
    close_window,
    prelude::*,
    reactive::{create_effect, RwSignal, SignalGet, SignalUpdate},
    style::{Style, TextOverflow, Transition},
    unit::{Pct, UnitExt},
    views::{clip, dropdown::Dropdown, dyn_container, img, scroll, slider, svg, RadioButton},
    window::WindowId,
    AnyView,
};
use otoa_input_platform::AudioCapture;
use std::str::FromStr;
use std::time::{Duration, Instant};

const LEVEL_TICK: Duration = Duration::from_millis(16);

const GEAR_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M9.7 3.2h4.6l.7 2.2a7.3 7.3 0 0 1 1.5.9l2.2-.7 2.3 4-1.7 1.5a7 7 0 0 1 0 1.8l1.7 1.5-2.3 4-2.2-.7a7.3 7.3 0 0 1-1.5.9l-.7 2.2H9.7L9 18.6a7.3 7.3 0 0 1-1.5-.9l-2.2.7-2.3-4 1.7-1.5a7 7 0 0 1 0-1.8L3 9.6l2.3-4 2.2.7A7.3 7.3 0 0 1 9 5.4l.7-2.2Z" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/><circle cx="12" cy="12" r="2.7" fill="none" stroke="currentColor" stroke-width="1.6"/></svg>"##;
const MIC_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M12 3a3 3 0 0 0-3 3v5a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3Z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/><path d="M6.5 10.5a5.5 5.5 0 0 0 11 0M12 16v4M9 20h6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg>"##;
const WAVE_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M3 13h3l2-6 4 12 2-7 2 3h5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const SLIDERS_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M4 6h16M4 12h16M4 18h16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/><circle cx="9" cy="6" r="2" fill="none" stroke="currentColor" stroke-width="1.8"/><circle cx="15" cy="12" r="2" fill="none" stroke="currentColor" stroke-width="1.8"/><circle cx="10" cy="18" r="2" fill="none" stroke="currentColor" stroke-width="1.8"/></svg>"##;
const PERSON_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="7" r="3" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M5 21a7 7 0 0 1 14 0" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg>"##;
const INFO_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M12 10.5v5M12 7.5v.2" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg>"##;

#[derive(Clone)]
struct MicrophoneChoice {
    id: String,
    label: String,
}

#[derive(Clone, PartialEq, Eq)]
struct AsrEngineChoice {
    id: String,
    label: String,
}

impl std::fmt::Display for AsrEngineChoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.label.fmt(formatter)
    }
}

impl std::fmt::Display for MicrophoneChoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.label.fmt(formatter)
    }
}

#[derive(Clone, Copy)]
struct FormState {
    server_url: RwSignal<String>,
    language: RwSignal<String>,
    listening_enabled: RwSignal<bool>,
    auto_paste: RwSignal<bool>,
    selected_asr_engine: RwSignal<AsrEngineChoice>,
    gain: RwSignal<Pct>,
    selected_microphone: RwSignal<MicrophoneChoice>,
    vad_model_path: RwSignal<String>,
    vad_threshold: RwSignal<Pct>,
    vad_min_silence_ms: RwSignal<String>,
    vad_min_speech_ms: RwSignal<String>,
    preroll_ms: RwSignal<String>,
    endpoint_max_delay_ms: RwSignal<String>,
    endpoint_sensitivity: RwSignal<String>,
    endpoint_latency_level: RwSignal<String>,
    idle_close_sec: RwSignal<String>,
    commit_hold_ms: RwSignal<Pct>,
    splash_ms: RwSignal<String>,
    overlay_position: RwSignal<String>,
    overlay_transparent: RwSignal<String>,
    reduce_motion: RwSignal<bool>,
}

pub fn view(
    settings: Settings,
    state: UiState,
    commands: Sender<ControllerCommand>,
    window_id: WindowId,
    initial_page: SettingsPage,
) -> impl IntoView {
    let server_url = RwSignal::new(settings.server_url.clone());
    let language = RwSignal::new(language_label(&settings.language_hints));
    let listening_enabled = RwSignal::new(settings.listening_enabled);
    let auto_paste = RwSignal::new(settings.auto_paste);
    let asr_engine_choices = asr_engine_choices();
    let selected_asr_engine = RwSignal::new(
        asr_engine_choices
            .iter()
            .find(|choice| choice.id == settings.asr_engine)
            .cloned()
            .unwrap_or_else(|| asr_engine_choices[0].clone()),
    );
    let gain = RwSignal::new(gain_to_pct(settings.input_gain));
    let vad_model_path = RwSignal::new(settings.vad_model_path.clone());
    let vad_threshold = RwSignal::new(threshold_to_pct(settings.vad_threshold));
    let vad_min_silence_ms = RwSignal::new(settings.vad_min_silence_ms.to_string());
    let vad_min_speech_ms = RwSignal::new(settings.vad_min_speech_ms.to_string());
    let preroll_ms = RwSignal::new(settings.preroll_ms.to_string());
    let endpoint_max_delay_ms = RwSignal::new(settings.endpoint_max_delay_ms.to_string());
    let endpoint_sensitivity = RwSignal::new(settings.endpoint_sensitivity.to_string());
    let endpoint_latency_level = RwSignal::new(settings.endpoint_latency_level.to_string());
    let idle_close_sec = RwSignal::new(settings.idle_close_sec.to_string());
    let commit_hold_ms = RwSignal::new(commit_hold_to_pct(settings.commit_hold_ms));
    let splash_ms = RwSignal::new(settings.splash_ms.to_string());
    let overlay_position =
        RwSignal::new(overlay_position_label(&settings.overlay_position).to_string());
    let overlay_transparent =
        RwSignal::new(overlay_transparent_label(&settings.overlay_transparent).to_string());
    let reduce_motion = RwSignal::new(settings.reduce_motion);

    let microphone_choices = microphone_choices(&settings.microphone);
    let selected_microphone = RwSignal::new(
        microphone_choices
            .iter()
            .find(|choice| {
                choice.id == settings.microphone
                    || (settings.microphone.is_empty() && choice.id.is_empty())
            })
            .cloned()
            .unwrap_or_else(|| microphone_choices[0].clone()),
    );

    let form = FormState {
        server_url,
        language,
        listening_enabled,
        auto_paste,
        selected_asr_engine,
        gain,
        selected_microphone,
        vad_model_path,
        vad_threshold,
        vad_min_silence_ms,
        vad_min_speech_ms,
        preroll_ms,
        endpoint_max_delay_ms,
        endpoint_sensitivity,
        endpoint_latency_level,
        idle_close_sec,
        commit_hold_ms,
        splash_ms,
        overlay_position,
        overlay_transparent,
        reduce_motion,
    };

    let page = RwSignal::new(
        if initial_page == SettingsPage::Account && !state.account_settings_available {
            SettingsPage::General
        } else {
            initial_page
        },
    );
    let smooth_level = RwSignal::new(state.level.get_untracked());
    let level_ticker_running = RwSignal::new(true);
    let level_ticker_generation = RwSignal::new(0_u64);
    let ticker = LevelTicker {
        state,
        page,
        smooth_level,
        running: level_ticker_running,
        generation: level_ticker_generation,
        generation_value: 0,
    };
    create_effect(move |_| {
        let selected_page = page.get();
        let generation_value = level_ticker_generation.get_untracked().wrapping_add(1);
        level_ticker_generation.set(generation_value);
        if selected_page == SettingsPage::Microphone {
            LevelTicker {
                generation_value,
                ..ticker
            }
            .schedule(Instant::now(), smooth_level.get_untracked());
        }
    });

    let save_message =
        RwSignal::new("保存すると反映されます。認識エンジンは再起動後。".to_string());
    let save_message_error = RwSignal::new(false);
    let save = {
        let commands = commands.clone();
        let original = settings.clone();
        move || {
            let mut next = original.clone();
            next.server_url = form.server_url.get_untracked();
            next.language_hints = language_hints(&form.language.get_untracked());
            next.listening_enabled = form.listening_enabled.get_untracked();
            next.auto_paste = form.auto_paste.get_untracked();
            next.asr_engine = form.selected_asr_engine.get_untracked().id;
            next.input_gain = gain_from_pct(form.gain.get_untracked());
            next.vad_model_path = form.vad_model_path.get_untracked();
            next.vad_threshold = threshold_from_pct(form.vad_threshold.get_untracked());
            next.vad_min_silence_ms = parse_or(
                &form.vad_min_silence_ms.get_untracked(),
                original.vad_min_silence_ms,
            );
            next.vad_min_speech_ms = parse_or(
                &form.vad_min_speech_ms.get_untracked(),
                original.vad_min_speech_ms,
            );
            next.preroll_ms = parse_or(&form.preroll_ms.get_untracked(), original.preroll_ms);
            next.endpoint_max_delay_ms = parse_or(
                &form.endpoint_max_delay_ms.get_untracked(),
                original.endpoint_max_delay_ms,
            );
            next.endpoint_sensitivity = parse_or(
                &form.endpoint_sensitivity.get_untracked(),
                original.endpoint_sensitivity,
            );
            next.endpoint_latency_level = parse_or(
                &form.endpoint_latency_level.get_untracked(),
                original.endpoint_latency_level,
            );
            next.idle_close_sec = parse_or(
                &form.idle_close_sec.get_untracked(),
                original.idle_close_sec,
            );
            next.commit_hold_ms = commit_hold_from_pct(form.commit_hold_ms.get_untracked());
            next.splash_ms = parse_or(&form.splash_ms.get_untracked(), original.splash_ms);
            next.overlay_position =
                overlay_position_value(&form.overlay_position.get_untracked()).to_string();
            next.overlay_transparent =
                overlay_transparent_value(&form.overlay_transparent.get_untracked()).to_string();
            next.reduce_motion = form.reduce_motion.get_untracked();
            next.microphone = form.selected_microphone.get_untracked().id;

            if let Err(error) = crate::settings_io::save(&next) {
                tracing::error!("設定の保存に失敗しました: {error:#}");
                save_message.set(format!("保存できませんでした: {error:#}"));
                save_message_error.set(true);
                return;
            }
            state.settings.set(next.clone());
            let _ = commands.send(ControllerCommand::UpdateSettings(Box::new(next)));
            close_window(window_id);
        }
    };

    let rail = rail_view(page, state);
    let pages = dyn_container(
        move || page.get(),
        move |selected_page| {
            page_view(
                selected_page,
                form,
                state,
                commands.clone(),
                settings.clone(),
                smooth_level,
            )
        },
    );
    let right = scroll(v_stack((pages,)).style(|style| style.width_full().items_center()))
        .scroll_style(|style| {
            style
                .handle_background(theme::color::LINE)
                .handle_border_radius(100.0.pct())
                .handle_rounded(true)
                .handle_thickness(6.0)
                .track_background(floem::peniko::Color::TRANSPARENT)
                .track_border(0.0)
                .track_thickness(6.0)
        })
        .style(|style| {
            style
                .width_full()
                .height_full()
                .flex_grow(1.0)
                .min_height(0.0)
                .padding(theme::space::XL)
                .background(theme::color::BG)
                .class(floem::views::scroll::Handle, |style| {
                    style
                        .background(theme::color::LINE)
                        .border_radius(100.0.pct())
                        .hover(|style| style.background(theme::color::NAVY_SOFT))
                })
                .class(floem::views::scroll::Track, |style| {
                    style.background(floem::peniko::Color::TRANSPARENT)
                })
        });

    let header = h_stack((
        app_mark(28.0),
        label(|| "Otoa Input の設定".to_string()).style(|style| {
            style
                .font_family(theme::font_family().to_string())
                .font_size(theme::text::TITLE)
                .font_weight(theme::text::TITLE_WEIGHT)
                .color(theme::color::INK)
        }),
    ))
    .style(|style| {
        style
            .height(56.0)
            .min_height(56.0)
            .items_center()
            .gap(theme::space::MD)
            .padding_horiz(theme::space::XL)
            .background(theme::color::BG)
            .border_bottom(1.0)
            .border_color(theme::color::LINE)
    });

    let footer = h_stack((
        label(move || save_message.get()).style(move |style| {
            style
                .flex_grow(1.0)
                .min_width(0.0)
                .font_family(theme::font_family().to_string())
                .font_size(theme::text::CAPTION)
                .font_weight(theme::text::CAPTION_WEIGHT)
                .line_height(1.4)
                .color(if save_message_error.get() {
                    theme::color::ERROR
                } else {
                    theme::color::INK_SOFT
                })
        }),
        button("キャンセル")
            .action(move || close_window(window_id))
            .style(secondary_button_style),
        button("保存").action(save).style(primary_button_style),
    ))
    .style(|style| {
        style
            .height(64.0)
            .min_height(64.0)
            .items_center()
            .gap(theme::space::SM)
            .padding_horiz(theme::space::XL)
            .background(theme::color::SURFACE)
            .border_top(1.0)
            .border_color(theme::color::LINE)
    });

    container(v_stack((
        header,
        h_stack((rail, right)).style(|style| style.width_full().flex_grow(1.0).min_height(0.0)),
        footer,
    )))
    .style(|style| {
        style
            .width_full()
            .height_full()
            .font_family(theme::font_family().to_string())
            .background(theme::color::BG)
            .color(theme::color::INK)
    })
    .on_cleanup(move || {
        level_ticker_running.set(false);
        level_ticker_generation.update(|generation| *generation += 1);
        state.settings_window_open.set(false);
    })
}

fn rail_view(page: RwSignal<SettingsPage>, state: UiState) -> impl IntoView {
    let mut items = vec![
        rail_item(page, SettingsPage::General, "一般", GEAR_ICON),
        rail_item(page, SettingsPage::Microphone, "マイク", MIC_ICON),
        rail_item(page, SettingsPage::Recognition, "認識", WAVE_ICON),
        rail_item(page, SettingsPage::Advanced, "詳細", SLIDERS_ICON),
    ];
    if state.account_settings_available {
        items.push(rail_item(
            page,
            SettingsPage::Account,
            "アカウント",
            PERSON_ICON,
        ));
    }
    items.push(rail_item(
        page,
        SettingsPage::About,
        "このアプリについて",
        INFO_ICON,
    ));
    v_stack_from_iter(items).style(|style| {
        style
            .width(184.0)
            .height_full()
            .padding_horiz(12.0)
            .padding_vert(theme::space::LG)
            .gap(theme::space::XS)
            .background(theme::color::BG)
    })
}

fn rail_item(
    page: RwSignal<SettingsPage>,
    target: SettingsPage,
    title: &'static str,
    icon: &'static str,
) -> AnyView {
    let contents = h_stack((
        svg(icon.to_string()).style(move |style| {
            style.size(16.0, 16.0).color(if page.get() == target {
                theme::color::BRAND_STRONG
            } else {
                theme::color::INK_SOFT
            })
        }),
        static_label(title).style(move |style| {
            style
                .font_family(theme::font_family().to_string())
                .font_size(theme::text::BODY)
                .font_weight(theme::text::BODY_SOFT_WEIGHT)
                .width(136.0)
                .color(if page.get() == target {
                    theme::color::BRAND_STRONG
                } else {
                    theme::color::INK_SOFT
                })
        }),
    ))
    .style(|style| style.size(160.0, 40.0).items_center().gap(theme::space::SM));
    container(contents)
        .on_click_stop(move |_| page.set(target))
        .style(move |style| {
            style
                .size(160.0, 40.0)
                .items_center()
                .gap(theme::space::SM)
                .padding_horiz(0.0)
                .border(0.0)
                .border_radius(12.0)
                .background(if page.get() == target {
                    theme::color::BRAND_TINT
                } else {
                    theme::color::BG
                })
                .color(if page.get() == target {
                    theme::color::BRAND_STRONG
                } else {
                    theme::color::INK_SOFT
                })
                .hover(move |style| {
                    style.background(if page.get() == target {
                        theme::color::BRAND_TINT
                    } else {
                        theme::color::SURFACE
                    })
                })
        })
        .into_any()
}

fn page_view(
    page: SettingsPage,
    form: FormState,
    state: UiState,
    commands: Sender<ControllerCommand>,
    settings: Settings,
    smooth_level: RwSignal<f64>,
) -> AnyView {
    match page {
        SettingsPage::General => general_page(form).into_any(),
        SettingsPage::Microphone => microphone_page(form, state, smooth_level).into_any(),
        SettingsPage::Recognition => recognition_page(form).into_any(),
        SettingsPage::Advanced => advanced_page(form).into_any(),
        SettingsPage::Account => account_page(state, commands).into_any(),
        SettingsPage::About => about_page(form, state, settings).into_any(),
    }
}

fn general_page(form: FormState) -> impl IntoView {
    section_page(
        "一般",
        "待受と貼り付けのふるまい。",
        vec![
            setting_row(
                "起動時に待受を始める",
                caption("起動したらすぐ話せる状態にします"),
                toggle_control(form.listening_enabled),
            )
            .into_any(),
            setting_row(
                "確定後に自動で貼り付ける",
                caption("オフのときはクリップボードに置くだけにします"),
                toggle_control(form.auto_paste),
            )
            .into_any(),
            setting_row(
                "言語",
                caption("同梱サーバーは日本語だけです。接続先によっては効きます"),
                Dropdown::new_rw(
                    form.language,
                    ["日本語".to_string(), "英語".to_string(), "自動".to_string()],
                )
                .style(dropdown_style),
            )
            .into_any(),
            setting_row(
                "バーの位置",
                caption("話している間に出る入力バーの場所"),
                Dropdown::new_rw(
                    form.overlay_position,
                    [
                        "中央".to_string(),
                        "画面の下".to_string(),
                        "画面の上".to_string(),
                    ],
                )
                .style(dropdown_style),
            )
            .into_any(),
            setting_row(
                "貼り付けたあとに結果を見せる時間",
                caption("0 にすると貼り付けと同時に消えます"),
                h_stack((
                    slider_control(form.commit_hold_ms),
                    label(move || {
                        format!("{} ms", commit_hold_from_pct(form.commit_hold_ms.get()))
                    })
                    .style(caption_style),
                ))
                .style(|style| style.width(240.0).items_center().gap(theme::space::MD)),
            )
            .into_any(),
            setting_row(
                "起動時にロゴを見せる時間",
                caption(""),
                numeric_field::<u32>(form.splash_ms, "2500", "ms"),
            )
            .into_any(),
            setting_row(
                "動きを減らす",
                caption("光の輪や点滅を止めます"),
                toggle_control(form.reduce_motion),
            )
            .into_any(),
        ],
    )
}

fn microphone_page(form: FormState, state: UiState, smooth_level: RwSignal<f64>) -> impl IntoView {
    let microphone_choices = microphone_choices(&form.selected_microphone.get_untracked().id);
    section_page(
        "マイク",
        "使うマイクと入力レベルを調整します。",
        vec![
            setting_row(
                "入力デバイス",
                caption(""),
                Dropdown::new(
                    move || form.selected_microphone.get(),
                    microphone_choices.clone(),
                )
                .on_accept(move |choice| form.selected_microphone.set(choice))
                .style(dropdown_style),
            )
            .into_any(),
            setting_row(
                "入力ゲイン",
                caption("小さすぎると拾えず、大きすぎると歪みます"),
                h_stack((
                    slider_control(form.gain),
                    label(move || format!("×{:.1}", gain_from_pct(form.gain.get())))
                        .style(caption_style),
                ))
                .style(|style| style.width(240.0).items_center().gap(theme::space::MD)),
            )
            .into_any(),
            setting_row(
                "入力レベル",
                caption("いま話して、青の範囲に入るように調整します"),
                level_meter(state, smooth_level),
            )
            .into_any(),
        ],
    )
}

fn recognition_page(form: FormState) -> impl IntoView {
    section_page(
        "認識",
        "音声を文字にする方法と、発話の区切りを調整します。",
        vec![
            engine_setting_row(form.selected_asr_engine).into_any(),
            setting_row(
                "発話の区切り（無音）",
                numeric_description::<u32>(
                    form.vad_min_silence_ms,
                    "これだけ黙ると、そこまでを文字にして貼り付けます",
                ),
                numeric_field::<u32>(form.vad_min_silence_ms, "300", "ms"),
            )
            .into_any(),
            setting_row(
                "発話とみなす最小の長さ",
                numeric_description::<u32>(form.vad_min_speech_ms, "これより短い音は無視します"),
                numeric_field::<u32>(form.vad_min_speech_ms, "200", "ms"),
            )
            .into_any(),
            setting_row(
                "拾いやすさ（VAD しきい値）",
                caption(""),
                threshold_control(form.vad_threshold),
            )
            .into_any(),
            setting_row(
                "話し始めをさかのぼる長さ",
                numeric_description::<u32>(form.preroll_ms, "検知が遅れたぶんを送ります"),
                numeric_field::<u32>(form.preroll_ms, "500", "ms"),
            )
            .into_any(),
        ],
    )
}

fn engine_setting_row(selected: RwSignal<AsrEngineChoice>) -> impl IntoView {
    v_stack((
        v_stack((
            label(|| "認識エンジン".to_string()).style(body_style),
            caption("変更は再起動後に反映されます"),
        ))
        .style(|style| style.width_full().min_width(0.0).gap(theme::space::XS)),
        engine_cards(selected),
    ))
    .style(|style| {
        style
            .width_full()
            .min_width(0.0)
            .min_height(72.0)
            .padding_horiz(theme::space::LG)
            .padding_vert(14.0)
            .gap(theme::space::SM)
            .border_bottom(1.0)
            .border_color(theme::color::LINE)
    })
}

fn advanced_page(form: FormState) -> impl IntoView {
    let endpoint_heading =
        label(|| "サーバー側の発話区切り（endpoint_mode=server の接続先だけ）".to_string()).style(
            |style| {
                style
                    .width_full()
                    .padding_left(16.0)
                    .padding_top(16.0)
                    .padding_bottom(4.0)
                    .font_family(theme::font_family().to_string())
                    .font_size(theme::text::BODY)
                    .font_weight(theme::text::BODY_WEIGHT)
                    .line_height(1.4)
                    .color(theme::color::INK)
            },
        );
    section_page(
        "詳細",
        "接続先と、細かな発話区切りを設定します。",
        vec![
            setting_row(
                "接続先サーバー URL",
                caption("空なら同梱のサーバーに繋ぎます"),
                text_field(form.server_url, "空なら同梱のサーバーに繋ぎます"),
            )
            .into_any(),
            setting_row(
                "VAD モデルのパス",
                caption("空なら同梱モデル"),
                text_field(form.vad_model_path, "空なら同梱モデル"),
            )
            .into_any(),
            setting_row(
                "発話が無いときに接続を閉じるまで",
                caption(""),
                numeric_field::<u32>(form.idle_close_sec, "15", "秒"),
            )
            .into_any(),
            setting_row(
                "透過表示",
                caption("表示が崩れるときだけ変えます。再起動後に反映"),
                Dropdown::new_rw(
                    form.overlay_transparent,
                    [
                        "自動".to_string(),
                        "使う".to_string(),
                        "使わない".to_string(),
                    ],
                )
                .style(dropdown_style),
            )
            .into_any(),
            endpoint_heading.into_any(),
            setting_row(
                "最大遅延",
                caption("同梱サーバーでは使いません"),
                numeric_field::<u32>(form.endpoint_max_delay_ms, "1500", "ms"),
            )
            .into_any(),
            setting_row(
                "感度",
                caption("同梱サーバーでは使いません"),
                numeric_field::<f32>(form.endpoint_sensitivity, "0.0", ""),
            )
            .into_any(),
            setting_row(
                "遅延調整レベル",
                caption("同梱サーバーでは使いません"),
                numeric_field::<u8>(form.endpoint_latency_level, "0", ""),
            )
            .into_any(),
        ],
    )
}

fn account_page(state: UiState, commands: Sender<ControllerCommand>) -> impl IntoView {
    let account_action = dyn_container(
        move || state.login_state.get(),
        move |login_state| match login_state {
            LoginState::InProgress => button("処理中…").style(disabled_button_style).into_any(),
            LoginState::LoggedIn { .. } => button("ログアウト")
                .action({
                    let commands = commands.clone();
                    move || {
                        let _ = commands.send(ControllerCommand::Logout);
                    }
                })
                .style(secondary_button_style)
                .into_any(),
            LoginState::LoggedOut | LoginState::Failed { .. } => button("ログイン")
                .action({
                    let commands = commands.clone();
                    move || {
                        let _ = commands.send(ControllerCommand::StartLogin);
                    }
                })
                .style(primary_button_style)
                .into_any(),
            LoginState::NotRequired => empty().into_any(),
        },
    );
    section_page(
        "アカウント",
        "ことのね共通アカウントで音声認識を利用します。",
        vec![setting_row(
            "ログイン状態",
            dyn_container(
                move || state.login_state.get(),
                move |login_state| {
                    let is_error = matches!(login_state, LoginState::Failed { .. });
                    label(move || login_state_text(&login_state)).style(if is_error {
                        error_text_style
                    } else {
                        body_soft_style
                    })
                },
            ),
            account_action,
        )
        .into_any()],
    )
}

fn about_page(form: FormState, state: UiState, _settings: Settings) -> impl IntoView {
    let settings_path = otoa_input_platform::settings_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|error| format!("取得できません: {error:#}"));
    let models_path = otoa_input_platform::data_directory()
        .map(|path| path.join("models").display().to_string())
        .unwrap_or_else(|error| format!("取得できません: {error:#}"));

    let brand = h_stack((
        app_mark(40.0),
        wordmark(156.0, 31.0),
        label(|| format!("v{}", env!("CARGO_PKG_VERSION"))).style(|style| {
            style
                .font_family(theme::font_family().to_string())
                .font_size(theme::text::CAPTION)
                .font_weight(theme::text::CAPTION_WEIGHT)
                .color(theme::color::INK_SOFT)
        }),
    ))
    .style(|style| style.width_full().items_center().gap(theme::space::MD));

    let content = v_stack((
        brand,
        divider(),
        about_info_block(
            "認識の場所",
            dyn_container(
                move || (state.route_local.get(), form.server_url.get()),
                move |(route, server_url)| {
                    label(move || route_description(route, &server_url)).style(caption_style)
                },
            ),
        ),
        about_info_block("設定ファイルの場所", caption_owned(settings_path)),
        about_info_block("モデルの置き場所", caption_owned(models_path)),
        divider(),
        label(|| "謝辞".to_string()).style(section_style),
        about_text("kodama-ja-streaming-small — ようさん（ayousanz）、Apache-2.0"),
        about_text("ReazonSpeech k2-v2 — Reazon Human Interaction Lab、Apache-2.0"),
        about_text("Silero VAD — Silero Team、MIT"),
        label(|| "ライセンス: MIT".to_string()).style(body_soft_style),
    ))
    .style(|style| style.width_full().gap(theme::space::MD).padding(20.0));

    v_stack((
        label(|| "このアプリについて".to_string()).style(title_style),
        label(|| "Otoa Input の情報とライセンスです。".to_string()).style(caption_style),
        container(content).style(card_style),
    ))
    .style(|style| style.width(488.0).max_width(560.0).gap(theme::space::MD))
}

fn about_info_block(title: &'static str, value: impl IntoView + 'static) -> impl IntoView {
    v_stack((
        label(move || title.to_string()).style(|style| {
            style
                .font_family(theme::font_family().to_string())
                .font_size(theme::text::BODY)
                .font_weight(theme::text::BODY_WEIGHT)
                .color(theme::color::INK)
        }),
        value,
    ))
    .style(|style| style.width_full().gap(theme::space::XS))
}

fn about_text(value: &'static str) -> impl IntoView {
    label(move || value.to_string()).style(caption_style)
}

fn route_description(route: Option<bool>, server_url: &str) -> String {
    match route {
        Some(true) => "この PC で認識しています".to_string(),
        Some(false) => format!(
            "サーバーで認識しています\n接続先: {}",
            if server_url.trim().is_empty() {
                "既定の接続先"
            } else {
                server_url
            }
        ),
        None => format!(
            "認識場所は接続後に表示します\n接続先: {}",
            if server_url.trim().is_empty() {
                "既定の接続先"
            } else {
                server_url
            }
        ),
    }
}

fn level_meter(state: UiState, smooth_level: RwSignal<f64>) -> impl IntoView {
    let bars = h_stack_from_iter((0..20).map(|index| {
        empty().style(move |style| {
            let lit = (smooth_level.get().clamp(0.0, 1.0) * 20.0).floor() as usize > index;
            let color = if !lit {
                theme::color::LINE
            } else if index < 14 {
                theme::color::BRAND
            } else if index < 18 {
                theme::color::AMBER
            } else {
                theme::color::ERROR
            };
            style.size(8.0, 28.0).border_radius(4.0).background(color)
        })
    }))
    .style(|style| style.size(240.0, 28.0).items_center().gap(4.0));
    v_stack((
        bars,
        label(move || level_message(state.level_status.get()).to_string()).style(move |style| {
            style
                .font_family(theme::font_family().to_string())
                .font_size(theme::text::CAPTION)
                .font_weight(theme::text::CAPTION_WEIGHT)
                .line_height(1.4)
                .color(level_message_color(state.level_status.get()))
        }),
    ))
    .style(|style| style.width(240.0).gap(theme::space::XS))
}

fn engine_cards(selected: RwSignal<AsrEngineChoice>) -> impl IntoView {
    h_stack((
        engine_card(
            selected,
            "reazonspeech",
            "ReazonSpeech k2-v2",
            "精度優先 ・ メモリ 約 1.3 GB",
            "長い発話にも強い。モデル 587 MB",
        ),
        engine_card(
            selected,
            "kodama",
            "kodama-ja-streaming-small",
            "軽量 ・ メモリ 約 450 MB",
            "雑音・短い発話・固有名詞・長い発話が苦手",
        ),
    ))
    .style(|style| style.width_full().min_width(0.0).gap(theme::space::SM))
}

fn engine_card(
    selected: RwSignal<AsrEngineChoice>,
    id: &'static str,
    name: &'static str,
    detail: &'static str,
    weakness: &'static str,
) -> impl IntoView {
    let radio = RadioButton::new_rw(
        AsrEngineChoice {
            id: id.to_string(),
            label: name.to_string(),
        },
        selected,
    );
    let card_content = v_stack((
        h_stack((
            radio,
            label(move || name.to_string()).style(|style| {
                body_style(style)
                    .flex_grow(1.0)
                    .min_width(0.0)
                    .text_overflow(TextOverflow::Wrap)
            }),
        ))
        .style(|style| {
            style
                .width_full()
                .min_width(0.0)
                .items_start()
                .gap(theme::space::SM)
        }),
        label(move || detail.to_string()).style(|style| {
            caption_style(style)
                .width_full()
                .min_width(0.0)
                .text_overflow(TextOverflow::Wrap)
        }),
        label(move || weakness.to_string()).style(|style| {
            style
                .width_full()
                .min_width(0.0)
                .font_family(theme::font_family().to_string())
                .font_size(theme::text::MICRO)
                .font_weight(theme::text::CAPTION_WEIGHT)
                .line_height(1.25)
                .text_overflow(TextOverflow::Wrap)
                .color(theme::color::INK_SOFT)
        }),
    ))
    .style(|style| style.width_full().min_width(0.0).gap(theme::space::XS));
    button(card_content)
        .action(move || {
            if selected.get().id != id {
                selected.update(|current| {
                    current.id = id.to_string();
                    current.label = name.to_string();
                });
            }
        })
        .style(move |style| {
            let selected_now = selected.get().id == id;
            style
                .flex_grow(1.0)
                .min_width(0.0)
                .padding(10.0)
                .gap(theme::space::XS)
                .border(if selected_now { 2.0 } else { 1.0 })
                .border_color(if selected_now {
                    theme::color::BRAND
                } else {
                    theme::color::LINE
                })
                .border_radius(16.0)
                .background(if selected_now {
                    theme::color::BRAND_TINT
                } else {
                    theme::color::SURFACE
                })
                .hover(|style| style.border_color(theme::color::BRAND_2))
        })
}

fn threshold_control(signal: RwSignal<Pct>) -> impl IntoView {
    v_stack((
        slider_control(signal),
        h_stack((
            label(|| "拾いやすい".to_string()).style(micro_style),
            empty().style(|style| style.flex_grow(1.0)),
            label(|| "誤検知を減らす".to_string()).style(micro_style),
        ))
        .style(|style| style.width(240.0).items_center()),
    ))
    .style(|style| style.width(240.0).gap(theme::space::XS))
}

fn slider_control(signal: RwSignal<Pct>) -> impl IntoView {
    let slider = slider::Slider::new_rw(signal)
        .slider_style(|style| {
            style
                .handle_color(Some(floem::peniko::Color::TRANSPARENT.into()))
                .handle_radius(9.0)
                .bar_color(theme::color::LINE)
                .bar_radius(3.0)
                .bar_height(6.0)
                .accent_bar_color(theme::color::BRAND)
                .accent_bar_radius(3.0)
                .accent_bar_height(6.0)
        })
        .style(|style| style.size(240.0, 24.0));
    let handle = empty().pointer_events(|| false).style(move |style| {
        let percent = signal.get().0.clamp(0.0, 100.0) / 100.0;
        style
            .absolute()
            .size(18.0, 18.0)
            .inset_left(222.0 * percent)
            .inset_top(3.0)
            .border(2.0)
            .border_radius(9.0)
            .border_color(theme::color::BRAND)
            .background(theme::color::ON_BRAND)
            .box_shadow_blur(theme::shadow::E1.blur)
            .box_shadow_color(theme::shadow::E1.color)
            .box_shadow_spread(theme::shadow::E1.spread)
            .box_shadow_h_offset(theme::shadow::E1.h_offset)
            .box_shadow_v_offset(theme::shadow::E1.v_offset)
    });
    stack((slider, handle)).style(|style| style.size(240.0, 24.0))
}

fn toggle_control(signal: RwSignal<bool>) -> impl IntoView {
    toggle_button(move || signal.get())
        .on_toggle(move |value| signal.set(value))
        .toggle_style(|style| {
            style
                .handle_color(theme::color::ON_BRAND)
                .handle_inset(2.0)
                .circle_rad(10.0)
        })
        .style(move |style| {
            style
                .size(44.0, 24.0)
                .border_radius(20.0)
                .background(if signal.get() {
                    theme::color::BRAND
                } else {
                    theme::color::LINE
                })
                .transition_background(Transition::ease_in_out(Duration::from_millis(200)))
        })
}

fn setting_row(
    title: &'static str,
    description: impl IntoView + 'static,
    control: impl IntoView + 'static,
) -> impl IntoView {
    h_stack((
        v_stack((
            label(move || title.to_string()).style(body_style),
            description,
        ))
        .style(|style| style.flex_grow(1.0).min_width(0.0).gap(theme::space::XS)),
        container(control).style(|style| style.flex_shrink(0.0)),
    ))
    .style(|style| {
        style
            .width_full()
            .min_width(0.0)
            .min_height(72.0)
            .padding_horiz(theme::space::LG)
            .padding_vert(14.0)
            .items_center()
            .gap(theme::space::LG)
            .border_bottom(1.0)
            .border_color(theme::color::LINE)
    })
}

fn section_page(
    title: &'static str,
    description: &'static str,
    rows: Vec<AnyView>,
) -> impl IntoView {
    v_stack((
        label(move || title.to_string()).style(title_style),
        label(move || description.to_string()).style(caption_style),
        container(v_stack_from_iter(rows).style(|style| style.width_full().min_width(0.0)))
            .style(card_style),
    ))
    .style(|style| style.width(488.0).max_width(560.0).gap(theme::space::MD))
}

fn numeric_field<T>(
    signal: RwSignal<String>,
    placeholder: &'static str,
    unit: &'static str,
) -> AnyView
where
    T: FromStr + 'static,
{
    let suffix = if unit.is_empty() {
        empty().into_any()
    } else {
        label(move || unit.to_string())
            .style(caption_style)
            .into_any()
    };
    h_stack((numeric_text_input::<T>(signal, placeholder), suffix))
        .style(|style| style.size(160.0, 36.0).items_center().gap(theme::space::SM))
        .into_any()
}

fn numeric_text_input<T>(signal: RwSignal<String>, placeholder: &'static str) -> impl IntoView
where
    T: FromStr + 'static,
{
    text_input(signal)
        .placeholder(placeholder)
        .style(move |style| {
            text_input_style(style)
                .size(120.0, 36.0)
                .flex_shrink(0.0)
                .border_color(if signal.get().trim().parse::<T>().is_err() {
                    theme::color::ERROR
                } else {
                    theme::color::LINE
                })
        })
}

fn numeric_description<T>(signal: RwSignal<String>, description: &'static str) -> AnyView
where
    T: FromStr + 'static,
{
    dyn_container(
        move || signal.get(),
        move |value| {
            if value.trim().parse::<T>().is_err() {
                label(|| "数字を入れてください".to_string())
                    .style(|style| error_text_style(style).width_full().min_width(0.0))
                    .into_any()
            } else {
                label(move || description.to_string())
                    .style(|style| caption_style(style).width_full().min_width(0.0))
                    .into_any()
            }
        },
    )
    .style(|style| style.width_full().min_width(0.0))
    .into_any()
}

fn text_field(signal: RwSignal<String>, placeholder: &'static str) -> impl IntoView {
    text_input(signal)
        .placeholder(placeholder)
        .style(text_input_style)
}

fn dropdown_style(style: Style) -> Style {
    style
        .size(240.0, 36.0)
        .padding_horiz(theme::space::MD)
        .font_family(theme::font_family().to_string())
        .font_size(theme::text::BODY_SOFT)
        .font_weight(theme::text::BODY_SOFT_WEIGHT)
        .background(theme::color::SURFACE)
        .color(theme::color::INK)
        .border(1.0)
        .border_color(theme::color::LINE)
        .border_radius(theme::radius::SM)
        .hover(|style| style.border_color(theme::color::BRAND_2))
        .focus(|style| {
            style
                .border_color(theme::color::BRAND)
                .box_shadow_blur(4.0)
                .box_shadow_color(theme::color::RING)
        })
}

fn text_input_style(style: Style) -> Style {
    style
        .size(240.0, 36.0)
        .padding_horiz(theme::space::MD)
        .font_family(theme::font_family().to_string())
        .font_size(theme::text::BODY_SOFT)
        .font_weight(theme::text::BODY_SOFT_WEIGHT)
        .background(theme::color::SURFACE)
        .color(theme::color::INK)
        .border(1.0)
        .border_color(theme::color::LINE)
        .border_radius(theme::radius::SM)
        .focus(|style| {
            style
                .border_color(theme::color::BRAND)
                .box_shadow_blur(4.0)
                .box_shadow_color(theme::color::RING)
        })
}

fn card_style(style: Style) -> Style {
    style
        .width_full()
        .background(theme::color::SURFACE)
        .border(1.0)
        .border_color(theme::color::LINE)
        .border_radius(theme::radius::CARD)
        .box_shadow_blur(theme::shadow::E1.blur)
        .box_shadow_color(theme::shadow::E1.color)
        .box_shadow_spread(theme::shadow::E1.spread)
        .box_shadow_h_offset(theme::shadow::E1.h_offset)
        .box_shadow_v_offset(theme::shadow::E1.v_offset)
}

fn primary_button_style(style: Style) -> Style {
    style
        .size(88.0, 40.0)
        .padding_horiz(theme::space::LG)
        .font_family(theme::font_family().to_string())
        .font_size(theme::text::BODY)
        .font_weight(theme::text::TITLE_WEIGHT)
        .background(theme::grad_brand())
        .color(theme::color::ON_BRAND)
        .border_radius(20.0)
        .box_shadow_blur(theme::shadow::BRAND_GLOW.blur)
        .box_shadow_color(theme::shadow::BRAND_GLOW.color)
        .box_shadow_spread(theme::shadow::BRAND_GLOW.spread)
        .box_shadow_h_offset(theme::shadow::BRAND_GLOW.h_offset)
        .box_shadow_v_offset(theme::shadow::BRAND_GLOW.v_offset)
        .hover(|style| style.box_shadow_blur(28.0).box_shadow_v_offset(12.0))
}

fn secondary_button_style(style: Style) -> Style {
    style
        .size(104.0, 40.0)
        .padding_horiz(theme::space::LG)
        .font_family(theme::font_family().to_string())
        .font_size(theme::text::BODY)
        .font_weight(theme::text::BODY_WEIGHT)
        .background(theme::color::SURFACE)
        .color(theme::color::BRAND_STRONG)
        .border(1.0)
        .border_color(floem::peniko::Color::rgb8(0xc9, 0xdc, 0xf6))
        .border_radius(20.0)
        .hover(|style| style.border_color(theme::color::BRAND_2))
}

fn disabled_button_style(style: Style) -> Style {
    secondary_button_style(style)
        .color(theme::color::INK_SOFT)
        .background(theme::color::BG)
}

fn title_style(style: Style) -> Style {
    style
        .font_family(theme::font_family().to_string())
        .font_size(theme::text::TITLE)
        .font_weight(theme::text::TITLE_WEIGHT)
        .color(theme::color::INK)
}

fn section_style(style: Style) -> Style {
    style
        .font_family(theme::font_family().to_string())
        .font_size(theme::text::SECTION)
        .font_weight(theme::text::SECTION_WEIGHT)
        .color(theme::color::INK)
}

fn body_style(style: Style) -> Style {
    style
        .font_family(theme::font_family().to_string())
        .font_size(theme::text::BODY)
        .font_weight(theme::text::BODY_WEIGHT)
        .line_height(1.5)
        .color(theme::color::INK)
}

fn body_soft_style(style: Style) -> Style {
    style
        .font_family(theme::font_family().to_string())
        .font_size(theme::text::BODY_SOFT)
        .font_weight(theme::text::BODY_SOFT_WEIGHT)
        .line_height(1.5)
        .color(theme::color::INK)
}

fn caption_style(style: Style) -> Style {
    style
        .font_family(theme::font_family().to_string())
        .font_size(theme::text::CAPTION)
        .font_weight(theme::text::CAPTION_WEIGHT)
        .line_height(1.6)
        .color(theme::color::INK_SOFT)
}

fn micro_style(style: Style) -> Style {
    style
        .font_family(theme::font_family().to_string())
        .font_size(theme::text::MICRO)
        .font_weight(theme::text::MICRO_WEIGHT)
        .color(theme::color::INK_SOFT)
}

fn error_text_style(style: Style) -> Style {
    caption_style(style).color(theme::color::ERROR)
}

fn caption(value: &'static str) -> impl IntoView {
    label(move || value.to_string()).style(caption_style)
}

fn caption_owned(value: String) -> impl IntoView {
    label(move || value.clone()).style(caption_style)
}

fn divider() -> impl IntoView {
    empty().style(|style| {
        style
            .width_full()
            .height(1.0)
            .background(theme::color::LINE)
    })
}

fn app_mark(size: f64) -> impl IntoView {
    clip(img(|| theme::LOGO_PNG.to_vec()).style(move |style| style.size(size, size)))
        .style(move |style| style.size(size, size))
}

fn wordmark(width: f64, height: f64) -> impl IntoView {
    stack((
        svg(theme::WORDMARK_OTOA_SVG.to_string()).style(move |style| {
            style
                .absolute()
                .size(width, height)
                .color(theme::color::NAVY)
        }),
        svg(theme::WORDMARK_INPUT_SVG.to_string()).style(move |style| {
            style
                .absolute()
                .size(width, height)
                .color(theme::color::BRAND_STRONG)
        }),
    ))
    .style(move |style| style.size(width, height))
}

fn level_message(status: LevelStatus) -> &'static str {
    match status {
        LevelStatus::Normal => "ちょうどいい大きさです",
        LevelStatus::TooQuiet => "小さすぎます。ゲインを上げるか、マイクに近づいてください",
        LevelStatus::Clipped => "大きすぎます。ゲインを下げてください",
    }
}

fn level_message_color(status: LevelStatus) -> floem::peniko::Color {
    match status {
        LevelStatus::Normal => theme::color::BRAND_STRONG,
        LevelStatus::TooQuiet => theme::color::INK_SOFT,
        LevelStatus::Clipped => theme::color::ERROR,
    }
}

fn login_state_text(state: &LoginState) -> String {
    match state {
        LoginState::LoggedOut => "未ログイン".to_string(),
        LoginState::InProgress => "ログイン処理中（ブラウザで操作待ち）".to_string(),
        LoginState::LoggedIn { email } => email.clone(),
        LoginState::Failed { reason } => format!("失敗: {reason}"),
        LoginState::NotRequired => "不要".to_string(),
    }
}

fn asr_engine_choices() -> Vec<AsrEngineChoice> {
    vec![
        AsrEngineChoice {
            id: "reazonspeech".to_string(),
            label: "ReazonSpeech k2-v2".to_string(),
        },
        AsrEngineChoice {
            id: "kodama".to_string(),
            label: "kodama-ja-streaming-small".to_string(),
        },
    ]
}

fn gain_to_pct(gain: f32) -> Pct {
    (f64::from((gain.clamp(0.1, 10.0) - 0.1) / 9.9) * 100.0).pct()
}

fn gain_from_pct(percent: Pct) -> f32 {
    (0.1 + (percent.0 as f32 / 100.0) * 9.9).clamp(0.1, 10.0)
}

fn commit_hold_to_pct(milliseconds: u32) -> Pct {
    (f64::from(milliseconds.min(3000)) / 3000.0 * 100.0).pct()
}

fn commit_hold_from_pct(percent: Pct) -> u32 {
    ((percent.0.clamp(0.0, 100.0) / 100.0) * 3000.0).round() as u32
}

fn threshold_to_pct(threshold: f32) -> Pct {
    (f64::from((threshold.clamp(0.1, 0.9) - 0.1) / 0.8) * 100.0).pct()
}

fn threshold_from_pct(percent: Pct) -> f32 {
    (0.1 + (percent.0 as f32 / 100.0) * 0.8).clamp(0.1, 0.9)
}

fn language_label(hints: &[String]) -> String {
    match hints.first().map(String::as_str) {
        Some("ja") => "日本語".to_string(),
        Some("en") => "英語".to_string(),
        _ => "自動".to_string(),
    }
}

fn language_hints(label: &str) -> Vec<String> {
    match label {
        "日本語" => vec!["ja".to_string()],
        "英語" => vec!["en".to_string()],
        _ => Vec::new(),
    }
}

fn overlay_position_label(value: &str) -> &'static str {
    match value {
        "top" => "画面の上",
        "bottom" => "画面の下",
        "center" => "中央",
        _ => "中央",
    }
}

fn overlay_position_value(label: &str) -> &'static str {
    match label {
        "画面の上" => "top",
        "画面の下" => "bottom",
        "中央" => "center",
        _ => "center",
    }
}

fn overlay_transparent_label(value: &str) -> &'static str {
    match value {
        "on" => "使う",
        "off" => "使わない",
        _ => "自動",
    }
}

fn overlay_transparent_value(label: &str) -> &'static str {
    match label {
        "使う" => "on",
        "使わない" => "off",
        _ => "auto",
    }
}

fn parse_or<T>(value: &str, fallback: T) -> T
where
    T: FromStr,
{
    value.trim().parse().unwrap_or(fallback)
}

fn microphone_choices(selected_id: &str) -> Vec<MicrophoneChoice> {
    match AudioCapture::list_devices() {
        Ok(devices) => microphone_choices_from_devices(devices, selected_id),
        Err(error) => {
            tracing::warn!(%error, "failed to list microphone devices for settings");
            let mut choices = vec![MicrophoneChoice {
                id: String::new(),
                label: "既定のデバイス".to_string(),
            }];
            add_selected_microphone(&mut choices, selected_id);
            choices
        }
    }
}

fn microphone_choices_from_devices(
    devices: Vec<otoa_input_platform::AudioDevice>,
    selected_id: &str,
) -> Vec<MicrophoneChoice> {
    let default_label = devices
        .iter()
        .find(|device| device.is_default)
        .map(|device| format!("既定のデバイス（{}）", device.name))
        .unwrap_or_else(|| "既定のデバイス".to_string());
    let mut choices = vec![MicrophoneChoice {
        id: String::new(),
        label: default_label,
    }];
    choices.extend(
        devices
            .into_iter()
            .filter(|device| !device.is_default)
            .map(|device| MicrophoneChoice {
                id: device.id.clone(),
                label: microphone_label(&device.id, &device.name, device.card_name.as_deref()),
            }),
    );
    add_selected_microphone(&mut choices, selected_id);
    choices
}

fn add_selected_microphone(choices: &mut Vec<MicrophoneChoice>, selected_id: &str) {
    if !selected_id.is_empty() && !choices.iter().any(|choice| choice.id == selected_id) {
        choices.push(MicrophoneChoice {
            id: selected_id.to_string(),
            label: microphone_label(selected_id, selected_id, None),
        });
    }
}

fn microphone_label(id: &str, name: &str, card_name: Option<&str>) -> String {
    match id {
        "default" => "既定のデバイス".to_string(),
        "pipewire" => "PipeWire".to_string(),
        "pulse" => "PulseAudio".to_string(),
        _ => card_name
            .filter(|card_name| !card_name.is_empty())
            .map(|card_name| format!("{card_name} ({name})"))
            .unwrap_or_else(|| name.to_string()),
    }
}

#[derive(Clone, Copy)]
struct LevelTicker {
    state: UiState,
    page: RwSignal<SettingsPage>,
    smooth_level: RwSignal<f64>,
    running: RwSignal<bool>,
    generation: RwSignal<u64>,
    generation_value: u64,
}

impl LevelTicker {
    fn schedule(self, last_tick: Instant, smooth_level: f64) {
        exec_after(LEVEL_TICK, move |_| {
            if !self.running.get_untracked()
                || self.generation.get_untracked() != self.generation_value
                || self.page.get_untracked() != SettingsPage::Microphone
            {
                return;
            }
            let now = Instant::now();
            let delta = now
                .saturating_duration_since(last_tick)
                .as_secs_f64()
                .clamp(0.0, 0.2);
            let target = self.state.level.get_untracked().clamp(0.0, 1.0);
            let next = if target >= smooth_level {
                target
            } else {
                let decay = 1.0 - (-delta / 0.12).exp();
                smooth_level + (target - smooth_level) * decay
            };
            if (next - smooth_level).abs() > f64::EPSILON {
                self.smooth_level.set(next);
            }
            self.schedule(now, next);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        commit_hold_from_pct, commit_hold_to_pct, gain_from_pct, gain_to_pct,
        microphone_choices_from_devices, microphone_label, overlay_position_label,
        overlay_position_value, overlay_transparent_label, overlay_transparent_value,
    };
    use floem::unit::UnitExt;
    use otoa_input_platform::AudioDevice;

    fn audio_device(id: &str, is_default: bool) -> AudioDevice {
        AudioDevice {
            id: id.to_string(),
            name: id.to_string(),
            is_default,
            card_name: None,
        }
    }

    #[test]
    fn default_microphone_is_not_listed_twice() {
        let choices = microphone_choices_from_devices(
            vec![audio_device("default", true), audio_device("other", false)],
            "",
        );

        assert_eq!(
            choices.iter().filter(|choice| choice.id.is_empty()).count(),
            1
        );
        assert_eq!(choices[0].label, "既定のデバイス（default）");
        assert_eq!(
            choices
                .iter()
                .map(|choice| choice.id.as_str())
                .collect::<Vec<_>>(),
            ["", "other"]
        );
    }

    #[test]
    fn microphone_labels_are_human_readable() {
        assert_eq!(
            microphone_label("default", "default", None),
            "既定のデバイス"
        );
        assert_eq!(microphone_label("pipewire", "pipewire", None), "PipeWire");
        assert_eq!(microphone_label("pulse", "pulse", None), "PulseAudio");
        assert_eq!(
            microphone_label(
                "hw:CARD=Device,DEV=0",
                "hw:CARD=Device,DEV=0",
                Some("USB PnP Audio Device"),
            ),
            "USB PnP Audio Device (hw:CARD=Device,DEV=0)"
        );
        assert_eq!(
            microphone_label("raw-device", "raw-device", None),
            "raw-device"
        );
    }

    #[test]
    fn gain_slider_round_trips_the_supported_range() {
        for gain in [0.1_f32, 1.0, 2.0, 10.0] {
            let round_trip = gain_from_pct(gain_to_pct(gain));
            assert!((round_trip - gain).abs() < 0.001);
        }
    }

    #[test]
    fn commit_hold_slider_round_trips_milliseconds() {
        for milliseconds in [0_u32, 900, 1500, 3000] {
            assert_eq!(
                commit_hold_from_pct(commit_hold_to_pct(milliseconds)),
                milliseconds
            );
        }
        assert_eq!(commit_hold_from_pct(150.pct()), 3000);
    }

    #[test]
    fn overlay_position_labels_round_trip() {
        for value in ["bottom", "top", "center"] {
            assert_eq!(overlay_position_value(overlay_position_label(value)), value);
        }
        assert_eq!(overlay_position_value("unknown"), "center");
    }

    #[test]
    fn overlay_transparency_labels_round_trip() {
        for value in ["auto", "on", "off"] {
            assert_eq!(
                overlay_transparent_value(overlay_transparent_label(value)),
                value
            );
        }
        assert_eq!(overlay_transparent_value("unknown"), "auto");
    }
}
