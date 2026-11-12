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

//! The insertion ladder, with the two guards that fix the most-reported bugs in
//! this category of app:
//!
//! ```text
//!   guard 1: foreground app changed since dictation started -> NEVER paste blind
//!   guard 2: a password field has focus -> refuse entirely, no clipboard leak
//!   guard 3: focus is provably not text (button, list, a web page outside
//!            any field) -> nothing sent
//!            (guards 1 and 3: the recovery window offers the text to copy)
//!
//!   tier 1: clipboard + synthesized Ctrl+V with guarded restore
//!   tier 2: the clipboard is unavailable: synthetic Unicode typing into a
//!           proven text control
//!   tier 3: text left on the clipboard, visible hint
//!
//!   receipt: a readable field must show the text right before the caret,
//!            or it is "Unconfirmed" (recovery window). A field UI Automation
//!            cannot read (Notepad++, classic Win32 edits) counts as inserted:
//!            not verifiable is not failed. Nothing is ever re-sent, and the
//!            user's clipboard comes back either way.
//! ```
//!
//! Paste comes first because it is one atomic insertion. Typed keys go through
//! the target's per-keystroke handling (autocorrect, suggestions, key-repeat
//! detection), none of it built for keys arriving at injection speed: the
//! Windows 11 Notepad (11.2607) garbles a typed burst into repeated letters and
//! "…", even at 5 ms per character, while the same text pasted lands exact.

use crate::transcription::DictationContext;
use crate::win32::{self, FocusKind};
use async_trait::async_trait;
use std::time::{Duration, Instant};

/// How long the clipboard keeps the transcript before the user's own contents
/// go back. Slow applications read the clipboard late; restoring too early
/// pastes the user's OLD clipboard.
pub const RESTORE_DELAY: Duration = Duration::from_secs(1);

/// How long a pasted text may take to show up in the field.
const RECEIPT_WAIT: Duration = Duration::from_secs(1);
/// Typed text lands at the target's own pace (the Notepad once took ~31 ms
/// per injected character).
const RECEIPT_WAIT_PER_TYPED_CHAR: Duration = Duration::from_millis(50);
const RECEIPT_POLL: Duration = Duration::from_millis(25);
/// Characters read before the inserted text, so an identical text already
/// sitting at the caret is not mistaken for ours.
const RECEIPT_CONTEXT: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertionOutcome {
    /// Sent, and read back at the caret — or sent into a field UI Automation
    /// cannot read, where delivery is not verifiable.
    Inserted,
    /// Sent into a readable field that does not show the text. Never re-sent,
    /// so it cannot land twice: the recovery window offers it to copy.
    Unconfirmed,
    /// Not sent: the user left the app they were dictating into.
    FrontmostChanged,
    /// Not sent: focus is provably not a text field.
    NoTextField,
    FellBackToClipboard,
    /// A password field had focus — the text stays in History, never on the
    /// clipboard.
    BlockedSecureField,
}

/// Insertion seam; tests use fakes.
#[async_trait]
pub trait TextInserting: Send + Sync {
    async fn insert(&self, text: &str, context: &DictationContext) -> InsertionOutcome;
}

#[derive(Default)]
pub struct InsertionCoordinator;

