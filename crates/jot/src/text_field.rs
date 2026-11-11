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

//! Text fields with IME support and an optional multiline editor.
//!
//! GPUI ships no text input at the pinned revision. Settings and search use
//! single-line fields; History editing opts into wrapped multiline text.
//!
//! Text does NOT arrive through key events. `WM_CHAR` and `WM_IME_COMPOSITION`
//! are routed by the platform into the [`EntityInputHandler`] installed during
//! paint, which is what makes Chinese, Japanese, Korean and Vietnamese input
//! work — the composition is marked, shown underlined, and committed as one
//! edit. Key events handle caret movement, deletion, selection, clipboard
//! commands, and multiline Enter because Windows filters control characters.
//!
//! This is not a full editor: there is no word-wise motion or double-click
//! selection. Multiline IME candidates follow the shaped caret.

use crate::i18n::{Msg, tr};
use crate::theme::{self, Theme};
use gpui::{
    Bounds, ClipboardItem, Context, ElementInputHandler, EntityInputHandler, FocusHandle,
    Focusable, Font, FontFeatures, FontStyle, FontWeight, KeyDownEvent, MouseDownEvent, Pixels,
    Point, Role, SharedString, TextRun, UTF16Selection, Window, WrappedLine, canvas, div, fill,
    point, prelude::*, px, size,
};
use std::ops::Range;

/// Maximum number of undo snapshots retained by a field.
const UNDO_DEPTH: usize = 64;

/// A restorable moment: the text and where the caret was in it.
#[derive(Clone, PartialEq, Eq)]
struct Snapshot {
    content: String,
    selection: Range<usize>,
}

/// Emitted on Enter in a single-line field, or Ctrl+Enter in a multiline field.
pub struct Submit;

/// Emitted by user edits; loading a value with `set_content` stays silent.
pub struct Changed;

const EDITOR_LINE_HEIGHT: Pixels = px(22.0);

struct EditorLine {
    start: usize,
    top: Pixels,
    shaped: WrappedLine,
}

struct EditorLayout {
    lines: Vec<EditorLine>,
    height: Pixels,
}

impl EditorLayout {
    fn position(&self, index: usize, wrap_end: bool) -> Point<Pixels> {
        for line in &self.lines {
            if index <= line.start + line.shaped.len() {
                if !wrap_end {
                    for (row, boundary) in line.shaped.wrap_boundaries().iter().enumerate() {
                        let glyph = &line.shaped.runs()[boundary.run_ix].glyphs[boundary.glyph_ix];
                        if line.start + glyph.index == index {
                            return point(px(0.0), line.top + EDITOR_LINE_HEIGHT * (row + 1));
                        }
                    }
                }
                let position = line
                    .shaped
                    .position_for_index(index.saturating_sub(line.start), EDITOR_LINE_HEIGHT)
                    .unwrap_or_default();
                return point(position.x, line.top + position.y);
            }
        }
        point(px(0.0), self.height - EDITOR_LINE_HEIGHT)
    }

    fn index_at(&self, position: Point<Pixels>) -> usize {
        let Some(line) = self
            .lines
            .iter()
            .rev()
            .find(|line| position.y >= line.top)
            .or_else(|| self.lines.first())
        else {
            return 0;
        };
        let y = (position.y - line.top).clamp(
            px(0.0),
            line.shaped.size(EDITOR_LINE_HEIGHT).height - px(1.0),
        );
        line.start
            + line
                .shaped
                .closest_index_for_position(point(position.x, y), EDITOR_LINE_HEIGHT)
                .unwrap_or_else(|index| index)
    }
}

pub enum FieldLabel {
    Fixed(&'static str),
    Translated(Msg),
}

impl FieldLabel {
    fn text(&self) -> &'static str {
        match *self {
            Self::Fixed(text) => text,
            Self::Translated(message) => tr(message),
        }
    }
}

impl From<&'static str> for FieldLabel {
    fn from(text: &'static str) -> Self {
        Self::Fixed(text)
    }
}

impl From<Msg> for FieldLabel {
    fn from(message: Msg) -> Self {
        Self::Translated(message)
    }
}

pub struct TextField {
    /// Stable and unique within its window. Several fields share a pane, and a
    /// duplicate id collapses two of them into one accessibility node.
    id: SharedString,
    content: String,
    /// Byte range. `start == end` is a caret; both ends always sit on a
    /// character boundary.
    selection: Range<usize>,
    /// Byte range of the in-progress IME composition, if any.
    marked: Option<Range<usize>>,
    placeholder: FieldLabel,
    /// Renders as bullets and never reaches the clipboard through this field.
    pub masked: bool,
    pub focus_handle: FocusHandle,
    /// Captured at paint so the IME candidate window has somewhere to go, and
    /// so a click can be turned into a caret position.
    last_bounds: Option<Bounds<Pixels>>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// True while a run of single characters is being typed, so that a word
    /// undoes as a word instead of one letter per press.
    coalescing: bool,
    multiline: bool,
    layout: Option<EditorLayout>,
    scroll_offset: Pixels,
    reveal_caret: bool,
    edit_revision: u64,
    wrap_end: bool,
    /// A single click is held: moving the mouse extends the selection.
    dragging: bool,
}

impl gpui::EventEmitter<Submit> for TextField {}
impl gpui::EventEmitter<Changed> for TextField {}

impl Focusable for TextField {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextField {
    pub fn new(
        id: impl Into<SharedString>,
        placeholder: impl Into<FieldLabel>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            id: id.into(),
            content: String::new(),
            selection: 0..0,
            marked: None,
            placeholder: placeholder.into(),
            masked: false,
            focus_handle: cx.focus_handle().tab_index(0).tab_stop(true),
            last_bounds: None,
            undo: Vec::new(),
            redo: Vec::new(),
            coalescing: false,
            multiline: false,
            layout: None,
            scroll_offset: px(0.0),
            reveal_caret: true,
            edit_revision: 0,
            wrap_end: false,
            dragging: false,
        }
    }

    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn set_content(&mut self, text: impl Into<String>) {
        self.content = text.into();
        self.wrap_end = false;
        self.layout = None;
        self.scroll_offset = px(0.0);
        self.reveal_caret = true;
        self.selection = self.content.len()..self.content.len();
        self.marked = None;
        // Filling a field from settings is not an edit the user made, so there
        // is nothing for them to undo back past.
        self.undo.clear();
        self.redo.clear();
        self.coalescing = false;
    }

