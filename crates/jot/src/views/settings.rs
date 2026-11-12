// Copyright 2026 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Settings: the dictation key, what the pill shows, how a transcript is
//! formatted, how long audio is kept, and the escape hatches.

use crate::i18n::{Msg, tr};
use crate::services::Services;
use crate::text_field::{Submit, TextField};
use crate::theme::{self, Theme};
use crate::views::widgets::{self, ButtonKind, StatusTone};
use gpui::{AnyElement, Context, Entity, Role, SharedString, Window, div, prelude::*, px};
use jot_core::audio;
use jot_core::credentials;
use jot_core::gemini::KeyCheck;
use jot_core::hotkey::{ChordProblem, DictationActivationMode, HotkeyBinding, HotkeyKey};
use jot_core::local::{self, Download, Pack};
use jot_core::settings::{
    LocalReformulation, LocalUnload, Settings, TranscriptionEngine, UiLanguage, UiTheme,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

fn ui(en: &'static str, fr: &'static str) -> &'static str {
    match crate::i18n::locale() {
        crate::i18n::Locale::En => en,
        crate::i18n::Locale::Fr => fr,
    }
}

/// "2,56 Go" / "2.56 GB".
pub(super) fn size_label(bytes: u64) -> String {
    let gb = format!("{:.2}", bytes as f64 / 1e9);
    match crate::i18n::locale() {
        crate::i18n::Locale::En => format!("{gb} GB"),
        crate::i18n::Locale::Fr => format!("{} Go", gb.replace('.', ",")),
    }
}

pub(super) fn pack_source(pack: Pack) -> &'static str {
    match pack {
        Pack::Transcription => ui(
            "llama.cpp b12289 (MIT) from github.com/ggml-org, Qwen3-ASR 1.7B Q8_0 (Apache-2.0) from huggingface.co/ggml-org",
            "llama.cpp b12289 (MIT) depuis github.com/ggml-org, Qwen3-ASR 1.7B Q8_0 (Apache-2.0) depuis huggingface.co/ggml-org",
        ),
        Pack::Fast => ui(
            "Qwen3.5 4B Q4_K_M (Apache-2.0), community conversion from huggingface.co/unsloth",
            "Qwen3.5 4B Q4_K_M (Apache-2.0), conversion communautaire depuis huggingface.co/unsloth",
        ),
        Pack::Quality => ui(
            "Qwen3.5 9B Q4_K_M (Apache-2.0), community conversion from huggingface.co/unsloth",
            "Qwen3.5 9B Q4_K_M (Apache-2.0), conversion communautaire depuis huggingface.co/unsloth",
        ),
    }
}

/// What the next local dictation will really get, one line per stage: an
/// installed transcription pack must never read as "reformulation ready".
fn local_state(
    mode: LocalReformulation,
    transcription: bool,
    reformulation: bool,
) -> [(String, StatusTone); 2] {
    let transcription = if transcription {
        (
            ui(
                "Local transcription: installed",
                "Transcription locale : installée",
            ),
            StatusTone::Good,
        )
    } else {
        (
            ui(
                "Local transcription: not installed — local dictation is unavailable until it is downloaded",
                "Transcription locale : non installée — la dictée locale est indisponible tant qu’elle n’est pas téléchargée",
            ),
            StatusTone::Bad,
        )
    };
    let name = match mode {
        LocalReformulation::Quality => ui("Quality editing", "Reformulation qualité"),
        _ => ui("Fast editing", "Reformulation rapide"),
    };
    let reformulation = match (mode, reformulation) {
        (LocalReformulation::Off, _) => (
            ui(
                "Local editing: off — the raw transcript is inserted",
                "Reformulation locale : désactivée — la transcription brute est insérée",
            )
            .to_string(),
            StatusTone::Neutral,
        ),
        (_, true) => (
            format!(
                "{name}{}",
                ui(
                    ": installed — available and active",
                    " : installée — disponible et active"
                )
            ),
            StatusTone::Good,
        ),
        (_, false) => (
            format!(
                "{name}{}",
                ui(
                    ": editing model not installed — download it below. Until then, the raw transcript is inserted.",
                    " : modèle de reformulation non installé — téléchargez-le ci-dessous. En attendant, la transcription brute est insérée.",
                )
            ),
            StatusTone::Bad,
        ),
    };
    [
        (transcription.0.to_string(), transcription.1),
        reformulation,
    ]
}

/// Audio retention options, in the order they read as a sentence.
const RETENTION_CHOICES: [(i64, Msg); 4] = [
    (-1, Msg::NeverKeep),
    (7, Msg::SevenDays),
    (30, Msg::ThirtyDays),
    (0, Msg::Forever),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    General,
    Shortcuts,
    Transcription,
    Audio,
    Appearance,
    Privacy,
    Advanced,
}

impl Category {
    const ALL: [Self; 7] = [
        Self::General,
        Self::Shortcuts,
        Self::Transcription,
        Self::Audio,
        Self::Appearance,
        Self::Privacy,
        Self::Advanced,
    ];

    fn label(self) -> Msg {
        match self {
            Self::General => Msg::General,
            Self::Shortcuts => Msg::Shortcuts,
            Self::Transcription => Msg::Transcription,
            Self::Audio => Msg::Audio,
            Self::Appearance => Msg::Appearance,
            Self::Privacy => Msg::Privacy,
            Self::Advanced => Msg::Advanced,
        }
    }
}

pub struct SettingsView {
    services: Services,
    api_key: Entity<TextField>,
    endpoint: Entity<TextField>,
    transcribe_model: Entity<TextField>,
    cleanup_model: Entity<TextField>,
    typing_wpm: Entity<TextField>,
    typing_status: Option<(Msg, StatusTone)>,
    _typing_submit: gpui::Subscription,
    key_status: Option<(Msg, StatusTone)>,
    capturing_hotkey: bool,
    captured_hotkey: Option<HotkeyBinding>,
    hotkey_status: Option<(Msg, StatusTone)>,
    advanced_status: Option<(Msg, StatusTone)>,
    /// True once the user has asked to wipe everything and been shown what
    /// that means.
    confirming_delete_all: bool,
    deletion_status: Option<Msg>,
    confirming_remove_key: bool,
    autostart_error: bool,
    category: Category,
    /// Mirrors the registry so the switch shows what the machine actually does,
    /// not what we last asked for.
    autostart_enabled: bool,
    /// A model download the user confirmed, with its live progress.
    download: Option<(Pack, Arc<Download>)>,
    /// The pack whose size and source are on screen awaiting a yes.
    confirming_download: Option<Pack>,
    download_status: Option<(String, StatusTone)>,
    download_poll: Option<gpui::Task<()>>,
    scroll: widgets::PageScroll,
}

