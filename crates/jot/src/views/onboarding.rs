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

//! Setup: about two minutes, and it makes you try the thing once so you believe it.
//!
//! The key check is deliberately three-valued. A key the server actively
//! rejected is a hard block; a check we could not perform — a captive portal, a
//! VPN coming up — must let the user continue, or setup strands people whose
//! key is fine.

use crate::i18n::{LocalizedText, Msg, hotkey_binding, tf, tr};
use crate::services::Services;
use crate::text_field::{Submit, TextField};
use crate::theme::{self, Theme};
use crate::views::widgets::{self, ButtonKind, StatusTone};
use gpui::{Context, Entity, Role, Window, div, prelude::*, px};
use jot_core::audio;
use jot_core::credentials;
use jot_core::gemini::KeyCheck;
use jot_core::hotkey::DictationActivationMode;
use jot_core::local::{self, Download, Pack};
use jot_core::settings::TranscriptionEngine;
use jot_core::state_machine::{DictationOutcome, DictationState};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Engine,
    Activation,
    Key,
    Models,
    Microphone,
    TryIt,
    Done,
}

/// What the first screen asks: no key is needed for local only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    Gemini,
    Local,
    Both,
}

/// The steps a choice goes through; before choosing, the Gemini path is shown.
fn route(choice: Option<Choice>) -> &'static [Step] {
    use Step::*;
    match choice {
        Some(Choice::Local) => &[Engine, Activation, Models, Microphone, TryIt, Done],
        Some(Choice::Both) => &[Engine, Activation, Key, Models, Microphone, TryIt, Done],
        _ => &[Engine, Activation, Key, Microphone, TryIt, Done],
    }
}

/// What the activation step shows first: Toggle on a new profile (the
/// preferred mode), the current mode when setup is reopened from the tray, so
/// rerunning the guide never changes a configured user's setting.
fn initial_activation(settings: &jot_core::settings::Settings) -> DictationActivationMode {
    if settings.has_completed_onboarding {
        settings.dictation_activation_mode
    } else {
        DictationActivationMode::Toggle
    }
}

/// The local packs the "100 % local" choice installs: transcription and the
/// fast editing model, the benchmarked default.
const LOCAL_PACKS: [Pack; 2] = [Pack::Transcription, Pack::Fast];

pub struct OnboardingView {
    services: Services,
    step: Step,
    choice: Option<Choice>,
    /// Selected on the activation step; written to settings on "Continue".
    activation: DictationActivationMode,
    /// The pack being downloaded, once the user confirmed.
    download: Option<Arc<Download>>,
    api_key: Entity<TextField>,
    status: Option<(LocalizedText, StatusTone)>,
    checking: bool,
    /// Set once a dictation actually completes while this window is open.
    tried_it: bool,
    scroll: widgets::PageScroll,
    _subscriptions: Vec<gpui::Subscription>,
}