    pub fn take_content(&mut self) -> String {
        self.selection = 0..0;
        self.marked = None;
        std::mem::take(&mut self.content)
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    // ----- UTF-16 conversion
    //
    // The platform speaks UTF-16 offsets and the buffer is UTF-8, so every
    // handler method converts at its boundary. Getting this wrong corrupts
    // exactly the text that needed an IME in the first place.

    fn byte_to_utf16(&self, byte: usize) -> usize {
        self.content[..byte.min(self.content.len())]
            .chars()
            .map(char::len_utf16)
            .sum()
    }

    fn utf16_to_byte(&self, offset: usize) -> usize {
        let mut seen = 0;
        for (byte, character) in self.content.char_indices() {
            if seen >= offset {
                return byte;
            }
            seen += character.len_utf16();
        }
        self.content.len()
    }

    fn utf16_range(&self, range: Range<usize>) -> Range<usize> {
        self.byte_to_utf16(range.start)..self.byte_to_utf16(range.end)
    }

    fn byte_range(&self, range: Range<usize>) -> Range<usize> {
        let start = self.utf16_to_byte(range.start);
        let end = self.utf16_to_byte(range.end);
        start.min(end)..start.max(end)
    }

    // ----- Editing

    /// Records the current text so the next edit can be undone.
    ///
    /// `coalesce` groups a run of single typed characters into one undo step:
    /// undoing a word one letter at a time is technically correct and nobody
    /// wants it. Anything else — a paste, a deletion, a replaced selection —
    /// breaks the run and becomes its own step.
    fn checkpoint(&mut self, coalesce: bool) {
        if coalesce && self.coalescing {
            return;
        }
        let snapshot = Snapshot {
            content: self.content.clone(),
            selection: self.selection.clone(),
        };
        if self.undo.last() != Some(&snapshot) {
            self.undo.push(snapshot);
            if self.undo.len() > UNDO_DEPTH {
                self.undo.remove(0);
            }
        }
        // A fresh edit starts a new branch of history, so anything undone past
        // this point can no longer be redone.
        self.redo.clear();
        self.coalescing = coalesce;
    }

    fn undo(&mut self) {
        let Some(previous) = self.undo.pop() else {
            return;
        };
        self.redo.push(self.snapshot());
        self.restore(previous);
    }

    fn redo(&mut self) {
        let Some(next) = self.redo.pop() else {
            return;
        };
        self.undo.push(self.snapshot());
        self.restore(next);
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            content: self.content.clone(),
            selection: self.selection.clone(),
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        if self.content != snapshot.content {
            self.edit_revision += 1;
        }
        self.content = snapshot.content;
        self.wrap_end = false;
        let end = self.content.len();
        self.selection = snapshot.selection.start.min(end)..snapshot.selection.end.min(end);
        // An in-flight composition describes text that no longer exists.
        self.marked = None;
        self.coalescing = false;
    }

    /// Replaces `range` with `text` and leaves the caret after it.
    ///
    /// Single-line fields replace newlines with spaces; multiline fields
    /// preserve line breaks and normalize CRLF/CR to LF.
    fn replace(&mut self, range: Range<usize>, text: &str) {
        let sanitized = self.sanitize(text);
        let range = range.start.min(range.end).min(self.content.len())
            ..range.start.max(range.end).min(self.content.len());
        if self.content[range.clone()] != sanitized {
            self.edit_revision += 1;
        }
        self.content.replace_range(range.clone(), &sanitized);
        let caret = range.start + sanitized.len();
        self.selection = caret..caret;
        self.marked = None;
        self.wrap_end = false;
    }

    fn delete_backwards(&mut self) {
        self.checkpoint(false);
        if self.selection.start != self.selection.end {
            let range = self.selection_range();
            self.replace(range, "");
            return;
        }
        let Some(previous) = self.content[..self.selection.start].chars().next_back() else {
            return;
        };
        let range = (self.selection.start - previous.len_utf8())..self.selection.start;
        self.replace(range, "");
    }

    fn delete_forwards(&mut self) {
        self.checkpoint(false);
        if self.selection.start != self.selection.end {
            let range = self.selection_range();
            self.replace(range, "");
            return;
        }
        let Some(next) = self.content[self.selection.end..].chars().next() else {
            return;
        };
        let range = self.selection.end..(self.selection.end + next.len_utf8());
        self.replace(range, "");
    }

    /// Moves the caret, or extends the selection when `extend` is set.
    fn move_caret(&mut self, to: usize, extend: bool) {
        if extend {
            self.selection = self.selection.start..to;
        } else {
            self.selection = to..to;
        }
    }

    /// One click places the caret (Shift extends), two select a word, three a
    /// paragraph, as in every Windows editor.
    fn click_at(&mut self, caret: usize, clicks: usize, extend: bool) {
        match clicks {
            2 => self.selection = self.word_at(caret),
            n if n >= 3 => self.selection = self.paragraph_at(caret),
            _ => self.move_caret(caret, extend),
        }
        self.dragging = clicks <= 1;
        self.coalescing = false;
    }

    fn drag_to(&mut self, at: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        if !self.dragging {
            return;
        }
        if let Some(caret) = self.caret_from_click(at, window)
            && caret != self.selection.end
        {
            self.selection = self.selection.start..caret;
            // Past the top or bottom edge: the view follows the caret.
            self.reveal_caret = true;
            cx.notify();
        }
    }

    /// The word under `index`, or the run of spaces or punctuation there.
    fn word_at(&self, index: usize) -> Range<usize> {
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let index = index.min(self.content.len());
        let Some(here) = self.content[index..]
            .chars()
            .next()
            .or_else(|| self.content[..index].chars().next_back())
        else {
            return index..index;
        };
        let kind = |c: char| (is_word(c), c.is_whitespace());
        let target = kind(here);
        let start = self.content[..index]
            .char_indices()
            .rev()
            .take_while(|(_, c)| kind(*c) == target && *c != '\n')
            .last()
            .map_or(index, |(offset, _)| offset);
        let end = self.content[index..]
            .char_indices()
            .take_while(|(_, c)| kind(*c) == target && *c != '\n')
            .last()
            .map_or(index, |(offset, c)| index + offset + c.len_utf8());
        start..end
    }

    /// The line between two newlines around `index`, without them.
    fn paragraph_at(&self, index: usize) -> Range<usize> {
        let index = index.min(self.content.len());
        let start = self.content[..index].rfind('\n').map_or(0, |at| at + 1);
        let end = self.content[index..]
            .find('\n')
            .map_or(self.content.len(), |at| index + at);
        start..end
    }

    fn selection_range(&self) -> Range<usize> {
        self.selection.start.min(self.selection.end)..self.selection.start.max(self.selection.end)
    }

    fn sanitize(&self, text: &str) -> String {
        if self.multiline && !self.masked {
            text.replace("\r\n", "\n").replace('\r', "\n")
        } else {
            text.replace(['\n', '\r'], " ")
        }
    }

    fn changed_since(&mut self, revision: u64, cx: &mut Context<Self>) {
        if revision != self.edit_revision {
            self.reveal_caret = true;
            cx.emit(Changed);
        }
    }

    fn previous_boundary(&self) -> usize {
        let from = self.selection.end;
        self.content[..from]
            .chars()
            .next_back()
            .map_or(from, |character| from - character.len_utf8())
    }

    fn next_boundary(&self) -> usize {
        let from = self.selection.end;
        self.content[from..]
            .chars()
            .next()
            .map_or(from, |character| from + character.len_utf8())
    }

    /// Turns a click into a caret position.
    ///
    /// The text is re-shaped with the same font and size it was painted with,
    /// so the boundary this lands on is the one under the pointer rather than a
    /// guess from an average character width. Masked fields shape their bullets
    /// instead, and the answer is mapped back through the character count —
    /// a bullet is three bytes and the key behind it usually is not.
    fn caret_from_click(&self, at: Point<Pixels>, window: &Window) -> Option<usize> {
        let bounds = self.last_bounds?;
        if self.multiline && !self.masked {
            let local = at - bounds.origin - point(theme::spacing::S, theme::spacing::S)
                + point(px(0.0), self.scroll_offset);
            return self
                .layout
                .as_ref()
                .map(|layout| layout.index_at(local).min(self.content.len()));
        }
        let visible = self.rendered(0..self.content.len());
        if visible.is_empty() {
            return Some(0);
        }
        let font = Font {
            family: if self.masked {
                theme::mono_font()
            } else {
                theme::ui_font()
            },
            features: FontFeatures::default(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        };
        let size = theme::type_scale::BODY;
        let run = TextRun {
            len: visible.len(),
            font,
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(
            SharedString::from(visible.clone()),
            size,
            &[run],
            None,
        );

        // `last_bounds` is the field's box; the text starts one horizontal
        // padding in.
        let x = at.x - bounds.origin.x - theme::spacing::S;
        let index = line.closest_index_for_x(x.max(px(0.0)));
        let characters = visible[..index.min(visible.len())].chars().count();
        Some(
            self.content
                .char_indices()
                .nth(characters)
                .map_or(self.content.len(), |(offset, _)| offset),
        )
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let revision = self.edit_revision;
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;

        if modifiers.control || modifiers.platform {
            match keystroke.key.as_str() {
                "enter" if self.multiline && !self.masked => {
                    cx.stop_propagation();
                    _window.prevent_default();
                    cx.emit(Submit);
                    return;
                }
                "home" => self.move_caret(0, modifiers.shift),
                "end" => self.move_caret(self.content.len(), modifiers.shift),
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.checkpoint(false);
                        let range = self.selection_range();
                        self.replace(range, &jot_core::win32::from_crlf(&text));
                    }
                }
                "a" => self.selection = 0..self.content.len(),
                "z" if modifiers.shift => self.redo(),
                "z" => self.undo(),
                // Windows' other redo binding. Supporting one and not the other
                // reads as a missing feature to whoever learned the other one.
                "y" => self.redo(),
                // Never from a masked field: the API key does not leave here.
                "c" if self.selection.start != self.selection.end && !self.masked => {
                    let selected = self.content[self.selection_range()].to_string();
                    set_clipboard_text(selected, cx);
                }
                "x" if self.selection.start != self.selection.end && !self.masked => {
                    let range = self.selection_range();
                    set_clipboard_text(self.content[range.clone()].to_string(), cx);
                    self.checkpoint(false);
                    self.replace(range, "");
                }
                _ => return,
            }
            self.changed_since(revision, cx);
            self.reveal_caret = true;
            self.coalescing = false;
            cx.notify();
            return;
        }

        match keystroke.key.as_str() {
            "backspace" => self.delete_backwards(),
            "delete" => self.delete_forwards(),
            "left" => {
                let to = if !modifiers.shift && self.selection.start != self.selection.end {
                    self.selection_range().start
                } else {
                    self.previous_boundary()
                };
                self.wrap_end = false;
                self.move_caret(to, modifiers.shift);
            }
            "right" => {
                let to = if !modifiers.shift && self.selection.start != self.selection.end {
                    self.selection_range().end
                } else {
                    self.next_boundary()
                };
                self.wrap_end = false;
                self.move_caret(to, modifiers.shift);
            }
            "up" | "down" | "home" | "end" if self.multiline && !self.masked => {
                if let Some(layout) = &self.layout {
                    let mut position = layout.position(self.selection.end, self.wrap_end);
                    match keystroke.key.as_str() {
                        "up" => position.y -= EDITOR_LINE_HEIGHT,
                        "down" => position.y += EDITOR_LINE_HEIGHT,
                        "home" => position.x = px(0.0),
                        "end" => position.x = Pixels::MAX,
                        _ => unreachable!(),
                    }
                    let to = layout.index_at(position);
                    self.wrap_end = layout.position(to, false).y > position.y;
                    self.move_caret(to, modifiers.shift);
                }
            }
            "home" => self.move_caret(0, modifiers.shift),
            "end" => self.move_caret(self.content.len(), modifiers.shift),
            "enter" if self.multiline && !self.masked => {
                // Enter commits an IME candidate while a composition is active.
                if self.marked.is_some() {
                    return;
                }
                self.checkpoint(false);
                self.replace(self.selection_range(), "\n");
                cx.stop_propagation();
                _window.prevent_default();
            }
            "enter" => {
                cx.emit(Submit);
                return;
            }
            // Anything else that produces text arrives through the input
            // handler, not here. Inserting `key_char` as well would type every
            // character twice and break composition outright.
            _ => return,
        }
        self.changed_since(revision, cx);
        self.reveal_caret = true;
        if !matches!(keystroke.key.as_str(), "backspace" | "delete") {
            self.coalescing = false;
        }
        cx.notify();
    }

    fn editor_layout(&self, width: Pixels, theme: Theme, window: &Window) -> EditorLayout {
        let display = if self.content.is_empty() {
            self.placeholder.text()
        } else {
            &self.content
        };
        let selection = self.selection_range();
        let mut layout = EditorLayout {
            lines: Vec::new(),
            height: px(0.0),
        };
        let mut start = 0;
        for paragraph in display.split('\n') {
            let end = start + paragraph.len();
            let mut boundaries = vec![start, end];
            if !self.content.is_empty() {
                boundaries.extend([
                    selection.start.clamp(start, end),
                    selection.end.clamp(start, end),
                ]);
                if let Some(marked) = &self.marked {
                    boundaries
                        .extend([marked.start.clamp(start, end), marked.end.clamp(start, end)]);
                }
            }
            boundaries.sort_unstable();
            boundaries.dedup();
            let runs: Vec<_> = boundaries
                .windows(2)
                .map(|range| TextRun {
                    len: range[1] - range[0],
                    font: Font {
                        family: theme::ui_font(),
                        features: FontFeatures::default(),
                        fallbacks: None,
                        weight: FontWeight::NORMAL,
                        style: FontStyle::Normal,
                    },
                    color: if self.content.is_empty() {
                        theme.on_surface_variant.into()
                    } else {
                        theme.on_surface.into()
                    },
                    background_color: (!self.content.is_empty()
                        && range[0] >= selection.start
                        && range[1] <= selection.end)
                        .then(|| crate::views::widgets::with_alpha(theme.primary, 0.35).into()),
                    underline: self
                        .marked
                        .as_ref()
                        .is_some_and(|marked| range[0] >= marked.start && range[1] <= marked.end)
                        .then_some(gpui::UnderlineStyle {
                            thickness: px(1.0),
                            color: None,
                            wavy: false,
                        }),
                    strikethrough: None,
                })
                .collect();
            match window.text_system().shape_text(
                paragraph.to_owned().into(),
                theme::type_scale::BODY,
                &runs,
                Some(width.max(px(1.0))),
                None,
            ) {
                Ok(shaped) => {
                    for shaped in shaped {
                        let height = shaped.size(EDITOR_LINE_HEIGHT).height;
                        layout.lines.push(EditorLine {
                            start,
                            top: layout.height,
                            shaped,
                        });
                        layout.height += height;
                    }
                }
                Err(error) => tracing::error!(%error, "Text field layout failed"),
            }
            start = end + 1;
        }
        layout.height = layout.height.max(EDITOR_LINE_HEIGHT);
        layout
    }

    fn render_multiline(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::ui(window);
        let focused = self.focus_handle.is_focused(window);
        let entity = cx.entity();
        let focus_handle = self.focus_handle.clone();
        div().id(self.id.clone()).role(Role::TextInput).aria_label(self.placeholder.text())
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, window, cx| {
                this.focus_handle.focus(window, cx);
                if let Some(bounds) = this.last_bounds
                    && event.position.x >= bounds.right() - px(12.0) {
                        let viewport = bounds.size.height - theme::spacing::S * 2.0;
                        let height = this.layout.as_ref().map_or(viewport, |layout| layout.height);
                        let progress = ((event.position.y - bounds.origin.y) / bounds.size.height).clamp(0.0, 1.0);
                        this.scroll_offset = (height - viewport).max(px(0.0)) * progress;
                        this.reveal_caret = false;
                        cx.notify();
                        return;
                }
                if let Some(caret) = this.caret_from_click(event.position, window) {
                    this.wrap_end = this.layout.as_ref().zip(this.last_bounds).is_some_and(|(layout, bounds)| {
                        layout.position(caret, false).y > event.position.y - bounds.origin.y - theme::spacing::S + this.scroll_offset
                    });
                    this.click_at(caret, event.click_count, event.modifiers.shift);
                    this.reveal_caret = true;
                }
                cx.notify();
            }))
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                if let (Some(bounds), Some(layout)) = (this.last_bounds, &this.layout) {
                    let viewport = bounds.size.height - theme::spacing::S * 2.0;
                    this.scroll_offset = (this.scroll_offset - event.delta.pixel_delta(EDITOR_LINE_HEIGHT).y)
                        .clamp(px(0.0), (layout.height - viewport).max(px(0.0)));
                    this.reveal_caret = false;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .w_full().h(px(220.0)).flex_none().rounded(theme::radius::SMALL)
            .border_1().border_color(if focused { theme.primary } else { theme.outline_variant })
            .bg(theme.surface).cursor_text().overflow_hidden()
            .child(canvas(move |bounds, _, _| bounds, move |bounds, _, window, cx| {
                entity.update(cx, |field, cx| {
                    field.last_bounds = Some(bounds);
                    let viewport = Bounds::new(bounds.origin + point(theme::spacing::S, theme::spacing::S),
                        size((bounds.size.width - theme::spacing::S * 2.0 - px(12.0)).max(px(1.0)),
                             (bounds.size.height - theme::spacing::S * 2.0).max(EDITOR_LINE_HEIGHT)));
                    let layout = field.editor_layout(viewport.size.width, theme, window);
                    let caret = if field.content.is_empty() { point(px(0.0), px(0.0)) } else { layout.position(field.selection.end, field.wrap_end) };
                    if field.reveal_caret {
                        if caret.y < field.scroll_offset { field.scroll_offset = caret.y; }
                        if caret.y + EDITOR_LINE_HEIGHT > field.scroll_offset + viewport.size.height {
                            field.scroll_offset = caret.y + EDITOR_LINE_HEIGHT - viewport.size.height;
                        }
                        field.reveal_caret = false;
                    }
                    let overflow = (layout.height - viewport.size.height).max(px(0.0));
                    field.scroll_offset = field.scroll_offset.clamp(px(0.0), overflow);
                    let origin = viewport.origin - point(px(0.0), field.scroll_offset);
                    window.with_content_mask(Some(gpui::ContentMask { bounds: viewport }), |window| {
                        for line in &layout.lines {
                            let at = origin + point(px(0.0), line.top);
                            let height = line.shaped.size(EDITOR_LINE_HEIGHT).height;
                            if at.y + height < viewport.origin.y || at.y > viewport.bottom() { continue; }
                            if let Err(error) = line.shaped.paint_background(at, EDITOR_LINE_HEIGHT, gpui::TextAlign::Left, None, window, cx) {
                                tracing::error!(%error, "Text field selection paint failed");
                            }
                            if let Err(error) = line.shaped.paint(at, EDITOR_LINE_HEIGHT, gpui::TextAlign::Left, None, window, cx) {
                                tracing::error!(%error, "Text field paint failed");
                            }
                        }
                        if focused {
                            window.paint_quad(fill(Bounds::new(origin + caret, size(px(1.5), EDITOR_LINE_HEIGHT)), theme.primary));
                        }
                    });
                    if overflow > px(0.0) {
                        let height = (viewport.size.height * (viewport.size.height / layout.height)).max(px(20.0)).min(viewport.size.height);
                        let top = (viewport.size.height - height) * (field.scroll_offset / overflow);
                        window.paint_quad(fill(Bounds::new(point(bounds.right() - px(8.0), viewport.origin.y + top), size(px(4.0), height)),
                            crate::views::widgets::with_alpha(theme.on_surface_variant, 0.6)).corner_radii(px(2.0)));
                    }
                    field.layout = Some(layout);
                });
                window.handle_input(&focus_handle, ElementInputHandler::new(bounds, entity.clone()), cx);
                // Window-wide, so a drag keeps selecting outside the box.
                window.on_mouse_event({
                    let entity = entity.clone();
                    move |event: &gpui::MouseMoveEvent, phase, window, cx| {
                        if phase == gpui::DispatchPhase::Bubble && event.pressed_button == Some(gpui::MouseButton::Left) {
                            entity.update(cx, |field, cx| field.drag_to(event.position, window, cx));
                        }
                    }
                });
                window.on_mouse_event({
                    let entity = entity.clone();
                    move |_: &gpui::MouseUpEvent, phase, _, cx| {
                        if phase == gpui::DispatchPhase::Bubble {
                            entity.update(cx, |field, _| field.dragging = false);
                        }
                    }
                });
            }).size_full()).into_any_element()
    }

