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

use crate::i18n::{LocalizedText, Msg, tf, tr};
use crate::services::Services;
use crate::text_field::{Submit, TextField};
use crate::theme::{self, Theme};
use crate::views::widgets::{self, ButtonKind, StatusTone};
use gpui::{Context, Entity, Role, SharedString, Window, div, prelude::*, px, uniform_list};
use jot_core::dictionary::{DictionaryEntry, TERM_LENGTH};

const ROW_HEIGHT: f32 = 64.0;

pub struct DictionaryView {
    services: Services,
    term: Entity<TextField>,
    misspelling: Entity<TextField>,
    entries: Vec<DictionaryEntry>,
    status: Option<(LocalizedText, StatusTone)>,
    _subscriptions: Vec<gpui::Subscription>,
}

impl DictionaryView {
    pub fn new(services: Services, cx: &mut Context<Self>) -> Self {
        // Placeholders describe the field rather than showing a specimen term.
        // "Kubernetes" and "cooper netties" read as entries already present,
        // so there was no way to tell an empty field from a filled one.
        let term = cx.new(|cx| TextField::new("dictionary-term", Msg::Term, cx));
        let misspelling =
            cx.new(|cx| TextField::new("dictionary-misspelling", Msg::Misspelling, cx));
        // Enter in either field adds the entry, so the whole flow is keyboard.
        let subscriptions = vec![
            cx.subscribe(&term, |this: &mut Self, _, _: &Submit, cx| this.add(cx)),
            cx.subscribe(&misspelling, |this: &mut Self, _, _: &Submit, cx| {
                this.add(cx)
            }),
        ];

        let entries = services.dictionary.entries();
        Self {
            services,
            term,
            misspelling,
            entries,
            status: None,
            _subscriptions: subscriptions,
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.entries = self.services.dictionary.entries();
        cx.notify();
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        let term = self.term.read(cx).content().trim().to_string();
        let misspelling = self.misspelling.read(cx).content().trim().to_string();

        if !TERM_LENGTH.contains(&term.chars().count()) {
            self.status = Some((
                LocalizedText::with(Msg::TermLength, &[TERM_LENGTH.start(), TERM_LENGTH.end()]),
                StatusTone::Bad,
            ));
            cx.notify();
            return;
        }

        let added = self.services.dictionary.add(
            &term,
            (!misspelling.is_empty()).then_some(misspelling.as_str()),
        );
        self.status = Some(if added {
            self.term.update(cx, |field, _| {
                field.take_content();
            });
            self.misspelling.update(cx, |field, _| {
                field.take_content();
            });
            (
                LocalizedText::with(Msg::TermAdded, &[&term]),
                StatusTone::Good,
            )
        } else {
            (
                LocalizedText::with(Msg::TermExists, &[&term]),
                StatusTone::Neutral,
            )
        });
        self.reload(cx);
    }

    fn row(
        &self,
        entry: &DictionaryEntry,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = entry.id;
        let starred = entry.starred;
        let services_star = self.services.clone();
        let services_remove = self.services.clone();

        div()
            .id(SharedString::from(format!("dictionary-row-{}", id)))
            .role(Role::ListItem)
            .aria_label(SharedString::from(entry.term.clone()))
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap(theme::spacing::S)
            .h(px(ROW_HEIGHT))
            .px(theme::spacing::S)
            .border_b_1()
            .border_color(theme.outline_variant)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(theme::type_scale::BODY_LARGE)
                            .text_color(theme.on_surface)
                            .truncate()
                            .child(entry.term.clone()),
                    )
                    .child(
                        div()
                            .text_size(theme::type_scale::LABEL)
                            .text_color(theme.on_surface_variant)
                            .truncate()
                            .child(match entry.misspelling.as_deref() {
                                Some(wrong) => tf(Msg::AlwaysCorrects, &[&wrong]),
                                None => tr(Msg::SpellingHint).to_string(),
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(theme::spacing::XS)
                    .child(widgets::button(
                        SharedString::from(format!("star-{}", id)),
                        if starred {
                            tr(Msg::Starred)
                        } else {
                            tr(Msg::Star)
                        },
                        if starred {
                            ButtonKind::Selected
                        } else {
                            ButtonKind::Secondary
                        },
                        true,
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            services_star.dictionary.toggle_star(id);
                            this.reload(cx);
                        }),
                    ))
                    .child(widgets::button(
                        SharedString::from(format!("remove-{}", id)),
                        tr(Msg::Remove),
                        ButtonKind::Destructive,
                        true,
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            services_remove.dictionary.remove(id);
                            this.reload(cx);
                        }),
                    )),
            )
    }
}

impl Render for DictionaryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::ui(window);
        let entries = self.entries.clone();
        let count = entries.len();

        let list = if count == 0 {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.on_surface_variant)
                .child(tr(Msg::DictionaryEmpty))
                .into_any_element()
        } else {
            div()
                .id("dictionary-list")
                .role(Role::List)
                .aria_label(tr(Msg::DictionaryList))
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .rounded(theme::radius::MEDIUM)
                .border_1()
                .border_color(theme.outline_variant)
                .bg(theme.surface)
                .child(
                    uniform_list(
                        "dictionary-rows",
                        count,
                        cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                            let theme = Theme::ui(window);
                            range
                                .filter_map(|index| {
                                    entries
                                        .get(index)
                                        .map(|entry| this.row(entry, theme, cx).into_any_element())
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
            .child(widgets::heading(Msg::Dictionary))
            .child(
                div()
                    .text_size(theme::type_scale::LABEL)
                    .text_color(theme.on_surface_variant)
                    .child(tr(Msg::DictionaryHint)),
            )
            .child(
                div()
                    .flex()
                    .gap(theme::spacing::XS)
                    .flex_wrap()
                    .child(div().flex_1().min_w(px(190.0)).child(self.term.clone()))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(220.0))
                            .child(self.misspelling.clone()),
                    )
                    .child(widgets::button(
                        "add-term",
                        tr(Msg::Add),
                        ButtonKind::Primary,
                        true,
                        theme,
                        cx.listener(|this, _, _, cx| this.add(cx)),
                    )),
            )
            .when_some(self.status.clone(), |column, (message, tone)| {
                column.child(widgets::status_line(
                    "dictionary-status",
                    message.render(),
                    tone,
                    theme,
                ))
            })
            .child(list)
            .child(
                div()
                    .flex()
                    .gap(theme::spacing::XS)
                    .child(widgets::button(
                        "export-csv",
                        tr(Msg::ExportCsv),
                        ButtonKind::Secondary,
                        count > 0,
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
                                match std::fs::write(&path, this.services.dictionary.export_csv()) {
                                    Ok(()) => (Msg::Exported.into(), StatusTone::Good),
                                    Err(error) => {
                                        tracing::error!(%error, "exporting the dictionary failed");
                                        (Msg::ExportFailed.into(), StatusTone::Bad)
                                    }
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
                            match std::fs::read_to_string(&path) {
                                Ok(csv) => {
                                    let imported = this.services.dictionary.import_csv(&csv);
                                    this.status = Some(if imported > 0 {
                                        (
                                            LocalizedText::with(Msg::Imported, &[&imported]),
                                            StatusTone::Good,
                                        )
                                    } else {
                                        (Msg::ImportEmpty.into(), StatusTone::Neutral)
                                    });
                                }
                                Err(error) => {
                                    tracing::error!(%error, "importing the dictionary failed");
                                    this.status = Some((Msg::ImportFailed.into(), StatusTone::Bad));
                                }
                            }
                            this.reload(cx);
                        }),
                    )),
            )
    }
}