impl InsertionCoordinator {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl TextInserting for InsertionCoordinator {
    async fn insert(&self, text: &str, context: &DictationContext) -> InsertionOutcome {
        // Guard 2: a password field, or the secure desktop. The transcript stays
        // in History only — putting it on the clipboard would leak it to every
        // clipboard reader on the machine.
        let focus = win32::focus_kind();
        if focus == FocusKind::Password {
            tracing::warn!("password field focused at insert time — refusing (History only)");
            return InsertionOutcome::BlockedSecureField;
        }

        // Guard 1: is the user still where they started dictating?
        if moved_away(context) {
            tracing::info!("foreground changed since dictation started — no blind paste");
            return InsertionOutcome::FrontmostChanged;
        }

        // Guard 3: a Ctrl+V on a button or a file list goes nowhere at best.
        if focus == FocusKind::NotText {
            tracing::info!("focus is not a text field — nothing sent");
            return InsertionOutcome::NoTextField;
        }

        // The field before the insertion, for the receipt. None: it has no UI
        // Automation text, so delivery cannot be confirmed.
        let reach = text.chars().count() + RECEIPT_CONTEXT;
        let prior = text_before_caret(context.target_pid, reach);

        // Guard 1 re-check: the focus queries above take a UI Automation
        // round-trip, and an Alt+Tab in that window would land the Ctrl+V in the
        // wrong app.
        if moved_away(context) {
            tracing::info!("foreground changed during the focus checks — no blind paste");
            return InsertionOutcome::FrontmostChanged;
        }

        // Tier 1: guarded paste.
        let readable = prior.is_some();
        if let Some(restore) = paste(text) {
            let landed = receipt(text, prior, context.target_pid, RECEIPT_WAIT).await;
            // The recovery window copies on demand: the user's own clipboard
            // comes back whatever happened.
            restore.later();
            return verdict(landed, readable, "paste");
        }

        // Tier 2: no clipboard, so type it. Never for several lines: a typed
        // line break is an Enter, which sends a chat message.
        if text.chars().count() <= win32::MAX_TYPED_CHARS
            && !text.contains('\n')
            && win32::focus_kind() == FocusKind::EditableText
            && win32::type_text(text)
        {
            let wait = RECEIPT_WAIT_PER_TYPED_CHAR * text.chars().count() as u32 + RECEIPT_WAIT;
            let landed = receipt(text, prior, context.target_pid, wait).await;
            return verdict(landed, readable, "typing");
        }

        // Tier 3: clipboard floor.
        copy_only(text);
        InsertionOutcome::FellBackToClipboard
    }
}

/// Read back: inserted. Unreadable field: not verifiable, which is not a
/// failure (Notepad++ never shows its text to UI Automation). Readable but the
/// text is not there: unconfirmed, for the recovery window.
fn verdict(landed: bool, readable: bool, how: &str) -> InsertionOutcome {
    if landed {
        tracing::info!(how, "inserted, read back at the caret");
        InsertionOutcome::Inserted
    } else if !readable {
        tracing::info!(
            how,
            "sent into a field UI Automation cannot read — not verifiable"
        );
        InsertionOutcome::Inserted
    } else {
        tracing::info!(how, "sent, but the readable field does not show it");
        InsertionOutcome::Unconfirmed
    }
}

/// True when the foreground process is provably not the one the user was
/// dictating into. An unknown pid on either side is NOT treated as a move: the
/// cost of a false positive is a chip the user has to click, every time.
fn moved_away(context: &DictationContext) -> bool {
    let Some(expected) = context.target_pid else {
        return false;
    };
    match win32::foreground_app().pid {
        Some(current) => current != expected,
        None => false,
    }
}

/// Puts text on the clipboard WITHOUT restore — the tier-3 floor, and the
/// recovery window's Copy button.
pub fn copy_only(text: &str) {
    win32::write_clipboard(text);
}

/// Writes the transcript and posts Ctrl+V. None when either failed (the text,
/// if written, stays on the clipboard as the fallback content).
fn paste(text: &str) -> Option<Restore> {
    let snapshot = win32::snapshot_clipboard();
    let sequence = win32::write_clipboard(text)?;
    if !win32::post_paste() {
        return None;
    }
    Some(Restore {
        snapshot,
        sequence,
        posted: Instant::now(),
    })
}

/// The user's clipboard as it was before a paste.
struct Restore {
    snapshot: win32::ClipboardSnapshot,
    sequence: u32,
    posted: Instant,
}

impl Restore {
    /// Puts the user's clipboard back a second after the paste — but only if
    /// nothing else has touched it in the meantime. Detached, so the session
    /// is not pinned in `Inserting` after the text visibly landed: that window
    /// used to silently reject the next dictation's `begin`.
    fn later(self) {
        crate::runtime::spawn(async move {
            tokio::time::sleep(RESTORE_DELAY.saturating_sub(self.posted.elapsed())).await;
            // A user copy mid-flight wins.
            if win32::clipboard_sequence_number() == self.sequence && !self.snapshot.is_empty() {
                win32::restore_clipboard(self.snapshot);
            }
        });
    }
}

/// Text right before the caret of the focused field of `pid`, line breaks
/// normalized, when UI Automation can read it.
fn text_before_caret(pid: Option<u32>, chars: usize) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        win32::on_input_thread(move || crate::app_learning::probe_focused(pid, chars, 0))
            .map(|read| crate::app_learning::normalize(&read.before))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (pid, chars);
        None
    }
}

