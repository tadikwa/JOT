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

//! Jot’s workspace and auxiliary windows. Closing them leaves dictation available.

pub mod dashboard;
pub mod dictionary;
pub mod history;
pub mod onboarding;
pub mod settings;
pub mod widgets;

use crate::i18n::{Msg, tf, tr};
use crate::services::Services;
use crate::theme::{self, Theme};
use crate::views::widgets::StatusTone;
use gpui::{
    AnyWindowHandle, App, Bounds, Context, Entity, Render, Role, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, relative, size,
};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Open tool windows, so a second request focuses the existing one instead of
/// stacking duplicates on top of each other.
static OPEN_WINDOWS: LazyLock<Mutex<HashMap<&'static str, AnyWindowHandle>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Opens a tool window, or brings the existing one forward.
fn open_tool_window<V: 'static + Render>(
    key: &'static str,
    title: &'static str,
    dimensions: (f32, f32),
    minimum: Option<(f32, f32)>,
    cx: &mut App,
    build: impl FnOnce(&mut gpui::Window, &mut App) -> gpui::Entity<V>,
) {
    if let Some(existing) = OPEN_WINDOWS.lock().get(key).copied()
        && cx.windows().contains(&existing)
    {
        let _ = existing.update(cx, |_, window, _| window.activate_window());
        return;
    }

    let (width, height) = dimensions;
    let mut preferred = size(px(width), px(height));
    let bounds = if let Some(display) = cx.primary_display() {
        let area = display.visible_bounds();
        preferred.width = preferred.width.min(area.size.width);
        preferred.height = preferred.height.min(area.size.height);
        Bounds::centered_at(area.center(), preferred)
    } else {
        Bounds::centered(None, preferred, cx)
    };
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(title.into()),
                ..Default::default()
            }),
            focus: true,
            show: true,
            app_id: Some("com.ammaar.jot".into()),
            is_resizable: minimum.is_some(),
            window_min_size: minimum
                .map(|(w, h)| size(px(w).min(preferred.width), px(h).min(preferred.height))),
            is_minimizable: true,
            ..Default::default()
        },
        |window, cx| build(window, cx),
    );

    match opened {
        Ok(handle) => {
            OPEN_WINDOWS.lock().insert(key, handle.into());
            cx.activate(true);
        }
        Err(error) => tracing::error!(%error, window = key, "could not open window"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Dashboard,
    History,
    Dictionary,
    Settings,
}

impl Route {
    const ALL: [Self; 4] = [
        Self::Dashboard,
        Self::History,
        Self::Dictionary,
        Self::Settings,
    ];

    fn label(self) -> Msg {
        match self {
            Self::Dashboard => Msg::Dashboard,
            Self::History => Msg::History,
            Self::Dictionary => Msg::Dictionary,
            Self::Settings => Msg::Settings,
        }
    }
}

const WORKSPACE: &str = "workspace";

pub fn open_workspace(route: Route, services: &Services, cx: &mut App) {
    let existing = OPEN_WINDOWS.lock().get(WORKSPACE).copied();
    if let Some(handle) = existing.and_then(|handle| handle.downcast::<WorkspaceView>())
        && handle
            .update(cx, |view, window, cx| {
                view.navigate(route, cx);
                window.activate_window();
            })
            .is_ok()
    {
        return;
    }

    let services = services.clone();
    open_tool_window(
        WORKSPACE,
        "Jot",
        (960.0, 690.0),
        Some((540.0, 480.0)),
        cx,
        move |_, cx| cx.new(|cx| WorkspaceView::new(route, services, cx)),
    );
}

struct WorkspaceView {
    route: Route,
    services: Services,
    dashboard: Option<Entity<dashboard::DashboardView>>,
    history: Option<Entity<history::HistoryView>>,
    dictionary: Option<Entity<dictionary::DictionaryView>>,
    settings: Option<Entity<settings::SettingsView>>,
}

impl WorkspaceView {
    fn new(route: Route, services: Services, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            route,
            services,
            dashboard: None,
            history: None,
            dictionary: None,
            settings: None,
        };
        view.navigate(route, cx);
        view
    }

    fn can_leave_history(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(history) = &self.history
            && history.read(cx).has_unsaved_changes(cx)
        {
            history.update(cx, history::HistoryView::prevent_navigation);
            return false;
        }
        true
    }

    fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        if self.route != route && !self.can_leave_history(cx) {
            return;
        }
        if self.route != route {
            match self.route {
                Route::Dashboard => {
                    if let Some(view) = &self.dashboard {
                        view.update(cx, dashboard::DashboardView::pause);
                    }
                }
                Route::History => {
                    if let Some(view) = &self.history {
                        view.update(cx, history::HistoryView::dismiss_confirmation);
                    }
                }
                Route::Settings => {
                    if let Some(view) = &self.settings {
                        view.update(cx, settings::SettingsView::dismiss_confirmations);
                    }
                }
                Route::Dictionary => {}
            }
        }
        match route {
            Route::Dashboard => {
                let dashboard = self.dashboard.get_or_insert_with(|| {
                    cx.new(|_| {
                        dashboard::DashboardView::new(
                            self.services.history.clone(),
                            self.services.settings.clone(),
                        )
                    })
                });
                dashboard.update(cx, dashboard::DashboardView::resume);
            }
            Route::History => {
                self.history.get_or_insert_with(|| {
                    cx.new(|cx| history::HistoryView::new(self.services.clone(), cx))
                });
            }
            Route::Dictionary => {
                self.dictionary.get_or_insert_with(|| {
                    cx.new(|cx| dictionary::DictionaryView::new(self.services.clone(), cx))
                });
            }
            Route::Settings => {
                self.settings.get_or_insert_with(|| {
                    cx.new(|cx| settings::SettingsView::new(self.services.clone(), cx))
                });
            }
        }
        self.route = route;
        cx.notify();
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            entity
                .update(cx, |view, cx| view.can_leave_history(cx))
                .unwrap_or(true)
        });
        let theme = Theme::ui(window);
        let wide = window.viewport_size().width >= px(820.0);
        let entity = cx.entity().downgrade();
        let navigation = route_tabs(wide, self.route, theme, move |route, cx| {
            let _ = entity.update(cx, |view, cx| view.navigate(route, cx));
        });
        let page = match self.route {
            Route::Dashboard => self.dashboard.as_ref().unwrap().clone().into_any_element(),
            Route::History => self.history.as_ref().unwrap().clone().into_any_element(),
            Route::Dictionary => self.dictionary.as_ref().unwrap().clone().into_any_element(),
            Route::Settings => self.settings.as_ref().unwrap().clone().into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .when(!wide, |root| root.flex_col())
            .bg(theme.window_background)
            .text_color(theme.on_surface)
            .font_family(theme::ui_font())
            .child(
                div()
                    .flex()
                    .flex_none()
                    .p(theme::spacing::M)
                    .gap(theme::spacing::M)
                    .bg(theme.surface_container)
                    .border_color(theme.outline_variant)
                    .when(wide, |rail| rail.w(px(184.0)).flex_col().border_r_1())
                    .when(!wide, |bar| bar.items_center().flex_wrap().border_b_1())
                    .child(
                        div()
                            .text_size(px(25.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Jot"),
                    )
                    .child(navigation),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(page),
            )
    }
}

fn route_tabs(
    wide: bool,
    current: Route,
    theme: Theme,
    select: impl Fn(Route, &mut App) + Clone + 'static,
) -> impl IntoElement {
    div()
        .id("main-navigation")
        .role(Role::TabList)
        .aria_label(tr(Msg::Navigation))
        .flex()
        .gap_2()
        .when(!wide, |bar| bar.flex_wrap())
        .when(wide, |bar| bar.flex_col().items_stretch())
        .children(Route::ALL.into_iter().enumerate().map(|(index, route)| {
            let select = select.clone();
            widgets::navigation_button(
                ("route", index),
                tr(route.label()),
                route == current,
                theme,
                move |_, _, cx| select(route, cx),
            )
        }))
}

pub fn refresh_language(cx: &mut App) {
    let windows = OPEN_WINDOWS.lock();
    for (key, title) in [("onboarding", Msg::Setup), ("about", Msg::About)] {
        if let Some(handle) = windows.get(key) {
            let _ = handle.update(cx, |_, window, _| window.set_window_title(tr(title)));
        }
    }
    cx.refresh_windows();
}

pub fn open_onboarding(services: &Services, cx: &mut App) {
    let services = services.clone();
    open_tool_window(
        "onboarding",
        tr(Msg::Setup),
        (580.0, 490.0),
        Some((460.0, 380.0)),
        cx,
        move |_, cx| cx.new(|cx| onboarding::OnboardingView::new(services, cx)),
    );
}

pub fn open_about(cx: &mut App) {
    open_tool_window(
        "about",
        tr(Msg::About),
        (480.0, 380.0),
        Some((380.0, 320.0)),
        cx,
        |_, cx| cx.new(AboutView::new),
    );
}

/// About, which is also where Jot asks whether it is out of date.
///
/// The check runs here rather than at startup on purpose: opening this window
/// is a deliberate act, so the one request Jot makes to anything other than the
/// Gemini API stays something the user asked for.
pub struct AboutView {
    update: UpdateState,
    scroll: widgets::PageScroll,
}

enum UpdateState {
    Checking,
    UpToDate,
    Available(jot_core::update::Update),
    /// GitHub was unreachable or unhappy. Worth saying, not worth retrying:
    /// this window is not the reason anyone opened Jot.
    Unknown,
}

impl AboutView {
    fn new(cx: &mut gpui::Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            let outcome =
                jot_core::runtime::spawn(jot_core::update::check(env!("CARGO_PKG_VERSION"))).await;
            let state = match outcome {
                Ok(Ok(Some(update))) => UpdateState::Available(update),
                Ok(Ok(None)) => UpdateState::UpToDate,
                Ok(Err(error)) => {
                    tracing::debug!(?error, "update check failed");
                    UpdateState::Unknown
                }
                Err(error) => {
                    tracing::debug!(?error, "update check did not run");
                    UpdateState::Unknown
                }
            };
            this.update(cx, |this, cx| {
                this.update = state;
                cx.notify();
            })
            .ok();
        })
        .detach();

        Self {
            update: UpdateState::Checking,
            scroll: widgets::PageScroll::new(),
        }
    }

    fn update_element(&self, theme: Theme) -> gpui::AnyElement {
        match &self.update {
            UpdateState::Checking => widgets::status_line(
                "update",
                tr(Msg::UpdateChecking),
                StatusTone::Neutral,
                theme,
            )
            .into_any_element(),
            UpdateState::UpToDate => {
                widgets::status_line("update", tr(Msg::UpdateCurrent), StatusTone::Good, theme)
                    .into_any_element()
            }
            UpdateState::Unknown => {
                widgets::status_line("update", tr(Msg::UpdateUnknown), StatusTone::Neutral, theme)
                    .into_any_element()
            }
            UpdateState::Available(update) => {
                let page = update.page.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(widgets::status_line(
                        "update",
                        tf(Msg::UpdateAvailable, &[&update.version]),
                        StatusTone::Good,
                        theme,
                    ))
                    .child(widgets::hug(widgets::button(
                        "open-release",
                        tr(Msg::UpdateDownload),
                        widgets::ButtonKind::Primary,
                        true,
                        theme,
                        move |_, _, cx| cx.open_url(&page),
                    )))
                    .into_any_element()
            }
        }
    }
}

