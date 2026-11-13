//! Opt-in learning from corrections made in another application, right after
//! Jot inserted a dictation there.
//!
//! ```text
//!   insert ─► anchor: the field's text just before the caret must END with the
//!             inserted text (that is the insertion receipt); keep ≤48 chars of
//!             context on each side, never the document
//!   observe ─► same focused element only, every 500 ms, for 30 s at most:
//!             re-find the segment between the two contexts in a window around
//!             the caret sized to the segment (tight around it, one wider read
//!             of at most its own length on each side when the caret jumped);
//!             ambiguous or missing → nothing learned
//!   finish ─► last edited segment + at least one real key press since the
//!             insertion → one `Correction` for the existing extractor
//! ```
//!
//! No screenshot, no clipboard, no new hook, no injection, no document read:
//! UI Automation text ranges around the caret, on a thread of its own with UIA
//! timeouts, so a frozen target app can never hold up an insertion.

use crate::history::Correction;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use time::OffsetDateTime;
use uuid::Uuid;

/// How long a field is watched after an insertion.
pub const OBSERVATION: Duration = Duration::from_secs(30);
/// Longer insertions are not watched. About five minutes of speech: a long
/// dictation fixed in one word must still teach that word, and the reads stay
/// sized to the segment, not to the page (see [`tick`]).
pub const MAX_INSERTED_CHARS: usize = 6000;
const CONTEXT: usize = 48;
const TICK: Duration = Duration::from_millis(500);
const ANCHOR_RETRY: Duration = Duration::from_millis(100);
/// Ctrl+V lands whenever the target app reads its queue.
const ANCHOR_WAIT: Duration = Duration::from_millis(1500);
/// Typed text lands at the target's own pace: the Windows 11 Notepad takes
/// about 31 ms per injected character, whatever the pacing.
const ANCHOR_WAIT_PER_CHAR: Duration = Duration::from_millis(50);

fn anchor_wait(inserted: &str) -> Duration {
    let chars = inserted.chars().count();
    // Longer text is always pasted: waiting for it to be typed only delayed
    // the verdict by 12 s on long dictations.
    if chars > crate::win32::MAX_TYPED_CHARS {
        return ANCHOR_WAIT;
    }
    ANCHOR_WAIT + ANCHOR_WAIT_PER_CHAR * chars as u32
}

static KEY_DOWNS: AtomicU64 = AtomicU64::new(0);

/// Called by the existing keyboard hook for every non-Jot key press. Only a
/// count: which key is never stored. An edit with no key press since the
/// insertion is the application's own (autocorrect, sync), not the user's.
pub fn note_key_down() {
    KEY_DOWNS.fetch_add(1, Ordering::Relaxed);
}

fn key_downs() -> u64 {
    KEY_DOWNS.load(Ordering::Relaxed)
}

/// Line endings differ between controls (`\r` in RichEdit, `\r\n` in Edit),
/// and a web editor turns pasted blank lines into paragraphs and keeps runs of
/// spaces as no-break spaces: what is compared is the text, not its layout.
pub fn normalize(text: &str) -> String {
    static BREAKS: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"[ \t]*\n[ \t\n]*").unwrap());
    let text = text
        .replace("\r\n", "\n")
        .replace(['\r', '\u{2028}', '\u{2029}'], "\n")
        .replace('\u{a0}', " ");
    BREAKS.replace_all(&text, "\n").into_owned()
}

/// Where the inserted text sits, as context strings rather than offsets:
/// offsets shift with every edit, the surrounding text does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub inserted: String,
    prefix: String,
    suffix: String,
    /// The prefix starts at the document start (it is all there is before).
    at_start: bool,
    at_end: bool,
}

/// `before`/`after`: text read just before/after the caret; `at_start`/`at_end`:
/// the read hit the document edge. None unless the field provably ends, at the
/// caret, with exactly what Jot inserted.
pub fn anchor(
    inserted: &str,
    before: &str,
    after: &str,
    at_start: bool,
    at_end: bool,
) -> Option<Anchor> {
    let inserted = normalize(inserted);
    let (before, after) = (normalize(before), normalize(after));
    if inserted.trim().is_empty() || inserted.chars().count() > MAX_INSERTED_CHARS {
        return None;
    }
    let head = before.strip_suffix(inserted.as_str())?;
    let head_chars = head.chars().count();
    let prefix: String = head
        .chars()
        .skip(head_chars.saturating_sub(CONTEXT))
        .collect();
    let suffix: String = after.chars().take(CONTEXT).collect();
    let at_start = at_start && head_chars <= CONTEXT;
    let at_end = at_end && after.chars().count() <= CONTEXT;
    // A few characters of context away from the document edge match too many
    // places to prove where the segment is.
    if (!at_start && prefix.chars().count() < CONTEXT / 2)
        || (!at_end && suffix.chars().count() < CONTEXT / 2)
    {
        return None;
    }
    Some(Anchor {
        inserted,
        prefix,
        suffix,
        at_start,
        at_end,
    })
}