impl SettingsView {
    pub fn new(services: Services, cx: &mut Context<Self>) -> Self {
        let settings = services.settings.get();
        let endpoint = cx.new(|cx| {
            TextField::new(
                "settings-endpoint",
                "https://generativelanguage.googleapis.com",
                cx,
            )
        });
        let transcribe_model =
            cx.new(|cx| TextField::new("settings-transcribe-model", "gemini-3.5-transcribe", cx));
        let cleanup_model =
            cx.new(|cx| TextField::new("settings-cleanup-model", "gemini-3.5-flash-lite", cx));
        let typing_wpm = cx.new(|cx| {
            let mut field = TextField::new("settings-typing-wpm", Msg::TypingSpeed, cx);
            field.set_content(settings.typing_wpm().to_string());
            field
        });
        let typing_submit = cx.subscribe(&typing_wpm, |this, _, _: &Submit, cx| {
            this.save_typing_wpm(cx);
        });

        endpoint.update(cx, |field, _| {
            field.set_content(settings.endpoint_override.clone().unwrap_or_default())
        });
        transcribe_model.update(cx, |field, _| {
            field.set_content(
                settings
                    .transcribe_model_override
                    .clone()
                    .unwrap_or_default(),
            )
        });
        cleanup_model.update(cx, |field, _| {
            field.set_content(settings.cleanup_model_override.clone().unwrap_or_default())
        });

        Self {
            services,
            // The stored key is never read back into the UI: it lives in the
            // credential manager and this field only ever writes a new one.
            api_key: cx.new(|cx| TextField::new("settings-api-key", Msg::PasteNewKey, cx).masked()),
            endpoint,
            transcribe_model,
            cleanup_model,
            typing_wpm,
            typing_status: None,
            _typing_submit: typing_submit,
            key_status: None,
            capturing_hotkey: false,
            captured_hotkey: None,
            hotkey_status: None,
            advanced_status: None,
            confirming_delete_all: false,
            deletion_status: None,
            confirming_remove_key: false,
            autostart_error: false,
            category: Category::Shortcuts,
            autostart_enabled: crate::autostart::is_enabled(),
            download: None,
            confirming_download: None,
            download_status: None,
            download_poll: None,
            scroll: widgets::PageScroll::new(),
        }
    }

    pub fn dismiss_confirmations(&mut self, cx: &mut Context<Self>) {
        self.confirming_delete_all = false;
        self.confirming_remove_key = false;
        self.cancel_hotkey_capture();
        cx.notify();
    }

    fn cancel_hotkey_capture(&mut self) {
        self.capturing_hotkey = false;
        self.captured_hotkey = None;
        crate::hotkey_hook::stop_capture();
    }