impl Render for AboutView {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let theme = Theme::ui(window);
        let update = self.update_element(theme);
        widgets::page(
            "about",
            theme,
            &self.scroll,
            vec![
                div()
                    .id("heading")
                    .role(Role::Heading)
                    .aria_level(1)
                    .text_size(theme::type_scale::HEADLINE)
                    .font_weight(theme::weight::MEDIUM)
                    .line_height(relative(theme::line_height::TIGHT))
                    .child("Jot")
                    .into_any_element(),
                div()
                    .text_size(theme::type_scale::BODY)
                    .text_color(theme.on_surface_variant)
                    .child(tf(Msg::Version, &[&env!("CARGO_PKG_VERSION")]))
                    .into_any_element(),
                update,
                div()
                    .text_size(theme::type_scale::BODY)
                    .child(tr(Msg::AboutDescription))
                    .into_any_element(),
                div()
                    .text_size(theme::type_scale::LABEL)
                    .text_color(theme.on_surface_variant)
                    .child(tr(Msg::AboutLicense))
                    .into_any_element(),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    struct TestNavigation(Route);

    impl Render for TestNavigation {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let entity = cx.entity().downgrade();
            route_tabs(true, self.0, Theme::light(), move |route, cx| {
                entity
                    .update(cx, |view, cx| {
                        view.0 = route;
                        cx.notify();
                    })
                    .unwrap();
            })
        }
    }

    #[test]
    fn every_workspace_tab_can_be_activated_from_the_keyboard() {
        let mut cx = TestAppContext::single();
        cx.update(widgets::install_keyboard_navigation);
        let window = cx.add_window(|_, _| TestNavigation(Route::Settings));
        let handle = window.into();
        cx.update_window(handle, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();

        for expected in Route::ALL {
            cx.simulate_keystrokes(handle, "tab");
            cx.update_window(handle, |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
            cx.simulate_keystrokes(handle, "enter");
            cx.update_window(handle, |_, window, cx| {
                window.dispatch_event(
                    gpui::PlatformInput::KeyUp(gpui::KeyUpEvent {
                        keystroke: gpui::Keystroke::parse("enter").unwrap(),
                    }),
                    cx,
                );
            })
            .unwrap();
            window
                .update(&mut cx, |view, _, _| assert_eq!(view.0, expected))
                .unwrap();
        }
    }
}
