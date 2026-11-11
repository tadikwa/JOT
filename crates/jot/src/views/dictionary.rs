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

//! The dictionary: your jargon, spelled right.
//!
//! Terms ride along with the audio so the model hears "Kubernetes" instead of
//! guessing "cooper netties" — corrected at the source. A term that also carries
//! the misspelling gets a second, deterministic guarantee applied after the
//! model, which is the only part that is not a suggestion.

use crate::i18n::{Locale, Msg, locale, tr};
use crate::services::Services;
use crate::text_field::{Changed, Submit, TextField};
use crate::theme::{self, Theme};
use crate::views::widgets::{self, ButtonKind, PageScroll, StatusTone};
use gpui::{
    Context, Entity, Focusable, Role, SharedString, Window, div, prelude::*, px, uniform_list,
};
use jot_core::dictionary::{DictionaryEntry, DictionarySuggestion, SuggestionStatus};
use uuid::Uuid;

fn ui(en: &'static str, fr: &'static str) -> &'static str {
    match locale() {
        Locale::En => en,
        Locale::Fr => fr,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Manual,
    Learned,
    Pending,
    Rejected,
}
impl Filter {
    const ALL: [Self; 5] = [
        Self::All,
        Self::Manual,
        Self::Learned,
        Self::Pending,
        Self::Rejected,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::All => ui("All", "Tout"),
            Self::Manual => ui("Manual", "Manuelles"),
            Self::Learned => ui("Validated", "Validées"),
            Self::Pending => ui("Suggestions", "Suggestions"),
            Self::Rejected => ui("Rejected", "Refusées"),
        }
    }
}
#[derive(Clone)]
enum Row {
    Entry(DictionaryEntry),
    Suggestion(DictionarySuggestion),
}

pub struct DictionaryView {
    services: Services,
    term: Entity<TextField>,
    misspelling: Entity<TextField>,
    search: Entity<TextField>,
    entries: Vec<DictionaryEntry>,
    suggestions: Vec<DictionarySuggestion>,
    filter: Filter,
    editing: Option<Uuid>,
    status: Option<(String, StatusTone)>,
    busy: bool,
    scroll: PageScroll,
    _subscriptions: Vec<gpui::Subscription>,
}

impl DictionaryView {
    pub fn new(services: Services, cx: &mut Context<Self>) -> Self {
        let term = cx.new(|cx| TextField::new("dictionary-term", Msg::Term, cx));
        let misspelling =
            cx.new(|cx| TextField::new("dictionary-misspelling", Msg::Misspelling, cx));
        let search = cx.new(|cx| TextField::new("dictionary-search", Msg::Search, cx));
        let subscriptions = vec![
            cx.subscribe(&term, |this: &mut Self, _, _: &Submit, cx| this.save(cx)),
            cx.subscribe(&misspelling, |this: &mut Self, _, _: &Submit, cx| {
                this.save(cx)
            }),
            cx.subscribe(&search, |_: &mut Self, _, _: &Changed, cx| cx.notify()),
        ];
        let entries = services.dictionary.entries();
        let suggestions = services.dictionary.suggestions();
        let mut changes = services.history.subscribe();
        cx.spawn(async move |this, cx| {
            while !matches!(
                changes.recv().await,
                Err(tokio::sync::broadcast::error::RecvError::Closed)
            ) {
                while changes.try_recv().is_ok() {}
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            services,
            term,
            misspelling,
            search,
            entries,
            suggestions,
            filter: Filter::All,
            editing: None,
            status: None,
            busy: false,
            scroll: PageScroll::new(),
            _subscriptions: subscriptions,
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.entries = self.services.dictionary.entries();
        self.suggestions = self.services.dictionary.suggestions();
        if self.services.dictionary.load_error().is_some() {
            self.status = Some((ui("Dictionary could not be loaded. Its file is preserved; no changes will be saved.", "Dictionnaire illisible. Son fichier est conservé ; aucune modification ne sera enregistrée.").into(), StatusTone::Bad));
        }
        cx.notify();
    }

    /// Refreshing never infers new rules; it only purges pending orphan sources.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
        if self.busy
            || !self
                .suggestions
                .iter()
                .any(|s| s.status == SuggestionStatus::Pending)
        {
            return;
        }
        let history = self.services.history.clone();
        let dictionary = self.services.dictionary.clone();
        let task = cx.background_executor().spawn(async move {
            dictionary
                .purge_orphan_suggestions(&history.correction_session_ids()?.into_iter().collect())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if result.is_err() {
                    this.failure(cx);
                } else {
                    this.reload(cx);
                }
            });
        })
        .detach();
    }