    fn rendered(&self, range: Range<usize>) -> String {
        if self.masked {
            "•".repeat(self.content[range].chars().count())
        } else {
            self.content[range].to_string()
        }
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.byte_range(range_utf16);
        *adjusted_range = Some(self.utf16_range(range.clone()));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.utf16_range(self.selection_range()),
            reversed: self.selection.start > self.selection.end,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let marked = self.marked.clone()?;
        Some(self.byte_to_utf16(marked.start)..self.byte_to_utf16(marked.end))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // No range means "whatever is marked, else the selection" — that is how
        // a committed composition replaces the text it was composing.
        let range = match range_utf16 {
            Some(range) => self.byte_range(range),
            None => self
                .marked
                .clone()
                .unwrap_or_else(|| self.selection_range()),
        };
        // One character landing on a caret is typing and coalesces; a longer
        // string is a committed composition or a platform paste, and is not.
        let revision = self.edit_revision;
        let typing = self.marked.is_none()
            && text.chars().count() == 1
            && text != "\n"
            && range.start == range.end;
        if self.marked.is_none() {
            self.checkpoint(typing);
        }
        self.replace(range, text);
        self.changed_since(revision, cx);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = match range_utf16 {
            Some(range) => self.byte_range(range),
            None => self
                .marked
                .clone()
                .unwrap_or_else(|| self.selection_range()),
        };
        // Only the start of a composition is worth remembering: every keystroke
        // inside one rewrites the same marked run, and each would otherwise
        // become its own undo step.
        if self.marked.is_none() {
            self.checkpoint(false);
        }
        let revision = self.edit_revision;
        let sanitized = self.sanitize(new_text);
        let range = range.start.min(range.end).min(self.content.len())
            ..range.start.max(range.end).min(self.content.len());
        if self.content[range.clone()] != sanitized {
            self.edit_revision += 1;
        }
        self.content.replace_range(range.clone(), &sanitized);

        let marked = range.start..(range.start + sanitized.len());
        // The caret sits where the IME says it is inside the composition, or at
        // its end when it does not say.
        self.selection = match new_selected_range {
            Some(selected) => {
                let start = self.byte_to_utf16(marked.start);
                let length = sanitized.encode_utf16().count();
                self.byte_range(
                    (start + selected.start.min(length))..(start + selected.end.min(length)),
                )
            }
            None => marked.end..marked.end,
        };
        self.marked = (!sanitized.is_empty()).then_some(marked);
        self.changed_since(revision, cx);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // Multiline fields use the same shaped coordinates as hit testing.
        if self.multiline && !self.masked {
            let bounds = self.last_bounds?;
            let position = self
                .layout
                .as_ref()?
                .position(self.utf16_to_byte(range_utf16.start), self.wrap_end);
            return Some(Bounds::new(
                bounds.origin + point(theme::spacing::S, theme::spacing::S) + position
                    - point(px(0.0), self.scroll_offset),
                size(px(1.5), EDITOR_LINE_HEIGHT),
            ));
        }
        Some(self.last_bounds.unwrap_or(element_bounds))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.caret_from_click(point, window)
            .map(|index| self.byte_to_utf16(index))
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.byte_to_utf16(self.content.len()))
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.multiline && !self.masked {
            return self.render_multiline(window, cx);
        }
        let theme = Theme::ui(window);
        let focused = self.focus_handle.is_focused(window);
        let empty = self.content.is_empty();

