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

//! History: the proof that nothing was ever lost.
//!
//! It is a library of words plus the things needing attention — never an event
//! log. Anything with a transcript shows; so do retryable failures, queued
//! dictations, and long cancelled recordings. Everything else was discarded at
//! the source.

use crate::i18n::{Locale, Msg, locale, tf, tr};
use crate::services::Services;
use crate::text_field::{self, Submit, TextField};
use crate::theme::{self, Theme};
use crate::views::widgets::{self, ButtonKind, StatusTone};
use gpui::{Context, Entity, Role, SharedString, Window, div, prelude::*, px, uniform_list};
use jot_core::history::DictationRecord;
use jot_core::meta::SessionStatus;
use jot_core::recovery::RetryOutcome;
use time::OffsetDateTime;
use time::macros::format_description;

const PAGE_SIZE: usize = 500;
const ROW_HEIGHT: f32 = 116.0;
/// The transcript line gets a fixed box; `flex_1` let the actions squeeze it
/// to zero and a failure row then explained nothing.
const TEXT_HEIGHT: f32 = 44.0;

pub struct HistoryView {
    services: Services,
    search: Entity<TextField>,
    records: Vec<DictationRecord>,
    pending_delete: Option<String>,
    status: Option<(Msg, StatusTone)>,
    _subscriptions: Vec<gpui::Subscription>,
}

impl HistoryView {
    pub fn new(services: Services, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextField::new("history-search", Msg::Search, cx));
        let subscription = cx.subscribe(&search, |this: &mut Self, _, _: &Submit, cx| {
            this.reload(cx);
        });