    fn record_hotkey(&mut self, cx: &mut Context<Self>) {
        if crate::hotkey_hook::binding_error() == Some(crate::hotkey_hook::BindError::Unavailable) {
            self.hotkey_status = Some((Msg::ShortcutUnavailable, StatusTone::Bad));
            cx.notify();
            return;
        }
        self.capturing_hotkey = true;
        self.captured_hotkey = None;
        self.hotkey_status = None;
        crate::hotkey_hook::start_capture();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor.timer(std::time::Duration::from_millis(50)).await;
                let keep = this
                    .update(cx, |this, cx| {
                        if !this.capturing_hotkey {
                            return false;
                        }
                        match crate::hotkey_hook::captured_binding() {
                            Some(Ok(binding)) => {
                                this.captured_hotkey = Some(binding);
                                cx.notify();
                            }
                            Some(Err(())) => {
                                this.cancel_hotkey_capture();
                                cx.notify();
                                return false;
                            }
                            None => {}
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn save_hotkey(&mut self, binding: HotkeyBinding, cx: &mut Context<Self>) {
        use crate::hotkey_hook::{self, BindError};
        let result = hotkey_hook::prepare_binding(binding);
        self.hotkey_status = match result {
            Err(error) => Some((
                match error {
                    BindError::Invalid => {
                        capture_problem(binding).unwrap_or(Msg::ShortcutUnsupportedKey)
                    }
                    BindError::Conflict => Msg::ShortcutConflict,
                    BindError::Unavailable => Msg::ShortcutUnavailable,
                },
                StatusTone::Bad,
            )),
            Ok(()) => {
                if self.services.settings.set_hotkey_binding(binding).is_err() {
                    let _ =
                        hotkey_hook::prepare_binding(self.services.settings.get().hotkey_binding());
                    Some((Msg::ShortcutSaveFailed, StatusTone::Bad))
                } else {
                    self.cancel_hotkey_capture();
                    self.watch_hotkey(cx);
                    None
                }
            }
        };
        cx.notify();
    }

    /// Repaint until the saved shortcut is really active (or failed): it waits
    /// for the end of a dictation in flight.
    fn watch_hotkey(&mut self, cx: &mut Context<Self>) {
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor.timer(Duration::from_millis(200)).await;
                let pending = this
                    .update(cx, |this, cx| {
                        cx.notify();
                        let saved = this.services.settings.get().hotkey_binding();
                        shortcut_state(
                            saved,
                            crate::hotkey_hook::active_binding(),
                            crate::hotkey_hook::binding_error(),
                        )
                        .0 == Msg::ShortcutPending
                    })
                    .unwrap_or(false);
                if !pending {
                    break;
                }
            }
        })
        .detach();
    }

    fn delete_all(&mut self, cx: &mut Context<Self>) {
        // The live session's folder is spared: deleting everything must not
        // destroy the dictation in flight.
        let active = self.services.coordinator.active_session_folder();
        self.services.history.delete_all(true, active.as_deref());
        let purge = self
            .services
            .history
            .correction_session_ids()
            .and_then(|ids| {
                self.services
                    .dictionary
                    .purge_orphan_suggestions(&ids.into_iter().collect())
            });
        self.deletion_status = purge.err().map(|_| Msg::PurgeSuggestionsFailed);
        // A user wiping their words expects them gone from the paste-last
        // buffer too.
        self.services.coordinator.clear_last_result();
        self.confirming_delete_all = false;
        cx.notify();
    }

    fn save_api_key(&mut self, cx: &mut Context<Self>) {
        self.confirming_remove_key = false;
        let key = self.api_key.update(cx, |field, cx| {
            cx.notify();
            field.take_content()
        });
        let key = key.trim().to_string();
        if key.is_empty() {
            self.key_status = Some((Msg::PasteKeyFirst, StatusTone::Bad));
            cx.notify();
            return;
        }
        if let Err(error) = credentials::set_api_key(&key) {
            // The message never contains the key itself.
            tracing::error!(%error, "storing the API key failed");
            self.key_status = Some((Msg::KeyStoreFailed, StatusTone::Bad));
            cx.notify();
            return;
        }
        self.key_status = Some((Msg::KeySavedChecking, StatusTone::Neutral));
        cx.notify();

        let client = self.services.client.clone();
        let endpoint = self.services.settings.get().gemini_config().endpoint;
        cx.spawn(async move |this, cx| {
            let check = client.validate_key(&endpoint).await;
            let status = match check {
                KeyCheck::Valid => (Msg::KeyWorks, StatusTone::Good),
                // The server answered and the answer was no.
                KeyCheck::Rejected(_) => (Msg::KeyRejected, StatusTone::Bad),
                // Never blame the key for a network we could not reach.
                KeyCheck::Unreachable => (Msg::KeyUnreachable, StatusTone::Neutral),
            };
            let _ = this.update(cx, |this, cx| {
                this.key_status = Some(status);
                cx.notify();
            });
        })
        .detach();
    }

    fn clear_api_key(&mut self, cx: &mut Context<Self>) {
        self.confirming_remove_key = false;
        match credentials::clear_api_key() {
            Ok(()) => self.key_status = Some((Msg::KeyRemoved, StatusTone::Neutral)),
            Err(error) => {
                tracing::error!(%error, "clearing the API key failed");
                self.key_status = Some((Msg::KeyRemoveFailed, StatusTone::Bad));
            }
        }
        cx.notify();
    }

    fn save_advanced(&mut self, cx: &mut Context<Self>) {
        let endpoint = self.endpoint.read(cx).content().trim().to_string();
        let transcribe = self.transcribe_model.read(cx).content().trim().to_string();
        let cleanup = self.cleanup_model.read(cx).content().trim().to_string();

        // The same predicate the effective config uses, so this message can
        // never disagree with which endpoint is actually in play.
        let endpoint_usable = jot_core::settings::Settings::usable_endpoint(Some(&endpoint));
        if !endpoint.is_empty() && endpoint_usable.is_none() {
            self.advanced_status = Some((Msg::InvalidEndpoint, StatusTone::Bad));
            cx.notify();
            return;
        }

        self.services.settings.update(Some("advanced"), |settings| {
            settings.endpoint_override = (!endpoint.is_empty()).then_some(endpoint);
            settings.transcribe_model_override = (!transcribe.is_empty()).then_some(transcribe);
            settings.cleanup_model_override = (!cleanup.is_empty()).then_some(cleanup);
        });
        self.advanced_status = Some((Msg::AdvancedSaved, StatusTone::Good));
        cx.notify();
    }

    fn save_typing_wpm(&mut self, cx: &mut Context<Self>) {
        let value = self.typing_wpm.read(cx).content().trim().parse::<u16>();
        self.typing_status = match value {
            Ok(wpm) if Settings::TYPING_WPM_RANGE.contains(&wpm) => {
                self.services
                    .settings
                    .update(Some("typingWpm"), |settings| settings.typing_wpm = wpm);
                self.typing_wpm.update(cx, |field, cx| {
                    field.set_content(wpm.to_string());
                    cx.notify();
                });
                Some((Msg::TypingSaved, StatusTone::Good))
            }
            _ => Some((Msg::TypingInvalid, StatusTone::Bad)),
        };
        cx.refresh_windows();
        cx.notify();
    }
}

impl SettingsView {
    fn general(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        widgets::section(
            tr(Msg::General),
            theme,
            vec![
                widgets::toggle(
                    "autostart",
                    tr(Msg::Autostart),
                    tr(Msg::AutostartHint),
                    self.autostart_enabled,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        if crate::autostart::is_locked() {
                            return;
                        }
                        let wanted = !this.autostart_enabled;
                        this.autostart_error = !crate::autostart::set_enabled(wanted);
                        if !this.autostart_error {
                            this.autostart_enabled = wanted;
                        } else {
                            // The registry refused; do not show a toggle that
                            // claims a state the machine does not have.
                            this.autostart_enabled = crate::autostart::is_enabled();
                        }
                        cx.notify();
                    }),
                )
                .when(crate::autostart::is_locked(), |row| {
                    row.opacity(0.5).cursor_default()
                })
                .into_any_element(),
                div()
                    .when(crate::autostart::is_locked(), |row| {
                        row.child(widgets::status_line(
                            "autostart-test-profile",
                            tr(Msg::AutostartTestProfile),
                            StatusTone::Neutral,
                            theme,
                        ))
                    })
                    .when(self.autostart_error, |row| {
                        row.child(widgets::status_line(
                            "autostart-error",
                            tr(Msg::AutostartError),
                            StatusTone::Bad,
                            theme,
                        ))
                    })
                    .into_any_element(),
                widgets::field_row(
                    tr(Msg::TypingSpeed),
                    tr(Msg::TypingSpeedHint),
                    theme,
                    div()
                        .flex()
                        .items_center()
                        .gap(theme::spacing::XS)
                        .child(div().w(px(110.0)).child(self.typing_wpm.clone()))
                        .child(widgets::button(
                            "save-typing-wpm",
                            tr(Msg::Save),
                            ButtonKind::Secondary,
                            true,
                            theme,
                            cx.listener(|this, _, _, cx| this.save_typing_wpm(cx)),
                        )),
                )
                .into_any_element(),
                div()
                    .when_some(self.typing_status, |row, (message, tone)| {
                        row.child(widgets::status_line(
                            "typing-status",
                            tr(message),
                            tone,
                            theme,
                        ))
                    })
                    .into_any_element(),
            ],
        )
        .into_any_element()
    }

    fn shortcuts(&self, theme: Theme, settings: &Settings, cx: &mut Context<Self>) -> AnyElement {
        let mut hotkey_row = div().flex().flex_wrap().gap(theme::spacing::XS);
        for key in HotkeyKey::ALL {
            let selected = settings.custom_hotkey.is_none() && settings.hotkey_key == key;
            hotkey_row = hotkey_row.child(widgets::button(
                ("hotkey", key.vk_code()),
                crate::i18n::hotkey(key),
                if selected {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Secondary
                },
                true,
                theme,
                cx.listener(move |this, _, _, cx| this.save_hotkey(HotkeyBinding::Bare(key), cx)),
            ));
        }

        let mut mode_row = div()
            .id("dictation-activation-options")
            .role(Role::RadioGroup)
            .aria_label(tr(Msg::Activation))
            .flex()
            .flex_wrap()
            .gap(theme::spacing::XS);
        for (id, mode, label, description) in [
            (
                "dictation-hold",
                DictationActivationMode::Hold,
                Msg::Hold,
                Msg::HoldHint,
            ),
            (
                "dictation-toggle",
                DictationActivationMode::Toggle,
                Msg::Toggle,
                Msg::ToggleHint,
            ),
        ] {
            let selected = settings.dictation_activation_mode == mode;
            mode_row = mode_row.child(
                widgets::button(
                    id,
                    tr(label),
                    if selected {
                        ButtonKind::Primary
                    } else {
                        ButtonKind::Secondary
                    },
                    true,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        this.services.settings.update(
                            Some("dictationActivationMode"),
                            |settings| {
                                settings.dictation_activation_mode = mode;
                            },
                        );
                        cx.notify();
                    }),
                )
                .flex_1()
                .min_w(px(190.0))
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(theme::type_scale::LABEL)
                        .child(tr(description)),
                )
                .role(Role::RadioButton)
                .aria_label(format!("{}. {}", tr(label), tr(description)))
                .aria_toggled(if selected {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                }),
            );
        }

        widgets::section(
            tr(Msg::Shortcuts),
            theme,
            vec![
                widgets::field_row(
                    tr(Msg::Activation),
                    tr(Msg::ActivationHint),
                    theme,
                    mode_row,
                )
                .into_any_element(),
                div()
                    .text_size(theme::type_scale::LABEL)
                    .text_color(theme.on_surface_variant)
                    .child(tr(Msg::CancelShortcut))
                    .into_any_element(),
                widgets::field_row(
                    tr(Msg::Hotkey),
                    tr(Msg::HotkeyHint),
                    theme,
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_size(theme::type_scale::BODY)
                                .child(crate::i18n::hotkey_binding(settings.hotkey_binding())),
                        )
                        .child(hotkey_row)
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                                .child(widgets::button(
                                    "record-hotkey",
                                    tr(Msg::RecordShortcut),
                                    ButtonKind::Secondary,
                                    // Cancelled and Failed persist until the next dictation:
                                    // they must not lock the shortcut editor.
                                    !self.capturing_hotkey && {
                                        let state = self.services.coordinator.state();
                                        state == jot_core::state_machine::DictationState::Idle
                                            || state.is_terminal()
                                    },
                                    theme,
                                    cx.listener(|this, _, _, cx| this.record_hotkey(cx)),
                                ))
                                .child(widgets::button(
                                    "reset-hotkey",
                                    tr(Msg::RestoreDefaultShortcut),
                                    ButtonKind::Secondary,
                                    !self.capturing_hotkey,
                                    theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.save_hotkey(
                                            HotkeyBinding::Bare(HotkeyKey::default()),
                                            cx,
                                        )
                                    }),
                                )),
                        )
                        .when(self.capturing_hotkey, |column| {
                            column
                                .child(widgets::status_line(
                                    "capture-instructions",
                                    tr(Msg::ShortcutListening),
                                    StatusTone::Neutral,
                                    theme,
                                ))
                                .child(
                                    div()
                                        .id("captured-hotkey")
                                        .text_size(theme::type_scale::BODY)
                                        .child(match self.captured_hotkey {
                                            Some(binding) => crate::i18n::hotkey_binding(binding),
                                            None => tr(Msg::ShortcutWaitingKeys).to_string(),
                                        }),
                                )
                                .when_some(
                                    self.captured_hotkey.and_then(capture_note),
                                    |column, (message, tone)| {
                                        column.child(widgets::status_line(
                                            "captured-hotkey-note",
                                            tr(message),
                                            tone,
                                            theme,
                                        ))
                                    },
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap_2()
                                        .child(widgets::button(
                                            "confirm-hotkey",
                                            tr(Msg::ConfirmShortcut),
                                            ButtonKind::Primary,
                                            crate::hotkey_hook::capture_ready()
                                                && self
                                                    .captured_hotkey
                                                    .is_some_and(|b| capture_problem(b).is_none()),
                                            theme,
                                            cx.listener(|this, _, _, cx| {
                                                if let Some(binding) = this.captured_hotkey {
                                                    this.save_hotkey(binding, cx);
                                                }
                                            }),
                                        ))
                                        .child(widgets::button(
                                            "cancel-hotkey",
                                            tr(Msg::Cancel),
                                            ButtonKind::Secondary,
                                            true,
                                            theme,
                                            cx.listener(|this, _, _, cx| {
                                                this.cancel_hotkey_capture();
                                                cx.notify();
                                            }),
                                        )),
                                )
                        })
                        .when_some(self.hotkey_status, |column, (message, tone)| {
                            column.child(widgets::status_line(
                                "hotkey-status",
                                tr(message),
                                tone,
                                theme,
                            ))
                        })
                        .child({
                            let saved = settings.hotkey_binding();
                            let (message, tone) = shortcut_state(
                                saved,
                                crate::hotkey_hook::active_binding(),
                                crate::hotkey_hook::binding_error(),
                            );
                            widgets::status_line(
                                "hotkey-state",
                                crate::i18n::LocalizedText::with(
                                    message,
                                    &[&crate::i18n::hotkey_binding(saved)],
                                )
                                .render(),
                                tone,
                                theme,
                            )
                        })
                        .child(
                            div()
                                .text_size(theme::type_scale::LABEL)
                                .text_color(theme.on_surface_variant)
                                .child(tr(Msg::ShortcutCollisionLimit)),
                        ),
                )
                .into_any_element(),
                widgets::toggle(
                    "double-tap",
                    tr(Msg::DoubleTap),
                    tr(Msg::DoubleTapHint),
                    settings.double_tap_lock,
                    theme,
                    {
                        let store = self.services.settings.clone();
                        move |_, _, cx| {
                            let next = !store.get().double_tap_lock;
                            store.update(Some("doubleTapLock"), |s| s.double_tap_lock = next);
                            cx.refresh_windows();
                        }
                    },
                )
                .into_any_element(),
            ],
        )
        .into_any_element()
    }

    fn start_download(&mut self, pack: Pack, cx: &mut Context<Self>) {
        let progress = Arc::new(Download::default());
        self.download = Some((pack, progress.clone()));
        self.confirming_download = None;
        self.download_status = None;
        jot_core::runtime::spawn(local::install(pack, progress.clone()));
        let executor = cx.background_executor().clone();
        self.download_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                executor.timer(Duration::from_millis(300)).await;
                let finished = progress.finished.load(Ordering::Acquire);
                let alive = this
                    .update(cx, |view, cx| {
                        if finished {
                            view.finish_download();
                        }
                        cx.notify();
                    })
                    .is_ok();
                if finished || !alive {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn finish_download(&mut self) {
        let Some((pack, progress)) = self.download.take() else {
            return;
        };
        let error = progress.error.lock().clone();
        let active =
            Pack::for_reformulation(self.services.settings.get().local_reformulation) == Some(pack);
        self.download_status = Some(match error {
            None => (
                format!(
                    "{}{}",
                    ui(
                        "Installed and verified (SHA-256).",
                        "Installé et vérifié (SHA-256).",
                    ),
                    if active {
                        ui(
                            " Local editing is now available and active.",
                            " La reformulation locale est maintenant disponible et active.",
                        )
                    } else {
                        ""
                    }
                ),
                StatusTone::Good,
            ),
            Some(error) if error == local::CANCELLED => (
                ui(
                    "Download cancelled. The unfinished file was deleted; verified files are kept.",
                    "Téléchargement annulé. Le fichier incomplet a été supprimé ; les fichiers déjà vérifiés sont conservés.",
                )
                .into(),
                StatusTone::Neutral,
            ),
            Some(error) => (
                format!(
                    "{}{error}",
                    ui("Download failed: ", "Échec du téléchargement : ")
                ),
                StatusTone::Bad,
            ),
        });
        self.download_poll = None;
    }

    fn pack_row(
        &self,
        pack: Pack,
        title: &'static str,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = match pack {
            Pack::Transcription => "transcription",
            Pack::Fast => "fast",
            Pack::Quality => "quality",
        };
        let control: AnyElement = match &self.download {
            Some((active, progress)) if *active == pack => {
                let progress = progress.clone();
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(format!(
                        "{} {:.0} % ({} / {})",
                        ui("Downloading", "Téléchargement"),
                        progress.fraction() * 100.0,
                        size_label(progress.done.load(Ordering::Relaxed)),
                        size_label(progress.total.load(Ordering::Relaxed)),
                    ))
                    .child(widgets::button(
                        SharedString::from(format!("local-cancel-{id}")),
                        ui("Cancel", "Annuler"),
                        ButtonKind::Secondary,
                        true,
                        theme,
                        move |_, _, _| progress.cancel.store(true, Ordering::Relaxed),
                    ))
                    .into_any_element()
            }
            _ if pack.installed() => widgets::status_line(
                SharedString::from(format!("local-installed-{id}")),
                ui("Installed", "Installé"),
                StatusTone::Good,
                theme,
            )
            .into_any_element(),
            _ if self.confirming_download == Some(pack) => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(format!(
                    "{} {} — {}. {} {}",
                    ui("Download", "Télécharger"),
                    size_label(pack.missing_bytes()),
                    pack_source(pack),
                    ui(
                        "Each file is checked against its SHA-256 before use and kept in",
                        "Chaque fichier est vérifié par SHA-256 avant usage et conservé dans",
                    ),
                    local::local_root().display(),
                ))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(widgets::button(
                            SharedString::from(format!("local-confirm-{id}")),
                            ui("Download now", "Télécharger maintenant"),
                            ButtonKind::Primary,
                            true,
                            theme,
                            cx.listener(move |this, _, _, cx| this.start_download(pack, cx)),
                        ))
                        .child(widgets::button(
                            SharedString::from(format!("local-decline-{id}")),
                            ui("Not now", "Pas maintenant"),
                            ButtonKind::Secondary,
                            true,
                            theme,
                            cx.listener(|this, _, _, cx| {
                                this.confirming_download = None;
                                cx.notify();
                            }),
                        )),
                )
                .into_any_element(),
            _ => widgets::button(
                SharedString::from(format!("local-download-{id}")),
                format!(
                    "{} ({})",
                    ui("Download", "Télécharger"),
                    size_label(pack.missing_bytes())
                ),
                ButtonKind::Secondary,
                self.download.is_none(),
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.confirming_download = Some(pack);
                    this.download_status = None;
                    cx.notify();
                }),
            )
            .into_any_element(),
        };
        widgets::field_row(title, pack_source(pack), theme, control).into_any_element()
    }

    fn local_rows(
        &self,
        theme: Theme,
        settings: &Settings,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows: Vec<AnyElement> = local_state(
            settings.local_reformulation,
            Pack::Transcription.installed(),
            Pack::for_reformulation(settings.local_reformulation).is_some_and(Pack::installed),
        )
        .into_iter()
        .zip(["local-state-transcription", "local-state-reformulation"])
        .map(|((message, tone), id)| {
            widgets::status_line(id, message, tone, theme).into_any_element()
        })
        .collect();
        rows.push(self.pack_row(
            Pack::Transcription,
            ui("Local transcription", "Transcription locale"),
            theme,
            cx,
        ));
        rows.push(
            widgets::field_row(
                tr(Msg::Reformulation),
                ui(
                    "Removes hesitations and fixes punctuation with a second local model. Off: Qwen3-ASR's text is inserted as is.",
                    "Supprime les hésitations et corrige la ponctuation avec un second modèle local. Désactivée : le texte de Qwen3-ASR est inséré tel quel.",
                ),
                theme,
                choices(
                    "local-reformulation",
                    Msg::Reformulation,
                    [
                        (LocalReformulation::Off, Msg::ReformOff),
                        (LocalReformulation::Fast, Msg::ReformFast),
                        (LocalReformulation::Quality, Msg::ReformQuality),
                    ],
                    settings.local_reformulation,
                    theme,
                    {
                        let store = self.services.settings.clone();
                        move |value, cx| {
                            store.update(Some("localReformulation"), |s| {
                                s.local_reformulation = value
                            });
                            cx.refresh_windows();
                        }
                    },
                ),
            )
            .into_any_element(),
        );
        if let Some(pack) = Pack::for_reformulation(settings.local_reformulation) {
            let title = match pack {
                Pack::Quality => ui(
                    "Quality model (about 6 GB of video memory)",
                    "Modèle qualité (environ 6 Go de mémoire vidéo)",
                ),
                _ => ui(
                    "Fast model (about 3.5 GB of video memory)",
                    "Modèle rapide (environ 3,5 Go de mémoire vidéo)",
                ),
            };
            rows.push(self.pack_row(pack, title, theme, cx));
        }
        if let Some((message, tone)) = &self.download_status {
            rows.push(
                widgets::status_line("local-download-status", message.clone(), *tone, theme)
                    .into_any_element(),
            );
        }
        rows.push(
            widgets::field_row(
                tr(Msg::LocalUnload),
                ui(
                    "Counted from the end of the last dictation. Never during a recording or its processing. The next dictation reloads them (a few seconds).",
                    "Compté depuis la fin de la dernière dictée. Jamais pendant un enregistrement ou son traitement. La dictée suivante les recharge (quelques secondes).",
                ),
                theme,
                choices(
                    "local-unload",
                    Msg::LocalUnload,
                    [
                        (LocalUnload::Immediately, Msg::UnloadNow),
                        (LocalUnload::OneMinute, Msg::UnloadOne),
                        (LocalUnload::FiveMinutes, Msg::UnloadFive),
                        (LocalUnload::TenMinutes, Msg::UnloadTen),
                        (LocalUnload::FifteenMinutes, Msg::UnloadFifteen),
                        (LocalUnload::ThirtyMinutes, Msg::UnloadThirty),
                        (LocalUnload::KeepLoaded, Msg::UnloadNever),
                    ],
                    settings.local_unload,
                    theme,
                    {
                        let store = self.services.settings.clone();
                        move |value, cx| {
                            store.update(Some("localUnload"), |s| s.local_unload = value);
                            cx.refresh_windows();
                        }
                    },
                ),
            )
            .into_any_element(),
        );
        let status = self.services.local_engine.status();
        let acceleration = match (status.running, status.loading, status.gpu) {
            (_, true, _) => ui(
                "Models: loading into memory…",
                "Modèles : chargement en mémoire…",
            )
            .into(),
            (true, _, Some(gpu)) => format!(
                "{}{gpu} (Vulkan)",
                ui("Models: ready, on the GPU: ", "Modèles : prêts, sur le GPU : ")
            ),
            (true, _, None) => ui(
                "Models: ready, on the CPU (no compatible dedicated GPU or not enough video memory). Dictation works, more slowly.",
                "Modèles : prêts, sur le processeur (pas de GPU dédié compatible ou mémoire vidéo insuffisante). La dictée fonctionne, plus lentement.",
            )
            .into(),
            (false, _, _) => ui(
                "Models: not loaded. They load when you press the dictation key, while you speak.",
                "Modèles : non chargés. Ils se chargent à l’appui sur la touche de dictée, pendant que vous parlez.",
            )
            .into(),
        };
        rows.push(
            widgets::status_line(
                "local-acceleration",
                acceleration,
                StatusTone::Neutral,
                theme,
            )
            .into_any_element(),
        );
        if status.running {
            let engine = self.services.local_engine.clone();
            rows.push(
                widgets::button(
                    "local-free-memory",
                    tr(Msg::FreeMemory),
                    ButtonKind::Secondary,
                    true,
                    theme,
                    move |_, window, _| {
                        let engine = engine.clone();
                        jot_core::runtime::spawn(async move { engine.release().await });
                        window.refresh();
                    },
                )
                .into_any_element(),
            );
        }
        rows.push(
            widgets::status_line(
                "local-privacy",
                ui(
                    "In local mode, audio and text never leave this computer. Jot only goes online to download models when you ask, and never falls back to Gemini.",
                    "En mode local, l’audio et le texte ne quittent pas cet ordinateur. Jot ne se connecte que pour télécharger les modèles à votre demande et ne bascule jamais vers Gemini.",
                ),
                StatusTone::Neutral,
                theme,
            )
            .into_any_element(),
        );
        rows
    }

    fn transcription(
        &self,
        theme: Theme,
        settings: &Settings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let engine = widgets::field_row(
            tr(Msg::Engine),
            ui(
                "Gemini: the recording is sent to Google's Gemini API with your key. 100% local: everything runs on this computer after a one-time download.",
                "Gemini : l’enregistrement est envoyé à l’API Gemini de Google avec votre clé. 100 % local : tout s’exécute sur cet ordinateur après un téléchargement unique.",
            ),
            theme,
            choices(
                "transcription-engine",
                Msg::Engine,
                [
                    (TranscriptionEngine::Gemini, Msg::EngineGemini),
                    (TranscriptionEngine::Local, Msg::EngineLocal),
                ],
                settings.transcription_engine,
                theme,
                {
                    let store = self.services.settings.clone();
                    let engine = self.services.local_engine.clone();
                    move |value, cx| {
                        store.update(Some("transcriptionEngine"), |s| {
                            s.transcription_engine = value
                        });
                        if value == TranscriptionEngine::Gemini {
                            // Give the video memory back as soon as local mode is
                            // left (after the dictation in progress, if any).
                            let engine = engine.clone();
                            jot_core::runtime::spawn(async move { engine.release().await });
                        }
                        cx.refresh_windows();
                    }
                },
            ),
        )
        .into_any_element();
        if settings.transcription_engine == TranscriptionEngine::Local {
            let mut rows = vec![engine];
            rows.extend(self.local_rows(theme, settings, cx));
            return widgets::section(tr(Msg::Transcription), theme, rows).into_any_element();
        }
        widgets::section(
            tr(Msg::Transcription),
            theme,
            vec![
                engine,
                widgets::toggle(
                    "smart-transcription",
                    tr(Msg::Smart),
                    tr(Msg::SmartHint),
                    settings.smart_transcription,
                    theme,
                    {
                        let store = self.services.settings.clone();
                        move |_, _, cx| {
                            let next = !store.get().smart_transcription;
                            store.update(Some("smartTranscription"), |s| {
                                s.smart_transcription = next
                            });
                            cx.refresh_windows();
                        }
                    },
                )
                .into_any_element(),
                widgets::toggle(
                    "tone-pass",
                    tr(Msg::Tone),
                    tr(Msg::ToneHint),
                    settings.smart_cleanup_pass,
                    theme,
                    {
                        let store = self.services.settings.clone();
                        move |_, _, cx| {
                            let next = !store.get().smart_cleanup_pass;
                            store.set_smart_cleanup_pass(next);
                            cx.refresh_windows();
                        }
                    },
                )
                .into_any_element(),
            ],
        )
        .into_any_element()
    }

    fn audio(&self, theme: Theme, settings: &Settings) -> AnyElement {
        let mut microphone_row = div().flex().flex_wrap().gap(theme::spacing::XS);
        let devices = audio::input_device_names();
        let chosen = settings.preferred_input_device.clone();
        for (index, choice) in std::iter::once(None)
            .chain(devices.into_iter().map(Some))
            .enumerate()
        {
            let selected = chosen == choice;
            let store = self.services.settings.clone();
            let label = choice
                .clone()
                .unwrap_or_else(|| tr(Msg::DefaultDevice).to_string());
            let value = choice.clone();
            microphone_row = microphone_row.child(widgets::button(
                SharedString::from(format!("microphone-{index}")),
                SharedString::from(label),
                if selected {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Secondary
                },
                true,
                theme,
                move |_, _, cx| {
                    let value = value.clone();
                    store.update(Some("preferredInputDevice"), |settings| {
                        settings.preferred_input_device = value;
                    });
                    cx.refresh_windows();
                },
            ));
        }

        widgets::section(
            tr(Msg::Audio),
            theme,
            vec![
                widgets::field_row(
                    tr(Msg::Microphone),
                    tr(Msg::MicrophoneHint),
                    theme,
                    microphone_row,
                )
                .into_any_element(),
                widgets::toggle(
                    "sounds",
                    tr(Msg::Sounds),
                    tr(Msg::SoundsHint),
                    settings.sounds_enabled,
                    theme,
                    {
                        let store = self.services.settings.clone();
                        move |_, _, cx| {
                            let next = !store.get().sounds_enabled;
                            store.update(Some("soundsEnabled"), |s| s.sounds_enabled = next);
                            cx.refresh_windows();
                        }
                    },
                )
                .into_any_element(),
                widgets::toggle(
                    "noise-handling",
                    tr(Msg::Noise),
                    tr(Msg::NoiseHint),
                    settings.experimental_noise_handling,
                    theme,
                    {
                        let store = self.services.settings.clone();
                        move |_, _, cx| {
                            let next = !store.get().experimental_noise_handling;
                            store.update(Some("experimentalNoiseHandling"), |s| {
                                s.experimental_noise_handling = next
                            });
                            cx.refresh_windows();
                        }
                    },
                )
                .into_any_element(),
            ],
        )
        .into_any_element()
    }

    fn privacy(&self, theme: Theme, settings: &Settings, cx: &mut Context<Self>) -> AnyElement {
        let mut retention_row = div().flex().flex_wrap().gap(theme::spacing::XS);
        for (days, label) in RETENTION_CHOICES {
            let selected = settings.audio_retention_days == days;
            let store = self.services.settings.clone();
            retention_row = retention_row.child(widgets::button(
                ("retention", days as u64),
                tr(label),
                if selected {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Secondary
                },
                true,
                theme,
                move |_, _, cx| {
                    store.update(Some("audioRetentionDays"), |settings| {
                        settings.audio_retention_days = days
                    });
                    cx.refresh_windows();
                },
            ));
        }

        widgets::section(
            tr(Msg::Privacy),
            theme,
            vec![
                div()
                    .text_size(theme::type_scale::BODY)
                    .text_color(theme.on_surface_variant)
                    .child(tr(Msg::VoicePrivacy))
                    .into_any_element(),
                widgets::field_row(
                    tr(Msg::Retention),
                    tr(Msg::RetentionHint),
                    theme,
                    retention_row,
                )
                .into_any_element(),
                widgets::toggle(
                    "check-updates",
                    tr(Msg::CheckUpdates),
                    tr(Msg::CheckUpdatesHint),
                    settings.check_updates,
                    theme,
                    {
                        let store = self.services.settings.clone();
                        move |_, _, cx| {
                            let next = !store.get().check_updates;
                            store.update(Some("checkUpdates"), |s| s.check_updates = next);
                            cx.refresh_windows();
                        }
                    },
                )
                .into_any_element(),
                widgets::toggle(
                    "include-prereleases",
                    tr(Msg::Prereleases),
                    tr(Msg::PrereleasesHint),
                    settings.include_prereleases,
                    theme,
                    {
                        let store = self.services.settings.clone();
                        move |_, _, cx| {
                            let next = !store.get().include_prereleases;
                            store.update(Some("includePrereleases"), |s| {
                                s.include_prereleases = next
                            });
                            cx.refresh_windows();
                        }
                    },
                )
                .into_any_element(),
                widgets::field_row(
                    tr(Msg::ApiKey),
                    tr(Msg::ApiKeyHint),
                    theme,
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::spacing::XS)
                        .child(self.api_key.clone())
                        .child(widgets::hug(widgets::button(
                            "get-gemini-key",
                            tr(Msg::GetGeminiKey),
                            ButtonKind::Secondary,
                            true,
                            theme,
                            cx.listener(|this, _, _, cx| {
                                if super::onboarding::open_gemini_key_page().is_err() {
                                    this.key_status =
                                        Some((Msg::BrowserOpenFailed, StatusTone::Bad));
                                    cx.notify();
                                }
                            }),
                        )))
                        .child(
                            div()
                                .text_size(theme::type_scale::LABEL)
                                .text_color(theme.on_surface_variant)
                                .child(tr(Msg::GetGeminiKeyHint)),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(theme::spacing::XS)
                                .child(widgets::button(
                                    "save-key",
                                    tr(Msg::SaveKey),
                                    ButtonKind::Primary,
                                    true,
                                    theme,
                                    cx.listener(|this, _, _, cx| this.save_api_key(cx)),
                                ))
                                .child(widgets::button(
                                    "clear-key",
                                    tr(Msg::RemoveKey),
                                    ButtonKind::Destructive,
                                    true,
                                    theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.confirming_remove_key = true;
                                        cx.notify();
                                    }),
                                )),
                        )
                        .when(self.confirming_remove_key, |column| {
                            column
                                .child(widgets::status_line(
                                    "remove-key-warning",
                                    tr(Msg::RemoveKeyConfirm),
                                    StatusTone::Bad,
                                    theme,
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap_2()
                                        .child(widgets::button(
                                            "remove-key-confirm",
                                            tr(Msg::RemoveKey),
                                            ButtonKind::Destructive,
                                            true,
                                            theme,
                                            cx.listener(|this, _, _, cx| this.clear_api_key(cx)),
                                        ))
                                        .child(widgets::button(
                                            "remove-key-cancel",
                                            tr(Msg::Cancel),
                                            ButtonKind::Secondary,
                                            true,
                                            theme,
                                            cx.listener(|this, _, _, cx| {
                                                this.confirming_remove_key = false;
                                                cx.notify();
                                            }),
                                        )),
                                )
                        })
                        .when_some(self.key_status, |column, (message, tone)| {
                            column.child(widgets::status_line(
                                "settings-key-status",
                                tr(message),
                                tone,
                                theme,
                            ))
                        }),
                )
                .into_any_element(),
                if self.confirming_delete_all {
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::spacing::XS)
                        .child(widgets::status_line(
                            "delete-all-warning",
                            tr(Msg::DeleteAllWarning),
                            StatusTone::Bad,
                            theme,
                        ))
                        .child(
                            div()
                                .flex()
                                .gap(theme::spacing::XS)
                                .child(widgets::button(
                                    "delete-all-confirm",
                                    tr(Msg::DeleteAllConfirm),
                                    ButtonKind::Destructive,
                                    true,
                                    theme,
                                    cx.listener(|this, _, _, cx| this.delete_all(cx)),
                                ))
                                .child(widgets::button(
                                    "delete-all-cancel",
                                    tr(Msg::Cancel),
                                    ButtonKind::Secondary,
                                    true,
                                    theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.confirming_delete_all = false;
                                        cx.notify();
                                    }),
                                )),
                        )
                        .into_any_element()
                } else {
                    widgets::hug(widgets::button(
                        "delete-all",
                        tr(Msg::DeleteAll),
                        ButtonKind::Destructive,
                        true,
                        theme,
                        cx.listener(|this, _, _, cx| {
                            this.confirming_delete_all = true;
                            cx.notify();
                        }),
                    ))
                    .into_any_element()
                },
                div()
                    .when_some(self.deletion_status, |column, message| {
                        column.child(widgets::status_line(
                            "purge-suggestions-status",
                            tr(message),
                            StatusTone::Bad,
                            theme,
                        ))
                    })
                    .into_any_element(),
            ],
        )
        .into_any_element()
    }

    fn advanced(&self, theme: Theme, settings: &Settings, cx: &mut Context<Self>) -> AnyElement {
        widgets::section(
            tr(Msg::Advanced),
            theme,
            vec![
                div()
                    .text_size(theme::type_scale::LABEL)
                    .text_color(theme.on_surface_variant)
                    .child(tr(Msg::AdvancedHint))
                    .into_any_element(),
                widgets::field_row(tr(Msg::Endpoint), "", theme, self.endpoint.clone())
                    .into_any_element(),
                widgets::field_row(
                    tr(Msg::TranscribeModel),
                    "",
                    theme,
                    self.transcribe_model.clone(),
                )
                .into_any_element(),
                widgets::field_row(tr(Msg::CleanupModel), "", theme, self.cleanup_model.clone())
                    .into_any_element(),
                widgets::toggle(
                    "legacy-endpoint",
                    tr(Msg::Legacy),
                    tr(Msg::LegacyHint),
                    settings.legacy_transcribe_endpoint,
                    theme,
                    {
                        let store = self.services.settings.clone();
                        move |_, _, cx| {
                            let next = !store.get().legacy_transcribe_endpoint;
                            store.update(Some("legacyTranscribeEndpoint"), |s| {
                                s.legacy_transcribe_endpoint = next
                            });
                            cx.refresh_windows();
                        }
                    },
                )
                .into_any_element(),
                div()
                    .flex()
                    .gap(theme::spacing::XS)
                    .child(widgets::button(
                        "save-advanced",
                        tr(Msg::Save),
                        ButtonKind::Primary,
                        true,
                        theme,
                        cx.listener(|this, _, _, cx| this.save_advanced(cx)),
                    ))
                    .into_any_element(),
                div()
                    .when_some(self.advanced_status, |row, (message, tone)| {
                        row.child(widgets::status_line(
                            "settings-advanced-status",
                            tr(message),
                            tone,
                            theme,
                        ))
                    })
                    .into_any_element(),
            ],
        )
        .into_any_element()
    }

    fn appearance(&self, theme: Theme, settings: &Settings) -> AnyElement {
        let language = choices(
            "ui-language",
            Msg::Language,
            [
                (UiLanguage::Automatic, Msg::Automatic),
                (UiLanguage::French, Msg::French),
                (UiLanguage::English, Msg::English),
            ],
            settings.ui_language,
            theme,
            {
                let store = self.services.settings.clone();
                move |language, cx| {
                    store.update(Some("uiLanguage"), |settings| {
                        settings.ui_language = language
                    });
                    crate::i18n::select(language);
                    crate::tray::refresh_tooltip();
                    super::refresh_language(cx);
                }
            },
        );
        let palette = choices(
            "ui-theme",
            Msg::Theme,
            [
                (UiTheme::Automatic, Msg::Automatic),
                (UiTheme::Light, Msg::ThemeLight),
                (UiTheme::Dark, Msg::ThemeDark),
            ],
            settings.ui_theme,
            theme,
            {
                let store = self.services.settings.clone();
                move |theme, cx| {
                    store.update(Some("uiTheme"), |settings| settings.ui_theme = theme);
                    cx.refresh_windows();
                }
            },
        );
        widgets::section(
            tr(Msg::Appearance),
            theme,
            vec![
                widgets::field_row(tr(Msg::Language), tr(Msg::LanguageHint), theme, language)
                    .into_any_element(),
                widgets::field_row(tr(Msg::Theme), "", theme, palette).into_any_element(),
                widgets::toggle(
                    "idle-indicator",
                    tr(Msg::IdleIndicator),
                    tr(Msg::IdleIndicatorHint),
                    settings.show_idle_indicator,
                    theme,
                    {
                        let store = self.services.settings.clone();
                        move |_, _, cx| {
                            let next = !store.get().show_idle_indicator;
                            store.update(Some("showIdleIndicator"), |s| {
                                s.show_idle_indicator = next
                            });
                            cx.refresh_windows();
                        }
                    },
                )
                .into_any_element(),
            ],
        )
        .into_any_element()
    }
}

