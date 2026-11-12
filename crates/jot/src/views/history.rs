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

//! A paginated library of dictations. Only previews are kept in the list;
//! full transcripts are loaded for the selected detail on a background worker.
use crate::i18n::{Locale, Msg, locale, tf, tr};
use crate::services::Services;
use crate::text_field::{self, Changed, Submit, TextField};
use crate::theme::{self, Theme};
use crate::views::widgets::{self, ButtonKind, PageScroll, StatusTone};
use gpui::{
    AnyElement, Context, Entity, HighlightStyle, Role, SharedString, StyledText, Task, Window, div,
    prelude::*, px, uniform_list,
};
use jot_core::history::{
    ApplicationFilter, Correction, DictationRecord, HistoryFilter, HistoryItem, StatusFilter,
    period_start,
};
use jot_core::meta::SessionStatus;
use jot_core::recovery::RetryOutcome;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;
use time::macros::format_description;

const PAGE_SIZE: usize = 100;
const ROW_HEIGHT: f32 = 142.0;

fn ui(en: &'static str, fr: &'static str) -> &'static str {
    match locale() {
        Locale::En => en,
        Locale::Fr => fr,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Period {
    Today,
    Week,
    Month,
    Quarter,
    All,
}
impl Period {
    const ALL: [Self; 5] = [
        Self::Today,
        Self::Week,
        Self::Month,
        Self::Quarter,
        Self::All,
    ];
    fn days(self) -> Option<u16> {
        match self {
            Self::Today => Some(1),
            Self::Week => Some(7),
            Self::Month => Some(30),
            Self::Quarter => Some(90),
            Self::All => None,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Today => ui("Today", "Aujourd’hui"),
            Self::Week => ui("7 days", "7 jours"),
            Self::Month => ui("30 days", "30 jours"),
            Self::Quarter => ui("90 days", "90 jours"),
            Self::All => ui("All dates", "Toutes les dates"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Version {
    Current,
    Raw,
    Cleaned,
    /// The rewrite the fidelity check refused; only with the opt-in diagnostic.
    Rejected,
    Corrections,
}
impl Version {
    const ALL: [Self; 5] = [
        Self::Current,
        Self::Raw,
        Self::Cleaned,
        Self::Rejected,
        Self::Corrections,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::Current => ui("Current text", "Texte actuel"),
            Self::Raw => ui("Raw", "Brut"),
            Self::Cleaned => ui("Cleaned", "Nettoyé"),
            Self::Rejected => ui("Refused rewrite", "Proposition refusée"),
            Self::Corrections => ui("Corrections", "Corrections"),
        }
    }
}

struct Detail {
    record: DictationRecord,
    word_count: usize,
    current: SharedString,
    raw: Option<SharedString>,
    cleaned: Option<SharedString>,
    corrections: Vec<(Correction, SharedString, SharedString)>,
    retryable: bool,
    /// Which engine and stages produced the text; None for dictations older than 1.5.
    engine: Option<String>,
    /// The refused rewrite and why, kept by the opt-in diagnostic.
    rejected: Option<(SharedString, SharedString)>,
}
impl Detail {
    fn new(record: DictationRecord, corrections: Vec<Correction>, retryable: bool) -> Self {
        Self {
            word_count: record.word_count(),
            current: record.display_text().to_owned().into(),
            raw: record.raw_transcript.clone().map(Into::into),
            cleaned: record.cleaned_transcript.clone().map(Into::into),
            corrections: corrections
                .into_iter()
                .map(|c| {
                    let before = SharedString::from(c.before.clone());
                    let after = SharedString::from(c.after.clone());
                    (c, before, after)
                })
                .collect(),
            engine: jot_core::meta::SessionMeta::read(&record.folder)
                .and_then(|meta| meta.model_id)
                .map(|id| engine_label(&id)),
            rejected: jot_core::diagnostic::RejectedReformulation::read(&record.folder).map(|d| {
                let why = format!(
                    "{} {} · {} ms",
                    ui("Refused by the check:", "Refusée par le contrôle :"),
                    d.detail.unwrap_or(d.reason),
                    d.reformulation_ms
                );
                (d.proposed.into(), why.into())
            }),
            record,
            retryable,
        }
    }
    fn text(&self, version: Version) -> Option<&SharedString> {
        match version {
            Version::Current => Some(&self.current),
            Version::Raw => self.raw.as_ref(),
            Version::Cleaned => self.cleaned.as_ref(),
            Version::Rejected => self.rejected.as_ref().map(|(text, _)| text),
            Version::Corrections => None,
        }
    }
}

pub struct HistoryView {
    services: Services,
    search: Entity<TextField>,
    editor: Entity<TextField>,
    items: Vec<HistoryItem>,
    total: usize,
    offset: usize,
    period: Period,
    application: ApplicationFilter,
    status_filter: StatusFilter,
    applications: Vec<String>,
    refresh_applications: bool,
    filters_open: bool,
    selected: HashSet<String>,
    detail: Option<Arc<Detail>>,
    detail_id: Option<String>,
    version: Version,
    editing: bool,
    pending_open: Option<Option<String>>,
    pending_delete: Option<Vec<String>>,
    message: Option<(String, StatusTone)>,
    /// Names typed in the last edit, offered as vocabulary terms (never as rules).
    term_offers: Vec<String>,
    loading: bool,
    detail_loading: bool,
    busy: bool,
    generation: u64,
    detail_generation: u64,
    query_task: Option<Task<()>>,
    detail_task: Option<Task<()>>,
    detail_scroll: PageScroll,
    _subscriptions: Vec<gpui::Subscription>,
}

impl HistoryView {
    pub fn new(services: Services, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextField::new("history-search", Msg::Search, cx));
        let editor = cx.new(|cx| TextField::new("history-editor", Msg::History, cx).multiline());
        let subscriptions = vec![
            cx.subscribe(&search, |this: &mut Self, _, _: &Changed, cx| {
                this.offset = 0;
                this.selected.clear();
                this.load(true, cx);
            }),
            cx.subscribe(&search, |this: &mut Self, _, _: &Submit, cx| {
                this.load(false, cx)
            }),
            cx.subscribe(&editor, |_: &mut Self, _, _: &Changed, cx| cx.notify()),
            cx.subscribe(&editor, |this: &mut Self, _, _: &Submit, cx| {
                this.save_editor(cx)
            }),
        ];
        let mut changes = services.history.subscribe();
        cx.spawn(async move |this, cx| {
            while !matches!(
                changes.recv().await,
                Err(tokio::sync::broadcast::error::RecvError::Closed)
            ) {
                while changes.try_recv().is_ok() {}
                if this
                    .update(cx, |this, cx| {
                        this.refresh_applications = true;
                        this.refresh(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let mut view = Self {
            services,
            search,
            editor,
            items: Vec::new(),
            total: 0,
            offset: 0,
            period: Period::All,
            application: ApplicationFilter::All,
            status_filter: StatusFilter::All,
            applications: Vec::new(),
            refresh_applications: true,
            filters_open: false,
            selected: HashSet::new(),
            detail: None,
            detail_id: None,
            version: Version::Current,
            editing: false,
            pending_open: None,
            pending_delete: None,
            message: None,
            term_offers: Vec::new(),
            loading: false,
            detail_loading: false,
            busy: false,
            generation: 0,
            detail_generation: 0,
            query_task: None,
            detail_task: None,
            detail_scroll: PageScroll::new(),
            _subscriptions: subscriptions,
        };
        view.load(false, cx);
        view
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.editing && !self.busy && self.detail_id.is_some() {
            self.open_now(self.detail_id.clone(), cx);
        }
        self.load(false, cx);
    }

    fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.search.update(cx, |field, cx| {
            field.set_content("");
            cx.notify();
        });
        self.offset = 0;
        self.selected.clear();
        self.pending_delete = None;
        self.load(false, cx);
    }

    fn load(&mut self, debounce: bool, cx: &mut Context<Self>) {
        self.pending_delete = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let store = self.services.history.clone();
        let executor = cx.background_executor().clone();
        let query = self.search.read(cx).content().to_owned();
        let application = self.application.clone();
        let status = self.status_filter;
        let period = self.period;
        let offset = self.offset;
        let refresh_applications = self.refresh_applications;
        self.loading = true;
        self.query_task = Some(cx.spawn(async move |this, cx| {
            if debounce {
                executor.timer(Duration::from_millis(180)).await;
            }
            let result = executor
                .spawn(async move {
                    let filter = HistoryFilter {
                        search: query,
                        since: period
                            .days()
                            .map(|days| period_start(days, OffsetDateTime::now_utc()))
                            .transpose()?,
                        until: None,
                        application,
                        status,
                    };
                    let page = store.page(&filter, offset, PAGE_SIZE)?;
                    let apps = if refresh_applications {
                        Some(store.applications()?)
                    } else {
                        None
                    };
                    anyhow::Ok((page, apps))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((page, apps)) => {
                        this.items = page.items;
                        this.total = page.total;
                        if let Some(apps) = apps {
                            this.applications = apps;
                            this.refresh_applications = false;
                        }
                        if this.items.is_empty() && this.offset > 0 {
                            this.offset = this.total.saturating_sub(1) / PAGE_SIZE * PAGE_SIZE;
                            this.load(false, cx);
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "history page query failed");
                        this.items.clear();
                        this.total = 0;
                        this.message = Some((
                            ui(
                                "Could not load history. Try again.",
                                "Impossible de charger l’historique. Réessayez.",
                            )
                            .into(),
                            StatusTone::Bad,
                        ));
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub fn has_unsaved_changes(&self, cx: &gpui::App) -> bool {
        self.busy
            || self.editing
                && self
                    .detail
                    .as_ref()
                    .is_some_and(|detail| self.editor.read(cx).content() != detail.current.as_ref())
    }

    pub fn prevent_navigation(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            self.message = Some((
                ui(
                    "Wait for the current operation to finish.",
                    "Attendez la fin de l’opération en cours.",
                )
                .into(),
                StatusTone::Neutral,
            ));
            cx.notify();
            return;
        }
        self.message = Some((
            ui(
                "Save or cancel your changes before leaving History.",
                "Enregistrez ou annulez vos modifications avant de quitter l’historique.",
            )
            .into(),
            StatusTone::Neutral,
        ));
        cx.notify();
    }

    pub fn dismiss_confirmation(&mut self, cx: &mut Context<Self>) {
        self.pending_delete = None;
        self.pending_open = None;
        cx.notify();
    }

    fn open(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if self.has_unsaved_changes(cx) {
            self.pending_open = Some(id);
            cx.notify();
            return;
        }
        self.open_now(id, cx);
    }

    fn open_now(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.pending_delete = None;
        self.detail_generation = self.detail_generation.wrapping_add(1);
        let generation = self.detail_generation;
        self.editing = false;
        self.pending_open = None;
        if self.detail_id != id {
            self.version = Version::Current;
            self.detail_scroll.reset();
            self.detail = None;
        }
        self.detail_id = id.clone();
        self.detail_task = None;
        self.detail_loading = id.is_some();
        if let Some(id) = id {
            let store = self.services.history.clone();
            let task = cx.background_executor().spawn(async move {
                let record = store
                    .record(&id)
                    .ok_or_else(|| anyhow::anyhow!("dictation unavailable"))?;
                let corrections = store.corrections(&id)?;
                let retryable = store.can_retry(&id);
                anyhow::Ok(Arc::new(Detail::new(record, corrections, retryable)))
            });
            self.detail_task = Some(cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    if generation != this.detail_generation {
                        return;
                    }
                    this.detail_loading = false;
                    match result {
                        Ok(detail) => this.detail = Some(detail),
                        Err(error) => {
                            tracing::warn!(%error, "history detail query failed");
                            this.detail_id = None;
                            this.message = Some((
                                ui(
                                    "This dictation is unavailable. Refresh history.",
                                    "Cette dictée est indisponible. Actualisez l’historique.",
                                )
                                .into(),
                                StatusTone::Bad,
                            ));
                        }
                    }
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }

    fn begin_edit(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.detail_loading {
            return;
        }
        if let Some(detail) = &self.detail {
            self.editor.update(cx, |field, cx| {
                field.set_content(detail.current.to_string());
                cx.notify();
            });
            self.version = Version::Current;
            self.editing = true;
            cx.notify();
        }
    }

    fn save_editor(&mut self, cx: &mut Context<Self>) {
        if self.editing && self.has_unsaved_changes(cx) {
            self.save(self.editor.read(cx).content().to_owned(), cx);
        }
    }

    fn save(&mut self, text: String, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(detail) = &self.detail else {
            return;
        };
        let expected = detail.current.to_string();
        let id = detail.record.id.clone();
        let store = self.services.history.clone();
        let dictionary = self.services.dictionary.clone();
        self.busy = true;
        let task = cx.background_executor().spawn(async move {
            store.edit(&id, &expected, &text)?;
            let corrections = store.corrections(&id);
            let learned = corrections
                .as_ref()
                .map_err(|e| anyhow::anyhow!("{e}"))
                .and_then(|corrections| dictionary.learn_from_corrections(corrections));
            // Already known, or already proposed as a correction: not offered twice.
            let known: Vec<String> = dictionary
                .entries()
                .into_iter()
                .map(|e| e.term)
                .chain(dictionary.suggestions().into_iter().map(|s| s.term))
                .map(|t| t.to_lowercase())
                .collect();
            let offers: Vec<String> = corrections
                .ok()
                .and_then(|c| c.last().map(jot_core::dictionary::new_terms))
                .unwrap_or_default()
                .into_iter()
                .filter(|term| !known.contains(&term.to_lowercase()))
                .collect();
            anyhow::Ok((id, learned, offers))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok((id, learned, offers)) => {
                        this.term_offers = offers;
                        this.message = Some(match learned {
                            Ok(count) if count > 0 => (format!("{} {count}. {}", ui("Changes saved. New dictionary suggestions:", "Modifications enregistrées. Nouvelles suggestions de dictionnaire :"), ui("Review them in Dictionary.", "Validez-les dans le Dictionnaire.")), StatusTone::Good),
                            Ok(_) => (ui("Changes saved. No new lexical suggestion.", "Modifications enregistrées. Aucune nouvelle suggestion lexicale.").into(), StatusTone::Good),
                            Err(_) => (ui("Correction saved. Dictionary suggestions could not be saved; retry the local analysis from Dictionary.", "Correction enregistrée. Les suggestions n’ont pas pu être sauvegardées ; relancez l’analyse locale depuis le Dictionnaire.").into(), StatusTone::Bad),
                        });
                        this.editing = false;
                        this.version = Version::Current;
                        this.detail_scroll.reset();
                        this.open_now(Some(id), cx);
                        this.load(false, cx);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "history edit failed");
                        this.message = Some((ui("Could not save. The dictation may have changed elsewhere. Your draft is kept; copy it before reloading.", "Enregistrement impossible. La dictée a peut-être changé ailleurs. Votre brouillon est conservé ; copiez-le avant de recharger.").into(), StatusTone::Bad));
                    }
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    /// A vocabulary term only: no replacement rule, so the misheard words keep
    /// their meaning everywhere else.
    fn remember_term(&mut self, term: &str, cx: &mut Context<Self>) {
        let added = self.services.dictionary.add(term, None);
        self.term_offers.retain(|t| t != term);
        let text = match (crate::i18n::locale(), added) {
            (crate::i18n::Locale::En, true) => {
                format!("“{term}” added to your vocabulary as a term (no replacement rule).")
            }
            (crate::i18n::Locale::Fr, true) => format!(
                "« {term} » ajouté à votre vocabulaire comme terme (aucune règle de remplacement)."
            ),
            (crate::i18n::Locale::En, false) => {
                format!("“{term}” could not be added: it may already be in the Dictionary.")
            }
            (crate::i18n::Locale::Fr, false) => format!(
                "« {term} » n’a pas pu être ajouté : il est peut-être déjà dans le Dictionnaire."
            ),
        };
        self.message = Some((
            text,
            if added {
                StatusTone::Good
            } else {
                StatusTone::Bad
            },
        ));
        cx.notify();
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.has_unsaved_changes(cx) {
            return;
        }
        let Some(ids) = self.pending_delete.take() else {
            return;
        };
        let store = self.services.history.clone();
        let dictionary = self.services.dictionary.clone();
        self.busy = true;
        // A deleted dictation is never learned from, even mid-watch.
        jot_core::app_learning::stop(false);
        let task = cx.background_executor().spawn(async move {
            let result = store.delete_many(&ids);
            let purged = if result.is_ok() {
                store.correction_session_ids().and_then(|ids| {
                    dictionary.purge_orphan_suggestions(
                        &ids.into_iter().collect(),
                        &store.all_ids().into_iter().collect(),
                    )
                })
            } else {
                Ok(0)
            };
            (result, ids, purged)
        });
        cx.spawn(async move |this, cx| {
            let (result, ids, purged) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(count) => {
                        this.selected.retain(|id| !ids.contains(id));
                        if this.detail_id.as_ref().is_some_and(|id| ids.contains(id)) { this.open_now(None, cx); }
                        this.message = Some(if purged.is_ok() {
                            (format!("{} {count}", ui("Deleted dictations:", "Dictées supprimées :")), StatusTone::Good)
                        } else {
                            (ui("Dictations deleted. Orphan suggestions could not be purged; refresh Dictionary to retry.", "Dictées supprimées. La purge des suggestions orphelines a échoué ; actualisez le Dictionnaire pour réessayer.").into(), StatusTone::Bad)
                        });
                    }
                    Err(error) => {
                        tracing::warn!(%error, "history deletion failed");
                        this.message = Some((ui("Deletion could not finish. Refresh and check the remaining dictations before retrying.", "La suppression n’a pas abouti. Actualisez et vérifiez les dictées restantes avant de réessayer.").into(), StatusTone::Bad));
                    }
                }
                this.load(false, cx);
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn retry(&mut self, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail else {
            return;
        };
        if self.busy || !detail.retryable || self.editing {
            return;
        }
        let record = detail.record.clone();
        let id = record.id.clone();
        self.busy = true;
        self.message = Some((tr(Msg::Retrying).into(), StatusTone::Neutral));
        let queue = self.services.retry_queue.clone();
        cx.spawn(async move |this, cx| {
            let outcome = queue.retry_single(&record).await;
            let (message, tone) = match outcome {
                RetryOutcome::Recovered => (Msg::RetryRecovered, StatusTone::Good),
                RetryOutcome::StillOffline => (Msg::RetryOffline, StatusTone::Neutral),
                RetryOutcome::Blocked => (Msg::RetryBlocked, StatusTone::Bad),
                RetryOutcome::Failed => (Msg::RetryFailed, StatusTone::Bad),
                RetryOutcome::AlreadyDone => (Msg::RetryAlreadyDone, StatusTone::Neutral),
                RetryOutcome::Busy => (Msg::RetryBusy, StatusTone::Neutral),
            };
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.message = Some((tr(message).into(), tone));
                this.open_now(Some(id), cx);
                this.load(false, cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn filters(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let period_buttons = Period::ALL
            .into_iter()
            .enumerate()
            .map(|(index, period)| {
                widgets::button(
                    ("history-period", index),
                    period.label(),
                    if self.period == period {
                        ButtonKind::Selected
                    } else {
                        ButtonKind::Secondary
                    },
                    !self.busy,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        this.period = period;
                        this.offset = 0;
                        this.selected.clear();
                        this.load(false, cx);
                    }),
                )
                .into_any_element()
            })
            .collect::<Vec<_>>();
        let statuses = [
            (StatusFilter::All, ui("All statuses", "Tous les statuts")),
            (StatusFilter::Completed, ui("Completed", "Terminées")),
            (StatusFilter::Pending, ui("Pending", "En attente")),
            (StatusFilter::Failed, tr(Msg::Failed)),
            (StatusFilter::Cancelled, tr(Msg::Cancelled)),
        ];
        let status_buttons = statuses
            .into_iter()
            .enumerate()
            .map(|(index, (status, label))| {
                let selected = self.status_filter == status;
                widgets::button(
                    ("history-status-filter", index),
                    label,
                    if selected {
                        ButtonKind::Selected
                    } else {
                        ButtonKind::Secondary
                    },
                    !self.busy,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        this.status_filter = status;
                        this.offset = 0;
                        this.selected.clear();
                        this.load(false, cx);
                    }),
                )
                .into_any_element()
            })
            .collect::<Vec<_>>();
        let apps = std::iter::once((
            ApplicationFilter::All,
            ui("All applications", "Toutes les applications").to_owned(),
        ))
        .chain(std::iter::once((
            ApplicationFilter::Unknown,
            tr(Msg::UnknownApp).to_owned(),
        )))
        .chain(
            self.applications
                .iter()
                .map(|app| (ApplicationFilter::Named(app.clone()), app.clone())),
        );
        let app_buttons = apps
            .enumerate()
            .map(|(index, (app, label))| {
                let selected = self.application == app;
                widgets::button(
                    ("history-app-filter", index),
                    label,
                    if selected {
                        ButtonKind::Selected
                    } else {
                        ButtonKind::Secondary
                    },
                    !self.busy,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        this.application = app.clone();
                        this.offset = 0;
                        this.selected.clear();
                        this.load(false, cx);
                    }),
                )
                .into_any_element()
            })
            .collect::<Vec<_>>();
        div()
            .id("history-filters")
            .debug_selector(|| "history-filters".into())
            .max_h(px(180.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(theme::spacing::XS)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(theme::spacing::XS)
                    .children(period_buttons),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(theme::spacing::XS)
                    .children(status_buttons),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(theme::spacing::XS)
                    .children(app_buttons),
            )
            .into_any_element()
    }

    fn row(
        &self,
        item: &HistoryItem,
        day: Option<String>,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = item.id.clone();
        let selected = self.selected.contains(&id);
        let active = self.detail_id.as_deref() == Some(id.as_str());
        let status = SessionStatus::parse(&item.status).unwrap_or(SessionStatus::Failed);
        let words = format!("{} {}", item.word_count, ui("words", "mots"));
        let meta = format!(
            "{} · {} · {}{}",
            format_clock(item.started_at),
            item.target_app_name
                .as_deref()
                .unwrap_or(tr(Msg::UnknownApp)),
            words,
            item.duration_seconds
                .map(|seconds| format!(" · {}", format_duration(seconds)))
                .unwrap_or_default()
        );
        div()
            .id(SharedString::from(format!("history-row-{id}")))
            .role(Role::ListItem)
            .aria_label(SharedString::from(format!("{} · {meta}", item.preview)))
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex()
            .flex_col()
            .gap(theme::spacing::XXS)
            .p(theme::spacing::S)
            .border_b_1()
            .border_color(theme.outline_variant)
            .when(active, |row| row.bg(theme.primary_container))
            .cursor_pointer()
            .on_click(cx.listener({
                let id = id.clone();
                move |this, _, _, cx| {
                    if !this.loading {
                        this.open(Some(id.clone()), cx);
                    }
                }
            }))
            .child(
                div()
                    .h(px(19.0))
                    .text_size(theme::type_scale::LABEL)
                    .text_color(theme.on_surface_variant)
                    .child(day.unwrap_or_default()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::spacing::XS)
                    .child(status_chip(status, theme))
                    .when(item.corrected, |row| {
                        row.child(
                            div()
                                .text_size(theme::type_scale::LABEL_SMALL)
                                .child(ui("Edited", "Modifiée")),
                        )
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::type_scale::LABEL)
                            .text_color(theme.on_surface_variant)
                            .child(meta),
                    ),
            )
            .child(
                div()
                    .h(px(23.0))
                    .debug_selector(|| "history-preview".into())
                    .overflow_hidden()
                    .truncate()
                    .text_size(theme::type_scale::BODY_LARGE)
                    .child({
                        let preview = if item.preview.is_empty() {
                            status_label(status).to_owned()
                        } else {
                            item.preview.clone()
                        };
                        let highlights =
                            search_highlights(&preview, self.search.read(cx).content())
                                .into_iter()
                                .map(|range| {
                                    (
                                        range,
                                        HighlightStyle {
                                            color: Some(theme.on_primary_container.into()),
                                            background_color: Some(theme.primary_container.into()),
                                            font_weight: Some(gpui::FontWeight::BOLD),
                                            ..Default::default()
                                        },
                                    )
                                })
                                .collect::<Vec<_>>();
                        StyledText::new(preview).with_highlights(highlights)
                    }),
            )
            .child(
                div()
                    .flex()
                    .gap(theme::spacing::XS)
                    .child(widgets::button(
                        SharedString::from(format!("history-open-{id}")),
                        ui("Open", "Ouvrir"),
                        if active {
                            ButtonKind::Selected
                        } else {
                            ButtonKind::Secondary
                        },
                        !self.busy && !self.loading,
                        theme,
                        cx.listener({
                            let id = id.clone();
                            move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.open(Some(id.clone()), cx);
                            }
                        }),
                    ))
                    .child(widgets::button(
                        SharedString::from(format!("history-select-{id}")),
                        if selected {
                            ui("Selected", "Sélectionnée")
                        } else {
                            ui("Select", "Sélectionner")
                        },
                        if selected {
                            ButtonKind::Selected
                        } else {
                            ButtonKind::Secondary
                        },
                        !self.busy && !self.loading,
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.pending_delete = None;
                            if !this.selected.remove(&id) {
                                this.selected.insert(id.clone());
                            }
                            cx.notify();
                        }),
                    )),
            )
            .into_any_element()
    }

    fn detail_panel(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut children = vec![
            div()
                .flex()
                .flex_wrap()
                .gap(theme::spacing::XS)
                .child(widgets::button(
                    "history-back",
                    ui("Back to list", "Retour à la liste"),
                    ButtonKind::Secondary,
                    !self.busy,
                    theme,
                    cx.listener(|this, _, _, cx| this.open(None, cx)),
                ))
                .into_any_element(),
        ];
        if self.detail_loading && self.detail.is_none() {
            children.push(
                div()
                    .child(ui("Loading dictation…", "Chargement de la dictée…"))
                    .into_any_element(),
            );
        }
        if let Some(detail) = &self.detail {
            let detail = detail.clone();
            let record = &detail.record;
            children.push(
                div()
                    .id("history-detail-heading")
                    .role(Role::Heading)
                    .aria_level(2)
                    .text_size(theme::type_scale::TITLE)
                    .child(format_time(record.started_at))
                    .into_any_element(),
            );
            children.push(
                div()
                    .text_color(theme.on_surface_variant)
                    .child(format!(
                        "{} · {} · {} {}{}",
                        record
                            .target_app_name
                            .as_deref()
                            .unwrap_or(tr(Msg::UnknownApp)),
                        status_label(record.status().unwrap_or(SessionStatus::Failed)),
                        detail.word_count,
                        ui("words", "mots"),
                        record
                            .duration_seconds
                            .map(|seconds| format!(" · {}", format_duration(seconds)))
                            .unwrap_or_default()
                    ))
                    .into_any_element(),
            );
            if let Some(engine) = &detail.engine {
                children.push(
                    div()
                        .text_color(theme.on_surface_variant)
                        .child(engine.clone())
                        .into_any_element(),
                );
            }
            if let Some(code) = record.error_code.as_deref() {
                children.push(
                    widgets::status_line(
                        "history-record-error",
                        error_summary(Some(code), record.status().unwrap_or(SessionStatus::Failed)),
                        StatusTone::Bad,
                        theme,
                    )
                    .into_any_element(),
                );
            }
            if let Some(message) = &record.error_message {
                children.push(
                    widgets::section(
                        ui("Error details", "Détails de l’erreur"),
                        theme,
                        vec![div().child(message.clone()).into_any_element()],
                    )
                    .into_any_element(),
                );
            }
            let mut actions = div().flex().flex_wrap().gap(theme::spacing::XS);
            let copy_text = if self.editing {
                None
            } else {
                detail.text(self.version).cloned()
            };
            actions = actions.child(widgets::button(
                "history-copy",
                tr(Msg::Copy),
                ButtonKind::Secondary,
                self.editing || copy_text.as_ref().is_some_and(|text| !text.is_empty()),
                theme,
                cx.listener(move |this, _, _, cx| {
                    let text = if this.editing {
                        this.editor.read(cx).content().to_owned()
                    } else {
                        copy_text
                            .as_ref()
                            .map(ToString::to_string)
                            .unwrap_or_default()
                    };
                    text_field::set_clipboard_text(text, cx);
                    this.message = Some((tr(Msg::CopiedFeedback).into(), StatusTone::Good));
                    cx.notify();
                }),
            ));
            if self.editing {
                actions = actions
                    .child(widgets::button(
                        "history-save",
                        ui("Save", "Enregistrer"),
                        ButtonKind::Primary,
                        !self.busy && self.has_unsaved_changes(cx),
                        theme,
                        cx.listener(|this, _, _, cx| this.save_editor(cx)),
                    ))
                    .child(widgets::button(
                        "history-cancel-edit",
                        tr(Msg::Cancel),
                        ButtonKind::Secondary,
                        !self.busy,
                        theme,
                        cx.listener(|this, _, _, cx| {
                            this.editing = false;
                            cx.notify();
                        }),
                    ));
            } else {
                actions = actions
                    .child(widgets::button(
                        "history-edit",
                        ui("Edit", "Modifier"),
                        ButtonKind::Primary,
                        !self.busy && !self.detail_loading && !detail.current.is_empty(),
                        theme,
                        cx.listener(|this, _, _, cx| this.begin_edit(cx)),
                    ))
                    .when(detail.retryable, |row| {
                        row.child(widgets::button(
                            "history-retry",
                            tr(Msg::Retry),
                            ButtonKind::Secondary,
                            !self.busy,
                            theme,
                            cx.listener(|this, _, _, cx| this.retry(cx)),
                        ))
                    })
                    .child(widgets::button(
                        "history-delete-detail",
                        tr(Msg::Delete),
                        ButtonKind::Destructive,
                        !self.busy,
                        theme,
                        cx.listener({
                            let id = record.id.clone();
                            move |this, _, _, cx| {
                                this.pending_delete = Some(vec![id.clone()]);
                                cx.notify();
                            }
                        }),
                    ));
            }
            children.push(actions.into_any_element());
            if self.editing {
                children.push(
                    div()
                        .text_size(theme::type_scale::LABEL)
                        .text_color(theme.on_surface_variant)
                        .child(ui(
                            "Enter: new line · Ctrl+Enter: save",
                            "Entrée : nouvelle ligne · Ctrl+Entrée : enregistrer",
                        ))
                        .into_any_element(),
                );
                children.push(if self.busy {
                    div()
                        .child(ui("Saving…", "Enregistrement…"))
                        .into_any_element()
                } else {
                    self.editor.clone().into_any_element()
                });
            } else {
                let versions = Version::ALL
                    .into_iter()
                    .enumerate()
                    .filter(|(_, version)| {
                        *version != Version::Rejected || detail.rejected.is_some()
                    })
                    .map(|(index, version)| {
                        widgets::button(
                            ("history-version", index),
                            version.label(),
                            if self.version == version {
                                ButtonKind::Selected
                            } else {
                                ButtonKind::Secondary
                            },
                            true,
                            theme,
                            cx.listener(move |this, _, _, cx| {
                                this.version = version;
                                this.detail_scroll.reset();
                                cx.notify();
                            }),
                        )
                        .into_any_element()
                    })
                    .collect::<Vec<_>>();
                children.push(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(theme::spacing::XS)
                        .children(versions)
                        .into_any_element(),
                );
                if self.version == Version::Corrections {
                    if detail.corrections.is_empty() {
                        children.push(
                            div()
                                .child(ui(
                                    "No saved corrections.",
                                    "Aucune correction enregistrée.",
                                ))
                                .into_any_element(),
                        );
                    }
                    for (index, (correction, before, after)) in
                        detail.corrections.iter().enumerate()
                    {
                        children.push(
                            widgets::section(
                                format_time(correction.corrected_at),
                                theme,
                                vec![
                                    div()
                                        .text_size(theme::type_scale::LABEL)
                                        .text_color(theme.on_surface_variant)
                                        .child(ui("Before", "Avant"))
                                        .into_any_element(),
                                    div().child(before.clone()).into_any_element(),
                                    div()
                                        .text_size(theme::type_scale::LABEL)
                                        .text_color(theme.on_surface_variant)
                                        .child(ui("After", "Après"))
                                        .into_any_element(),
                                    div().child(after.clone()).into_any_element(),
                                    widgets::hug(widgets::button(
                                        ("history-restore", index),
                                        ui(
                                            "Restore this previous version",
                                            "Restaurer cette version précédente",
                                        ),
                                        ButtonKind::Secondary,
                                        !self.busy && before != &detail.current,
                                        theme,
                                        cx.listener({
                                            let before = before.clone();
                                            move |this, _, _, cx| this.save(before.to_string(), cx)
                                        }),
                                    ))
                                    .into_any_element(),
                                ],
                            )
                            .into_any_element(),
                        );
                    }
                } else {
                    if let (Version::Rejected, Some((_, why))) = (self.version, &detail.rejected) {
                        children.push(
                            div()
                                .text_size(theme::type_scale::LABEL)
                                .text_color(theme.on_surface_variant)
                                .child(why.clone())
                                .into_any_element(),
                        );
                    }
                    children.push(
                        div()
                            .p(theme::spacing::S)
                            .rounded(theme::radius::SMALL)
                            .bg(theme.surface)
                            .child(detail.text(self.version).cloned().unwrap_or_else(|| {
                                ui(
                                    "No text for this version.",
                                    "Aucun texte pour cette version.",
                                )
                                .into()
                            }))
                            .into_any_element(),
                    );
                }
            }
        }
        widgets::page("history-detail", theme, &self.detail_scroll, children).into_any_element()
    }
}

impl Render for HistoryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::ui(window);
        let wide = window.viewport_size().width >= px(1120.0);
        let compact_filters = self.filters_open && window.viewport_size().height < px(620.0);
        let show_list = !compact_filters && (wide || self.detail_id.is_none());
        let show_detail = !compact_filters && self.detail_id.is_some();
        let mut body = div()
            .debug_selector(|| "history-body".into())
            .flex()
            .flex_1()
            .min_h_0()
            .gap(theme::spacing::S);
        if compact_filters {
            body = body.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.filters(theme, cx)),
            );
        }
        if show_list {
            let count = self.items.len();
            let list = if count == 0 {
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.on_surface_variant)
                    .child(if self.loading {
                        ui("Loading history…", "Chargement de l’historique…")
                    } else if self.total == 0
                        && self.search.read(cx).is_empty()
                        && self.period == Period::All
                        && self.application == ApplicationFilter::All
                        && self.status_filter == StatusFilter::All
                    {
                        tr(Msg::HistoryHint)
                    } else {
                        tr(Msg::NoMatches)
                    })
                    .into_any_element()
            } else {
                uniform_list(
                    "history-rows",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                        let theme = Theme::ui(window);
                        range
                            .filter_map(|index| {
                                let item = this.items.get(index)?.clone();
                                let day = day_heading(&this.items, index);
                                Some(this.row(&item, day, theme, cx))
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .size_full()
                .into_any_element()
            };
            let page_label = if self.total == 0 {
                "0".to_owned()
            } else {
                format!(
                    "{}–{} / {}",
                    self.offset + 1,
                    self.offset + count,
                    self.total
                )
            };
            let list_column = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .gap(theme::spacing::XS)
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(theme::spacing::XS)
                        .child(widgets::button(
                            "history-select-page",
                            ui("Select page", "Sélectionner la page"),
                            ButtonKind::Secondary,
                            !self.busy && count > 0 && !self.loading,
                            theme,
                            cx.listener(|this, _, _, cx| {
                                this.selected
                                    .extend(this.items.iter().map(|item| item.id.clone()));
                                cx.notify();
                            }),
                        ))
                        .when(!self.selected.is_empty(), |row| {
                            row.child(widgets::button(
                                "history-clear-selection",
                                ui("Clear selection", "Désélectionner"),
                                ButtonKind::Secondary,
                                !self.busy,
                                theme,
                                cx.listener(|this, _, _, cx| {
                                    this.selected.clear();
                                    cx.notify();
                                }),
                            ))
                            .child(widgets::button(
                                "history-delete-selected",
                                format!("{} ({})", tr(Msg::Delete), self.selected.len()),
                                ButtonKind::Destructive,
                                !self.busy && !self.has_unsaved_changes(cx),
                                theme,
                                cx.listener(|this, _, _, cx| {
                                    this.pending_delete =
                                        Some(this.selected.iter().cloned().collect());
                                    cx.notify();
                                }),
                            ))
                        }),
                )
                .child(
                    div()
                        .id("history-list")
                        .role(Role::List)
                        .aria_label(tr(Msg::DictationList))
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden()
                        .border_1()
                        .rounded(theme::radius::MEDIUM)
                        .border_color(theme.outline_variant)
                        .bg(theme.surface)
                        .child(list),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(theme::spacing::XS)
                        .child(widgets::button(
                            "history-previous-page",
                            ui("Previous", "Précédent"),
                            ButtonKind::Secondary,
                            !self.loading && self.offset > 0 && !self.busy,
                            theme,
                            cx.listener(|this, _, _, cx| {
                                this.offset = this.offset.saturating_sub(PAGE_SIZE);
                                this.load(false, cx);
                            }),
                        ))
                        .child(div().text_size(theme::type_scale::LABEL).child(page_label))
                        .child(widgets::button(
                            "history-next-page",
                            ui("Next", "Suivant"),
                            ButtonKind::Secondary,
                            !self.loading && self.offset + count < self.total && !self.busy,
                            theme,
                            cx.listener(|this, _, _, cx| {
                                this.offset += PAGE_SIZE;
                                this.load(false, cx);
                            }),
                        )),
                );
            body = body.child(list_column);
        }
        if show_detail {
            body = body.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .border_1()
                    .rounded(theme::radius::MEDIUM)
                    .border_color(theme.outline_variant)
                    .overflow_hidden()
                    .debug_selector(|| "history-detail-panel".into())
                    .child(self.detail_panel(theme, cx)),
            );
        }
        let mut page = div()
            .size_full()
            .flex()
            .flex_col()
            .gap(theme::spacing::S)
            .p(theme::spacing::M)
            .font_family(theme::ui_font())
            .bg(theme.window_background)
            .text_color(theme.on_surface)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(theme::spacing::S)
                    .child(widgets::heading(Msg::History))
                    .child(
                        div()
                            .text_size(theme::type_scale::LABEL)
                            .text_color(theme.on_surface_variant)
                            .child(if self.loading {
                                ui("Searching…", "Recherche…").to_owned()
                            } else {
                                format!(
                                    "{} {}",
                                    self.total,
                                    if self.total == 1 {
                                        ui("dictation", "dictée")
                                    } else {
                                        ui("dictations", "dictées")
                                    }
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::spacing::XS)
                    .child(div().flex_1().min_w_0().child(self.search.clone()))
                    .when(!self.search.read(cx).is_empty(), |row| {
                        row.child(widgets::button(
                            "history-clear-search",
                            ui("Clear", "Effacer"),
                            ButtonKind::Secondary,
                            true,
                            theme,
                            cx.listener(|this, _, _, cx| this.clear_search(cx)),
                        ))
                    }),
            )
            .child(
                div()
                    .text_size(theme::type_scale::LABEL)
                    .text_color(theme.on_surface_variant)
                    .child(ui(
                        "Search words and prefixes; all words must match.",
                        "Recherchez des mots ou leurs débuts ; tous les mots doivent correspondre.",
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(theme::spacing::XS)
                    .child(widgets::button(
                        "history-toggle-filters",
                        format!(
                            "{} · {} · {} · {}",
                            if self.filters_open {
                                ui("Close filters", "Fermer les filtres")
                            } else {
                                ui("Filters", "Filtres")
                            },
                            self.period.label(),
                            application_label(&self.application),
                            status_filter_label(self.status_filter)
                        ),
                        if self.filters_open {
                            ButtonKind::Selected
                        } else {
                            ButtonKind::Secondary
                        },
                        true,
                        theme,
                        cx.listener(|this, _, _, cx| {
                            this.filters_open = !this.filters_open;
                            cx.notify();
                        }),
                    ))
                    .child(widgets::button(
                        "history-refresh",
                        ui("Refresh", "Actualiser"),
                        ButtonKind::Secondary,
                        !self.loading,
                        theme,
                        cx.listener(|this, _, _, cx| this.refresh(cx)),
                    )),
            )
            .when(self.filters_open && !compact_filters, |page| {
                page.child(self.filters(theme, cx))
            });
        if let Some((message, tone)) = &self.message {
            page = page.child(widgets::status_line(
                "history-feedback",
                message.clone(),
                *tone,
                theme,
            ));
        }
        if !self.term_offers.is_empty() {
            page = page.child(div().flex().flex_wrap().gap(theme::spacing::XS).children(
                self.term_offers.iter().enumerate().map(|(index, term)| {
                    let term = term.clone();
                    widgets::button(
                        ("history-remember-term", index),
                        match crate::i18n::locale() {
                            crate::i18n::Locale::En => {
                                format!("Remember “{term}” in my vocabulary")
                            }
                            crate::i18n::Locale::Fr => {
                                format!("Mémoriser « {term} » dans mon vocabulaire")
                            }
                        },
                        ButtonKind::Secondary,
                        !self.busy,
                        theme,
                        cx.listener(move |this, _, _, cx| this.remember_term(&term, cx)),
                    )
                }),
            ));
        }
        if self.pending_open.is_some() {
            page = page.child(widgets::section(
                ui("Unsaved changes", "Modifications non enregistrées"),
                theme,
                vec![
                    div()
                        .child(ui(
                            "Your draft has not been saved. Discard it and continue?",
                            "Votre brouillon n’est pas enregistré. L’abandonner pour continuer ?",
                        ))
                        .into_any_element(),
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(theme::spacing::XS)
                        .child(widgets::button(
                            "history-keep-draft",
                            ui("Keep editing", "Continuer à modifier"),
                            ButtonKind::Primary,
                            true,
                            theme,
                            cx.listener(|this, _, _, cx| {
                                this.pending_open = None;
                                cx.notify();
                            }),
                        ))
                        .child(widgets::button(
                            "history-discard-draft",
                            ui(
                                "Discard draft and continue",
                                "Abandonner le brouillon et continuer",
                            ),
                            ButtonKind::Destructive,
                            !self.busy,
                            theme,
                            cx.listener(|this, _, _, cx| {
                                if let Some(id) = this.pending_open.take() {
                                    this.open_now(id, cx);
                                }
                            }),
                        ))
                        .into_any_element(),
                ],
            ));
        }
        if let Some(ids) = &self.pending_delete {
            page = page.child(widgets::section(ui("Confirm permanent deletion", "Confirmer la suppression définitive"), theme, vec![
                div().child(format!("{} {} {}", ui("Permanently delete", "Supprimer définitivement"), ids.len(), ui("dictation(s), their versions and associated audio? This cannot be undone.", "dictée(s), leurs versions et les fichiers audio associés ? Cette action est irréversible."))).into_any_element(),
                div().flex().flex_wrap().gap(theme::spacing::XS)
                    .child(widgets::button("history-cancel-delete", tr(Msg::Cancel), ButtonKind::Secondary, !self.busy, theme,
                        cx.listener(|this, _, _, cx| this.dismiss_confirmation(cx))))
                    .child(widgets::button("history-confirm-delete", ui("Delete permanently, including audio", "Supprimer définitivement, audio compris"), ButtonKind::Destructive, !self.busy && !self.has_unsaved_changes(cx), theme,
                        cx.listener(|this, _, _, cx| this.delete(cx)))).into_any_element(),
            ]));
        }
        page.child(body)
    }
}

fn status_filter_label(status: StatusFilter) -> &'static str {
    match status {
        StatusFilter::All => ui("All statuses", "Tous les statuts"),
        StatusFilter::Completed => ui("Completed", "Terminées"),
        StatusFilter::Pending => ui("Pending", "En attente"),
        StatusFilter::Failed => tr(Msg::Failed),
        StatusFilter::Cancelled => tr(Msg::Cancelled),
    }
}

fn application_label(application: &ApplicationFilter) -> String {
    match application {
        ApplicationFilter::All => ui("All applications", "Toutes les applications").into(),
        ApplicationFilter::Unknown => tr(Msg::UnknownApp).into(),
        ApplicationFilter::Named(name) => name.clone(),
    }
}

fn local_stamp(at: OffsetDateTime) -> (OffsetDateTime, bool) {
    match jot_core::activity::local_datetime(at) {
        Ok(local) => (local, false),
        Err(_) => (at.to_offset(time::UtcOffset::UTC), true),
    }
}

fn day_heading(items: &[HistoryItem], index: usize) -> Option<String> {
    let at = items.get(index)?.started_at;
    let (local, fallback) = local_stamp(at);
    if index > 0 {
        let previous = items[index - 1].started_at;
        let (previous, previous_fallback) = local_stamp(previous);
        if local.date() == previous.date() && fallback == previous_fallback {
            return None;
        }
    }
    let pattern = match locale() {
        Locale::En => format_description!("[year]-[month]-[day]"),
        Locale::Fr => format_description!("[day]/[month]/[year]"),
    };
    local.format(pattern).ok().map(|date| {
        if fallback {
            format!("{date} UTC")
        } else {
            date
        }
    })
}

fn format_clock(at: OffsetDateTime) -> String {
    let (local, fallback) = local_stamp(at);
    let clock = local
        .format(format_description!("[hour]:[minute]"))
        .unwrap_or_default();
    if fallback {
        format!("{clock} UTC")
    } else {
        clock
    }
}

fn format_duration(seconds: f64) -> String {
    let value = format!("{seconds:.1}");
    tf(
        Msg::Duration,
        &[&if locale() == Locale::Fr {
            value.replace('.', ",")
        } else {
            value
        }],
    )
}
fn format_time(at: OffsetDateTime) -> String {
    let (local, fallback) = local_stamp(at);
    let pattern = match locale() {
        Locale::En => format_description!("[day] [month repr:short] [year] [hour]:[minute]"),
        Locale::Fr => format_description!("[day]/[month]/[year] à [hour]:[minute]"),
    };
    let formatted = local
        .format(pattern)
        .unwrap_or_else(|_| tr(Msg::UnknownTime).into());
    if fallback {
        format!("{formatted} UTC")
    } else {
        formatted
    }
}

/// The short word that goes in the chip. The row's body line carries the
/// explanation, so a chip repeating a whole sentence just crowds the row.
fn status_chip_label(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Inserted => tr(Msg::Inserted),
        SessionStatus::CopiedToClipboard => tr(Msg::Copied),
        SessionStatus::AwaitingChip => tr(Msg::NotInserted),
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
        SessionStatus::AwaitingChip => tr(Msg::NotInsertedDetail),
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

/// Names the engine and the stage whose text was kept, from `meta.model_id`.
fn engine_label(model_id: &str) -> String {
    let Some(local) = model_id.strip_prefix("local/") else {
        return format!(
            "{} · {model_id}",
            ui("Engine: Gemini (online)", "Moteur : Gemini (en ligne)")
        );
    };
    let (asr, rest) = local.split_once('+').unwrap_or((local, ""));
    let asr = if asr == "qwen3-asr-1.7b" {
        "Qwen3-ASR 1.7B"
    } else {
        asr
    };
    let head = format!(
        "{} · {asr}",
        ui("Engine: 100% local", "Moteur : 100 % local")
    );
    if rest.is_empty() {
        return format!("{head} · {}", ui("raw transcript", "transcription brute"));
    }
    let (model, outcome) = rest.split_once(':').unwrap_or((rest, ""));
    let model = match model {
        "qwen3.5-4b" => "Qwen3.5 4B",
        "qwen3.5-9b" => "Qwen3.5 9B",
        other => other,
    };
    if outcome.is_empty() {
        return format!("{head} → {model}");
    }
    // 1.5.0 stored a bare "rejected"; 1.5.1 adds the check that failed.
    let (outcome, reason) = outcome.split_once('/').unwrap_or((outcome, ""));
    let why = match (outcome, reason) {
        ("rejected", "hedge_lost") => ui(
            "rejected: a nuance (maybe, I think…) was lost",
            "refusé : une nuance (peut-être, je pense…) a disparu",
        ),
        ("rejected", "negation_lost") => ui(
            "rejected: a negation was lost",
            "refusé : une négation a disparu",
        ),
        ("rejected", "number_changed" | "number_lost") => ui(
            "rejected: a number was changed or lost",
            "refusé : un nombre a été modifié ou perdu",
        ),
        ("rejected", "unit_added") => ui(
            "rejected: a unit was added (minutes, %, GB…)",
            "refusé : une unité a été ajoutée (minutes, %, Go…)",
        ),
        ("rejected", "restriction_lost") => ui(
            "rejected: a condition (only if, unless…) was lost",
            "refusé : une condition (uniquement si, sauf si…) a disparu",
        ),
        ("rejected", "qualifier_lost") => ui(
            "rejected: a degree (not necessarily…) was lost",
            "refusé : un degré (pas forcément…) a disparu",
        ),
        ("rejected", _) => ui("rejected by the safety check", "refusé par le contrôle"),
        ("missing", _) => ui("not installed", "non installé"),
        ("timeout", _) => ui("too slow (time limit reached)", "trop lent (délai dépassé)"),
        ("startfail", _) => ui("did not start", "n’a pas démarré"),
        ("error", _) => ui("engine error", "erreur du moteur"),
        _ => ui("unavailable", "indisponible"),
    };
    format!(
        "{head} · {model} {why} — {}",
        ui("raw transcript kept", "transcription brute conservée")
    )
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
        Some("local") => tr(Msg::HistoryLocal),
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
// Prefix highlighting mirrors the word-oriented search for French and English.
// shortcut: accent folding covers Latin French, extend it if another UI language is added.
fn fold_search(word: &str) -> String {
    word.chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| {
            Some(match c {
                'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => 'a',
                'é' | 'è' | 'ê' | 'ë' => 'e',
                'î' | 'ï' | 'í' | 'ì' => 'i',
                'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
                'ù' | 'û' | 'ü' | 'ú' => 'u',
                'ç' => 'c',
                '\u{0300}'..='\u{036f}' => return None,
                _ => c,
            })
        })
        .collect()
}

fn word_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = None;
    for (index, c) in text.char_indices() {
        if c.is_alphanumeric() || ('\u{0300}'..='\u{036f}').contains(&c) && start.is_some() {
            start.get_or_insert(index);
        } else if let Some(start) = start.take() {
            ranges.push(start..index);
        }
    }
    if let Some(start) = start {
        ranges.push(start..text.len());
    }
    ranges
}

fn search_highlights(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let words = word_ranges(query)
        .into_iter()
        .map(|range| fold_search(&query[range]))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    if words.is_empty() {
        return Vec::new();
    }
    word_ranges(text)
        .into_iter()
        .filter(|range| {
            let word = fold_search(&text[range.clone()]);
            words.iter().any(|query| word.starts_with(query))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_label_names_the_stage_whose_text_was_kept() {
        assert!(
            engine_label("gemini-3.5-transcribe/smart").contains("gemini-3.5-transcribe/smart")
        );
        let raw = engine_label("local/qwen3-asr-1.7b");
        assert!(
            raw.contains("Qwen3-ASR 1.7B") && !raw.contains("→"),
            "{raw}"
        );
        assert!(engine_label("local/qwen3-asr-1.7b+qwen3.5-4b").ends_with("→ Qwen3.5 4B"));
        let rejected = engine_label("local/qwen3-asr-1.7b+qwen3.5-9b:rejected");
        assert!(
            rejected.contains("Qwen3.5 9B") && !rejected.contains("→"),
            "{rejected}"
        );
        let missing = engine_label("local/qwen3-asr-1.7b+qwen3.5-4b:missing");
        assert_ne!(
            missing,
            engine_label("local/qwen3-asr-1.7b+qwen3.5-4b:error")
        );
        assert!(!missing.contains("→"), "{missing}");
        // Every way the raw text was kept reads differently, and says so.
        let kept: Vec<String> = [
            "rejected",
            "rejected/hedge_lost",
            "rejected/negation_lost",
            "rejected/number_changed",
            "timeout",
            "startfail",
            "error",
            "missing",
        ]
        .iter()
        .map(|o| engine_label(&format!("local/qwen3-asr-1.7b+qwen3.5-4b:{o}")))
        .collect();
        for (i, label) in kept.iter().enumerate() {
            assert!(!label.contains("→"), "{label}");
            assert!(
                label.contains("raw transcript kept")
                    || label.contains("transcription brute conservée"),
                "{label}"
            );
            assert!(!kept[..i].contains(label), "duplicate: {label}");
        }
    }

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
        // Not inserted means not copied either, until the user asks.
        assert!(!status_label(SessionStatus::AwaitingChip).contains("Copied"));
        assert!(!status_label(SessionStatus::AwaitingChip).contains("paste"));
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
        assert!(!formatted.is_empty(), "{formatted}");
    }

    #[test]
    fn search_highlights_case_accents_prefixes_and_utf8_boundaries() {
        let text = "🎙 ÉTÉ café cafe\u{0301} réécrire fin";
        let ranges = search_highlights(text, "ete CAF réé");
        let words = ranges
            .iter()
            .map(|range| &text[range.clone()])
            .collect::<Vec<_>>();
        assert_eq!(words, ["ÉTÉ", "café", "cafe\u{0301}", "réécrire"]);
        assert!(search_highlights(text, "té").is_empty());
        assert!(search_highlights(text, "  ").is_empty());
    }

    fn services(root: &std::path::Path) -> Services {
        use jot_core::{
            coordinator::{CoordinatorDeps, DictationCoordinator},
            dictionary::DictionaryStore,
            gemini::GeminiClient,
            history::HistoryStore,
            insertion::InsertionCoordinator,
            recovery::RetryQueue,
            settings::SettingsStore,
            transcription::{GeminiTranscriptionService, TranscriptionServicing},
        };
        let settings = Arc::new(SettingsStore::open(root.join("settings.json")));
        let dictionary = Arc::new(DictionaryStore::open(root.join("dictionary.json")));
        let history = Arc::new(HistoryStore::open(&root.join("history.sqlite")).unwrap());
        let client = Arc::new(GeminiClient::new(Box::new(|| None)));
        let transcription: Arc<dyn TranscriptionServicing> = Arc::new(
            GeminiTranscriptionService::new(client.clone(), settings.clone(), dictionary.clone()),
        );
        let coordinator = DictationCoordinator::new(CoordinatorDeps {
            audio_factory: Box::new(|| panic!("audio must not run in History tests")),
            transcription: transcription.clone(),
            insertion: Arc::new(InsertionCoordinator::new()),
            context_provider: Box::new(Default::default),
            now: Box::new(OffsetDateTime::now_utc),
            noise_handling_enabled: Box::new(|| false),
            secure_field_focused: Box::new(|| false),
            after_insert: Box::new(|_, _, _| {}),
        });
        let retry_queue = Arc::new(RetryQueue::new(history.clone(), transcription.clone()));
        Services {
            settings,
            dictionary,
            history,
            client,
            transcription,
            coordinator,
            retry_queue,
            local_engine: jot_core::local::LocalEngine::new(),
        }
    }

    fn add_record(services: &Services, root: &std::path::Path, text: &str) -> String {
        let mut meta = jot_core::meta::SessionMeta::new(
            uuid::Uuid::new_v4(),
            OffsetDateTime::now_utc(),
            SessionStatus::Inserted,
        );
        meta.cleaned_transcript = Some(text.into());
        services.history.upsert(&meta, root);
        meta.id.to_string()
    }

    #[test]
    fn dirty_draft_blocks_navigation_and_refresh_keeps_it() {
        use gpui::TestAppContext;
        let temp = tempfile::tempdir().unwrap();
        let services = services(temp.path());
        let id = add_record(&services, temp.path(), "Original");
        let mut cx = TestAppContext::single();
        let view = cx.new(|cx| HistoryView::new(services, cx));
        cx.run_until_parked();
        view.update(&mut cx, |view, cx| view.open(Some(id.clone()), cx));
        cx.run_until_parked();
        view.update(&mut cx, |view, cx| {
            view.begin_edit(cx);
            view.editor
                .update(cx, |field, _| field.set_content("Mon brouillon\nconservé"));
            assert!(view.has_unsaved_changes(cx));
            view.open(None, cx);
            assert_eq!(view.pending_open, Some(None));
            assert_eq!(view.detail_id.as_deref(), Some(id.as_str()));
            view.dismiss_confirmation(cx);
            view.refresh(cx);
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, cx| {
            assert!(view.editing);
            assert_eq!(view.editor.read(cx).content(), "Mon brouillon\nconservé");
            assert_eq!(view.detail_id.as_deref(), Some(id.as_str()));
        });
        view.update(&mut cx, |view, cx| {
            view.editing = false;
            view.open(None, cx);
            assert!(view.detail_id.is_none());
        });
    }

    #[test]
    fn search_debounce_uses_the_latest_query_and_pages_can_reach_past_500() {
        use gpui::TestAppContext;
        let temp = tempfile::tempdir().unwrap();
        let services = services(temp.path());
        for index in 0..505 {
            add_record(&services, temp.path(), &format!("Exemple {index}"));
        }
        let mut cx = TestAppContext::single();
        let view = cx.new(|cx| HistoryView::new(services, cx));
        cx.run_until_parked();
        view.update(&mut cx, |view, cx| {
            assert_eq!(view.total, 505);
            assert_eq!(view.items.len(), PAGE_SIZE);
            view.offset = 500;
            view.load(false, cx);
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| assert_eq!(view.items.len(), 5));
        view.update(&mut cx, |view, cx| {
            view.search
                .update(cx, |field, _| field.set_content("absent"));
            view.offset = 0;
            view.load(true, cx);
            view.search
                .update(cx, |field, _| field.set_content("exemple"));
            view.load(true, cx);
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(181));
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.total, 505);
            assert!(!view.loading);
        });
        view.update(&mut cx, |view, cx| {
            view.offset = 500;
            view.selected.insert("old-selection".into());
            view.pending_delete = Some(vec!["old-selection".into()]);
            view.clear_search(cx);
            assert!(view.search.read(cx).is_empty());
            assert!(view.selected.is_empty());
            assert!(view.pending_delete.is_none());
            assert_eq!(view.offset, 0);
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.total, 505);
            assert_eq!(view.items.len(), PAGE_SIZE);
        });
    }

    #[test]
    fn editing_waits_for_the_refreshed_detail_and_uses_its_text() {
        use gpui::TestAppContext;
        let temp = tempfile::tempdir().unwrap();
        let services = services(temp.path());
        let id = add_record(&services, temp.path(), "Original A");
        let mut meta = jot_core::meta::SessionMeta::new(
            uuid::Uuid::parse_str(&id).unwrap(),
            OffsetDateTime::now_utc(),
            SessionStatus::Inserted,
        );
        meta.cleaned_transcript = Some("Original A".into());
        meta.write(temp.path());
        let mut cx = TestAppContext::single();
        let view = cx.new(|cx| HistoryView::new(services, cx));
        cx.run_until_parked();
        view.update(&mut cx, |view, cx| view.open(Some(id.clone()), cx));
        cx.run_until_parked();
        view.update(&mut cx, |view, cx| {
            view.services
                .history
                .edit(&id, "Original A", "New version B")
                .unwrap();
            view.refresh(cx);
            assert!(view.detail_loading);
            view.begin_edit(cx);
            assert!(
                !view.editing,
                "a draft cannot start from a stale cached detail"
            );
        });
        cx.run_until_parked();
        view.update(&mut cx, |view, cx| {
            assert!(!view.detail_loading);
            view.begin_edit(cx);
            assert_eq!(view.editor.read(cx).content(), "New version B");
        });
    }

    #[test]
    fn refresh_preserves_the_selected_version_and_cached_detail() {
        use gpui::TestAppContext;
        let temp = tempfile::tempdir().unwrap();
        let services = services(temp.path());
        let id = add_record(&services, temp.path(), "A saved dictation");
        let mut cx = TestAppContext::single();
        let view = cx.new(|cx| HistoryView::new(services, cx));
        cx.run_until_parked();
        view.update(&mut cx, |view, cx| view.open(Some(id.clone()), cx));
        cx.run_until_parked();
        view.update(&mut cx, |view, cx| {
            view.version = Version::Raw;
            view.refresh(cx);
            assert!(view.version == Version::Raw);
            assert!(view.detail.is_some());
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(view.version == Version::Raw);
            assert_eq!(view.detail_id.as_deref(), Some(id.as_str()));
        });
    }

    #[test]
    fn a_small_window_opens_the_preview_and_renders_a_long_detail_and_filters() {
        use gpui::{TestAppContext, VisualTestContext, size};
        let temp = tempfile::tempdir().unwrap();
        let services = services(temp.path());
        let text = "Longue dictée avec un paragraphe Unicode été.\n".repeat(2500);
        assert!(text.len() >= 100_000);
        let id = add_record(&services, temp.path(), &text);
        let mut cx = TestAppContext::single();
        let window = cx.add_window(|_, cx| HistoryView::new(services, cx));
        let view = window.root(&mut cx).unwrap();
        cx.run_until_parked();
        let handle = window.into();
        cx.simulate_window_resize(handle, size(px(540.0), px(480.0)));
        cx.update_window(handle, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        let mut visual = VisualTestContext::from_window(handle, &cx);
        let body = visual.debug_bounds("history-body").unwrap();
        assert!(body.size.height > px(50.0), "{body:?}");
        assert!(body.bottom() <= px(480.0), "{body:?}");
        let preview = visual.debug_bounds("history-preview").unwrap();
        visual.simulate_click(preview.center(), Default::default());
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.detail_id.as_deref(), Some(id.as_str()));
            assert_eq!(view.detail.as_ref().unwrap().current.len(), text.len());
        });
        let detail = visual.debug_bounds("history-detail-panel").unwrap();
        assert!(detail.size.height > px(50.0), "{detail:?}");
        assert!(detail.bottom() <= px(480.0), "{detail:?}");
        view.update(&mut cx, |view, cx| {
            view.filters_open = true;
            cx.notify();
        });
        cx.update_window(handle, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        let filters = visual.debug_bounds("history-filters").unwrap();
        assert!(filters.size.height > px(50.0), "{filters:?}");
        assert!(filters.bottom() <= px(480.0), "{filters:?}");
    }
}