        // History refreshes as dictations land, retries drain, and rows are
        // deleted — no polling.
        let mut changes = services.history.subscribe();
        cx.spawn(async move |this, cx| {
            while changes.recv().await.is_ok() {
                if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        let mut view = Self {
            services,
            search,
            records: Vec::new(),
            pending_delete: None,
            status: None,
            _subscriptions: vec![subscription],
        };
        view.reload(cx);
        view
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).content().to_string();
        let query = (!query.trim().is_empty()).then_some(query);
        self.records = self.services.history.records(query.as_deref(), PAGE_SIZE);
        cx.notify();
    }

    fn retry(&mut self, record: DictationRecord, cx: &mut Context<Self>) {
        self.status = Some((Msg::Retrying, StatusTone::Neutral));
        cx.notify();
        let queue = self.services.retry_queue.clone();
        cx.spawn(async move |this, cx| {
            let outcome = queue.retry_single(&record).await;
            // A silent no-op reads as broken, so every outcome says something.
            let status = match outcome {
                RetryOutcome::Recovered => (Msg::RetryRecovered, StatusTone::Good),
                RetryOutcome::StillOffline => (Msg::RetryOffline, StatusTone::Neutral),
                RetryOutcome::Blocked => (Msg::RetryBlocked, StatusTone::Bad),
                RetryOutcome::Failed => (Msg::RetryFailed, StatusTone::Bad),
                RetryOutcome::AlreadyDone => (Msg::RetryAlreadyDone, StatusTone::Neutral),
                RetryOutcome::Busy => (Msg::RetryBusy, StatusTone::Neutral),
            };
            let _ = this.update(cx, |this, cx| {
                this.status = Some(status);
                this.reload(cx);
            });
        })
        .detach();
    }

    pub fn dismiss_confirmation(&mut self, cx: &mut Context<Self>) {
        if self.pending_delete.take().is_some() {
            cx.notify();
        }
    }

    fn row(
        &self,
        record: &DictationRecord,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let status = record.status().unwrap_or(SessionStatus::Failed);
        let text = record.display_text().to_string();
        let has_text = !text.is_empty();
        let retryable = matches!(
            status,
            SessionStatus::Failed | SessionStatus::QueuedForRetry | SessionStatus::Cancelled
        ) && !has_text;

        let subtitle = format!(
            "{} · {}{}",
            format_time(record.started_at),
            record
                .target_app_name
                .as_deref()
                .unwrap_or(tr(Msg::UnknownApp)),
            record
                .duration_seconds
                .map(|seconds| {
                    let mut value = format!("{seconds:.1}");
                    if locale() == Locale::Fr {
                        value = value.replace('.', ",");
                    }
                    format!(" · {}", tf(Msg::Duration, &[&value]))
                })
                .unwrap_or_default()
        );

        let id = record.id.clone();
        let confirming = self.pending_delete.as_deref() == Some(id.as_str());
        let record_for_retry = record.clone();

        div()
            .id(SharedString::from(format!(
                "history-row-{}",
                record.id.clone()
            )))
            .role(Role::ListItem)
            .aria_label(if has_text {
                SharedString::from(text.clone())
            } else {
                SharedString::from(status_label(status).to_string())
            })
            // Without this the row shrinks to its content and the separator
            // beneath it stops half way across the list.
            .w_full()
            .flex()
            .flex_col()
            .gap(theme::spacing::XXS)
            .h(px(ROW_HEIGHT))
            .p(theme::spacing::S)
            .border_b_1()
            .border_color(theme.outline_variant)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::spacing::XS)
                    .child(status_chip(status, theme))
                    .child(
                        div()
                            .text_size(theme::type_scale::LABEL)
                            .text_color(theme.on_surface_variant)
                            .child(subtitle),
                    ),
            )
            .child(
                div()
                    .h(px(TEXT_HEIGHT))
                    .overflow_hidden()
                    .text_size(theme::type_scale::BODY_LARGE)
                    .text_color(if has_text {
                        theme.on_surface
                    } else {
                        theme.on_surface_variant
                    })
                    .child(if confirming {
                        tr(Msg::DeleteDictation).to_string()
                    } else if has_text {
                        text.clone()
                    } else {
                        error_summary(record.error_code.as_deref(), status)
                    }),
            )
            .child(
                div()
                    .flex()
                    .gap(theme::spacing::XS)
                    .when(confirming, |row| {
                        row.child(widgets::button(
                            SharedString::from(format!("cancel-delete-{id}")),
                            tr(Msg::Cancel),
                            ButtonKind::Secondary,
                            true,
                            theme,
                            cx.listener(|this, _, _, cx| this.dismiss_confirmation(cx)),
                        ))
                    })
                    .when(has_text && !confirming, |row| {
                        let text = text.clone();
                        row.child(widgets::button(
                            SharedString::from(format!("copy-{}", id.clone())),
                            tr(Msg::Copy),
                            ButtonKind::Secondary,
                            true,
                            theme,
                            cx.listener(move |this, _, _, cx| {
                                text_field::set_clipboard_text(text.clone(), cx);
                                this.status = Some((Msg::CopiedFeedback, StatusTone::Good));
                                cx.notify();
                            }),
                        ))
                    })
                    .when(retryable && !confirming, |row| {
                        row.child(widgets::button(
                            SharedString::from(format!("retry-{}", id.clone())),
                            tr(Msg::Retry),
                            ButtonKind::Primary,
                            true,
                            theme,
                            cx.listener({
                                let record = record_for_retry.clone();
                                move |this, _, _, cx| this.retry(record.clone(), cx)
                            }),
                        ))
                    })
                    .child(widgets::button(
                        SharedString::from(format!("delete-{}", id.clone())),
                        tr(Msg::Delete),
                        ButtonKind::Destructive,
                        true,
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            if confirming && this.pending_delete.as_deref() == Some(id.as_str()) {
                                this.pending_delete = None;
                                this.services.history.delete(&id, true);
                                this.reload(cx);
                            } else {
                                this.pending_delete = Some(id.clone());
                                cx.notify();
                            }
                        }),
                    )),
            )
    }
}

