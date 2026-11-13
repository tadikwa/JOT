//! Partial rescue of a long local rewrite that the safety check refused or
//! that ran out of time. Experimental and off by default (Settings).
//!
//! The dictation is rewritten again piece by piece, cut only between sentences
//! where no meaning straddles the cut. A piece that still fails gets one finer
//! retry, then stays raw. The joined text goes through the global check again.
//! Nothing here logs dictated text: only the optional local diagnostic keeps it.

use crate::local::reformulation_verdict;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// Below this the global rewrite is all there is.
pub const MIN_WORDS: usize = 60;
/// Pieces grow sentence by sentence up to about this many words.
const TARGET_WORDS: usize = 45;
/// A run with no safe cut longer than this is not sent: it stays raw.
const MAX_PIECE_WORDS: usize = 120;
/// Read-only context given with a piece: the end of the one before.
const CONTEXT_WORDS: usize = 25;
/// Model calls per dictation, finer retries included.
pub const MAX_CALLS: usize = 8;
/// Wall-clock cap per dictation, on top of the refused global attempt.
pub const TIME_CAP: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Every piece rewritten.
    Complete,
    /// Some pieces rewritten, the others raw.
    Partial,
    /// Nothing usable: the raw transcript is inserted, as before.
    Raw,
}

/// One unit of the final text, for the local diagnostic only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PieceReport {
    pub raw: String,
    /// "rewritten", or why it stayed raw: "no_safe_cut", "budget", "timeout",
    /// "error", "context_echo" or "refused/<check>".
    pub outcome: String,
    /// The model's text, kept or refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed: Option<String>,
    /// Second, finer attempt (half of a refused piece).
    #[serde(default)]
    pub finer: bool,
    pub ms: u64,
}

#[derive(Debug, Clone)]
pub struct Rescue {
    pub text: String,
    pub status: Status,
    pub rewritten: usize,
    pub calls: usize,
    pub pieces: Vec<PieceReport>,
    /// The joined text failed the global check (everything went back to raw).
    pub global_refusal: Option<String>,
}

impl Rescue {
    /// Stored after `:` in the History's `model_id`: "rescued/3-of-3",
    /// "partial/2-of-3". `None` when the raw transcript is inserted.
    pub fn label(&self) -> Option<String> {
        let kind = match self.status {
            Status::Complete => "rescued",
            Status::Partial => "partial",
            Status::Raw => return None,
        };
        Some(format!(
            "{kind}/{}-of-{}",
            self.rewritten,
            self.pieces.len()
        ))
    }
}

pub(crate) fn words(text: &str) -> usize {
    text.split_whitespace().count()
}

/// A sentence that refers back to the one before it: correction ("Non,
/// plutôt…"), consequence or alternative ("Alors…", "Sinon…"), restriction.
static BACK_REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^[«“\x22\s-]*(non|pas|ni|jamais|plutôt|enfin|pardon|sinon|alors|dans ce cas|sauf|je veux dire|c['’]est-à-dire|autrement dit|ou|no|not|rather|or|otherwise|then|unless|i mean)\b",
    )
    .unwrap()
});

/// Short words that end with a dot without ending a sentence.
const ABBREVIATIONS: &[&str] = &[
    "mme", "mlle", "dr", "pr", "st", "etc", "cf", "ex", "vs", "mr", "mrs", "no",
];

/// Sentences of `text`, cut only where the meaning does not straddle the cut:
/// after `.`, `!`, `?` or `…` followed by a space and a capital, never inside
/// quotes, a number, an address or a path (no space after their dots), after
/// an initial or an abbreviation, or before a sentence that refers back.
pub fn sentences(text: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut start = 0;
    let mut guillemets = 0i32;
    let mut straight = false;
    for (i, &(at, c)) in chars.iter().enumerate() {
        match c {
            '«' | '“' => guillemets += 1,
            '»' | '”' => guillemets = (guillemets - 1).max(0),
            '"' => straight = !straight,
            _ => {}
        }
        if !matches!(c, '.' | '!' | '?' | '…') || guillemets > 0 || straight {
            continue;
        }
        let Some(&(end, next)) = chars.get(i + 1) else {
            continue;
        };
        if !next.is_whitespace() {
            continue;
        }
        let last_word = text[start..at]
            .rsplit(char::is_whitespace)
            .next()
            .unwrap_or("")
            .trim_matches(|c: char| !c.is_alphanumeric());
        if c == '.'
            && (last_word.chars().count() == 1
                || ABBREVIATIONS.contains(&last_word.to_lowercase().as_str()))
        {
            continue;
        }
        let rest = text[end..].trim_start();
        let capital = rest
            .trim_start_matches(['«', '“', '"', '-', ' '])
            .chars()
            .next()
            .is_some_and(|c| c.is_uppercase() || c.is_ascii_digit());
        if !capital || BACK_REFERENCE.is_match(rest) {
            continue;
        }
        out.push(text[start..end].trim());
        start = end;
    }
    out.push(text[start..].trim());
    out.retain(|s| !s.is_empty());
    out
}