/// The segment as it reads now in `window` (text around the caret, flags as for
/// [`anchor`]). None when the contexts are gone (edited, scrolled away) or
/// match more than one place.
pub fn locate(anchor: &Anchor, window: &str, at_start: bool, at_end: bool) -> Option<String> {
    locate_at(anchor, &normalize(window), at_start, at_end).map(|(_, segment)| segment)
}

/// [`locate`] on an already normalized window, with the segment's byte offset.
fn locate_at(
    anchor: &Anchor,
    window: &str,
    at_start: bool,
    at_end: bool,
) -> Option<(usize, String)> {
    let starts: Vec<usize> = if anchor.at_start {
        (at_start && window.starts_with(&anchor.prefix))
            .then_some(anchor.prefix.len())
            .into_iter()
            .collect()
    } else {
        window
            .match_indices(&anchor.prefix)
            .map(|(i, _)| i + anchor.prefix.len())
            .collect()
    };
    let limit = anchor.inserted.len() * 2 + 256;
    let mut found: Option<(usize, &str)> = None;
    for start in starts {
        let rest = &window[start..];
        let end = if anchor.at_end {
            (at_end && rest.ends_with(&anchor.suffix)).then(|| rest.len() - anchor.suffix.len())
        } else {
            rest.find(&anchor.suffix)
        };
        let Some(segment) = end.map(|end| &rest[..end]) else {
            continue;
        };
        // Two different candidates, or one swallowing far more than was
        // inserted (the suffix matched much further on): not provable.
        if segment.len() > limit || found.is_some_and(|(_, f)| f != segment) {
            return None;
        }
        found = Some((start, segment));
    }
    found.map(|(start, segment)| (start, segment.to_string()))
}

/// What a finished observation hands to the dictionary, if anything.
pub fn evidence(
    dictation_id: Uuid,
    anchor: &Anchor,
    edited: Option<&str>,
    typed: bool,
    now: OffsetDateTime,
) -> Option<Correction> {
    let edited = edited?;
    (typed && edited != anchor.inserted).then(|| Correction {
        dictation_id: dictation_id.to_string(),
        before: anchor.inserted.clone(),
        after: edited.to_string(),
        corrected_at: now,
    })
}

// ---------------------------------------------------------------------------
// The watcher thread
// ---------------------------------------------------------------------------

pub type Sink = Box<dyn FnOnce(Correction) + Send>;

/// One insertion to watch.
pub struct Watch {
    pub dictation_id: Uuid,
    pub inserted: String,
    /// The process the dictation targeted; the focused element must belong to it.
    pub target_pid: Option<u32>,
    pub sink: Sink,
}

enum Command {
    Start(Watch),
    Stop { learn: bool },
}

/// The watcher thread exists only once a watch was asked for: with the option
/// off, nothing of this runs at all.
static COMMANDS: std::sync::OnceLock<parking_lot::Mutex<std::sync::mpsc::Sender<Command>>> =
    std::sync::OnceLock::new();

/// Starts watching, ending any previous watch first (a new dictation ends it).
pub fn watch(watch: Watch) {
    let commands = COMMANDS.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("jot-correction-watch".into())
            .spawn(move || platform::run(rx))
            .expect("spawning the correction watcher");
        parking_lot::Mutex::new(tx)
    });
    let _ = commands.lock().send(Command::Start(watch));
}

/// Ends the current watch now. `learn`: a new dictation starts, so what the
/// user already fixed still counts; false when the option is turned off or the
/// dictation deleted, which discard it.
pub fn stop(learn: bool) {
    if let Some(commands) = COMMANDS.get() {
        let _ = commands.lock().send(Command::Stop { learn });
    }
}

/// Text around the caret of the focused field, as read through UIA.
pub struct CaretText {
    pub before: String,
    pub after: String,
    pub at_start: bool,
    pub at_end: bool,
}

struct Observation<E> {
    watch: Watch,
    element: E,
    anchor: Anchor,
    keys_at_start: u64,
    /// The last reading, so a state counts only once seen twice in a row: a
    /// word half-retyped when focus moves is not a correction.
    seen: Option<String>,
    edited: Option<String>,
    deadline: std::time::Instant,
    /// Consecutive ticks the field was [`Unread::Changing`].
    changing: u32,
    /// Characters from the segment's start to the caret at the last reading,
    /// so the next one reads just the segment and its contexts.
    caret: usize,
}