        // Split the text at every boundary that changes how it is drawn, so the
        // caret, the selection and the IME composition can all be shown at once.
        let mut boundaries = vec![
            0,
            self.content.len(),
            self.selection.start,
            self.selection.end,
        ];
        if let Some(marked) = &self.marked {
            boundaries.push(marked.start);
            boundaries.push(marked.end);
        }
        boundaries.retain(|at| *at <= self.content.len() && self.content.is_char_boundary(*at));
        boundaries.sort_unstable();
        boundaries.dedup();

        let caret_at = self.selection.end;
        let has_selection = self.selection.start != self.selection.end;
        let mut runs = div().flex().items_center();
        for pair in boundaries.windows(2) {
            let (start, end) = (pair[0], pair[1]);
            if focused && !has_selection && start == caret_at {
                runs = runs.child(caret(theme));
            }
            let selected = has_selection
                && start >= self.selection_range().start
                && end <= self.selection_range().end;
            let composing = self
                .marked
                .as_ref()
                .is_some_and(|marked| start >= marked.start && end <= marked.end);
            let mut run = div().child(self.rendered(start..end));
            if selected {
                run = run.bg(crate::views::widgets::with_alpha(theme.primary, 0.35));
            }
            if composing {
                // Underlined, because an IME composition is provisional text
                // and must not look like something already committed.
                run = run.text_decoration_1().text_decoration_solid();
            }
            runs = runs.child(run);
        }
        if focused && !has_selection && caret_at == self.content.len() {
            runs = runs.child(caret(theme));
        }

