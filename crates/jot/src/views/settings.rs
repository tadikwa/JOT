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
use jot_core::hotkey::{DictationActivationMode, HotkeyBinding, HotkeyKey};
use jot_core::settings::{Settings, UiLanguage, UiTheme};

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
        self.hotkey_status = Some(match result {
            Err(error) => (
                match error {
                    BindError::Invalid => Msg::ShortcutInvalid,
                    BindError::Conflict => Msg::ShortcutConflict,
                    BindError::Unavailable => Msg::ShortcutUnavailable,
                },
                StatusTone::Bad,
            ),
            Ok(()) => {
                if self.services.settings.set_hotkey_binding(binding).is_err() {
                    let _ =
                        hotkey_hook::prepare_binding(self.services.settings.get().hotkey_binding());
                    (Msg::ShortcutSaveFailed, StatusTone::Bad)
                } else {
                    self.cancel_hotkey_capture();
                    (Msg::ShortcutSaved, StatusTone::Good)
                }
            }
        });
        cx.notify();
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
                                    !self.capturing_hotkey
                                        && matches!(
                                            self.services.coordinator.state(),
                                            jot_core::state_machine::DictationState::Idle
                                                | jot_core::state_machine::DictationState::Done(_)
                                        ),
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
                                .when_some(self.captured_hotkey, |column, binding| {
                                    column.child(div().child(crate::i18n::hotkey_binding(binding)))
                                })
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap_2()
                                        .child(widgets::button(
                                            "confirm-hotkey",
                                            tr(Msg::ConfirmShortcut),
                                            ButtonKind::Primary,
                                            self.captured_hotkey.is_some(),
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
                        .when(crate::hotkey_hook::binding_error().is_some(), |column| {
                            column.child(widgets::status_line(
                                "hotkey-inactive",
                                tr(Msg::ShortcutInactive),
                                StatusTone::Bad,
                                theme,
                            ))
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

    fn transcription(&self, theme: Theme, settings: &Settings) -> AnyElement {
        widgets::section(
            tr(Msg::Transcription),
            theme,
            vec![
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
            Category::Transcription => self.transcription(theme, &settings),
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

impl Drop for SettingsView {
    fn drop(&mut self) {
        if self.capturing_hotkey {
            crate::hotkey_hook::stop_capture();
        }
    }
}