/// Waits up to `wait` for the field to show `text` at the caret. False at once
/// when the field was unreadable before the insertion.
async fn receipt(text: &str, prior: Option<String>, pid: Option<u32>, wait: Duration) -> bool {
    let Some(prior) = prior else {
        return false;
    };
    let reach = text.chars().count() + RECEIPT_CONTEXT;
    let started = Instant::now();
    loop {
        if let Some(now) = text_before_caret(pid, reach)
            && landed(text, &prior, &now)
        {
            return true;
        }
        if started.elapsed() >= wait {
            return false;
        }
        tokio::time::sleep(RECEIPT_POLL).await;
    }
}

/// The field now ends, at the caret, with `text`, and did not already before
/// (the same dictation twice in a row).
fn landed(text: &str, prior: &str, now: &str) -> bool {
    let text = crate::app_learning::normalize(text);
    !text.trim().is_empty() && now != prior && now.ends_with(text.as_str())
}

// ---------------------------------------------------------------------------
// Test double
// ---------------------------------------------------------------------------

/// Records what it was asked to insert and returns a scripted outcome.
pub struct FakeInserter {
    pub outcome: InsertionOutcome,
    pub inserted: parking_lot::Mutex<Vec<String>>,
}

impl Default for FakeInserter {
    fn default() -> Self {
        Self {
            outcome: InsertionOutcome::Inserted,
            inserted: parking_lot::Mutex::new(Vec::new()),
        }
    }
}

impl FakeInserter {
    pub fn with_outcome(outcome: InsertionOutcome) -> Self {
        Self {
            outcome,
            ..Default::default()
        }
    }
}

#[async_trait]
impl TextInserting for FakeInserter {
    async fn insert(&self, text: &str, _context: &DictationContext) -> InsertionOutcome {
        self.inserted.lock().push(text.to_string());
        self.outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_readable_field_without_the_text_is_unconfirmed() {
        assert_eq!(verdict(true, true, "paste"), InsertionOutcome::Inserted);
        // Notepad++ and classic Edit fields: sent, just not verifiable.
        assert_eq!(verdict(false, false, "paste"), InsertionOutcome::Inserted);
        assert_eq!(verdict(false, true, "paste"), InsertionOutcome::Unconfirmed);
    }

    #[test]
    fn an_unknown_target_pid_never_reads_as_a_move() {
        // Refusing to paste whenever we cannot prove where we are would put a
        // chip in front of the user on every single dictation.
        assert!(!moved_away(&DictationContext::default()));
    }

    #[test]
    fn a_known_target_pid_is_compared_against_the_foreground() {
        let context = DictationContext {
            target_pid: Some(u32::MAX), // no real process can own this
            ..Default::default()
        };
        // Either the foreground pid is unknown (no move) or it differs (move).
        // Both are correct here; what must never happen is a panic or a claim
        // that a nonexistent process is still in front.
        let foreground = win32::foreground_app().pid;
        assert_eq!(moved_away(&context), foreground.is_some());
    }

    #[test]
    fn a_receipt_needs_the_text_at_the_caret_and_a_change() {
        let text = "Je vais ouvrir le rapport.";
        assert!(landed(
            text,
            "Bonjour.\n",
            "Bonjour.\nJe vais ouvrir le rapport."
        ));
        // The garbled Notepad burst, the text missing, or still on its way.
        assert!(!landed(text, "", "Je aais ouvrir ……….."));
        assert!(!landed(text, "", "Je vais ouvrir le"));
        // Already there before: an identical dictation that did not land.
        let before = "Bonjour. Je vais ouvrir le rapport.";
        assert!(!landed(text, before, before));
        // Line endings differ between controls.
        assert!(landed("un\r\ndeux", "x", "x un\ndeux"));
        assert!(!landed("   ", "a", "a   "));
    }

    #[tokio::test]
    async fn the_fake_records_what_it_was_handed() {
        let inserter = FakeInserter::with_outcome(InsertionOutcome::FellBackToClipboard);
        let outcome = inserter
            .insert("hello there", &DictationContext::default())
            .await;
        assert_eq!(outcome, InsertionOutcome::FellBackToClipboard);
        assert_eq!(inserter.inserted.lock().as_slice(), ["hello there"]);
    }
}