        let entity = cx.entity();
        let focus_handle = self.focus_handle.clone();

        div()
            .id(self.id.clone())
            .role(Role::TextInput)
            .aria_label(self.placeholder.text())
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.focus_handle.focus(window, cx);
                    if let Some(caret) = this.caret_from_click(event.position, window) {
                        this.click_at(caret, event.click_count, event.modifiers.shift);
                        // Dragging is only tracked in the multi-line editor.
                        this.dragging = false;
                    }
                    cx.notify();
                }),
            )
            .flex()
            .items_center()
            .w_full()
            .h(px(34.0))
            .px(theme::spacing::S)
            .rounded(theme::radius::SMALL)
            .border_1()
            .border_color(if focused {
                theme.primary
            } else {
                theme.outline_variant
            })
            .bg(theme.surface)
            .cursor_text()
            .font_family(if self.masked {
                theme::mono_font()
            } else {
                theme::ui_font()
            })
            .text_size(theme::type_scale::BODY)
            // Installs the platform input handler for the next frame. This is
            // the only place text can arrive from, so it has to happen on every
            // paint while focused.
            .relative()
            .child(
                canvas(
                    move |bounds, _, _| bounds,
                    move |bounds, _, window, cx| {
                        entity.update(cx, |field, _| field.last_bounds = Some(bounds));
                        window.handle_input(
                            &focus_handle,
                            ElementInputHandler::new(bounds, entity.clone()),
                            cx,
                        );
                    },
                )
                // Absolute so measuring the field costs it no layout space: an
                // in-flow canvas would take width from the text beside it.
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(if empty {
                div()
                    .flex()
                    .items_center()
                    .flex_1()
                    .overflow_hidden()
                    // A focused empty field drew no caret at all, because the
                    // placeholder branch never reached the run loop. Clicking a
                    // field and seeing nothing change reads as a dead control.
                    .when(focused, |row| row.child(caret(theme)))
                    .child(
                        div()
                            .text_color(theme.on_surface_variant)
                            .child(self.placeholder.text()),
                    )
                    .into_any_element()
            } else {
                runs.flex_1()
                    .overflow_hidden()
                    .text_color(theme.on_surface)
                    .into_any_element()
            })
            .into_any_element()
    }
}