fn choices<T: Copy + PartialEq + 'static, const N: usize>(
    id: &'static str,
    group_label: Msg,
    choices: [(T, Msg); N],
    current: T,
    theme: Theme,
    select: impl Fn(T, &mut gpui::App) + Clone + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .role(Role::RadioGroup)
        .aria_label(tr(group_label))
        .flex()
        .flex_wrap()
        .gap_2()
        .children(
            choices
                .into_iter()
                .enumerate()
                .map(|(index, (value, label))| {
                    let selected = value == current;
                    let select = select.clone();
                    widgets::button(
                        (id, index),
                        tr(label),
                        if selected {
                            ButtonKind::Selected
                        } else {
                            ButtonKind::Secondary
                        },
                        true,
                        theme,
                        move |_, _, cx| select(value, cx),
                    )
                    .role(Role::RadioButton)
                    .aria_toggled(if selected {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                }),
        )
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::ui(window);
        let settings = self.services.settings.get();
        let menu = div()
            .id("settings-navigation")
            .role(Role::TabList)
            .aria_label(tr(Msg::SettingsNavigation))
            .flex()
            .flex_wrap()
            .gap_2()
            .children(
                Category::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, category)| {
                        let selected = self.category == category;
                        widgets::navigation_button(
                            ("settings-category", index),
                            tr(category.label()),
                            selected,
                            theme,
                            cx.listener(move |this, _, _, cx| {
                                this.category = category;
                                this.cancel_hotkey_capture();
                                this.confirming_delete_all = false;
                                this.confirming_remove_key = false;
                                this.scroll.reset();
                                cx.notify();
                            }),
                        )
                    }),
            );
        let section = match self.category {
            Category::General => self.general(theme, cx),
            Category::Shortcuts => self.shortcuts(theme, &settings, cx),
            Category::Transcription => self.transcription(theme, &settings, cx),
            Category::Audio => self.audio(theme, &settings),
            Category::Appearance => self.appearance(theme, &settings),
            Category::Privacy => self.privacy(theme, &settings, cx),
            Category::Advanced => self.advanced(theme, &settings, cx),
        };

        widgets::page(
            "settings",
            theme,
            &self.scroll,
            vec![
                widgets::heading(Msg::Settings).into_any_element(),
                menu.into_any_element(),
                section,
            ],
        )
    }
}