impl OnboardingView {
    pub fn new(services: Services, cx: &mut Context<Self>) -> Self {
        let api_key = cx.new(|cx| TextField::new("onboarding-api-key", "AIza…", cx).masked());
        let subscription = cx.subscribe(&api_key, |this: &mut Self, _, _: &Submit, cx| {
            this.save_key(cx)
        });

        // Step 3 advances by itself the moment a dictation lands, so the user
        // is told they succeeded by the thing succeeding.
        let mut updates = services.coordinator.subscribe();
        cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                let landed = matches!(
                    update,
                    jot_core::coordinator::CoordinatorUpdate::State(DictationState::Done(
                        DictationOutcome::Inserted
                            | DictationOutcome::Unconfirmed
                            | DictationOutcome::CopiedToClipboard
                            | DictationOutcome::AwaitingChip
                    ))
                );
                if !landed {
                    continue;
                }
                let updated = this.update(cx, |this, cx| {
                    this.tried_it = true;
                    if this.step == Step::TryIt {
                        this.step = Step::Done;
                    }
                    cx.notify();
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();

        let activation = initial_activation(&services.settings.get());
        Self {
            services,
            step: Step::Engine,
            choice: None,
            activation,
            download: None,
            api_key,
            status: None,
            checking: false,
            tried_it: false,
            scroll: widgets::PageScroll::new(),
            _subscriptions: vec![subscription],
        }
    }

    fn save_key(&mut self, cx: &mut Context<Self>) {
        let key = self
            .api_key
            .update(cx, |field, _| field.content().trim().to_string());
        if key.is_empty() {
            self.status = Some((Msg::PasteKeyFirst.into(), StatusTone::Bad));
            cx.notify();
            return;
        }
        if let Err(error) = credentials::set_api_key(&key) {
            tracing::error!(%error, "storing the API key failed");
            self.status = Some((Msg::KeyStoreFailed.into(), StatusTone::Bad));
            cx.notify();
            return;
        }
        self.api_key.update(cx, |field, _| {
            field.take_content();
        });

        self.checking = true;
        self.status = Some((Msg::KeyChecking.into(), StatusTone::Neutral));
        cx.notify();

        let client = self.services.client.clone();
        let config = self.services.settings.get().gemini_config();
        cx.spawn(async move |this, cx| {
            let check = client.validate_key(&config.endpoint).await;
            // "Your key works" should mean the whole pipeline works, not just
            // that the key authenticates — so the model is checked too.
            let model = if check == KeyCheck::Valid {
                client
                    .resolve_available_model(
                        std::slice::from_ref(&config.transcribe_model),
                        &config.endpoint,
                    )
                    .await
            } else {
                None
            };

            let _ = this.update(cx, |this, cx| {
                this.checking = false;
                match check {
                    KeyCheck::Valid if model.is_some() => {
                        this.status = Some((Msg::KeyWorks.into(), StatusTone::Good));
                        this.step = this.after(Step::Key);
                    }
                    KeyCheck::Valid => {
                        // The key is fine; this model is not available to it.
                        // Naming the model sends them to the right fix.
                        this.status = Some((
                            LocalizedText::with(
                                Msg::KeyModelUnavailable,
                                &[&config.transcribe_model],
                            ),
                            StatusTone::Bad,
                        ));
                    }
                    KeyCheck::Rejected(_) => {
                        this.status = Some((Msg::KeyRejected.into(), StatusTone::Bad));
                    }
                    KeyCheck::Unreachable => {
                        // Never strand someone behind a captive portal.
                        this.status = Some((Msg::KeyUnreachable.into(), StatusTone::Neutral));
                        this.step = this.after(Step::Key);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The step after `step` on the chosen route, past what is already done:
    /// a stored key, installed models.
    fn after(&self, step: Step) -> Step {
        let route = route(self.choice);
        let position = route.iter().position(|s| *s == step).unwrap_or(0);
        route[position + 1..]
            .iter()
            .copied()
            .find(|next| match next {
                Step::Key => credentials::api_key().is_none(),
                Step::Models => !LOCAL_PACKS.iter().all(|p| p.installed()),
                _ => true,
            })
            .unwrap_or(Step::Done)
    }

    /// The step before `step` on the chosen route, skipping what `after` skips.
    fn before(&self, step: Step) -> Step {
        let route = route(self.choice);
        let position = route.iter().position(|s| *s == step).unwrap_or(0);
        route[..position]
            .iter()
            .rev()
            .copied()
            .find(|previous| match previous {
                Step::Key => credentials::api_key().is_none(),
                Step::Models => !LOCAL_PACKS.iter().all(|p| p.installed()),
                _ => true,
            })
            .unwrap_or(Step::Engine)
    }

    /// Writes the existing setting the hotkey hook and Settings already read.
    fn confirm_activation(&mut self, cx: &mut Context<Self>) {
        let mode = self.activation;
        self.services
            .settings
            .update(Some("dictationActivationMode"), |s| {
                s.dictation_activation_mode = mode
            });
        self.status = None;
        self.step = self.after(Step::Activation);
        cx.notify();
    }

    fn choose(&mut self, choice: Choice, cx: &mut Context<Self>) {
        self.choice = Some(choice);
        // "Both" starts on Gemini, which works the moment the key is checked;
        // the local models are one click away in Settings once downloaded.
        let engine = match choice {
            Choice::Local => TranscriptionEngine::Local,
            Choice::Gemini | Choice::Both => TranscriptionEngine::Gemini,
        };
        self.services
            .settings
            .update(Some("transcriptionEngine"), |s| {
                s.transcription_engine = engine
            });
        self.status = None;
        self.step = self.after(Step::Engine);
        cx.notify();
    }

    /// Only after the explicit click on the button that states the size.
    fn download_models(&mut self, cx: &mut Context<Self>) {
        self.status = None;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            for pack in LOCAL_PACKS.into_iter().filter(|p| !p.installed()) {
                let progress = Arc::new(Download::default());
                if this
                    .update(cx, |this, cx| {
                        this.download = Some(progress.clone());
                        cx.notify();
                    })
                    .is_err()
                {
                    progress.cancel.store(true, Ordering::Relaxed);
                    return;
                }
                jot_core::runtime::spawn(local::install(pack, progress.clone()));
                while !progress.finished.load(Ordering::Acquire) {
                    executor.timer(Duration::from_millis(300)).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        // The window closed: do not keep downloading unseen.
                        progress.cancel.store(true, Ordering::Relaxed);
                        return;
                    }
                }
                let error = progress.error.lock().clone();
                if let Some(error) = error {
                    let _ = this.update(cx, |this, cx| {
                        this.download = None;
                        this.status = Some(if error == local::CANCELLED {
                            (Msg::ModelsLater.into(), StatusTone::Neutral)
                        } else {
                            (
                                LocalizedText::with(Msg::ModelsFailed, &[&error]),
                                StatusTone::Bad,
                            )
                        });
                        cx.notify();
                    });
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.download = None;
                this.status = Some((Msg::ModelsReady.into(), StatusTone::Good));
                this.step = this.after(Step::Models);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn check_microphone(&mut self, cx: &mut Context<Self>) {
        let devices = audio::input_device_names();
        if devices.is_empty() {
            self.status = Some((Msg::SetupNoMic.into(), StatusTone::Bad));
        } else {
            self.status = Some((
                LocalizedText::with(Msg::SetupFoundMic, &[&devices.join(", ")]),
                StatusTone::Good,
            ));
            self.step = Step::TryIt;
        }
        cx.notify();
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        // The one celebratory sound in the app, for the one moment that earns
        // it — and only if they actually got a dictation through.
        if self.tried_it {
            crate::sound::play(
                crate::sound::Earcon::Celebration,
                self.services.settings.get().sounds_enabled,
            );
        }
        self.services
            .settings
            .update(Some("hasCompletedOnboarding"), |settings| {
                // A deliberate "I'll do it later" must not re-trap them in the
                // wizard on every launch.
                settings.has_completed_onboarding = true;
            });
        super::open_workspace(super::Route::Dashboard, &self.services, cx);
        cx.notify();
    }

    fn body(&self, theme: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let settings = self.services.settings.get();
        let key = hotkey_binding(settings.hotkey_binding());
        let toggle = settings.dictation_activation_mode == DictationActivationMode::Toggle;
        match self.step {
            Step::Engine => {
                let local_size = super::settings::size_label(
                    LOCAL_PACKS.iter().map(|p| p.missing_bytes()).sum(),
                );
                let option = |id: &'static str,
                              label: Msg,
                              hint: String,
                              choice: Choice,
                              cx: &mut Context<Self>| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::spacing::XS)
                        .child(widgets::hug(widgets::button(
                            id,
                            tr(label),
                            ButtonKind::Secondary,
                            true,
                            theme,
                            cx.listener(move |this, _, _, cx| this.choose(choice, cx)),
                        )))
                        .child(paragraph(&hint, theme))
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::spacing::M)
                    .child(paragraph(tr(Msg::SetupEngineHint), theme))
                    .child(option(
                        "choose-gemini",
                        Msg::ChooseGemini,
                        tr(Msg::ChooseGeminiHint).to_string(),
                        Choice::Gemini,
                        cx,
                    ))
                    .child(option(
                        "choose-local",
                        Msg::ChooseLocal,
                        tf(Msg::ChooseLocalHint, &[&local_size]),
                        Choice::Local,
                        cx,
                    ))
                    .child(option(
                        "choose-both",
                        Msg::ChooseBoth,
                        tr(Msg::ChooseBothHint).to_string(),
                        Choice::Both,
                        cx,
                    ))
                    .into_any_element()
            }

            Step::Activation => {
                let mut options = div()
                    .id("onboarding-activation-options")
                    .role(Role::RadioGroup)
                    .aria_label(tr(Msg::StepActivation))
                    .flex()
                    .flex_wrap()
                    .gap(theme::spacing::XS);
                for (id, mode, label, description) in [
                    (
                        "setup-toggle",
                        DictationActivationMode::Toggle,
                        Msg::Toggle,
                        Msg::ToggleHint,
                    ),
                    (
                        "setup-hold",
                        DictationActivationMode::Hold,
                        Msg::Hold,
                        Msg::HoldHint,
                    ),
                ] {
                    let selected = self.activation == mode;
                    options = options.child(
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
                                this.activation = mode;
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
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::spacing::S)
                    .child(paragraph(tr(Msg::SetupActivationHint), theme))
                    .child(options)
                    .child(widgets::hug(widgets::button(
                        "confirm-activation",
                        tr(Msg::Continue),
                        ButtonKind::Primary,
                        true,
                        theme,
                        cx.listener(|this, _, _, cx| this.confirm_activation(cx)),
                    )))
                    .into_any_element()
            }

            Step::Models => {
                let missing: u64 = LOCAL_PACKS.iter().map(|p| p.missing_bytes()).sum();
                let files = LOCAL_PACKS
                    .iter()
                    .flat_map(|pack| pack.artifacts())
                    .map(|artifact| {
                        paragraph(
                            &format!(
                                "• {} — {} — SHA-256 {}…",
                                artifact.file,
                                super::settings::size_label(artifact.bytes),
                                &artifact.sha256[..16]
                            ),
                            theme,
                        )
                    });
                let licences = LOCAL_PACKS
                    .iter()
                    .map(|pack| paragraph(super::settings::pack_source(*pack), theme));
                let action = match &self.download {
                    Some(progress) => {
                        let progress = progress.clone();
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(tf(
                                Msg::DownloadingModels,
                                &[&format!("{:.0}", progress.fraction() * 100.0)],
                            ))
                            .child(widgets::button(
                                "cancel-models",
                                tr(Msg::CancelDownload),
                                ButtonKind::Secondary,
                                true,
                                theme,
                                move |_, _, _| progress.cancel.store(true, Ordering::Relaxed),
                            ))
                            .into_any_element()
                    }
                    None => div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(widgets::button(
                            "download-models",
                            tf(
                                Msg::DownloadModels,
                                &[&super::settings::size_label(missing)],
                            ),
                            ButtonKind::Primary,
                            true,
                            theme,
                            cx.listener(|this, _, _, cx| this.download_models(cx)),
                        ))
                        .child(widgets::button(
                            "models-later",
                            tr(Msg::ModelsLater),
                            ButtonKind::Secondary,
                            true,
                            theme,
                            cx.listener(|this, _, _, cx| {
                                this.step = this.after(Step::Models);
                                cx.notify();
                            }),
                        ))
                        .into_any_element(),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::spacing::S)
                    .child(paragraph(tr(Msg::SetupModelsHint), theme))
                    .children(files)
                    .children(licences)
                    .child(paragraph(
                        &tf(
                            Msg::ModelsVerified,
                            &[&local::local_root().display().to_string()],
                        ),
                        theme,
                    ))
                    .child(action)
                    .into_any_element()
            }

            Step::Key => div()
                .flex()
                .flex_col()
                .gap(theme::spacing::S)
                .child(paragraph(tr(Msg::SetupKeyHint), theme))
                .child(widgets::hug(widgets::button(
                    "open-key-page",
                    tr(Msg::GetGeminiKey),
                    ButtonKind::Secondary,
                    true,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        if open_gemini_key_page().is_err() {
                            this.status = Some((Msg::BrowserOpenFailed.into(), StatusTone::Bad));
                            cx.notify();
                        }
                    }),
                )))
                .child(paragraph(tr(Msg::GetGeminiKeyHint), theme))
                .child(self.api_key.clone())
                .child(widgets::hug(widgets::button(
                    "save-key",
                    if self.checking {
                        tr(Msg::KeyChecking)
                    } else {
                        tr(Msg::SaveAndCheck)
                    },
                    ButtonKind::Primary,
                    !self.checking,
                    theme,
                    cx.listener(|this, _, _, cx| this.save_key(cx)),
                )))
                .into_any_element(),

            Step::Microphone => div()
                .flex()
                .flex_col()
                .gap(theme::spacing::S)
                .child(paragraph(tr(Msg::SetupMicHint), theme))
                .child(widgets::hug(widgets::button(
                    "check-mic",
                    tr(Msg::CheckMic),
                    ButtonKind::Primary,
                    true,
                    theme,
                    cx.listener(|this, _, _, cx| this.check_microphone(cx)),
                )))
                .into_any_element(),

            Step::TryIt => div()
                .flex()
                .flex_col()
                .gap(theme::spacing::S)
                .child(paragraph(
                    &tf(
                        if toggle {
                            Msg::SetupTryToggle
                        } else {
                            Msg::SetupTryHold
                        },
                        &[&key],
                    ),
                    theme,
                ))
                .child(paragraph(tr(Msg::SetupTryExample), theme))
                .child(widgets::hug(widgets::button(
                    "skip-try",
                    tr(Msg::Skip),
                    ButtonKind::Secondary,
                    true,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.step = Step::Done;
                        cx.notify();
                    }),
                )))
                .into_any_element(),

            Step::Done => div()
                .flex()
                .flex_col()
                .gap(theme::spacing::S)
                .child(paragraph(tr(Msg::SetupDone), theme))
                .child(paragraph(
                    &tf(
                        if toggle {
                            Msg::SetupToggleShortcut
                        } else {
                            Msg::SetupHoldShortcut
                        },
                        &[&key],
                    ),
                    theme,
                ))
                .child(paragraph(tr(Msg::SetupDoneGuide), theme))
                .child(widgets::hug(widgets::button(
                    "finish",
                    tr(Msg::Done),
                    ButtonKind::Primary,
                    true,
                    theme,
                    cx.listener(|this, _, window, cx| {
                        this.finish(cx);
                        window.remove_window();
                    }),
                )))
                .into_any_element(),
        }
    }
}

impl Render for OnboardingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::ui(window);
        let step = self.step;
        let body = self.body(theme, cx);

        widgets::page(
            "onboarding",
            theme,
            &self.scroll,
            vec![
                widgets::heading(Msg::Setup).into_any_element(),
                progress(step, self.choice, theme).into_any_element(),
                body,
                div()
                    .when_some(self.status.as_ref(), |row, (message, tone)| {
                        row.child(widgets::status_line(
                            "onboarding-status",
                            message.render(),
                            *tone,
                            theme,
                        ))
                    })
                    .into_any_element(),
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::spacing::S)
                    .child(div().h(px(1.0)).w_full().bg(theme.outline_variant))
                    .when(step != Step::Engine, |column| {
                        column.child(widgets::hug(widgets::button(
                            "back",
                            tr(Msg::Back),
                            ButtonKind::Secondary,
                            true,
                            theme,
                            cx.listener(|this, _, _, cx| {
                                this.status = None;
                                this.step = this.before(this.step);
                                cx.notify();
                            }),
                        )))
                    })
                    .child(widgets::hug(widgets::button(
                        "later",
                        tr(Msg::Later),
                        ButtonKind::Secondary,
                        true,
                        theme,
                        cx.listener(|this, _, window, cx| {
                            this.finish(cx);
                            window.remove_window();
                        }),
                    )))
                    .into_any_element(),
            ],
        )
    }
}

fn progress(step: Step, choice: Option<Choice>, theme: Theme) -> impl IntoElement {
    let route = route(choice);
    let index = route.iter().position(|s| *s == step).unwrap_or(0) + 1;
    let total = route.len();
    let label = match step {
        Step::Engine => Msg::StepEngine,
        Step::Activation => Msg::StepActivation,
        Step::Models => Msg::StepModels,
        Step::Key => Msg::StepKey,
        Step::Microphone => Msg::StepMicrophone,
        Step::TryIt => Msg::StepTry,
        Step::Done => Msg::StepDone,
    };
    div()
        .id("progress")
        .role(Role::Status)
        .flex()
        .flex_col()
        .gap(theme::spacing::XS)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme::spacing::XS)
                .children((1..=total).map(|dot| {
                    div()
                        .h(px(6.0))
                        .w(if dot == index { px(24.0) } else { px(6.0) })
                        .rounded_full()
                        .bg(if dot <= index {
                            theme.primary
                        } else {
                            theme.outline_variant
                        })
                })),
        )
        .child(
            div()
                .flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_size(theme::type_scale::LABEL)
                        .text_color(theme.on_surface_variant)
                        .child(tf(Msg::Step, &[&index, &total])),
                )
                .child(div().font_weight(theme::weight::MEDIUM).child(tr(label))),
        )
}