impl Render for HistoryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::ui(window);
        let stats = self.services.history.stats();
        let count = self.records.len();
        let records = self.records.clone();

        let summary = if count == 0 && stats.total_dictations == 0 {
            None
        } else if stats.total_dictations == 0 {
            Some(tf(Msg::SavedPending, &[&count]))
        } else if stats.average_wpm > 0 {
            Some(tf(
                Msg::HistorySummarySpeed,
                &[
                    &stats.total_dictations,
                    &stats.total_words,
                    &stats.average_wpm,
                ],
            ))
        } else {
            Some(tf(
                Msg::HistorySummary,
                &[&stats.total_dictations, &stats.total_words],
            ))
        };

        let list = if count == 0 {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.on_surface_variant)
                .child(if self.search.read(cx).is_empty() {
                    tr(Msg::HistoryHint)
                } else {
                    tr(Msg::NoMatches)
                })
                .into_any_element()
        } else {
            div()
                .id("history-list")
                .role(Role::List)
                .aria_label(tr(Msg::DictationList))
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .rounded(theme::radius::MEDIUM)
                .border_1()
                .border_color(theme.outline_variant)
                .bg(theme.surface)
                .child(
                    // Virtualized: a heavy user's History is thousands of rows,
                    // and rendering all of them would stall the window.
                    uniform_list(
                        "history-rows",
                        count,
                        cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                            let theme = Theme::ui(window);
                            range
                                .filter_map(|index| {
                                    records.get(index).map(|record| {
                                        this.row(record, theme, cx).into_any_element()
                                    })
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .size_full(),
                )
                .into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(theme::spacing::S)
            .p(theme::spacing::L)
            .font_family(theme::ui_font())
            .bg(theme.window_background)
            .text_color(theme.on_surface)
            .child(widgets::heading(Msg::History))
            .when_some(summary, |column, summary| {
                column.child(
                    div()
                        .text_size(theme::type_scale::LABEL)
                        .text_color(theme.on_surface_variant)
                        .child(summary),
                )
            })
            .child(self.search.clone())
            .when_some(self.status, |column, (message, tone)| {
                column.child(widgets::status_line(
                    "history-status",
                    tr(message),
                    tone,
                    theme,
                ))
            })
            .child(list)
    }
}

fn format_time(at: OffsetDateTime) -> String {
    let local = time::UtcOffset::current_local_offset()
        .map(|offset| at.to_offset(offset))
        .unwrap_or(at);
    let pattern = match locale() {
        Locale::En => format_description!("[day] [month repr:short] [hour]:[minute]"),
        Locale::Fr => format_description!("[day]/[month]/[year] à [hour]:[minute]"),
    };
    local
        .format(pattern)
        .unwrap_or_else(|_| tr(Msg::UnknownTime).into())
}

/// The short word that goes in the chip. The row's body line carries the
/// explanation, so a chip repeating a whole sentence just crowds the row.
fn status_chip_label(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Inserted => tr(Msg::Inserted),
        SessionStatus::CopiedToClipboard | SessionStatus::AwaitingChip => tr(Msg::Copied),
        SessionStatus::HeldSecure => tr(Msg::Held),
        SessionStatus::QueuedForRetry => tr(Msg::Queued),
        SessionStatus::Recovered => tr(Msg::Recovered),
        SessionStatus::Cancelled => tr(Msg::Cancelled),
        SessionStatus::Failed => tr(Msg::Failed),
        SessionStatus::Silent => tr(Msg::NoSpeech),
        SessionStatus::Recording | SessionStatus::Recorded | SessionStatus::Transcribing => {
            tr(Msg::InFlight)
        }
    }
}

fn status_label(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Inserted => tr(Msg::Inserted),
        SessionStatus::CopiedToClipboard => tr(Msg::CopiedClipboard),
        SessionStatus::AwaitingChip => tr(Msg::CopiedReady),
        SessionStatus::HeldSecure => tr(Msg::HeldPassword),
        SessionStatus::QueuedForRetry => tr(Msg::QueuedDetail),
        SessionStatus::Recovered => tr(Msg::RecoveredDetail),
        SessionStatus::Cancelled => tr(Msg::CancelledDetail),
        SessionStatus::Failed => tr(Msg::Failed),
        SessionStatus::Silent => tr(Msg::NoSpeech),
        SessionStatus::Recording | SessionStatus::Recorded | SessionStatus::Transcribing => {
            tr(Msg::InFlight)
        }
    }
}

/// Display copy for stored error codes. Raw API diagnostics stay in the record.
fn error_summary(code: Option<&str>, status: SessionStatus) -> String {
    match code {
        Some("offline") => tr(Msg::HistoryOffline),
        Some("network") => tr(Msg::HistoryNetwork),
        Some("timeout") => tr(Msg::HistoryTimeout),
        Some("auth") => tr(Msg::Auth),
        Some("model") => tr(Msg::ModelAccess),
        Some("quota") => tr(Msg::Quota),
        Some("rate_limit") => tr(Msg::HistoryRate),
        Some("bad_request") => tr(Msg::HistoryBadRequest),
        Some("safety") => tr(Msg::HistorySafety),
        Some("empty") => tr(Msg::HistoryEmpty),
        Some("tooNoisy") => tr(Msg::HistoryNoise),
        Some("disk_write") => tr(Msg::HistoryDisk),
        Some("engine_died") => tr(Msg::HistoryMic),
        Some("audio_purged") | Some("no_audio_file") => tr(Msg::HistoryNoAudio),
        _ => status_label(status),
    }
    .to_string()
}

