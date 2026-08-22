use super::{theme, UiState};
use crate::controller::LoginState;
use crate::controller::{ControllerCommand, LevelStatus};
use crate::settings::Settings;
use crossbeam_channel::Sender;
use floem::{
    close_window,
    peniko::kurbo::Size,
    prelude::*,
    reactive::{RwSignal, SignalGet, SignalUpdate},
    style::Style,
    unit::UnitExt,
    views::{dropdown::Dropdown, dyn_container, slider},
    window::WindowId,
    WindowIdExt,
};
use otoa_input_platform::AudioCapture;

#[derive(Clone)]
struct MicrophoneChoice {
    id: String,
    label: String,
}

#[derive(Clone)]
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

pub fn view(
    settings: Settings,
    state: UiState,
    commands: Sender<ControllerCommand>,
    window_id: WindowId,
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
    let vad_threshold = RwSignal::new(settings.vad_threshold.to_string());
    let vad_min_speech_ms = RwSignal::new(settings.vad_min_speech_ms.to_string());
    let preroll_ms = RwSignal::new(settings.preroll_ms.to_string());
    let endpoint_max_delay_ms = RwSignal::new(settings.endpoint_max_delay_ms.to_string());
    let endpoint_sensitivity = RwSignal::new(settings.endpoint_sensitivity.to_string());
    let endpoint_latency_level = RwSignal::new(settings.endpoint_latency_level.to_string());
    let idle_close_sec = RwSignal::new(settings.idle_close_sec.to_string());
    let commit_hold_ms = RwSignal::new(settings.commit_hold_ms.to_string());
    let splash_ms = RwSignal::new(settings.splash_ms.to_string());

    let microphone = RwSignal::new(settings.microphone.clone());
    let microphone_choices = microphone_choices(&settings.microphone);
    let selected_microphone = RwSignal::new(
        microphone_choices
            .iter()
            .find(|choice| {
                choice.id == settings.microphone
                    || (settings.microphone.is_empty() && choice.id == "default")
            })
            .cloned()
            .unwrap_or_else(|| microphone_choices[0].clone()),
    );
    let details_open = RwSignal::new(false);
    let title_height = RwSignal::new(0.0);
    let content_height = RwSignal::new(0.0);
    let footer_height = RwSignal::new(0.0);

    let save = {
        let commands = commands.clone();
        move || {
            let mut next = settings.clone();
            next.server_url = server_url.get_untracked();
            next.language_hints = language_hints(&language.get_untracked());
            next.listening_enabled = listening_enabled.get_untracked();
            next.auto_paste = auto_paste.get_untracked();
            next.asr_engine = selected_asr_engine.get_untracked().id;
            next.input_gain = gain_from_pct(gain.get_untracked());
            next.vad_model_path = vad_model_path.get_untracked();
            next.vad_threshold = parse_or(&vad_threshold.get_untracked(), settings.vad_threshold);
            next.vad_min_speech_ms = parse_or(
                &vad_min_speech_ms.get_untracked(),
                settings.vad_min_speech_ms,
            );
            next.preroll_ms = parse_or(&preroll_ms.get_untracked(), settings.preroll_ms);
            next.endpoint_max_delay_ms = parse_or(
                &endpoint_max_delay_ms.get_untracked(),
                settings.endpoint_max_delay_ms,
            );
            next.endpoint_sensitivity = parse_or(
                &endpoint_sensitivity.get_untracked(),
                settings.endpoint_sensitivity,
            );
            next.endpoint_latency_level = parse_or(
                &endpoint_latency_level.get_untracked(),
                settings.endpoint_latency_level,
            );
            next.idle_close_sec =
                parse_or(&idle_close_sec.get_untracked(), settings.idle_close_sec);
            next.commit_hold_ms =
                parse_or(&commit_hold_ms.get_untracked(), settings.commit_hold_ms);
            next.splash_ms = parse_or(&splash_ms.get_untracked(), settings.splash_ms);
            next.microphone = microphone.get_untracked();
            if let Err(error) = crate::settings_io::save(&next) {
                tracing::error!("設定の保存に失敗しました: {error:#}");
                return;
            }
            state.settings.set(next.clone());
            let _ = commands.send(ControllerCommand::UpdateSettings(Box::new(next)));
            close_window(window_id);
        }
    };

    let account = {
        let account_commands = commands.clone();
        let account_actions = dyn_container(
            move || state.login_state.get(),
            move |login_state| match login_state {
                LoginState::InProgress => button("ログイン処理中…")
                    .style(secondary_button_style)
                    .into_any(),
                LoginState::LoggedIn { .. } => button("ログアウト")
                    .action({
                        let commands = account_commands.clone();
                        move || {
                            let _ = commands.send(ControllerCommand::Logout);
                        }
                    })
                    .style(secondary_button_style)
                    .into_any(),
                LoginState::LoggedOut | LoginState::Failed { .. } => button("ログイン")
                    .action({
                        let commands = account_commands.clone();
                        move || {
                            let _ = commands.send(ControllerCommand::StartLogin);
                        }
                    })
                    .style(primary_button_style)
                    .into_any(),
                LoginState::NotRequired => empty().into_any(),
            },
        );

        section(
            "アカウント",
            "ことのね共通アカウントで音声認識を利用します。",
            v_stack((
                field(
                    "ログイン状態",
                    label(move || match state.login_state.get() {
                        LoginState::LoggedOut => "未ログイン".to_string(),
                        LoginState::InProgress => {
                            "ログイン処理中（ブラウザで操作待ち）".to_string()
                        }
                        LoginState::LoggedIn { email } => email,
                        LoginState::Failed { reason } => format!("失敗: {reason}"),
                        LoginState::NotRequired => "不要".to_string(),
                    })
                    .style(text_style),
                ),
                account_actions,
            ))
            .style(stack_style),
        )
    };

    let microphone_section = section(
        "マイク",
        "入力レベルを見ながら、使うデバイスとゲインを調整します。",
        v_stack((
            field(
                "入力デバイス",
                Dropdown::new(
                    move || selected_microphone.get(),
                    microphone_choices.clone(),
                )
                .on_accept(move |choice| {
                    microphone.set(choice.id.clone());
                    selected_microphone.set(choice);
                })
                .style(input_style),
            ),
            field(
                "入力ゲイン",
                v_stack((
                    slider::Slider::new_rw(gain)
                        .slider_style(|style| {
                            style
                                .handle_color(Some(theme::color::BRAND.into()))
                                .bar_color(theme::color::BORDER)
                                .accent_bar_color(theme::color::BRAND)
                        })
                        .style(|style| style.width_full()),
                    label(move || format!("{:.1}", gain_from_pct(gain.get()))).style(text_style),
                ))
                .style(stack_style),
            ),
            field(
                "入力レベル",
                v_stack((
                    container(empty().style(move |style| {
                        style
                            .width_pct(state.level.get() * 100.0)
                            .height_full()
                            .background(level_color(state.level_status.get()))
                    }))
                    .style(|style| {
                        style
                            .width_full()
                            .height(10.0)
                            .border(1.0)
                            .border_color(theme::color::BORDER)
                            .background(theme::color::SURFACE)
                    }),
                    label(move || level_message(state.level_status.get()).to_string())
                        .style(text_style),
                )),
            ),
        ))
        .style(stack_style),
    );

    let input_section = section(
        "入力",
        "待受と確定結果の扱いを設定します。",
        v_stack((
            field(
                "認識エンジン（再起動後に反映）",
                Dropdown::new(
                    move || selected_asr_engine.get(),
                    asr_engine_choices.clone(),
                )
                .on_accept(move |choice| selected_asr_engine.set(choice))
                .style(input_style),
            ),
            field(
                "待受を起動時に有効にする",
                labeled_checkbox(
                    move || listening_enabled.get(),
                    || "発話の自動検知を有効にする",
                )
                .on_update(move |value| listening_enabled.set(value)),
            ),
            field(
                "確定後に自動で貼り付ける",
                labeled_checkbox(
                    move || auto_paste.get(),
                    || "クリップボードへ置いて貼り付ける",
                )
                .on_update(move |value| auto_paste.set(value)),
            ),
            field(
                "言語",
                Dropdown::new_rw(
                    language,
                    ["日本語".to_string(), "英語".to_string(), "自動".to_string()],
                )
                .style(input_style),
            ),
        ))
        .style(stack_style),
    );

    let detail_content = dyn_container(
        move || details_open.get(),
        move |open| {
            if !open {
                return empty().into_any();
            }
            v_stack((
                field(
                    "サーバー URL",
                    text_field(server_url, "空なら既定の接続先を使う"),
                ),
                field("VAD しきい値", text_field(vad_threshold, "0.5")),
                field("最小発話時間（ms）", text_field(vad_min_speech_ms, "200")),
                field("プリロール（ms）", text_field(preroll_ms, "500")),
                field(
                    "ASR サーバー側の発話区切り最大遅延（ms）",
                    text_field(endpoint_max_delay_ms, "1500"),
                ),
                field(
                    "ASR サーバー側の発話区切り感度",
                    text_field(endpoint_sensitivity, "0.0"),
                ),
                field(
                    "ASR サーバー側の発話区切り遅延調整レベル",
                    text_field(endpoint_latency_level, "0"),
                ),
                field(
                    "VAD モデルパス",
                    text_field(vad_model_path, "空なら同梱モデル"),
                ),
                field(
                    "発話区切り後のセッションアイドル上限（秒）",
                    text_field(idle_close_sec, "15"),
                ),
                field(
                    "確定後の表示保持（ms）",
                    text_field(commit_hold_ms, "0（確定と同時に非表示）"),
                ),
                field("スプラッシュ表示時間（ms）", text_field(splash_ms, "2500")),
            ))
            .style(stack_style)
            .into_any()
        },
    );

    let details_header = button("詳細")
        .on_click_stop(move |_| details_open.update(|open| *open = !*open))
        .style(|style| secondary_button_style(style).font_size(theme::text::SECTION));

    let title = label(|| "Otoa Input 設定".to_string())
        .style(|style| {
            style
                .font_family(theme::font_family().to_string())
                .font_size(theme::text::TITLE)
                .font_bold()
                .color(theme::color::TEXT)
        })
        .on_resize({
            move |rect| {
                title_height.set(rect.height());
                resize_settings_window(window_id, title_height, content_height, footer_height);
            }
        });
    let content = v_stack((
        account,
        microphone_section,
        input_section,
        v_stack((
            details_header,
            empty().style(|style| {
                style
                    .width_full()
                    .height(1.0)
                    .background(theme::color::BORDER)
            }),
            detail_content,
        ))
        .style(stack_style),
    ))
    .style(|style| style.width_full().gap(theme::space::XL))
    .on_resize({
        move |rect| {
            content_height.set(rect.height());
            resize_settings_window(window_id, title_height, content_height, footer_height);
        }
    });
    let footer = h_stack((
        empty().style(|style| style.flex_grow(1.0)),
        button("閉じる")
            .action(move || close_window(window_id))
            .style(secondary_button_style),
        button("保存").action(save).style(primary_button_style),
    ))
    .style(|style| style.width_full().items_center().gap(theme::space::SM))
    .on_resize({
        move |rect| {
            footer_height.set(rect.height());
            resize_settings_window(window_id, title_height, content_height, footer_height);
        }
    });

    container(
        v_stack((
            title,
            scroll(content).style(|style| style.width_full().flex_grow(1.0)),
            footer,
        ))
        .style(|style| style.width_full().height_full().gap(theme::space::LG)),
    )
    .style(|style| {
        style
            .width_full()
            .height_full()
            .font_family(theme::font_family().to_string())
            .padding(theme::space::LG)
            .background(theme::color::SURFACE)
            .color(theme::color::TEXT)
    })
    .on_cleanup(move || state.settings_window_open.set(false))
}