fn paragraph(text: &str, theme: Theme) -> impl IntoElement + use<> {
    div()
        .text_size(theme::type_scale::BODY)
        .text_color(theme.on_surface)
        .child(text.to_string())
}

/// ShellExecute reports association/launch failures that GPUI's fire-and-forget open_url hides.
pub(super) fn open_gemini_key_page() -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use windows::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
        use windows::core::w;
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                w!("https://aistudio.google.com/apikey"),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
        anyhow::ensure!(
            result.0 as isize > 32,
            "Windows could not launch the browser"
        );
        Ok(())
    }
    #[cfg(not(windows))]
    anyhow::bail!("browser launch is implemented for Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_only_never_asks_for_a_key_and_every_route_ends_done() {
        assert!(!route(Some(Choice::Local)).contains(&Step::Key));
        assert!(!route(Some(Choice::Gemini)).contains(&Step::Models));
        assert!(route(Some(Choice::Both)).contains(&Step::Key));
        assert!(route(Some(Choice::Both)).contains(&Step::Models));
        for choice in [
            None,
            Some(Choice::Gemini),
            Some(Choice::Local),
            Some(Choice::Both),
        ] {
            let steps = route(choice);
            assert_eq!(steps.first(), Some(&Step::Engine));
            assert_eq!(steps.get(1), Some(&Step::Activation));
            assert_eq!(steps.last(), Some(&Step::Done));
        }
    }

    #[test]
    fn a_new_profile_starts_on_toggle_and_a_configured_one_keeps_its_mode() {
        use jot_core::settings::Settings;
        let fresh = Settings::default();
        assert_eq!(initial_activation(&fresh), DictationActivationMode::Toggle);
        for mode in [
            DictationActivationMode::Hold,
            DictationActivationMode::Toggle,
        ] {
            let configured = Settings {
                has_completed_onboarding: true,
                dictation_activation_mode: mode,
                ..Settings::default()
            };
            assert_eq!(initial_activation(&configured), mode);
        }
    }
}
