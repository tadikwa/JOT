//! The recovery window: a dictation Jot could not put into an app.
//!
//! Shown only when insertion was really impossible (the user left the app, the
//! focus is not a text field, or a readable field does not show the text) —
//! never for a field UI Automation merely cannot read. It never takes focus,
//! never re-sends the text, and copies only when asked. History keeps the text
//! either way.

use crate::i18n::{Msg, tr};
use crate::theme::{self, Theme};
use crate::views::widgets::{self, ButtonKind};
use gpui::{
    App, AppContext as _, Bounds, Context, SharedString, WindowBackgroundAppearance, WindowBounds,
    WindowHandle, WindowKind, WindowOptions, div, prelude::*, px, relative, size,
};
use std::time::Duration;

const SIZE: (f32, f32) = (460.0, 220.0);
/// Above the pill, so both can show at once.
const BOTTOM_MARGIN: f32 = crate::pill::HUD_BOTTOM_MARGIN + crate::pill::HUD_SIZE.1;

pub struct RecoveryView {
    text: SharedString,
    copied: bool,
}

impl RecoveryView {
    fn copy(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        jot_core::insertion::copy_only(&self.text);
        self.copied = true;
        cx.notify();
        // "Copied" is seen, then the window gets out of the way.
        let handle = window.window_handle();
        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1200))
                .await;
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        })
        .detach();
    }
}

/// Shows `text`, reusing the open window if there is one.
pub fn show(
    current: Option<WindowHandle<RecoveryView>>,
    text: String,
    cx: &mut App,
) -> Option<WindowHandle<RecoveryView>> {
    if let Some(handle) = current
        && handle
            .update(cx, |view, _, cx| {
                view.text = text.clone().into();
                view.copied = false;
                cx.notify();
            })
            .is_ok()
    {
        return Some(handle);
    }
    let origin = cx
        .primary_display()
        .map(|display| display.bounds().origin)
        .unwrap_or_default();
    let mut shell = Ok(());
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin,
                size: size(px(SIZE.0), px(SIZE.1)),
            })),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            app_id: Some("com.ammaar.jot".into()),
            ..Default::default()
        },
        |window, cx| {
            // Clickable, but never activated: the user's app keeps the focus.
            shell = crate::window_shell::make_popup(window, BOTTOM_MARGIN);
            cx.new(|_| RecoveryView {
                text: text.into(),
                copied: false,
            })
        },
    );
    match (opened, shell) {
        (Ok(handle), Ok(())) => Some(handle),
        (Ok(handle), Err(error)) => {
            tracing::error!(%error, "recovery window could not be made a popup");
            let _ = handle.update(cx, |_, window, _| window.remove_window());
            None
        }
        (Err(error), _) => {
            tracing::error!(%error, "could not open the recovery window");
            None
        }
    }
}

impl Render for RecoveryView {
    fn render(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::current(window.appearance(), crate::window_shell::high_contrast());
        let close = div()
            .id("recovery-close")
            .role(gpui::Role::Button)
            .aria_label(tr(Msg::RecoveryClose))
            .tab_index(0)
            .cursor_pointer()
            .w(px(28.0))
            .h(px(28.0))
            .rounded(theme::radius::SMALL)
            .flex()
            .items_center()
            .justify_center()
            .text_size(theme::type_scale::BODY_LARGE)
            .text_color(theme.on_surface_variant)
            .hover(move |style| style.bg(theme.surface_container))
            .on_click(|_, window, _| window.remove_window())
            .child("×");
        let copy = widgets::button(
            "recovery-copy",
            tr(if self.copied {
                Msg::RecoveryCopied
            } else {
                Msg::RecoveryCopy
            }),
            ButtonKind::Primary,
            !self.copied,
            theme,
            cx.listener(|this, _, window, cx| this.copy(window, cx)),
        );
        div()
            .size_full()
            .p(theme::spacing::XS)
            .font_family(theme::ui_font())
            .child(
                div()
                    .id("recovery")
                    .role(gpui::Role::Dialog)
                    .aria_label(tr(Msg::RecoveryTitle))
                    .size_full()
                    .flex()
                    .flex_col()
                    .gap(theme::spacing::XS)
                    .p(theme::spacing::S)
                    .rounded(theme::radius::MEDIUM)
                    .border_1()
                    .border_color(widgets::with_alpha(theme.outline_variant, 0.6))
                    .bg(theme.surface)
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(theme::type_scale::LABEL)
                                    .font_weight(theme::weight::MEDIUM)
                                    .text_color(theme.on_surface_variant)
                                    .child(tr(Msg::RecoveryTitle)),
                            )
                            .child(close),
                    )
                    .child(
                        div()
                            .id("recovery-text")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .text_size(theme::type_scale::BODY)
                            .line_height(relative(theme::line_height::BODY))
                            .text_color(theme.on_surface)
                            .child(self.text.clone()),
                    )
                    .child(div().flex().justify_end().child(copy)),
            )
    }
}