    fn failure(&mut self, cx: &mut Context<Self>) {
        self.status = Some((ui("Change not saved. Check the file access, term length, duplicates or conflicting rules. Existing data is preserved.", "Modification non enregistrée. Vérifiez l’accès au fichier, la longueur, les doublons ou les règles contradictoires. Les données existantes sont conservées.").into(), StatusTone::Bad));
        cx.notify();
    }

    fn clear_editor(&mut self, cx: &mut Context<Self>) {
        self.editing = None;
        self.term.update(cx, |field, cx| {
            field.take_content();
            cx.notify();
        });
        self.misspelling.update(cx, |field, cx| {
            field.take_content();
            cx.notify();
        });
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let term = self.term.read(cx).content().trim().to_owned();
        let wrong = self.misspelling.read(cx).content().trim().to_owned();
        let wrong = (!wrong.is_empty()).then_some(wrong.as_str());
        let result = match self.editing {
            Some(id) => self.services.dictionary.update_entry(id, &term, wrong),
            None => self.services.dictionary.add_checked(&term, wrong),
        };
        if result.is_err() {
            self.failure(cx);
            return;
        }
        self.clear_editor(cx);
        self.status = Some((
            ui(
                "Entry saved. It applies to the next dictation.",
                "Entrée enregistrée. Elle s’applique à la prochaine dictée.",
            )
            .into(),
            StatusTone::Good,
        ));
        self.reload(cx);
    }