/// A caret that does not blink: it still says "typing lands here", and a blink
/// is an animation nobody asked for.
fn caret(theme: Theme) -> impl IntoElement {
    div().w(px(1.5)).h(px(18.0)).bg(theme.primary)
}

pub fn set_clipboard_text(text: impl Into<String>, cx: &gpui::App) {
    cx.write_to_clipboard(ClipboardItem::new_string(jot_core::win32::to_crlf(
        &text.into(),
    )));
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn field(cx: &mut TestAppContext) -> gpui::Entity<TextField> {
        cx.new(|cx| TextField::new("test-field", "placeholder", cx))
    }

    /// What typing a word actually looks like: one character at a time through
    /// the platform input handler.
    fn type_text(
        field: &mut TextField,
        text: &str,
        window: &mut Window,
        cx: &mut Context<TextField>,
    ) {
        for character in text.chars() {
            field.replace_text_in_range(None, &character.to_string(), window, cx);
        }
    }

    #[gpui::test]
    fn a_typed_word_undoes_as_a_word(cx: &mut TestAppContext) {
        let field = field(cx);
        let window = cx.add_window(|_, _| gpui::Empty);
        cx.update_window(window.into(), |_, window, cx| {
            field.update(cx, |field, cx| {
                type_text(field, "hello", window, cx);
                assert_eq!(field.content(), "hello");
                field.undo();
                assert_eq!(field.content(), "", "five keystrokes are one edit");
                field.redo();
                assert_eq!(field.content(), "hello");
            });
        })
        .unwrap();
    }

    #[gpui::test]
    fn a_deletion_is_its_own_step(cx: &mut TestAppContext) {
        let field = field(cx);
        let window = cx.add_window(|_, _| gpui::Empty);
        cx.update_window(window.into(), |_, window, cx| {
            field.update(cx, |field, cx| {
                type_text(field, "abc", window, cx);
                field.delete_backwards();
                assert_eq!(field.content(), "ab");
                field.undo();
                assert_eq!(field.content(), "abc", "the delete undoes on its own");
                field.undo();
                assert_eq!(field.content(), "", "and the typing behind it");
            });
        })
        .unwrap();
    }

    #[gpui::test]
    fn editing_after_an_undo_drops_the_redo(cx: &mut TestAppContext) {
        let field = field(cx);
        let window = cx.add_window(|_, _| gpui::Empty);
        cx.update_window(window.into(), |_, window, cx| {
            field.update(cx, |field, cx| {
                type_text(field, "one", window, cx);
                field.undo();
                type_text(field, "two", window, cx);
                field.redo();
                assert_eq!(field.content(), "two", "the abandoned branch is gone");
            });
        })
        .unwrap();
    }

    #[gpui::test]
    fn undo_on_an_untouched_field_does_nothing(cx: &mut TestAppContext) {
        let field = field(cx);
        field.update(cx, |field, _| {
            field.undo();
            field.redo();
            assert_eq!(field.content(), "");
        });
    }

    #[gpui::test]
    fn loading_a_value_leaves_nothing_to_undo_back_past(cx: &mut TestAppContext) {
        let field = field(cx);
        let window = cx.add_window(|_, _| gpui::Empty);
        cx.update_window(window.into(), |_, window, cx| {
            field.update(cx, |field, cx| {
                // Settings fills the field, then the user edits it.
                field.set_content("gemini-3.5-flash-lite");
                type_text(field, "X", window, cx);
                field.undo();
                assert_eq!(field.content(), "gemini-3.5-flash-lite");
                field.undo();
                assert_eq!(
                    field.content(),
                    "gemini-3.5-flash-lite",
                    "undo must not empty a field the user never emptied"
                );
            });
        })
        .unwrap();
    }

    #[gpui::test]
    fn history_is_bounded(cx: &mut TestAppContext) {
        let field = field(cx);
        field.update(cx, |field, _| {
            for index in 0..(UNDO_DEPTH * 2) {
                field.checkpoint(false);
                field.content.push_str(&index.to_string());
            }
            assert_eq!(field.undo.len(), UNDO_DEPTH);
        });
    }

    #[gpui::test]
    fn editing_keeps_the_caret_on_character_boundaries(cx: &mut TestAppContext) {
        let field = field(cx);
        field.update(cx, |field, _| {
            field.replace(0..0, "naïve café");
            assert_eq!(field.content(), "naïve café");

            field.delete_backwards();
            assert_eq!(field.content(), "naïve caf");

            let to = field.previous_boundary();
            field.move_caret(to, false);
            let to = field.previous_boundary();
            field.move_caret(to, false);
            field.replace(field.selection.clone(), "X");
            assert_eq!(field.content(), "naïve cXaf");
        });
    }

    #[gpui::test]
    fn utf16_offsets_round_trip_through_astral_characters(cx: &mut TestAppContext) {
        let field = field(cx);
        field.update(cx, |field, _| {
            // An emoji is two UTF-16 units and four UTF-8 bytes; the platform
            // counts the former and the buffer stores the latter.
            field.set_content("a😀b");
            assert_eq!(field.byte_to_utf16(field.content.len()), 4);
            assert_eq!(field.utf16_to_byte(0), 0);
            assert_eq!(field.utf16_to_byte(1), 1);
            assert_eq!(field.utf16_to_byte(3), 5);
            assert_eq!(field.utf16_to_byte(4), 6);
            // Past the end clamps rather than panicking.
            assert_eq!(field.utf16_to_byte(99), field.content.len());
        });
    }

    #[gpui::test]
    fn a_composition_is_marked_then_committed_as_one_edit(cx: &mut TestAppContext) {
        let field = field(cx);
        let window = cx.add_window(|_, _| gpui::Empty);
        cx.update_window(window.into(), |_, window, cx| {
            field.update(cx, |field, cx| {
                // Typing "ni" in a Chinese IME: provisional, and marked.
                field.replace_and_mark_text_in_range(None, "ni", None, window, cx);
                assert_eq!(field.content(), "ni");
                assert_eq!(field.marked, Some(0..2));

                // Choosing the candidate replaces the whole composition rather
                // than appending to it.
                field.replace_text_in_range(None, "\u{4f60}", window, cx);
                assert_eq!(field.content(), "\u{4f60}");
                assert!(field.marked.is_none(), "committed text is not provisional");
                assert_eq!(field.selection, 3..3, "caret follows the commit");
            });
        })
        .unwrap();
    }

    #[gpui::test]
    fn an_abandoned_composition_leaves_no_marked_text(cx: &mut TestAppContext) {
        let field = field(cx);
        let window = cx.add_window(|_, _| gpui::Empty);
        cx.update_window(window.into(), |_, window, cx| {
            field.update(cx, |field, cx| {
                field.replace_and_mark_text_in_range(None, "kan", None, window, cx);
                assert!(field.marked.is_some());
                field.unmark_text(window, cx);
                assert!(field.marked.is_none());
            });
        })
        .unwrap();
    }

    #[gpui::test]
    fn a_pasted_newline_never_truncates_the_value(cx: &mut TestAppContext) {
        let field = field(cx);
        field.update(cx, |field, _| {
            // An API key copied out of a web page often carries a trailing
            // newline; losing everything after it would be silent data loss.
            field.replace(0..0, "AIza-part-one\nAIza-part-two");
            assert_eq!(field.content(), "AIza-part-one AIza-part-two");
        });
    }

    #[gpui::test]
    fn a_masked_field_never_renders_its_value(cx: &mut TestAppContext) {
        let field = field(cx);
        field.update(cx, |field, _| {
            field.masked = true;
            field.set_content("AIzaSyExample");
            let shown = field.rendered(0..field.content.len());
            assert_eq!(shown, "•".repeat(13));
            assert!(!shown.contains("AIza"));
        });
    }

    #[gpui::test]
    fn selecting_then_typing_replaces_the_selection(cx: &mut TestAppContext) {
        let field = field(cx);
        field.update(cx, |field, _| {
            field.set_content("hello world");
            field.selection = 0..5;
            field.replace(field.selection.clone(), "goodbye");
            assert_eq!(field.content(), "goodbye world");
            assert_eq!(field.selection, 7..7);
        });
    }

    #[gpui::test]
    fn deleting_past_either_end_is_a_no_op(cx: &mut TestAppContext) {
        let field = field(cx);
        field.update(cx, |field, _| {
            field.delete_backwards();
            field.delete_forwards();
            assert_eq!(field.content(), "");

            field.replace(0..0, "ab");
            field.delete_forwards();
            assert_eq!(field.content(), "ab", "caret is already at the end");
            field.selection = 0..0;
            field.delete_backwards();
            assert_eq!(field.content(), "ab");
        });
    }

    #[gpui::test]
    fn taking_the_content_empties_the_field(cx: &mut TestAppContext) {
        let field = field(cx);
        field.update(cx, |field, _| {
            field.set_content("Kubernetes");
            assert_eq!(field.take_content(), "Kubernetes");
            assert!(field.is_empty());
            assert_eq!(field.selection, 0..0);
        });
    }
    #[gpui::test]
    fn multiline_paste_preserves_lines_and_undo_selection(cx: &mut TestAppContext) {
        let field = cx.new(|cx| TextField::new("editor", "text", cx).multiline());
        let window = cx.add_window(|_, _| gpui::Empty);
        cx.update_window(window.into(), |_, window, cx| {
            field.update(cx, |field, cx| {
                field.set_content("d\u{e9}but \u{1f600} fin");
                field.selection = 7..11;
                field.replace_text_in_range(None, "un\r\ndeux\rtrois", window, cx);
                assert_eq!(field.content(), "d\u{e9}but un\ndeux\ntrois fin");
                field.undo();
                assert_eq!(field.content(), "d\u{e9}but \u{1f600} fin");
                assert_eq!(field.selection, 7..11);
                field.redo();
                assert_eq!(field.content(), "d\u{e9}but un\ndeux\ntrois fin");
            });
        })
        .unwrap();
    }

    #[gpui::test]
    fn backwards_selection_can_be_replaced_and_reported_to_ime(cx: &mut TestAppContext) {
        let field = field(cx);
        let window = cx.add_window(|_, _| gpui::Empty);
        cx.update_window(window.into(), |_, window, cx| {
            field.update(cx, |field, cx| {
                field.set_content("a\u{1f600}b");
                field.move_caret(5, false);
                field.move_caret(1, true);
                let selected = field.selected_text_range(false, window, cx).unwrap();
                assert_eq!(selected.range, 1..3);
                assert!(selected.reversed);
                field.replace_text_in_range(None, "\u{e9}", window, cx);
                assert_eq!(field.content(), "a\u{e9}b");
            });
        })
        .unwrap();
    }
    fn key(field: &mut TextField, key: &str, window: &mut Window, cx: &mut Context<TextField>) {
        field.on_key_down(
            &KeyDownEvent {
                keystroke: gpui::Keystroke::parse(key).unwrap(),
                is_held: false,
                prefer_character_input: false,
            },
            window,
            cx,
        );
    }

    #[gpui::test]
    fn clicks_select_a_word_a_paragraph_or_extend_and_ctrl_x_cuts(cx: &mut TestAppContext) {
        let field = field(cx);
        let window = cx.add_window(|_, _| gpui::Empty);
        cx.update_window(window.into(), |_, window, cx| {
            field.update(cx, |field, cx| {
                field.multiline = true;
                field.set_content("1. Installe Proxmox ce soir.\n2. Lance l'été");
                let proxmox = field.content.find("Proxmox").unwrap() + 3;
                field.click_at(proxmox, 2, false);
                assert_eq!(&field.content[field.selection_range()], "Proxmox");
                field.click_at(3, 3, false);
                assert_eq!(
                    &field.content[field.selection_range()],
                    "1. Installe Proxmox ce soir."
                );
                let ete = field.content.find("été").unwrap() + 2;
                field.click_at(ete, 2, false);
                assert_eq!(
                    &field.content[field.selection_range()],
                    "été",
                    "accents stay in the word"
                );
                field.click_at(field.content.len(), 3, false);
                assert_eq!(&field.content[field.selection_range()], "2. Lance l'été");

                field.click_at(3, 1, false);
                assert!(field.dragging);
                field.click_at(19, 1, true);
                assert_eq!(&field.content[field.selection_range()], "Installe Proxmox");
                key(field, "ctrl-x", window, cx);
                assert_eq!(field.content(), "1.  ce soir.\n2. Lance l'été");
                assert_eq!(
                    cx.read_from_clipboard()
                        .and_then(|item| item.text())
                        .as_deref(),
                    Some("Installe Proxmox")
                );
                key(field, "ctrl-z", window, cx);
                assert_eq!(
                    field.content(),
                    "1. Installe Proxmox ce soir.\n2. Lance l'été"
                );
            });
        })
        .unwrap();
    }

    #[gpui::test]
    fn editor_layout_tracks_newlines_wrapping_and_click_positions(cx: &mut TestAppContext) {
        let field = cx.new(|cx| TextField::new("editor", "text", cx).multiline());
        let window = cx.add_window(|_, _| gpui::Empty);
        cx.update_window(window.into(), |_, window, cx| {
            field.update(cx, |field, _| {
                field.set_content("ab\n\u{1f600}\nlast\n");
                let layout = field.editor_layout(px(400.0), Theme::ui(window), window);
                assert_eq!(
                    layout
                        .lines
                        .iter()
                        .map(|line| line.start)
                        .collect::<Vec<_>>(),
                    [0, 3, 8, 13]
                );
                assert_eq!(layout.position(13, false).y, EDITOR_LINE_HEIGHT * 3);
                assert_eq!(layout.index_at(point(px(0.0), EDITOR_LINE_HEIGHT)), 3);
                assert_eq!(layout.index_at(point(px(500.0), EDITOR_LINE_HEIGHT)), 7);

                field.set_content("alpha beta gamma delta epsilon zeta eta theta");
                let layout = field.editor_layout(px(80.0), Theme::ui(window), window);
                assert!(!layout.lines[0].shaped.wrap_boundaries().is_empty());
                let boundary = layout.lines[0].shaped.wrap_boundaries()[0];
                let index =
                    layout.lines[0].shaped.runs()[boundary.run_ix].glyphs[boundary.glyph_ix].index;
                let next_row = layout.position(index, false);
                assert_eq!(next_row.x, px(0.0));
                assert_eq!(next_row.y, EDITOR_LINE_HEIGHT);
                assert_eq!(layout.position(index, true).y, px(0.0));
                assert_eq!(layout.index_at(next_row), index);
                field.last_bounds = Some(Bounds::new(
                    point(px(10.0), px(20.0)),
                    size(px(120.0), px(220.0)),
                ));
                field.layout = Some(layout);
                assert_eq!(
                    field.caret_from_click(
                        point(
                            px(10.0) + theme::spacing::S,
                            px(20.0) + theme::spacing::S + EDITOR_LINE_HEIGHT
                        ),
                        window
                    ),
                    Some(index)
                );
            });
        })
        .unwrap();
    }

    #[gpui::test]
    fn multiline_keys_move_by_line_and_enter_inserts_one_undoable_newline(cx: &mut TestAppContext) {
        let field = cx.new(|cx| TextField::new("editor", "text", cx).multiline());
        let submitted = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = submitted.clone();
        let _subscription = cx
            .update(|cx| cx.subscribe(&field, move |_, _: &Submit, _| count.set(count.get() + 1)));
        let window = cx.add_window(|_, _| gpui::Empty);
        cx.update_window(window.into(), |_, window, cx| {
            field.update(cx, |field, cx| {
                field.set_content("ab\ncd\nef");
                field.layout = Some(field.editor_layout(px(400.0), Theme::ui(window), window));
                field.move_caret(1, false);
                key(field, "down", window, cx);
                assert_eq!(field.selection, 4..4);
                key(field, "home", window, cx);
                assert_eq!(field.selection, 3..3);
                key(field, "shift-end", window, cx);
                assert_eq!(field.selection, 3..5);
                key(field, "left", window, cx);
                assert_eq!(
                    field.selection,
                    3..3,
                    "left collapses a selection to its start"
                );
                key(field, "enter", window, cx);
                assert_eq!(field.content(), "ab\n\ncd\nef");
                key(field, "ctrl-z", window, cx);
                assert_eq!(field.content(), "ab\ncd\nef");
                key(field, "ctrl-enter", window, cx);
                assert_eq!(field.content(), "ab\ncd\nef");
            });
        })
        .unwrap();
        assert_eq!(submitted.get(), 1);
    }

    #[gpui::test]
    fn changed_reports_user_edits_undo_redo_but_not_reload_or_noop(cx: &mut TestAppContext) {
        let field = field(cx);
        let changed = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = changed.clone();
        let _subscription = cx
            .update(|cx| cx.subscribe(&field, move |_, _: &Changed, _| count.set(count.get() + 1)));
        let window = cx.add_window(|_, _| gpui::Empty);
        cx.update_window(window.into(), |_, window, cx| {
            field.update(cx, |field, cx| {
                field.set_content("loaded");
                field.replace_text_in_range(None, "", window, cx);
            });
        })
        .unwrap();
        assert_eq!(changed.get(), 0);
        cx.update_window(window.into(), |_, window, cx| {
            field.update(cx, |field, cx| {
                field.replace_text_in_range(None, "!", window, cx);
                key(field, "ctrl-z", window, cx);
                key(field, "ctrl-y", window, cx);
            });
        })
        .unwrap();
        assert_eq!(changed.get(), 3);
    }

    #[gpui::test]
    fn ime_selection_counts_utf16_and_composition_undoes_once(cx: &mut TestAppContext) {
        let field = field(cx);
        let window = cx.add_window(|_, _| gpui::Empty);
        cx.update_window(window.into(), |_, window, cx| {
            field.update(cx, |field, cx| {
                field.set_content("x");
                field.replace_and_mark_text_in_range(
                    None,
                    "\u{1f600}\u{e9}",
                    Some(2..3),
                    window,
                    cx,
                );
                assert_eq!(field.selection, 5..7);
                field.replace_text_in_range(None, "ok", window, cx);
                assert_eq!(field.content(), "xok");
                field.undo();
                assert_eq!(field.content(), "x");
                assert!(field.undo.is_empty());
            });
        })
        .unwrap();
    }

    #[gpui::test]
    fn painting_a_long_editor_keeps_the_caret_in_the_scrolling_viewport(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| TextField::new("editor", "text", cx).multiline());
        let field = window.root(cx).unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            field.update(cx, |field, cx| {
                field.set_content("a long editable paragraph with Unicode \u{1f600}\n".repeat(100));
                field.focus_handle.focus(window, cx);
            });
            window.draw(cx).clear(cx);
            field.update(cx, |field, cx| {
                assert!(field.scroll_offset > px(0.0));
                let bounds = field.last_bounds.unwrap();
                let length = field.text_length_utf16(window, cx).unwrap();
                let caret = field
                    .bounds_for_range(length..length, bounds, window, cx)
                    .unwrap();
                assert!(caret.origin.y >= bounds.origin.y);
                assert!(caret.bottom() <= bounds.bottom());
                key(field, "ctrl-home", window, cx);
            });
            window.draw(cx).clear(cx);
            field.update(cx, |field, _| assert_eq!(field.scroll_offset, px(0.0)));
        })
        .unwrap();
    }
}
