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
use jot_core::state_machine::{DictationOutcome, DictationState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Key,
    Microphone,
    TryIt,
    Done,
}

impl Step {
    fn index(self) -> usize {
        match self {
            Step::Key => 1,
            Step::Microphone => 2,
            Step::TryIt => 3,
            Step::Done => 4,
        }
    }
}

const TOTAL_STEPS: usize = 4;

pub struct OnboardingView {
    services: Services,
    step: Step,
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

        // Someone reopening setup with a key already stored starts past step 1.
        let step = if credentials::api_key().is_some() {
            Step::Microphone
        } else {
            Step::Key
        };

        Self {
            services,
            step,
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
                        this.step = Step::Microphone;
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
                        this.step = Step::Microphone;
                    }
                }
                cx.notify();
            });
        })
        .detach();
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
                progress(step, theme).into_any_element(),
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

fn progress(step: Step, theme: Theme) -> impl IntoElement {
    let label = match step {
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
                .children((1..=TOTAL_STEPS).map(|index| {
                    div()
                        .h(px(6.0))
                        .w(if index == step.index() {
                            px(24.0)
                        } else {
                            px(6.0)
                        })
                        .rounded_full()
                        .bg(if index <= step.index() {
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
                        .child(tf(Msg::Step, &[&step.index(), &TOTAL_STEPS])),
                )
                .child(div().font_weight(theme::weight::MEDIUM).child(tr(label))),
        )
}

fn paragraph(text: &str, theme: Theme) -> impl IntoElement {
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
    fn every_step_is_within_the_advertised_count() {
        for step in [Step::Key, Step::Microphone, Step::TryIt, Step::Done] {
            assert!(step.index() >= 1 && step.index() <= TOTAL_STEPS);
        }
        assert_eq!(Step::Done.index(), TOTAL_STEPS);
    }
}