    fn scan(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let history = self.services.history.clone();
        let dictionary = self.services.dictionary.clone();
        let task = cx.background_executor().spawn(async move {
            let mut added = 0;
            for id in history.correction_session_ids()? {
                // Deletion may race this user-requested scan; never resurrect a source.
                if history.record(&id).is_none() {
                    continue;
                }
                added += dictionary.learn_from_corrections(&history.corrections(&id)?)?;
            }
            dictionary.purge_orphan_suggestions(
                &history.correction_session_ids()?.into_iter().collect(),
            )?;
            anyhow::Ok(added)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.status = Some(match result {
                    Ok(added) => (format!("{} {added}", ui("New suggestions:", "Nouvelles suggestions :")), StatusTone::Good),
                    Err(_) => (ui("Analysis incomplete. Some suggestions may have been saved; no rule was activated. Check your local files and retry.", "Analyse incomplète. Certaines suggestions peuvent avoir été enregistrées ; aucune règle n’a été activée. Vérifiez les fichiers locaux puis réessayez.").into(), StatusTone::Bad),
                });
                this.reload(cx);
            });
        }).detach();
        cx.notify();
    }

    fn rows(&self, query: &str) -> Vec<Row> {
        let mut rows = Vec::new();
        for suggestion in &self.suggestions {
            if ((suggestion.status == SuggestionStatus::Pending
                && matches!(self.filter, Filter::All | Filter::Pending))
                || (suggestion.status == SuggestionStatus::Rejected
                    && self.filter == Filter::Rejected))
                && matches_query(&suggestion.term, Some(&suggestion.wrong), query)
            {
                rows.push(Row::Suggestion(suggestion.clone()));
            }
        }
        for entry in &self.entries {
            if (self.filter == Filter::All
                || (self.filter == Filter::Manual && entry.provenance.is_none())
                || (self.filter == Filter::Learned && entry.provenance.is_some()))
                && matches_query(&entry.term, entry.misspelling.as_deref(), query)
            {
                rows.push(Row::Entry(entry.clone()));
            }
        }
        rows
    }

    fn row(&self, row: &Row, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let (id, term, wrong, description) = match row {
            Row::Entry(entry) => (
                entry.id,
                entry.term.clone(),
                entry.misspelling.clone(),
                format!(
                    "{} · {} · {}",
                    if entry.provenance.is_some() {
                        ui("Validated", "Validée")
                    } else {
                        ui("Manual", "Manuelle")
                    },
                    if entry.enabled {
                        ui("Active", "Active")
                    } else {
                        ui("Disabled", "Désactivée")
                    },
                    entry.created_at.date()
                ),
            ),
            Row::Suggestion(suggestion) => (
                suggestion.id,
                suggestion.term.clone(),
                Some(suggestion.wrong.clone()),
                format!(
                    "{} · {} {}% · {} {}",
                    if suggestion.status == SuggestionStatus::Pending {
                        ui("Pending validation", "À valider")
                    } else {
                        ui("Rejected", "Refusée")
                    },
                    ui("lexical score", "score lexical"),
                    suggestion.confidence,
                    suggestion.sources.len(),
                    ui("source(s)", "source(s)")
                ),
            ),
        };
        let mut actions = div().flex().gap(theme::spacing::XS).flex_wrap();
        match row {
            Row::Entry(entry) => {
                let entry = entry.clone();
                let enabled = entry.enabled;
                let starred = entry.starred;
                actions = actions
                    .child(widgets::button(
                        SharedString::from(format!("edit-entry-{id}")),
                        ui("Edit", "Modifier"),
                        ButtonKind::Secondary,
                        true,
                        theme,
                        cx.listener(move |this, _, window, cx| {
                            this.editing = Some(entry.id);
                            this.term.update(cx, |field, cx| {
                                field.set_content(entry.term.clone());
                                cx.notify();
                            });
                            this.misspelling.update(cx, |field, cx| {
                                field.set_content(entry.misspelling.clone().unwrap_or_default());
                                cx.notify();
                            });
                            this.scroll.reset();
                            window.focus(&this.term.read(cx).focus_handle(cx), cx);
                            cx.notify();
                        }),
                    ))
                    .child(widgets::button(
                        SharedString::from(format!("enable-entry-{id}")),
                        if enabled {
                            ui("Disable", "Désactiver")
                        } else {
                            ui("Enable", "Activer")
                        },
                        ButtonKind::Secondary,
                        true,
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            if this.services.dictionary.set_enabled(id, !enabled).is_err() {
                                this.failure(cx);
                            }
                            this.reload(cx);
                        }),
                    ))
                    .child(widgets::button(
                        SharedString::from(format!("star-entry-{id}")),
                        if starred {
                            tr(Msg::Starred)
                        } else {
                            tr(Msg::Star)
                        },
                        ButtonKind::Secondary,
                        true,
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            if this.services.dictionary.toggle_star_checked(id).is_err() {
                                this.failure(cx);
                            }
                            this.reload(cx);
                        }),
                    ))
                    .child(widgets::button(
                        SharedString::from(format!("remove-entry-{id}")),
                        tr(Msg::Remove),
                        ButtonKind::Destructive,
                        true,
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            if this.services.dictionary.remove_checked(id).is_err() {
                                this.failure(cx);
                            } else if this.editing == Some(id) {
                                this.clear_editor(cx);
                            }
                            this.reload(cx);
                        }),
                    ));
            }
            Row::Suggestion(suggestion) if suggestion.status == SuggestionStatus::Pending => {
                actions = actions
                    .child(widgets::button(
                        SharedString::from(format!("accept-suggestion-{id}")),
                        ui("Accept", "Accepter"),
                        ButtonKind::Primary,
                        true,
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            if this.services.dictionary.accept_suggestion(id).is_err() {
                                this.failure(cx);
                            } else {
                                this.status = Some((
                                    ui(
                                        "Rule accepted. Active for the next dictation.",
                                        "Règle acceptée. Active dès la prochaine dictée.",
                                    )
                                    .into(),
                                    StatusTone::Good,
                                ));
                            }
                            this.reload(cx);
                        }),
                    ))
                    .child(widgets::button(
                        SharedString::from(format!("reject-suggestion-{id}")),
                        ui("Reject", "Refuser"),
                        ButtonKind::Secondary,
                        true,
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            if this.services.dictionary.reject_suggestion(id).is_err() {
                                this.failure(cx);
                            }
                            this.reload(cx);
                        }),
                    ));
            }
            Row::Suggestion(_) => {}
        }
        div()
            .id(SharedString::from(format!("dictionary-row-{id}")))
            .role(Role::ListItem)
            .aria_label(SharedString::from(term.clone()))
            .w_full()
            .h(px(126.0))
            .p(theme::spacing::S)
            .flex()
            .flex_col()
            .gap(px(4.0))
            .border_b_1()
            .border_color(theme.outline_variant)
            .child(
                div()
                    .text_size(theme::type_scale::BODY_LARGE)
                    .truncate()
                    .child(match wrong {
                        Some(wrong) => format!("{wrong} → {term}"),
                        None => term,
                    }),
            )
            .child(
                div()
                    .text_size(theme::type_scale::LABEL)
                    .text_color(theme.on_surface_variant)
                    .truncate()
                    .child(description),
            )
            .child(actions)
    }
}