/// Why a field could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unread {
    /// The app replaced the text under the caret while it was read: the same
    /// field, mid-edit. Read again on the next tick.
    Changing,
    Unreadable,
}

/// Consecutive [`Unread::Changing`] ticks tolerated (1.5 s): an edit settles
/// within one; a field that stays so is not readable.
const CHANGING_TICKS: u32 = 3;

/// The field access the watcher needs; real UIA on Windows, a fake in tests.
trait Field {
    type Element;
    /// The focused text element, if it is a readable, non-password text field
    /// of `pid` (when known).
    fn focused(&self, pid: Option<u32>) -> Option<Self::Element>;
    fn same(&self, a: &Self::Element, b: &Self::Element) -> bool;
    fn read(
        &self,
        element: &Self::Element,
        back: usize,
        forward: usize,
    ) -> Result<CaretText, Unread>;
}

/// Tries to anchor once. Err: not (yet) provably inserted here, and why.
fn try_anchor<F: Field>(field: &F, watch: &Watch) -> Result<(F::Element, Anchor), &'static str> {
    let element = field
        .focused(watch.target_pid)
        .ok_or("no readable text field focused")?;
    let length = watch.inserted.chars().count();
    let text = field
        .read(&element, length + CONTEXT, CONTEXT)
        .map_err(|_| "field unreadable")?;
    let anchor = anchor(
        &watch.inserted,
        &text.before,
        &text.after,
        text.at_start,
        text.at_end,
    )
    .ok_or("inserted text not found before the caret")?;
    Ok((element, anchor))
}

/// One observation tick. Err: the observation is over, and why (logged).
fn tick<F: Field>(
    field: &F,
    observation: &mut Observation<F::Element>,
) -> Result<(), &'static str> {
    if std::time::Instant::now() >= observation.deadline {
        return Err("window elapsed");
    }
    // Focus elsewhere, another window, a password box, a destroyed element.
    let Some(focused) = field.focused(observation.watch.target_pid) else {
        return Err("focus left the field");
    };
    if !field.same(&focused, &observation.element) {
        return Err("focus moved to another element");
    }
    let length = observation
        .seen
        .as_ref()
        .unwrap_or(&observation.anchor.inserted)
        .chars()
        .count();
    // Room for the contexts and for what an edit adds to the segment.
    let slack = CONTEXT + 64 + length / 8;
    let caret = observation.caret.min(length);
    // Tight first: the segment around where the caret was. If the caret
    // jumped (Ctrl+Home, a click), one read of the whole segment from
    // wherever it is now; never more than the segment's length beyond it.
    let mut found = None;
    for (back, forward) in [
        (caret + slack, length - caret + slack),
        (length + slack, length + slack),
    ] {
        // Focus was just checked to be this field, so a stale range means the
        // field is mid-edit: skip the tick (nothing counts as seen) a few times.
        let text = match field.read(&observation.element, back, forward) {
            Ok(text) => text,
            Err(Unread::Changing) if observation.changing < CHANGING_TICKS => {
                observation.changing += 1;
                return Ok(());
            }
            Err(_) => return Err("field unreadable"),
        };
        observation.changing = 0;
        let caret_chars = normalize(&text.before).chars().count();
        let window = normalize(&format!("{}{}", text.before, text.after));
        if let Some((start, segment)) =
            locate_at(&observation.anchor, &window, text.at_start, text.at_end)
        {
            let start_chars = window[..start].chars().count();
            found = Some((segment, caret_chars.saturating_sub(start_chars)));
            break;
        }
    }
    // The contexts are gone (edited, caret moved far away): stop, keeping only
    // the last stable reading.
    let Some((segment, caret)) = found else {
        return Err("segment context lost");
    };
    observation.caret = caret;
    if observation.seen.as_ref() == Some(&segment) {
        observation.edited = (segment != observation.anchor.inserted).then(|| segment.clone());
    }
    observation.seen = Some(segment);
    Ok(())
}

fn finish<E>(observation: Observation<E>, why: &str) {
    let typed = key_downs() > observation.keys_at_start;
    // Never the text: only why the watch ended and what it saw.
    tracing::info!(
        why,
        edited = observation.edited.is_some(),
        typed,
        "correction watch: ended"
    );
    if let Some(correction) = evidence(
        observation.watch.dictation_id,
        &observation.anchor,
        observation.edited.as_deref(),
        typed,
        OffsetDateTime::now_utc(),
    ) {
        (observation.watch.sink)(correction);
    } else if observation.edited.is_some() {
        tracing::info!("correction watch: edit seen without a key press — not learned");
    }
}

