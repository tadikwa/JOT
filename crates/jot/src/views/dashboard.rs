use super::widgets::{self, ButtonKind, PageScroll};
use crate::i18n::{Locale, Msg, locale, tf, tr};
use crate::theme::{self, Theme};
use gpui::{
    AppContext as _, Context, Div, Render, Role, SharedString, Task, Window, div, prelude::*, px,
};
use jot_core::activity::{Activity, Period, Span, Totals, until_next_local_day};
use jot_core::history::HistoryStore;
use jot_core::settings::SettingsStore;
use std::sync::Arc;
use std::time::Duration;
use time::{Date, Duration as Days, OffsetDateTime, Weekday};
use tokio::sync::broadcast::error::RecvError;

const CALENDAR_WEEKS: i64 = 26;
const CHART_HEIGHT: f32 = 136.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Metric {
    Words,
    Dictations,
    Duration,
    Speed,
}

impl Metric {
    const ALL: [Self; 4] = [Self::Words, Self::Dictations, Self::Duration, Self::Speed];

    fn label(self) -> Msg {
        match self {
            Self::Words => Msg::StatsWordsLabel,
            Self::Dictations => Msg::StatsDictationsLabel,
            Self::Duration => Msg::StatsAudioLabel,
            Self::Speed => Msg::StatsSpeedLabel,
        }
    }

    fn value(self, totals: Totals) -> Option<f64> {
        match self {
            Self::Words => Some(totals.words as f64),
            Self::Dictations => Some(totals.dictations as f64),
            Self::Duration => (totals.timed_dictations > 0).then_some(totals.audio_seconds),
            Self::Speed => totals.wpm(),
        }
    }

    fn display(self, value: f64) -> String {
        match self {
            Self::Duration => duration(value),
            Self::Speed => tf(Msg::StatsWpm, &[&number(value.round() as u64)]),
            _ => number(value.round() as u64),
        }
    }
}

pub struct DashboardView {
    history: Arc<HistoryStore>,
    settings: Arc<SettingsStore>,
    activity: Option<Activity>,
    error: bool,
    period: Period,
    metric: Metric,
    scroll: PageScroll,
    refresh: Option<Task<()>>,
}

impl DashboardView {
    pub fn new(history: Arc<HistoryStore>, settings: Arc<SettingsStore>) -> Self {
        Self {
            history,
            settings,
            activity: None,
            error: false,
            period: Period::Month,
            metric: Metric::Words,
            scroll: PageScroll::new(),
            refresh: None,
        }
    }

    pub fn resume(&mut self, cx: &mut Context<Self>) {
        if self.refresh.is_some() {
            return;
        }
        let mut changes = self.history.subscribe();
        let store = self.history.clone();
        let executor = cx.background_executor().clone();
        self.refresh = Some(cx.spawn(async move |this, cx| {
            loop {
                let history = store.clone();
                let (result, until_midnight) = executor
                    .spawn(async move {
                        let now = OffsetDateTime::now_utc();
                        (history.activity(now), until_next_local_day(now).ok())
                    })
                    .await;
                if this
                    .update(cx, |view, cx| {
                        view.error = result.is_err();
                        view.activity = match result {
                            Ok(activity) => Some(activity),
                            Err(error) => {
                                tracing::warn!(%error, "dashboard activity query failed");
                                None
                            }
                        };
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }

                let midnight = async {
                    if let Some(wait) = until_midnight {
                        executor.timer(wait).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                };
                let change = async {
                    loop {
                        match changes.recv().await {
                            Ok(false) => continue,
                            Ok(true) | Err(RecvError::Lagged(_)) => return true,
                            Err(RecvError::Closed) => return false,
                        }
                    }
                };
                tokio::select! {
                    _ = midnight => {}
                    open = change => {
                        if !open {
                            break;
                        }
                        // The coordinator and startup reindex can publish several writes together.
                        executor.timer(Duration::from_millis(120)).await;
                        while !changes.is_empty() {
                            let _ = changes.try_recv();
                        }
                    }
                }
            }
        }));
    }

    pub fn pause(&mut self, _: &mut Context<Self>) {
        self.refresh = None;
    }

    fn period_options(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let labels = [
            Msg::Stats7Days,
            Msg::Stats30Days,
            Msg::Stats90Days,
            Msg::StatsYear,
            Msg::StatsAll,
        ];
        div()
            .id("stats-period")
            .role(Role::RadioGroup)
            .aria_label(tr(Msg::StatsPeriod))
            .flex()
            .flex_wrap()
            .gap(theme::spacing::XS)
            .children(Period::ALL.into_iter().zip(labels).enumerate().map(
                |(i, (period, label))| {
                    option(
                        ("stats-period", i),
                        label,
                        self.period == period,
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            this.period = period;
                            cx.notify();
                        }),
                    )
                },
            ))
    }