fn matches_query(term: &str, wrong: Option<&str>, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    term.to_lowercase().contains(&query)
        || wrong.is_some_and(|value| value.to_lowercase().contains(&query))
}

impl Render for DictionaryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::ui(window);
        let rows = self.rows(self.search.read(cx).content());
        let count = rows.len();
        let mut filters = div().flex().flex_wrap().gap(theme::spacing::XS);
        for (index, filter) in Filter::ALL.into_iter().enumerate() {
            filters = filters.child(widgets::button(
                ("dictionary-filter", index),
                filter.label(),
                if self.filter == filter {
                    ButtonKind::Selected
                } else {
                    ButtonKind::Secondary
                },
                true,
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.filter = filter;
                    cx.notify();
                }),
            ));
        }
        let mut editor = div()
            .flex()
            .flex_wrap()
            .gap(theme::spacing::XS)
            .child(div().flex_1().min_w(px(190.0)).child(self.term.clone()))
            .child(
                div()
                    .flex_1()
                    .min_w(px(190.0))
                    .child(self.misspelling.clone()),
            )
            .child(widgets::button(
                "save-term",
                if self.editing.is_some() {
                    ui("Save changes", "Enregistrer")
                } else {
                    tr(Msg::Add)
                },
                ButtonKind::Primary,
                true,
                theme,
                cx.listener(|this, _, _, cx| this.save(cx)),
            ));
        if self.editing.is_some() {
            editor = editor.child(widgets::button(
                "cancel-term",
                tr(Msg::Cancel),
                ButtonKind::Secondary,
                true,
                theme,
                cx.listener(|this, _, _, cx| {
                    this.clear_editor(cx);
                    cx.notify();
                }),
            ));
        }
        let list = div()
            .id("dictionary-list")
            .role(Role::List)
            .aria_label(tr(Msg::DictionaryList))
            .h(px(390.0))
            .w_full()
            .border_1()
            .border_color(theme.outline_variant)
            .rounded(theme::radius::MEDIUM)
            .bg(theme.surface)
            .when(count == 0, |list| {
                list.child(div().p(theme::spacing::M).child(ui(
                    "No matching entries or suggestions.",
                    "Aucune entrée ou suggestion correspondante.",
                )))
            })
            .when(count > 0, |list| {
                list.child(
                    uniform_list(
                        "dictionary-rows",
                        count,
                        cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                            let theme = Theme::ui(window);
                            range
                                .filter_map(|index| {
                                    rows.get(index)
                                        .map(|row| this.row(row, theme, cx).into_any_element())
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .size_full(),
                )
            });
        let data_actions = div()
            .flex()
            .flex_wrap()
            .gap(theme::spacing::XS)
            .child(widgets::button(
                "export-csv",
                tr(Msg::ExportCsv),
                ButtonKind::Secondary,
                !self.entries.is_empty(),
                theme,
                cx.listener(|this, _, _, cx| {
                    let Some(path) = crate::file_dialog::save(
                        tr(Msg::ExportDictionary),
                        crate::file_dialog::CSV,
                        "jot-dictionary.csv",
                    ) else {
                        return;
                    };
                    this.status = Some(
                        match std::fs::write(path, this.services.dictionary.export_csv()) {
                            Ok(()) => (tr(Msg::Exported).into(), StatusTone::Good),
                            Err(_) => (tr(Msg::ExportFailed).into(), StatusTone::Bad),
                        },
                    );
                    cx.notify();
                }),
            ))
            .child(widgets::button(
                "import-csv",
                tr(Msg::ImportCsv),
                ButtonKind::Secondary,
                true,
                theme,
                cx.listener(|this, _, _, cx| {
                    let Some(path) = crate::file_dialog::open(
                        tr(Msg::ImportDictionary),
                        crate::file_dialog::CSV,
                    ) else {
                        return;
                    };
                    let result = std::fs::read_to_string(path)
                        .map_err(anyhow::Error::from)
                        .and_then(|csv| this.services.dictionary.import_csv_checked(&csv));
                    match result {
                        Ok(count) => {
                            this.status = Some((
                                format!(
                                    "{} {count}",
                                    ui("Imported entries:", "Entrées importées :")
                                ),
                                StatusTone::Good,
                            ))
                        }
                        Err(_) => this.failure(cx),
                    }
                    this.reload(cx);
                }),
            ))
            .child(widgets::button(
                "learn-history",
                ui(
                    "Enrich from my corrections",
                    "Enrichir à partir de mes corrections",
                ),
                ButtonKind::Secondary,
                !self.busy,
                theme,
                cx.listener(|this, _, _, cx| this.scan(cx)),
            ))
            .child(widgets::button(
                "refresh-dictionary",
                ui(
                    "Refresh / purge orphan suggestions",
                    "Actualiser / purger les suggestions orphelines",
                ),
                ButtonKind::Secondary,
                !self.busy,
                theme,
                cx.listener(|this, _, _, cx| this.refresh(cx)),
            ));
        let mut children = vec![widgets::heading(Msg::Dictionary).into_any_element(),
            div().text_color(theme.on_surface_variant).text_size(theme::type_scale::LABEL).child(ui(
                "Suggestions come only from corrections saved in Jot. Local lexical analysis; no extra API call. Review the meaning before accepting: the score measures spelling similarity, not certainty. Accepted rules survive deletion of their source dictation.",
                "Suggestions issues uniquement des corrections enregistrées dans Jot. Analyse lexicale locale, sans appel API supplémentaire. Vérifiez le sens avant d’accepter : le score mesure la proximité orthographique, pas une certitude. Les règles acceptées survivent à la suppression de leur dictée source.")).into_any_element(),
            self.search.clone().into_any_element(), filters.into_any_element(), editor.into_any_element(), data_actions.into_any_element()];
        if self.busy {
            children.push(
                div()
                    .child(ui(
                        "Analyzing local corrections…",
                        "Analyse des corrections locales…",
                    ))
                    .into_any_element(),
            );
        }
        if let Some((message, tone)) = &self.status {
            children.push(
                widgets::status_line("dictionary-status", message.clone(), *tone, theme)
                    .into_any_element(),
            );
        }
        children.push(list.into_any_element());
        widgets::page("dictionary-page", theme, &self.scroll, children)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_matches_canonical_and_wrong_forms_with_unicode_casing() {
        assert!(matches_query("Élodie", Some("Elodi"), "élod"));
        assert!(matches_query("Claude Code", Some("Klod Code"), "KLOD"));
        assert!(matches_query("Ansible", None, ""));
        assert!(!matches_query("Proxmox", None, "Kubernetes"));
    }
}