fn asr_engine_choices() -> Vec<AsrEngineChoice> {
    vec![
        AsrEngineChoice {
            id: "reazonspeech".to_string(),
            label: "ReazonSpeech k2-v2（精度優先・メモリ 約1.3GB）".to_string(),
        },
        AsrEngineChoice {
            id: "kodama".to_string(),
            label: "kodama（軽量・メモリ 約450MB）".to_string(),
        },
    ]
}

fn resize_settings_window(
    window_id: WindowId,
    title_height: RwSignal<f64>,
    content_height: RwSignal<f64>,
    footer_height: RwSignal<f64>,
) {
    let title_height = title_height.get_untracked();
    let content_height = content_height.get_untracked();
    let footer_height = footer_height.get_untracked();
    if !(title_height > 0.0 && content_height > 0.0 && footer_height > 0.0) {
        return;
    }

    let natural_height = theme::space::LG * 2.0
        + title_height
        + content_height
        + footer_height
        + theme::space::LG * 2.0;
    let max_height = otoa_input_platform::primary_screen_size()
        .map(|(_, height)| height * 0.9)
        .unwrap_or(natural_height);
    let height = natural_height.min(max_height);
    window_id.set_content_size(Size::new(560.0, height));
}

fn level_color(status: LevelStatus) -> floem::peniko::Color {
    match status {
        LevelStatus::Clipped => theme::color::ERROR,
        LevelStatus::TooQuiet => theme::color::IDLE,
        LevelStatus::Normal => theme::color::ACTIVE,
    }
}