    fn trends(
        &self,
        activity: &Activity,
        span: Span,
        totals: Totals,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let controls = div()
            .id("chart-metric")
            .role(Role::RadioGroup)
            .aria_label(tr(Msg::StatsMetric))
            .flex()
            .flex_wrap()
            .gap(theme::spacing::XS)
            .children(Metric::ALL.into_iter().enumerate().map(|(i, metric)| {
                option(
                    ("metric", i),
                    metric.label(),
                    self.metric == metric,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        this.metric = metric;
                        cx.notify();
                    }),
                )
            }));
        let graph = if totals.dictations == 0 {
            note(tr(Msg::StatsPeriodEmpty), theme).into_any_element()
        } else if self.metric.value(totals).is_none() {
            note(tr(Msg::StatsNoDuration), theme).into_any_element()
        } else {
            chart(activity, span, self.metric, theme).into_any_element()
        };
        widgets::section(
            tr(Msg::StatsTrends),
            theme,
            vec![controls.into_any_element(), graph],
        )
    }
}

impl Render for DashboardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::ui(window);
        let four_columns = window.viewport_size().width >= px(900.0);
        let mut body = vec![widgets::heading(Msg::DashboardTitle).into_any_element()];
        if let Some(activity) = &self.activity {
            let first = activity.days.keys().next().copied();
            let span = self.period.span(activity.today, first);
            let previous = (self.period != Period::All)
                .then(|| span.previous())
                .flatten();
            let totals = activity.total(span);
            let prior = previous.map(|span| activity.total(span));
            let lifetime = activity.total(Period::All.span(activity.today, first));
            let typing_wpm = self.settings.get().typing_wpm();
            body.push(self.period_options(theme, cx).into_any_element());
            body.push(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_between()
                    .gap_x(theme::spacing::XS)
                    .child(note(span_label(span), theme))
                    .when_some(previous, |row, span| {
                        row.child(note(tf(Msg::StatsPrior, &[&span_label(span)]), theme))
                    })
                    .into_any_element(),
            );
            if lifetime.dictations == 0 {
                body.push(
                    widgets::section(
                        tr(Msg::StatsEmptyTitle),
                        theme,
                        vec![note(tr(Msg::StatsEmptyBody), theme).into_any_element()],
                    )
                    .into_any_element(),
                );
            }
            body.push(
                summary(totals, prior, lifetime, typing_wpm, four_columns, theme)
                    .into_any_element(),
            );
            if lifetime.dictations > 0 {
                body.push(
                    self.trends(activity, span, totals, theme, cx)
                        .into_any_element(),
                );
            }
            body.push(calendar(activity, theme).into_any_element());
        } else {
            body.push(
                note(
                    tr(if self.error {
                        Msg::StatsLoadError
                    } else {
                        Msg::StatsLoading
                    }),
                    theme,
                )
                .into_any_element(),
            );
        }
        widgets::page("dashboard", theme, &self.scroll, body)
    }
}