/// Pieces of about [`TARGET_WORDS`], each with whether it may be sent: a run
/// longer than [`MAX_PIECE_WORDS`] without a safe cut stays raw.
pub fn pieces(raw: &str) -> Vec<(String, bool)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    let mut current = String::new();
    let mut count = 0;
    for sentence in sentences(raw) {
        let n = words(sentence);
        if count > 0 && count + n > MAX_PIECE_WORDS {
            out.push((std::mem::take(&mut current), count));
            count = 0;
        }
        if count > 0 {
            current.push(' ');
        }
        current.push_str(sentence);
        count += n;
        if count >= TARGET_WORDS {
            out.push((std::mem::take(&mut current), count));
            count = 0;
        }
    }
    if count > 0 {
        // A short tail joins the piece before it when that stays sendable.
        match out.last_mut() {
            Some((last, last_count))
                if count < TARGET_WORDS / 3 && *last_count + count <= MAX_PIECE_WORDS =>
            {
                last.push(' ');
                last.push_str(&current);
                *last_count += count;
            }
            _ => out.push((current, count)),
        }
    }
    out.into_iter()
        .map(|(text, n)| (text, n <= MAX_PIECE_WORDS))
        .collect()
}

fn tail(text: &str, n: usize) -> String {
    let all: Vec<&str> = text.split_whitespace().collect();
    all[all.len().saturating_sub(n)..].join(" ")
}

/// Two halves of a piece at the sentence nearest its middle, if it has two.
fn halves(piece: &str) -> Option<(String, String)> {
    let parts = sentences(piece);
    if parts.len() < 2 {
        return None;
    }
    let total = words(piece);
    let mut seen = 0;
    let mut split = 1;
    for (i, part) in parts.iter().enumerate().take(parts.len() - 1) {
        seen += words(part);
        split = i + 1;
        if seen * 2 >= total {
            break;
        }
    }
    Some((parts[..split].join(" "), parts[split..].join(" ")))
}

struct Budget {
    deadline: tokio::time::Instant,
    calls: usize,
    words_left: usize,
}

/// One model call on `text` and its check. `Err((outcome, proposed))`.
async fn attempt<F, Fut>(
    call: &mut F,
    budget: &mut Budget,
    text: &str,
    context: Option<String>,
) -> Result<String, (String, Option<String>)>
where
    F: FnMut(String, Option<String>) -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    let n = words(text);
    if budget.calls >= MAX_CALLS
        || n > budget.words_left
        || tokio::time::Instant::now() >= budget.deadline
    {
        return Err(("budget".into(), None));
    }
    budget.calls += 1;
    budget.words_left -= n;
    let echo = context.as_deref().map(|c| tail(c, 6));
    let reply = tokio::time::timeout_at(budget.deadline, call(text.to_string(), context)).await;
    let proposed = match reply {
        Err(_) => return Err(("budget".into(), None)),
        Ok(Err(class)) => return Err((class, None)),
        Ok(Ok(proposed)) => proposed,
    };
    // The context is there to be read, not written: a copy of it is an
    // addition, unless the piece itself says the same words.
    if let Some(echo) = echo.map(|e| e.to_lowercase()).filter(|e| words(e) >= 4)
        && proposed.to_lowercase().contains(&echo)
        && !text.to_lowercase().contains(&echo)
    {
        return Err(("context_echo".into(), Some(proposed)));
    }
    match reformulation_verdict(text, &proposed) {
        Ok(()) => Ok(proposed),
        Err(reason) => Err((
            format!("refused/{}", reason.replace(' ', "_")),
            Some(proposed),
        )),
    }
}