fn level_message(status: LevelStatus) -> &'static str {
    match status {
        LevelStatus::Clipped => "音が大きすぎます",
        LevelStatus::TooQuiet => "音が小さすぎます",
        LevelStatus::Normal => "",
    }
}

fn section(
    title: &'static str,
    description: &'static str,
    content: impl IntoView + 'static,
) -> impl IntoView {
    v_stack((
        label(move || title.to_string()).style(|style| {
            style
                .font_family(theme::font_family().to_string())
                .font_size(theme::text::SECTION)
                .font_bold()
                .color(theme::color::TEXT)
        }),
        empty().style(|style| {
            style
                .width_full()
                .height(1.0)
                .background(theme::color::BORDER)
        }),
        label(move || description.to_string()).style(|style| {
            style
                .font_family(theme::font_family().to_string())
                .font_size(theme::text::CAPTION)
                .line_height(1.5)
                .color(theme::color::TEXT_MUTED)
        }),
        content,
    ))
    .style(|style| style.width_full().gap(theme::space::MD))
}

fn field(label_text: &'static str, control: impl IntoView + 'static) -> impl IntoView {
    v_stack((
        label(move || label_text.to_string()).style(|style| {
            style
                .font_family(theme::font_family().to_string())
                .font_size(theme::text::BODY)
                .color(theme::color::TEXT)
        }),
        control,
    ))
    .style(|style| style.width_full().gap(theme::space::XS))
}