fn option(
    id: (&'static str, usize),
    message: Msg,
    selected: bool,
    theme: Theme,
    handler: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    widgets::button(
        id,
        tr(message),
        if selected {
            ButtonKind::Selected
        } else {
            ButtonKind::Secondary
        },
        true,
        theme,
        handler,
    )
    .role(Role::RadioButton)
    .aria_toggled(if selected {
        gpui::Toggled::True
    } else {
        gpui::Toggled::False
    })
}

fn summary(
    totals: Totals,
    prior: Option<Totals>,
    lifetime: Totals,
    typing_wpm: u16,
    four_columns: bool,
    theme: Theme,
) -> impl IntoElement {
    let comparison = |now: u64, before: u64| {
        let value = match now.cmp(&before) {
            std::cmp::Ordering::Greater => format!("+{}", number(now - before)),
            std::cmp::Ordering::Less => format!("−{}", number(before - now)),
            std::cmp::Ordering::Equal => "0".to_string(),
        };
        tf(Msg::StatsComparison, &[&value])
    };
    let coverage = || {
        if totals.timed_dictations == 0 {
            tr(Msg::StatsNoDuration).to_string()
        } else {
            tf(
                Msg::StatsTimed,
                &[&number(totals.timed_dictations), &number(totals.dictations)],
            )
        }
    };
    let words = card(
        Msg::StatsWords,
        number(totals.words),
        prior
            .map(|prior| comparison(totals.words, prior.words))
            .into_iter()
            .chain([tf(Msg::StatsLifetime, &[&number(lifetime.words)])]),
        theme,
    );
    let time_saved = card(
        Msg::StatsEstimate,
        totals
            .estimated_seconds_saved(typing_wpm)
            .map(duration)
            .unwrap_or_else(unavailable),
        [
            tf(Msg::StatsReference, &[&tf(Msg::StatsWpm, &[&typing_wpm])]),
            coverage(),
        ],
        theme,
    );
    let speed = card(
        Msg::StatsSpeed,
        totals
            .wpm()
            .map(|value| number(value.round() as u64))
            .unwrap_or_else(unavailable),
        [tr(Msg::StatsWpmUnit).to_string(), coverage()],
        theme,
    );
    let dictations = card(
        Msg::StatsDictations,
        number(totals.dictations),
        prior
            .map(|prior| comparison(totals.dictations, prior.dictations))
            .into_iter()
            .chain([tf(Msg::StatsLifetime, &[&number(lifetime.dictations)])]),
        theme,
    );
    let cards = if four_columns {
        card_row().children([words, time_saved, speed, dictations])
    } else {
        div()
            .flex()
            .flex_col()
            .gap(theme::spacing::S)
            .child(card_row().children([words, time_saved]))
            .child(card_row().children([speed, dictations]))
    };
    let has_outcomes = totals.failed + totals.cancelled + totals.queued > 0;
    div()
        .flex()
        .flex_col()
        .gap(theme::spacing::XS)
        .child(cards)
        .when(has_outcomes, |column| {
            column.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(theme::spacing::S)
                    .child(note(tr(Msg::StatsOutcomes), theme))
                    .children(
                        [
                            tf(Msg::StatsFailures, &[&number(totals.failed)]),
                            tf(Msg::StatsCancellations, &[&number(totals.cancelled)]),
                            tf(Msg::StatsQueued, &[&number(totals.queued)]),
                        ]
                        .into_iter()
                        .map(|label| note(label, theme)),
                    ),
            )
        })
        .when(totals.timed_dictations > 0, |column| {
            column.child(note(
                tf(
                    Msg::StatsPipeline,
                    &[
                        &number(totals.measured_pipeline),
                        &number(totals.timed_dictations),
                    ],
                ),
                theme,
            ))
        })
        .child(note(tr(Msg::StatsEstimateHint), theme))
}

fn card_row() -> Div {
    div().flex().gap(theme::spacing::S)
}

fn card(
    label: Msg,
    value: String,
    notes: impl IntoIterator<Item = String>,
    theme: Theme,
) -> gpui::Stateful<Div> {
    let notes: Vec<_> = notes.into_iter().collect();
    let accessible = format!("{} · {} · {}", tr(label), value, notes.join(" · "));
    div()
        .id(SharedString::from(format!("stat-card-{}", tr(label))))
        .role(Role::Group)
        .aria_label(accessible)
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap(theme::spacing::XXS)
        .p(theme::spacing::S)
        .rounded(theme::radius::MEDIUM)
        .border_1()
        .border_color(theme.outline_variant)
        .bg(theme.surface)
        .child(
            div()
                .min_h(px(36.0))
                .text_size(theme::type_scale::BODY)
                .font_weight(theme::weight::MEDIUM)
                .child(tr(label)),
        )
        .child(value_text(value).text_color(theme.primary))
        .children(notes.into_iter().map(|line| note(line, theme)))
}