fn status_chip(status: SessionStatus, theme: Theme) -> impl IntoElement {
    let (background, foreground) = match status {
        SessionStatus::Inserted | SessionStatus::Recovered => {
            (widgets::with_alpha(theme.success, 0.16), theme.success)
        }
        SessionStatus::Failed => (theme.error_container, theme.on_error_container),
        SessionStatus::QueuedForRetry | SessionStatus::Cancelled => (
            widgets::with_alpha(theme.on_surface_variant, 0.14),
            theme.on_surface_variant,
        ),
        _ => (theme.primary_container, theme.on_primary_container),
    };
    div()
        .px(theme::spacing::XS)
        .py(px(2.0))
        .rounded(theme::radius::XS)
        .bg(background)
        .text_size(theme::type_scale::LABEL_SMALL)
        .font_weight(theme::weight::MEDIUM)
        .text_color(foreground)
        .child(status_chip_label(status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_has_copy_and_none_of_it_lies_about_the_clipboard() {
        for status in [
            SessionStatus::Inserted,
            SessionStatus::CopiedToClipboard,
            SessionStatus::AwaitingChip,
            SessionStatus::HeldSecure,
            SessionStatus::QueuedForRetry,
            SessionStatus::Recovered,
            SessionStatus::Cancelled,
            SessionStatus::Failed,
            SessionStatus::Silent,
        ] {
            let label = status_label(status);
            assert!(!label.is_empty());
        }
        // A recovered dictation was never put on the clipboard, so its row must
        // not imply it is ready to paste.
        assert!(!status_label(SessionStatus::Recovered).contains("paste"));
        assert!(!status_label(SessionStatus::HeldSecure).contains("Copied"));
    }

    #[test]
    fn an_error_row_explains_itself_without_an_api_message() {
        // The chip already says "Failed"; the line under it has to add
        // something the user can act on.
        for code in [
            "offline",
            "network",
            "timeout",
            "auth",
            "model",
            "quota",
            "rate_limit",
            "bad_request",
            "safety",
            "empty",
            "tooNoisy",
            "disk_write",
            "engine_died",
            "audio_purged",
        ] {
            let summary = error_summary(Some(code), SessionStatus::Failed);
            assert_ne!(summary, "Failed", "{code} adds nothing");
            assert!(!summary.is_empty());
        }
        // An unrecognised code still says something rather than nothing.
        assert_eq!(
            error_summary(Some("who-knows"), SessionStatus::Failed),
            "Failed"
        );
        assert_eq!(error_summary(None, SessionStatus::Failed), "Failed");
    }

    #[test]
    fn chip_labels_stay_short_while_the_row_body_explains() {
        for status in [
            SessionStatus::Inserted,
            SessionStatus::CopiedToClipboard,
            SessionStatus::AwaitingChip,
            SessionStatus::HeldSecure,
            SessionStatus::QueuedForRetry,
            SessionStatus::Recovered,
            SessionStatus::Cancelled,
            SessionStatus::Failed,
            SessionStatus::Silent,
        ] {
            let chip = status_chip_label(status);
            assert!(!chip.is_empty());
            // A chip wide enough to hold a sentence pushes the timestamp and
            // the app name off the row.
            assert!(chip.len() <= 12, "{status:?} chip too long: {chip}");
        }
    }

    #[test]
    fn a_cancelled_row_says_the_audio_survived() {
        assert!(status_label(SessionStatus::Cancelled).contains("kept"));
    }

    #[test]
    fn timestamps_render_rather_than_panicking_on_a_missing_local_offset() {
        let formatted = format_time(OffsetDateTime::UNIX_EPOCH);
        assert!(formatted.contains("Jan"), "{formatted}");
    }
}