/// Saved vs. really listening: a shortcut saved mid-dictation waits for its end.
fn shortcut_state(
    saved: HotkeyBinding,
    active: Option<HotkeyBinding>,
    error: Option<crate::hotkey_hook::BindError>,
) -> (Msg, StatusTone) {
    use crate::hotkey_hook::BindError;
    match (active == Some(saved), error) {
        (true, _) => (Msg::ShortcutActive, StatusTone::Good),
        (false, Some(BindError::Unavailable)) => (Msg::ShortcutUnavailable, StatusTone::Bad),
        (false, Some(_)) => (Msg::ShortcutInactive, StatusTone::Bad),
        (false, None) => (Msg::ShortcutPending, StatusTone::Neutral),
    }
}

fn capture_problem(binding: HotkeyBinding) -> Option<Msg> {
    let HotkeyBinding::Chord(chord) = binding else {
        return None;
    };
    chord.validate().err().map(|problem| match problem {
        ChordProblem::NeedsModifier => Msg::ShortcutNeedsModifier,
        ChordProblem::WindowsKey => Msg::ShortcutWindowsKey,
        ChordProblem::UnsupportedKey => Msg::ShortcutUnsupportedKey,
        ChordProblem::SystemMenu => Msg::ShortcutSystemMenu,
        ChordProblem::Reserved => Msg::ShortcutReserved,
        ChordProblem::NeedsTwoKeys => Msg::ShortcutNeedsTwoKeys,
    })
}