fn value_text(value: String) -> Div {
    div()
        .text_size(px(26.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .line_height(gpui::relative(theme::line_height::TIGHT))
        .child(value)
}

fn note(text: impl Into<SharedString>, theme: Theme) -> Div {
    div()
        .text_size(theme::type_scale::LABEL)
        .text_color(theme.on_surface_variant)
        .child(text.into())
}

fn chart(activity: &Activity, span: Span, metric: Metric, theme: Theme) -> impl IntoElement {
    let samples: Vec<_> = span
        .buckets()
        .into_iter()
        .map(|bucket| (bucket, activity.total(bucket)))
        .collect();
    let max = samples
        .iter()
        .filter_map(|(_, totals)| metric.value(*totals))
        .fold(0.0, f64::max);
    let step = axis_step(max);
    let grid = div()
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        .justify_between()
        .children((0..5).map(|_| div().h(px(1.0)).bg(theme.outline_variant)));
    let bars = div()
        .relative()
        .h(px(CHART_HEIGHT))
        .flex()
        .gap_x(px(if samples.len() > 15 { 3.0 } else { 6.0 }))
        .child(grid)
        .children(samples.iter().enumerate().map(|(index, &(bucket, total))| {
            let value = metric.value(total).unwrap_or(0.0);
            let label = details(bucket, total, true);
            div()
                .id(("chart-bucket", index))
                .role(Role::Group)
                .aria_label(label.clone())
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .justify_end()
                .rounded(px(4.0))
                .hover(move |style| style.bg(theme.surface_container))
                .tooltip(move |_, cx| cx.new(|_| DataTooltip(label.clone())).into())
                .child(
                    div()
                        .h(px(if value > 0.0 {
                            (CHART_HEIGHT * (value / (step * 4.0)) as f32).max(2.0)
                        } else {
                            0.0
                        }))
                        .rounded(px(3.0))
                        .bg(theme.primary),
                )
        }));
    div()
        .flex()
        .gap(theme::spacing::XS)
        .pt(theme::spacing::XS)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .h(px(CHART_HEIGHT))
                .justify_between()
                .children(
                    (0..5)
                        .rev()
                        .map(|i| note(metric.display(step * f64::from(i)), theme)),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(theme::spacing::XXS)
                .child(bars)
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(note(short_date(span.start), theme))
                        .when(span.start != span.end, |row| {
                            row.child(note(short_date(span.end), theme))
                        }),
                ),
        )
}

fn axis_step(max: f64) -> f64 {
    let target = (max / 4.0).max(1.0);
    let power = 10_f64.powf(target.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|factor| power * factor)
        .find(|step| *step >= target)
        .unwrap_or(target)
}

fn calendar(activity: &Activity, theme: Theme) -> impl IntoElement {
    let streaks = activity.streaks();
    let offset = i64::from(activity.today.weekday().number_days_from_monday());
    let first_week = activity.today - Days::days(offset + 7 * (CALENDAR_WEEKS - 1));
    let weeks = 0..CALENDAR_WEEKS;
    let counts = div().flex().gap(theme::spacing::S).children(
        [
            (Msg::StatsActiveDays, streaks.active_days),
            (Msg::StatsCurrentStreak, streaks.current),
            (Msg::StatsLongestStreak, streaks.longest),
        ]
        .into_iter()
        .map(|(label, value)| {
            div()
                .flex_1()
                .min_w_0()
                .child(value_text(number(value)))
                .child(note(tr(label), theme))
        }),
    );
    let month_labels = div()
        .flex()
        .gap_x(px(3.0))
        .h(px(19.0))
        .children(weeks.clone().map(|week| {
            let monday = first_week + Days::days(week * 7);
            let sunday = (monday + Days::days(6)).min(activity.today);
            let label = if week == 0 && monday.day() <= 23 {
                Some(month(monday))
            } else if monday.day() == 1 || monday.month() != sunday.month() {
                Some(month(sunday))
            } else {
                None
            };
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .when_some(label, |cell, label| {
                    cell.child(
                        note(label, theme)
                            .absolute()
                            .top_0()
                            .left_0()
                            .whitespace_nowrap(),
                    )
                })
        }));
    let cells = div().flex().gap_x(px(3.0)).children(weeks.map(|week| {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap_y(px(3.0))
            .children((0..7).map(move |weekday| {
                let date = first_week + Days::days(week * 7 + weekday);
                let size = div().h(px(12.0)).rounded(px(3.0));
                if date > activity.today {
                    return size.into_any_element();
                }
                let day = activity.days.get(&date).copied().unwrap_or_default();
                let label = details(
                    Span {
                        start: date,
                        end: date,
                    },
                    day,
                    false,
                );
                size.id(("calendar-day", date.to_julian_day() as usize))
                    .role(Role::Group)
                    .aria_label(label.clone())
                    .border_1()
                    .border_color(theme.outline_variant)
                    .bg(intensity(intensity_level(day.words), theme))
                    .hover(move |style| style.border_color(theme.on_surface))
                    .tooltip(move |_, cx| cx.new(|_| DataTooltip(label.clone())).into())
                    .into_any_element()
            }))
    }));
    let weekdays = div()
        .flex()
        .flex_col()
        .gap_y(px(3.0))
        .w(px(29.0))
        .flex_none()
        .children(
            [Weekday::Monday, Weekday::Wednesday, Weekday::Friday]
                .into_iter()
                .enumerate()
                .map(|(index, weekday)| {
                    note(weekday_label(weekday), theme)
                        .h(px(if index == 2 { 12.0 } else { 27.0 }))
                        .line_height(px(12.0))
                }),
        );
    let legend = div()
        .flex()
        .items_center()
        .justify_end()
        .gap(theme::spacing::XXS)
        .child(note(tr(Msg::StatsLess), theme))
        .children((0..=4).map(|level| {
            div()
                .size(px(11.0))
                .rounded(px(2.0))
                .bg(intensity(level, theme))
        }))
        .child(note(tr(Msg::StatsMore), theme));
    widgets::section(
        tr(Msg::StatsCalendar),
        theme,
        vec![
            note(tr(Msg::StatsCalendarHint), theme).into_any_element(),
            counts.into_any_element(),
            div()
                .flex()
                .items_end()
                .gap(theme::spacing::XXS)
                .child(weekdays.pb(px(30.0)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .max_w(px(468.0))
                        .child(month_labels)
                        .child(cells),
                )
                .into_any_element(),
            legend.into_any_element(),
        ],
    )
}

fn intensity_level(words: u64) -> u8 {
    match words {
        0 => 0,
        1..=49 => 1,
        50..=149 => 2,
        150..=349 => 3,
        _ => 4,
    }
}

fn intensity(level: u8, theme: Theme) -> gpui::Rgba {
    match level {
        0 => theme.surface_container,
        _ => widgets::state_layer(
            theme.surface,
            theme.primary,
            [0.0, 0.25, 0.45, 0.68, 0.92][usize::from(level)],
        ),
    }
}

struct DataTooltip(String);

impl Render for DataTooltip {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::ui(window);
        div()
            .flex()
            .flex_col()
            .p(theme::spacing::XS)
            .rounded(theme::radius::SMALL)
            .border_1()
            .border_color(theme.outline)
            .bg(theme.surface)
            .text_color(theme.on_surface)
            .font_family(theme::ui_font())
            .text_size(theme::type_scale::LABEL)
            .children(self.0.lines().map(|line| div().child(line.to_string())))
    }
}

fn details(span: Span, totals: Totals, show_speed: bool) -> String {
    let audio = Metric::Duration
        .value(totals)
        .map(duration)
        .unwrap_or_else(|| {
            if totals.dictations == 0 {
                duration(0.0)
            } else {
                unavailable()
            }
        });
    let mut lines = vec![
        span_label(span),
        format!("{}  {}", tr(Msg::StatsWordsLabel), number(totals.words)),
        format!(
            "{}  {}",
            tr(Msg::StatsDictationsLabel),
            number(totals.dictations)
        ),
        format!("{}  {audio}", tr(Msg::StatsAudioLabel)),
    ];
    if totals.dictations == 0 {
        lines.push(tr(Msg::StatsNone).to_string());
    } else if totals.timed_dictations < totals.dictations {
        lines.push(tf(
            Msg::StatsTimed,
            &[&number(totals.timed_dictations), &number(totals.dictations)],
        ));
    }
    if show_speed && let Some(wpm) = totals.wpm() {
        lines.push(Metric::Speed.display(wpm));
    }
    lines.join("\n")
}

fn unavailable() -> String {
    "—".to_string()
}

fn number(value: u64) -> String {
    let digits = value.to_string();
    let separator = if locale() == Locale::Fr {
        '\u{202f}'
    } else {
        ','
    };
    let mut output = String::new();
    for (index, digit) in digits.bytes().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            output.push(separator);
        }
        output.push(char::from(digit));
    }
    output
}