fn text_field(signal: RwSignal<String>, placeholder: &'static str) -> impl IntoView {
    text_input(signal)
        .placeholder(placeholder)
        .style(input_style)
}

fn input_style(style: Style) -> Style {
    style
        .width_full()
        .font_family(theme::font_family().to_string())
        .font_size(theme::text::BODY)
        .background(theme::color::SURFACE)
        .color(theme::color::TEXT)
}

fn text_style(style: Style) -> Style {
    style
        .font_family(theme::font_family().to_string())
        .font_size(theme::text::BODY)
        .color(theme::color::TEXT)
}

fn primary_button_style(style: Style) -> Style {
    style
        .font_family(theme::font_family().to_string())
        .font_size(theme::text::BODY)
        .background(theme::color::BRAND)
        .color(theme::color::ON_BRAND)
}

fn secondary_button_style(style: Style) -> Style {
    style
        .font_family(theme::font_family().to_string())
        .font_size(theme::text::BODY)
        .background(theme::color::SURFACE)
        .color(theme::color::TEXT)
        .border(1.0)
        .border_color(theme::color::BORDER)
}

fn stack_style(style: Style) -> Style {
    style.width_full().gap(theme::space::MD)
}

fn gain_to_pct(gain: f32) -> floem::unit::Pct {
    (f64::from((gain.clamp(0.1, 10.0) - 0.1) / 9.9) * 100.0).pct()
}

fn gain_from_pct(percent: floem::unit::Pct) -> f32 {
    (0.1 + (percent.0 as f32 / 100.0) * 9.9).clamp(0.1, 10.0)
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

fn parse_or<T>(value: &str, fallback: T) -> T
where
    T: std::str::FromStr,
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

#[cfg(test)]
mod tests {
    use super::{microphone_choices_from_devices, microphone_label};
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
}