/// What to say under a captured candidate: why it is refused, or a caveat.
fn capture_note(binding: HotkeyBinding) -> Option<(Msg, StatusTone)> {
    if let Some(problem) = capture_problem(binding) {
        return Some((problem, StatusTone::Bad));
    }
    let HotkeyBinding::Chord(chord) = binding else {
        return None;
    };
    if !chord.is_modifier_only() {
        return Some((Msg::ShortcutBothSides, StatusTone::Neutral));
    }
    // Left Alt+Shift and Ctrl+Shift are the Windows layout-switch hotkeys.
    let shift = chord.modifiers & 0x3000;
    let other = chord.modifiers & !0x3000;
    (shift.count_ones() == 1 && matches!(other, 0x100 | 0x200 | 0x400))
        .then_some((Msg::ShortcutLayoutSwitch, StatusTone::Neutral))
}

impl Drop for SettingsView {
    fn drop(&mut self) {
        if self.capturing_hotkey {
            crate::hotkey_hook::stop_capture();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_states_are_explicit() {
        use crate::hotkey_hook::BindError;
        let ctrl = HotkeyBinding::Bare(HotkeyKey::RightControl);
        let alt = HotkeyBinding::Bare(HotkeyKey::RightAlt);
        assert_eq!(
            shortcut_state(ctrl, Some(ctrl), None).0,
            Msg::ShortcutActive
        );
        assert_eq!(
            shortcut_state(alt, Some(ctrl), None).0,
            Msg::ShortcutPending
        );
        assert_eq!(
            shortcut_state(alt, Some(ctrl), Some(BindError::Conflict)).0,
            Msg::ShortcutInactive
        );
        assert_eq!(
            shortcut_state(alt, None, Some(BindError::Unavailable)).0,
            Msg::ShortcutUnavailable
        );
    }

    #[test]
    fn capture_notes_explain_refusals_and_layout_switches() {
        use jot_core::hotkey::HotkeyChord;
        let chord = |modifiers, vk| HotkeyBinding::Chord(HotkeyChord { modifiers, vk });
        assert_eq!(
            capture_note(chord(HotkeyChord::ALT, 0x20)).map(|n| n.0),
            Some(Msg::ShortcutSystemMenu)
        );
        assert_eq!(
            capture_note(chord(HotkeyChord::CTRL | HotkeyChord::WIN, 0x4C)).map(|n| n.0),
            Some(Msg::ShortcutWindowsKey)
        );
        assert_eq!(
            capture_note(chord(0x100, 0)).map(|n| n.0),
            Some(Msg::ShortcutNeedsTwoKeys)
        );
        for layout_switch in [0x400 | 0x1000, 0x100 | 0x1000, 0x200 | 0x2000] {
            assert_eq!(
                capture_note(chord(layout_switch, 0)).map(|n| n.0),
                Some(Msg::ShortcutLayoutSwitch)
            );
        }
        assert_eq!(capture_note(chord(0x800 | 0x2000, 0)), None);
        assert_eq!(capture_note(chord(0x4000 | 0x400, 0)), None);
        assert_eq!(
            capture_note(chord(HotkeyChord::CTRL | HotkeyChord::SHIFT, 0x44)).map(|n| n.0),
            Some(Msg::ShortcutBothSides)
        );
        assert_eq!(capture_note(HotkeyBinding::Bare(HotkeyKey::RightWin)), None);
    }

    #[test]
    fn local_state_never_reports_a_missing_editing_model_as_ready() {
        let tones = |mode, asr, llm| local_state(mode, asr, llm).map(|(_, tone)| tone);
        assert_eq!(
            tones(LocalReformulation::Fast, true, false),
            [StatusTone::Good, StatusTone::Bad]
        );
        assert_eq!(
            tones(LocalReformulation::Quality, true, true),
            [StatusTone::Good, StatusTone::Good]
        );
        assert_eq!(
            tones(LocalReformulation::Off, false, false),
            [StatusTone::Bad, StatusTone::Neutral]
        );
        let [_, (missing, _)] = local_state(LocalReformulation::Fast, true, false);
        let [_, (ready, _)] = local_state(LocalReformulation::Fast, true, true);
        assert_ne!(missing, ready);
    }
}