fn duration(seconds: f64) -> String {
    let rounded = seconds.round();
    let absolute = rounded.abs() as u64;
    let sign = if rounded < 0.0 { "−" } else { "" };
    let (hours, minutes, seconds) = (absolute / 3600, (absolute / 60) % 60, absolute % 60);
    if hours > 0 {
        format!("{sign}{} h {minutes:02} min", number(hours))
    } else if minutes > 0 {
        format!("{sign}{minutes} min {seconds:02} s")
    } else {
        format!("{sign}{seconds} s")
    }
}

fn month(date: Date) -> &'static str {
    let names = match locale() {
        Locale::En => [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ],
        Locale::Fr => [
            "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.",
            "nov.", "déc.",
        ],
    };
    names[usize::from(u8::from(date.month()) - 1)]
}

fn weekday_label(day: Weekday) -> &'static str {
    let names = match locale() {
        Locale::En => ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
        Locale::Fr => ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."],
    };
    names[usize::from(day.number_days_from_monday())]
}

fn short_date(date: Date) -> String {
    match locale() {
        Locale::Fr => format!("{} {}", date.day(), month(date)),
        Locale::En => format!("{} {}", month(date), date.day()),
    }
}

fn span_label(span: Span) -> String {
    let full_date = |date: Date| format!("{} {}", short_date(date), date.year());
    if span.start == span.end {
        full_date(span.end)
    } else if span.start.year() == span.end.year() {
        format!("{} – {}", short_date(span.start), full_date(span.end))
    } else {
        format!("{} – {}", full_date(span.start), full_date(span.end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_events_refresh_the_dashboard_and_reopening_reads_missed_changes() {
        use gpui::TestAppContext;
        use jot_core::meta::{SessionMeta, SessionStatus};
        let temporary = tempfile::tempdir().unwrap();
        let history =
            Arc::new(HistoryStore::open(&temporary.path().join("history.sqlite")).unwrap());
        let settings = Arc::new(SettingsStore::open(temporary.path().join("settings.json")));
        let mut cx = TestAppContext::single();
        let view = cx.new(|_| DashboardView::new(history.clone(), settings));
        view.update(&mut cx, DashboardView::resume);
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(view.activity.as_ref().unwrap().days.is_empty())
        });

        let mut session = SessionMeta::new(
            uuid::Uuid::new_v4(),
            OffsetDateTime::now_utc(),
            SessionStatus::Inserted,
        );
        session.cleaned_transcript = Some("une première dictée".into());
        session.audio_duration_seconds = Some(3.0);
        let refresh = |cx: &mut TestAppContext| {
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(125));
            cx.run_until_parked();
        };
        let words = |cx: &TestAppContext| {
            cx.read_entity(&view, |view, _| {
                view.activity
                    .as_ref()
                    .unwrap()
                    .days
                    .values()
                    .map(|day| day.words)
                    .sum::<u64>()
            })
        };
        history.upsert(&session, temporary.path());
        refresh(&mut cx);
        assert_eq!(words(&cx), 3);
        session.cleaned_transcript = Some("texte corrigé".into());
        history.upsert(&session, temporary.path());
        refresh(&mut cx);
        assert_eq!(words(&cx), 2);
        for _ in 0..40 {
            history.upsert(&session, temporary.path());
        }
        refresh(&mut cx);
        assert_eq!(words(&cx), 2);
        history.delete(&session.id.to_string(), false);
        refresh(&mut cx);
        assert_eq!(words(&cx), 0);

        view.update(&mut cx, DashboardView::pause);
        history.upsert(&session, temporary.path());
        refresh(&mut cx);
        assert_eq!(words(&cx), 0);
        view.update(&mut cx, DashboardView::resume);
        cx.run_until_parked();
        assert_eq!(words(&cx), 2);
    }

    #[test]
    fn chart_scale_covers_small_and_large_values() {
        for (value, expected) in [
            (0.0, 1.0),
            (4.0, 1.0),
            (9.0, 5.0),
            (78.0, 20.0),
            (5700.0, 2000.0),
        ] {
            assert_eq!(axis_step(value), expected);
            assert!(axis_step(value) * 4.0 >= value);
        }
    }

    #[test]
    fn inactive_days_are_distinct_and_estimates_can_be_negative() {
        assert_eq!(intensity_level(0), 0);
        assert_eq!(intensity_level(1), 1);
        assert_eq!(intensity_level(350), 4);
        assert_eq!(duration(-0.1), "0 s");
        assert_eq!(duration(-64.0), "−1 min 04 s");
    }
}