/// The watcher loop, generic over the field access so it is testable headless.
fn run_with<F: Field>(field: &F, rx: std::sync::mpsc::Receiver<Command>) {
    use std::sync::mpsc::RecvTimeoutError;
    let mut pending: Option<(Watch, std::time::Instant, u64)> = None;
    let mut active: Option<Observation<F::Element>> = None;
    loop {
        let command = if pending.is_some() {
            rx.recv_timeout(ANCHOR_RETRY)
        } else if active.is_some() {
            rx.recv_timeout(TICK)
        } else {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        match command {
            Ok(Command::Start(watch)) => {
                if let Some(observation) = active.take() {
                    finish(observation, "new dictation");
                }
                let chars = watch.inserted.chars().count();
                if chars > MAX_INSERTED_CHARS {
                    pending = None;
                    tracing::info!(
                        chars,
                        limit = MAX_INSERTED_CHARS,
                        "correction watch: dictation too long — nothing observed"
                    );
                    continue;
                }
                let started = std::time::Instant::now();
                pending = Some((watch, started, key_downs()));
            }
            Ok(Command::Stop { learn }) => {
                pending = None;
                if let Some(observation) = active.take()
                    && learn
                {
                    finish(observation, "stopped");
                }
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if let Some((watch, started, keys_at_start)) = pending.take() {
            match try_anchor(field, &watch) {
                Ok((element, anchor)) => {
                    tracing::info!(
                        after_ms = started.elapsed().as_millis() as u64,
                        "correction watch: anchored"
                    );
                    active = Some(Observation {
                        watch,
                        element,
                        keys_at_start,
                        seen: None,
                        edited: None,
                        deadline: started + OBSERVATION,
                        changing: 0,
                        caret: anchor.inserted.chars().count(),
                        anchor,
                    });
                }
                Err(_) if started.elapsed() < anchor_wait(&watch.inserted) => {
                    pending = Some((watch, started, keys_at_start))
                }
                // Never the text: the step that failed, so "not a text field"
                // and "not found" read apart from "too long" in the log.
                Err(why) => tracing::info!(
                    why,
                    chars = watch.inserted.chars().count(),
                    "correction watch: field not compatible — nothing observed"
                ),
            }
            continue;
        }
        if let Some(observation) = active.as_mut()
            && let Err(why) = tick(field, observation)
        {
            finish(active.take().expect("checked above"), why);
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::{CaretText, Command, Field, Unread};
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationElement,
        IUIAutomationTextPattern, IUIAutomationTextPattern2, IUIAutomationTextRange,
        TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start, TextUnit_Character,
        UIA_E_ELEMENTNOTAVAILABLE, UIA_TextPattern2Id, UIA_TextPatternId,
    };
    use windows::core::{HRESULT, Interface};

    pub struct Uia(IUIAutomation);

    /// A selection longer than this is not a word being fixed.
    const MAX_SELECTION: i32 = 4096;

    /// Logs the failing UIA call and its HRESULT (never the text). A range that
    /// is no longer available was made stale by the page editing the field
    /// under it (ChatGPT's composer replaces the text node on a key press).
    fn failed(step: &'static str) -> impl Fn(windows::core::Error) -> Unread {
        move |error| {
            tracing::info!(step, code = ?error.code(), "correction watch: read failed");
            if error.code() == HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32) {
                Unread::Changing
            } else {
                Unread::Unreadable
            }
        }
    }

    pub fn run(rx: std::sync::mpsc::Receiver<Command>) {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let Some(uia) = Uia::new() else {
            tracing::warn!("correction watch: UI Automation unavailable");
            while rx.recv().is_ok() {}
            return;
        };
        super::run_with(&uia, rx);
    }

    impl Uia {
        pub fn new() -> Option<Self> {
            let automation: IUIAutomation =
                unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()? };
            // A hung target must cost one bounded wait, not a stuck watcher.
            if let Ok(bounded) = automation.cast::<IUIAutomation2>() {
                unsafe {
                    let _ = bounded.SetConnectionTimeout(1000);
                    let _ = bounded.SetTransactionTimeout(500);
                }
            }
            Some(Self(automation))
        }

        /// The caret, or the selection when there is one (selecting the wrong
        /// word is the usual way to fix it), with the field's own document
        /// range: Chromium moves ranges out of the field into the page.
        fn caret(
            element: &IUIAutomationElement,
        ) -> Result<(IUIAutomationTextRange, IUIAutomationTextRange), Unread> {
            unsafe {
                if let Ok(pattern) =
                    element.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id)
                {
                    let mut active = Default::default();
                    if let (Ok(range), Ok(document)) =
                        (pattern.GetCaretRange(&mut active), pattern.DocumentRange())
                    {
                        return Ok((range, document));
                    }
                }
                let pattern = element
                    .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
                    .map_err(failed("text pattern"))?;
                let selection = pattern.GetSelection().map_err(failed("selection"))?;
                let count = selection.Length().map_err(failed("selection"))?;
                if count != 1 {
                    tracing::info!(count, "correction watch: read failed: selection count");
                    return Err(Unread::Unreadable);
                }
                Ok((
                    selection.GetElement(0).map_err(failed("selection"))?,
                    pattern.DocumentRange().map_err(failed("document range"))?,
                ))
            }
        }
    }

    impl Field for Uia {
        type Element = IUIAutomationElement;

        fn focused(&self, pid: Option<u32>) -> Option<IUIAutomationElement> {
            unsafe {
                let element = self.0.GetFocusedElement().ok()?;
                if element.CurrentIsPassword().ok()?.as_bool() {
                    return None;
                }
                if let Some(pid) = pid
                    && element.CurrentProcessId().ok()? as u32 != pid
                {
                    return None;
                }
                element
                    .GetCurrentPattern(UIA_TextPatternId)
                    .ok()
                    .map(|_| element)
            }
        }

        fn same(&self, a: &IUIAutomationElement, b: &IUIAutomationElement) -> bool {
            unsafe {
                self.0
                    .CompareElements(a, b)
                    .is_ok_and(|same| same.as_bool())
            }
        }

        fn read(
            &self,
            element: &IUIAutomationElement,
            back: usize,
            forward: usize,
        ) -> Result<CaretText, Unread> {
            Self::read_steps(element, back, forward)
        }
    }

    impl Uia {
        /// [`Field::read`], also for the probe (no `self` needed).
        pub fn read_steps(
            element: &IUIAutomationElement,
            back: usize,
            forward: usize,
        ) -> Result<CaretText, Unread> {
            let (caret, document) = Self::caret(element)?;
            unsafe {
                // [start - back, caret start] and [caret start, caret end + forward]:
                // together they cover a selection, so a word being retyped is seen.
                let before = caret.Clone().map_err(failed("clone before"))?;
                before
                    .MoveEndpointByRange(
                        TextPatternRangeEndpoint_End,
                        &caret,
                        TextPatternRangeEndpoint_Start,
                    )
                    .map_err(failed("collapse before"))?;
                let moved_back = before
                    .MoveEndpointByUnit(
                        TextPatternRangeEndpoint_Start,
                        TextUnit_Character,
                        -(back as i32),
                    )
                    .map_err(failed("move back"))?;
                let after = caret.Clone().map_err(failed("clone after"))?;
                after
                    .MoveEndpointByRange(
                        TextPatternRangeEndpoint_Start,
                        &caret,
                        TextPatternRangeEndpoint_Start,
                    )
                    .map_err(failed("collapse after"))?;
                let moved_forward = after
                    .MoveEndpointByUnit(
                        TextPatternRangeEndpoint_End,
                        TextUnit_Character,
                        forward as i32,
                    )
                    .map_err(failed("move forward"))?;
                // Never past the field: page text around a web composer changes on
                // its own (and is not ours to read).
                let past_start = before
                    .CompareEndpoints(
                        TextPatternRangeEndpoint_Start,
                        &document,
                        TextPatternRangeEndpoint_Start,
                    )
                    .map_err(failed("compare start"))?
                    < 0;
                if past_start {
                    before
                        .MoveEndpointByRange(
                            TextPatternRangeEndpoint_Start,
                            &document,
                            TextPatternRangeEndpoint_Start,
                        )
                        .map_err(failed("clamp start"))?;
                }
                let past_end = after
                    .CompareEndpoints(
                        TextPatternRangeEndpoint_End,
                        &document,
                        TextPatternRangeEndpoint_End,
                    )
                    .map_err(failed("compare end"))?
                    > 0;
                if past_end {
                    after
                        .MoveEndpointByRange(
                            TextPatternRangeEndpoint_End,
                            &document,
                            TextPatternRangeEndpoint_End,
                        )
                        .map_err(failed("clamp end"))?;
                }
                // Bounded reads: twice the units covers CRLF and surrogate pairs;
                // the selection itself counts in `after`.
                let selected = caret
                    .GetText(MAX_SELECTION)
                    .map_err(failed("text selection"))?
                    .len() as i32;
                let before_text = before
                    .GetText(back as i32 * 2 + 8)
                    .map_err(failed("text before"))?
                    .to_string();
                let after_text = after
                    .GetText(forward as i32 * 2 + 8 + selected)
                    .map_err(failed("text after"))?
                    .to_string();
                Ok(CaretText {
                    before: before_text,
                    after: after_text,
                    at_start: past_start || moved_back.unsigned_abs() < back as u32,
                    at_end: past_end || (moved_forward as u32) < forward as u32,
                })
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub use platform::Uia;

/// Debug and audit seam: what the watcher would read right now.
#[cfg(target_os = "windows")]
pub fn probe_focused(pid: Option<u32>, back: usize, forward: usize) -> Option<CaretText> {
    let uia = Uia::new()?;
    let element = uia.focused(pid)?;
    uia.read(&element, back, forward).ok()
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::Command;
    pub fn run(rx: std::sync::mpsc::Receiver<Command>) {
        while rx.recv().is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::Arc;

    const HEAD: &str = "Avant : un premier paragraphe qui précède la dictée de l’utilisateur. ";
    const TAIL: &str = " Ensuite un autre paragraphe qui suit la dictée, assez long aussi.";

    #[test]
    fn anchoring_requires_the_inserted_text_right_before_the_caret() {
        let a = anchor(
            "Je vais ouvrir Loustream.",
            "Je vais ouvrir Loustream.",
            "",
            true,
            true,
        )
        .unwrap();
        assert_eq!(
            locate(&a, "Je vais ouvrir Loostream.", true, true).as_deref(),
            Some("Je vais ouvrir Loostream.")
        );
        // Not (yet) in the field: no receipt, no watch.
        assert!(
            anchor(
                "Je vais ouvrir Loustream.",
                "Je vais ouvrir",
                "",
                true,
                true
            )
            .is_none()
        );
        // Short context away from the edges proves nothing.
        assert!(anchor("Loustream.", "ab Loustream.", "cd", false, false).is_none());
        // CRLF, CR and LF are one line break.
        let multi = anchor(
            "réseau\nKubernetes",
            "réseau\r\nKubernetes",
            "\r",
            true,
            true,
        )
        .unwrap();
        assert_eq!(
            locate(&multi, "réseau\rKubernètes\r", true, true).as_deref(),
            Some("réseau\nKubernètes")
        );
    }

    #[test]
    fn edits_outside_the_segment_are_never_attributed_to_it() {
        let inserted = "Le service unburn répond.";
        let a = anchor(inserted, &format!("{HEAD}{inserted}"), TAIL, false, false).unwrap();
        let read = |text: String| locate(&a, &text, false, false);
        // Fixed inside the segment.
        assert_eq!(
            read(format!("{HEAD}Le service Unbound répond.{TAIL}")).as_deref(),
            Some("Le service Unbound répond.")
        );
        // The neighbour text that is our context was edited: unknown.
        let edited_head = HEAD.replace("utilisateur.", "utilisatrice.");
        assert!(read(format!("{edited_head}{inserted}{TAIL}")).is_none());
        // The paragraph twice, one copy fixed: which one is ours is unknown.
        let twice = format!("{HEAD}Le service Unbound répond.{TAIL}{HEAD}{inserted}{TAIL}");
        assert!(read(twice).is_none());
        // Edits further away do not matter.
        assert_eq!(
            read(format!("Tout autre chose. {HEAD}{inserted}{TAIL} Fin.")).as_deref(),
            Some(inserted)
        );
    }

    #[test]
    fn evidence_needs_a_change_and_a_key_press() {
        let a = anchor("Le projet jott", "Le projet jott", "", true, true).unwrap();
        let id = Uuid::nil();
        let now = OffsetDateTime::UNIX_EPOCH;
        assert!(evidence(id, &a, None, true, now).is_none());
        assert!(evidence(id, &a, Some(&a.inserted.clone()), true, now).is_none());
        assert!(
            evidence(id, &a, Some("Le projet Jot"), false, now).is_none(),
            "autocorrect"
        );
        let c = evidence(id, &a, Some("Le projet Jot"), true, now).unwrap();
        assert_eq!(
            (c.before.as_str(), c.after.as_str()),
            ("Le projet jott", "Le projet Jot")
        );
    }

    /// A scripted field: the "document", the caret, the focused element id,
    /// and how many reads to come find it mid-edit.
    #[derive(Default)]
    struct FakeState {
        text: String,
        caret: usize,
        focus: Option<u32>,
        changing_reads: u32,
        /// The longest read asked for, either side of the caret.
        widest: usize,
    }

    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<FakeState>>);

    impl Fake {
        fn set(&self, text: &str, caret: usize, focus: Option<u32>) {
            let mut state = self.0.lock();
            (state.text, state.caret, state.focus) = (text.into(), caret, focus);
        }
        fn changing_for(&self, reads: u32) {
            self.0.lock().changing_reads = reads;
        }
    }

    impl Field for Fake {
        type Element = u32;
        fn focused(&self, _pid: Option<u32>) -> Option<u32> {
            self.0.lock().focus
        }
        fn same(&self, a: &u32, b: &u32) -> bool {
            a == b
        }
        fn read(&self, _element: &u32, back: usize, forward: usize) -> Result<CaretText, Unread> {
            let mut state = self.0.lock();
            if state.changing_reads > 0 {
                state.changing_reads -= 1;
                return Err(Unread::Changing);
            }
            state.widest = state.widest.max(back).max(forward);
            let (text, caret) = (state.text.clone(), state.caret);
            let chars: Vec<char> = text.chars().collect();
            let start = caret.saturating_sub(back);
            let end = (caret + forward).min(chars.len());
            Ok(CaretText {
                before: chars[start..caret].iter().collect(),
                after: chars[caret..end].iter().collect(),
                at_start: start == 0,
                at_end: end == chars.len(),
            })
        }
    }

    const INSERTED: &str = "Je vais ouvrir Loustream.";
    const FIXED: &str = "Je vais ouvrir Loostream.";

    fn wait(ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    /// Runs the real watcher loop on a fake field holding `text` (focus id 1,
    /// caret at the end), starts a watch for [`INSERTED`], then lets `script`
    /// edit the field or send commands. Returns what reached the dictionary.
    fn run_case(
        text: &str,
        script: impl FnOnce(&Fake, &std::sync::mpsc::Sender<Command>),
    ) -> Vec<Correction> {
        run_case_with(INSERTED, text, text.chars().count(), script).0
    }

    /// [`run_case`] for any inserted text and caret; also returns the widest read.
    fn run_case_with(
        inserted: &str,
        text: &str,
        caret: usize,
        script: impl FnOnce(&Fake, &std::sync::mpsc::Sender<Command>),
    ) -> (Vec<Correction>, usize) {
        let fake = Fake::default();
        fake.set(text, caret, Some(1));
        let learned = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = {
            let fake = fake.clone();
            std::thread::spawn(move || run_with(&fake, rx))
        };
        let sink = learned.clone();
        tx.send(Command::Start(Watch {
            dictation_id: Uuid::nil(),
            inserted: inserted.into(),
            target_pid: None,
            sink: Box::new(move |c| sink.lock().push(c)),
        }))
        .unwrap();
        wait(200);
        script(&fake, &tx);
        tx.send(Command::Stop { learn: true }).unwrap();
        drop(tx);
        worker.join().unwrap();
        let widest = fake.0.lock().widest;
        (learned.lock().clone(), widest)
    }

    /// About two minutes of speech (≈ 2,000 characters, several paragraphs),
    /// no two sentences alike, with "Loustream" at the start, middle and end.
    fn long_dictation() -> String {
        let mut text = String::from("Loustream démarre le matin. ");
        for i in 1..=36 {
            text.push_str(&format!(
                "Point {i} : le service numéro {i} redémarre puis PowerShell relance la file {i}. "
            ));
            if i == 12 || i == 24 {
                text.push_str("\n\n");
            }
            if i == 18 {
                text.push_str("Ensuite Loustream reprend la main. ");
            }
        }
        text.push_str("Enfin on ferme Loustream.");
        text
    }

    /// The Brave/ChatGPT case of 1.6: a long dictation fixed in one word is
    /// learned wherever the word is, and every read stays sized to the
    /// dictation, never to the page around it.
    #[test]
    fn a_long_dictation_fixed_in_one_word_is_learned_at_start_middle_and_end() {
        let inserted = long_dictation();
        assert!(inserted.chars().count() > 2_000 && inserted.chars().count() < MAX_INSERTED_CHARS);
        let page_head: String = (0..200)
            .map(|i| format!("Message précédent {i} de la conversation. "))
            .collect();
        let page_tail: String = (0..200)
            .map(|i| format!(" Suggestion {i} du site."))
            .collect();
        let document = |body: &str| format!("{page_head}{body}{page_tail}");
        let segment_start = page_head.chars().count();
        let length = inserted.chars().count();
        let occurrences: Vec<usize> = inserted
            .match_indices("Loustream")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(occurrences.len(), 3);
        for (place, byte) in occurrences.into_iter().enumerate() {
            let fixed = format!("{}Loostream{}", &inserted[..byte], &inserted[byte + 9..]);
            // The user clicked into the word: the caret jumps there.
            let caret = segment_start + inserted[..byte].chars().count() + 4;
            let (found, widest) = run_case_with(
                &inserted,
                &document(&inserted),
                segment_start + length,
                |fake, _| {
                    fake.set(&document(&fixed), caret, Some(1));
                    note_key_down();
                    wait(1600);
                },
            );
            assert_eq!(found.len(), 1, "place {place}");
            assert_eq!(found[0].after, normalize(&fixed), "place {place}");
            let learned = crate::dictionary::app_suggestions(&found[0]);
            assert_eq!(
                learned
                    .iter()
                    .map(|s| (s.wrong.as_str(), s.term.as_str()))
                    .collect::<Vec<_>>(),
                [("Loustream", "Loostream")],
                "place {place}"
            );
            // One segment plus its contexts and slack, whatever the page holds.
            assert!(
                widest <= length + length / 8 + CONTEXT + 64 + 32,
                "read {widest} chars around a {length}-char dictation"
            );
        }
    }

    #[test]
    fn a_dictation_over_the_limit_is_not_watched_and_says_so() {
        let inserted = "Loustream. ".repeat(MAX_INSERTED_CHARS / 10);
        let (found, widest) =
            run_case_with(&inserted, &inserted, inserted.chars().count(), |fake, _| {
                fake.set(&inserted.replacen("Loustream", "Loostream", 1), 3, Some(1));
                note_key_down();
                wait(600);
            });
        assert!(found.is_empty());
        assert_eq!(widest, 0, "a too long dictation is never read");
    }

    /// A web editor shows pasted blank lines as paragraphs and keeps double
    /// spaces as no-break spaces; the receipt must still hold.
    #[test]
    fn paragraphs_and_no_break_spaces_still_anchor() {
        let inserted = "Premier paragraphe.\n\nDeuxième  paragraphe avec Loustream.";
        let shown = "Premier paragraphe.\nDeuxième\u{a0} paragraphe avec Loustream.";
        let a = anchor(inserted, shown, "", true, true).unwrap();
        assert_eq!(
            locate(&a, &shown.replace("Loustream", "Loostream"), true, true).as_deref(),
            Some("Premier paragraphe.\nDeuxième  paragraphe avec Loostream.")
        );
    }

    #[test]
    fn a_pasted_dictation_is_not_waited_for_as_if_typed() {
        let long = "x".repeat(crate::win32::MAX_TYPED_CHARS + 1);
        assert_eq!(anchor_wait(&long), ANCHOR_WAIT);
        assert!(anchor_wait("court") > ANCHOR_WAIT);
    }

    #[test]
    fn the_watcher_learns_a_stable_fix_and_stops_on_focus_change() {
        let found = run_case(INSERTED, |fake, _| {
            fake.set(FIXED, 18, Some(1));
            note_key_down();
            wait(1200);
            // Another field: the watch is over, edits there are not ours.
            fake.set("Je vais ouvrir Lxxstream.", 18, Some(2));
            wait(700);
        });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].after, FIXED);
    }

    /// The Windows 11 Notepad shows typed text ~31 ms per character late: the
    /// receipt must wait for it rather than give the field up.
    #[test]
    fn a_slow_target_is_still_anchored() {
        let found = run_case("", |fake, _| {
            wait(1700);
            fake.set(INSERTED, 25, Some(1));
            wait(300);
            fake.set(FIXED, 25, Some(1));
            note_key_down();
            wait(1200);
        });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].after, FIXED);
    }

    /// ChatGPT's composer replaces the text node on a key press: a read that
    /// lands then finds a stale range. The watch goes on; one that never
    /// settles ends it.
    #[test]
    fn a_field_mid_edit_is_read_again_not_given_up() {
        let found = run_case(INSERTED, |fake, _| {
            fake.changing_for(2);
            fake.set(FIXED, 18, Some(1));
            note_key_down();
            wait(2400);
        });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].after, FIXED);
        let never = run_case(INSERTED, |fake, _| {
            fake.changing_for(u32::MAX);
            fake.set(FIXED, 18, Some(1));
            note_key_down();
            wait(2400);
        });
        assert!(never.is_empty(), "nothing was ever read after the edit");
    }

    #[test]
    fn nothing_is_learned_without_a_receipt_an_edit_or_a_survivor() {
        // The text never landed (clipboard fallback, blocked paste).
        let never = run_case("autre chose", |fake, _| {
            wait(1800);
            fake.set(FIXED, 18, Some(1));
            note_key_down();
            wait(1200);
        });
        assert!(never.is_empty());
        // Fixed, then undone.
        let undone = run_case(INSERTED, |fake, _| {
            fake.set(FIXED, 18, Some(1));
            note_key_down();
            wait(1200);
            fake.set(INSERTED, 25, Some(1));
            wait(1200);
        });
        assert!(undone.is_empty());
        // The option turned off (or the dictation deleted) mid-watch.
        let discarded = run_case(INSERTED, |fake, tx| {
            fake.set(FIXED, 18, Some(1));
            note_key_down();
            wait(1200);
            tx.send(Command::Stop { learn: false }).unwrap();
            wait(100);
        });
        assert!(discarded.is_empty());
    }
}