/// Rewrites `raw` piece by piece with `call(piece, context)`, which returns
/// the model's cleaned text or a failure class. Bounded by [`MAX_CALLS`],
/// [`TIME_CAP`] and twice the dictation's words sent in all.
pub async fn rescue<F, Fut>(raw: &str, mut call: F) -> Rescue
where
    F: FnMut(String, Option<String>) -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    let mut budget = Budget {
        deadline: tokio::time::Instant::now() + TIME_CAP,
        calls: 0,
        words_left: 2 * words(raw),
    };
    let mut units: Vec<String> = Vec::new();
    let mut reports: Vec<PieceReport> = Vec::new();
    let mut rewritten = 0;
    let mut previous: Option<String> = None;
    for (piece, sendable) in pieces(raw) {
        let started = Instant::now();
        let context = previous.as_deref().map(|p| tail(p, CONTEXT_WORDS));
        previous = Some(piece.clone());
        if !sendable {
            reports.push(report(&piece, "no_safe_cut".into(), None, false, started));
            units.push(piece);
            continue;
        }
        let (outcome, proposed) =
            match attempt(&mut call, &mut budget, &piece, context.clone()).await {
                Ok(text) => {
                    reports.push(report(
                        &piece,
                        "rewritten".into(),
                        Some(text.clone()),
                        false,
                        started,
                    ));
                    units.push(text);
                    rewritten += 1;
                    continue;
                }
                Err(failure) => failure,
            };
        // One finer retry: the two halves, each on its own.
        let split = (outcome != "budget").then(|| halves(&piece)).flatten();
        let Some((first, second)) = split else {
            reports.push(report(&piece, outcome, proposed, false, started));
            units.push(piece);
            continue;
        };
        let mut context = context;
        for half in [first, second] {
            let started = Instant::now();
            match attempt(&mut call, &mut budget, &half, context.take()).await {
                Ok(text) => {
                    reports.push(report(
                        &half,
                        "rewritten".into(),
                        Some(text.clone()),
                        true,
                        started,
                    ));
                    units.push(text);
                    rewritten += 1;
                }
                Err((outcome, proposed)) => {
                    reports.push(report(&half, outcome, proposed, true, started));
                    units.push(half.clone());
                }
            }
            context = Some(tail(&half, CONTEXT_WORDS));
        }
    }

    let joined = units.join(" ");
    let mut rescue = Rescue {
        status: match rewritten {
            0 => Status::Raw,
            n if n == reports.len() => Status::Complete,
            _ => Status::Partial,
        },
        text: joined,
        rewritten,
        calls: budget.calls,
        pieces: reports,
        global_refusal: None,
    };
    if rescue.status != Status::Raw
        && let Err(reason) = reformulation_verdict(raw, &rescue.text)
    {
        rescue.global_refusal = Some(reason);
        rescue.status = Status::Raw;
    }
    if rescue.status == Status::Raw {
        rescue.text = raw.to_string();
    }
    rescue
}

fn report(
    raw: &str,
    outcome: String,
    proposed: Option<String>,
    finer: bool,
    started: Instant,
) -> PieceReport {
    PieceReport {
        raw: raw.to_string(),
        outcome,
        proposed,
        finer,
        ms: started.elapsed().as_millis() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn run<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn cuts_only_between_sentences_that_stand_alone() {
        let text = "Le serveur est en 192.168.1.10. Le fichier est C:\\Users\\bird\\notes.txt. \
            Il pèse 3.5 Go. Il a dit « Je pars. Tu restes. » hier. Envoie-le à M. Dupont. \
            On garde la version deux. Non, plutôt la trois. Si ça casse. Alors on revient en arrière. \
            Ce n'est pas urgent. Pas avant lundi.";
        assert_eq!(
            sentences(text),
            vec![
                "Le serveur est en 192.168.1.10.",
                "Le fichier est C:\\Users\\bird\\notes.txt.",
                "Il pèse 3.5 Go.",
                "Il a dit « Je pars. Tu restes. » hier.",
                "Envoie-le à M. Dupont.",
                "On garde la version deux. Non, plutôt la trois.",
                "Si ça casse. Alors on revient en arrière.",
                "Ce n'est pas urgent. Pas avant lundi.",
            ]
        );
    }

    #[test]
    fn a_long_run_without_a_safe_cut_stays_raw() {
        let run_on = "mot ".repeat(130);
        let text = format!("Une phrase courte pour commencer ici. {run_on}");
        let parts = pieces(&text);
        assert!(
            parts
                .iter()
                .any(|(p, sendable)| !sendable && words(p) > MAX_PIECE_WORDS)
        );
        assert!(
            parts
                .iter()
                .all(|(p, sendable)| *sendable == (words(p) <= MAX_PIECE_WORDS))
        );
        // Nothing is lost or reordered.
        let joined: Vec<String> = parts.into_iter().map(|(p, _)| p).collect();
        assert_eq!(words(&joined.join(" ")), words(&text));
    }

    fn sentence(i: usize) -> String {
        format!(
            "Euh la tâche numéro {i} avance bien et nous la terminerons demain matin sans faute."
        )
    }

    #[test]
    fn every_piece_rewritten_is_complete_and_gets_the_context() {
        let raw: String = (1..=8).map(sentence).collect::<Vec<_>>().join(" ");
        let contexts = Cell::new(0);
        let rescue = run(rescue(&raw, |piece, context| {
            if context.is_some() {
                contexts.set(contexts.get() + 1);
            }
            async move { Ok(piece.replace("Euh la", "La")) }
        }));
        assert_eq!(rescue.status, Status::Complete, "{rescue:?}");
        assert!(!rescue.text.contains("Euh"));
        assert_eq!(rescue.calls, rescue.pieces.len());
        assert_eq!(contexts.get(), rescue.pieces.len() - 1);
        assert_eq!(
            rescue.label(),
            Some(format!("rescued/{0}-of-{0}", rescue.pieces.len()))
        );
    }

    #[test]
    fn a_piece_that_drops_a_negation_stays_raw_after_one_finer_retry() {
        let mut parts: Vec<String> = (1..=8).map(sentence).collect();
        parts[1] =
            "Je ne veux pas que la tâche numéro deux parte en production avant la revue complète."
                .into();
        let raw = parts.join(" ");
        let rescue = run(rescue(&raw, |piece: String, _| async move {
            Ok(piece.replace("Euh la", "La").replace("ne veux pas", "veux"))
        }));
        assert_eq!(rescue.status, Status::Partial, "{rescue:?}");
        assert!(
            rescue
                .text
                .contains("Je ne veux pas que la tâche numéro deux")
        );
        let refused: Vec<_> = rescue
            .pieces
            .iter()
            .filter(|p| p.outcome.starts_with("refused/"))
            .collect();
        assert_eq!(refused.len(), 1);
        assert!(refused[0].finer);
        assert!(rescue.calls <= MAX_CALLS);
        assert!(rescue.label().unwrap().starts_with("partial/"));
    }

    #[test]
    fn a_failing_model_is_bounded_and_leaves_the_raw_text() {
        let raw: String = (1..=12).map(sentence).collect::<Vec<_>>().join(" ");
        let calls = Cell::new(0);
        let rescue = run(rescue(&raw, |_, _| {
            calls.set(calls.get() + 1);
            async { Err::<String, _>("timeout".to_string()) }
        }));
        assert_eq!(rescue.status, Status::Raw);
        assert_eq!(rescue.text, raw);
        assert_eq!(rescue.label(), None);
        assert!(calls.get() <= MAX_CALLS);
    }

    #[test]
    fn a_copy_of_the_context_is_refused() {
        let mut parts: Vec<String> = (1..=6).map(sentence).collect();
        parts[2] = "Le comité validera ensuite le budget prévu pour la seconde partie du grand projet européen.".into();
        let raw = parts.join(" ");
        let rescue = run(rescue(&raw, |piece, context| async move {
            Ok(match context {
                Some(context) => format!("{context} {piece}"),
                None => piece,
            })
        }));
        assert!(
            rescue.pieces.iter().any(|p| p.outcome == "context_echo"),
            "{rescue:?}"
        );
    }
}
