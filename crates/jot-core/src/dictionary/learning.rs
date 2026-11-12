//! Conservative local vocabulary suggestions, never active before human review.
use crate::history::Correction;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use unicode_segmentation::UnicodeSegmentation;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LearningSource {
    pub dictation_id: String,
    #[serde(with = "time::serde::rfc3339")]
    pub corrected_at: OffsetDateTime,
    /// Absent in files written before 1.6: those all came from History.
    #[serde(default, skip_serializing_if = "LearningOrigin::is_history")]
    pub origin: LearningOrigin,
}

/// Where the correction was made.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LearningOrigin {
    /// Edited in Jot's History.
    #[default]
    History,
    /// Fixed in the application right after Jot inserted the dictation.
    Application,
}

impl LearningOrigin {
    fn is_history(&self) -> bool {
        *self == Self::History
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SuggestionStatus {
    Pending,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DictionarySuggestion {
    pub id: Uuid,
    pub wrong: String,
    pub term: String,
    pub sources: Vec<LearningSource>,
    pub status: SuggestionStatus,
    pub confidence: u8,
    /// Offer `term` as vocabulary only, never as a `wrong → term` rule.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub term_only: bool,
}

/// Token alignment is deliberately bounded; a rewrite is not vocabulary evidence.
/// Confidence is an explainable spelling-similarity heuristic, not a probability.
pub fn extract_suggestions(correction: &Correction) -> Vec<DictionarySuggestion> {
    let before: Vec<_> = correction
        .before
        .unicode_word_indices()
        .take(4097)
        .collect();
    let after: Vec<_> = correction.after.unicode_word_indices().take(4097).collect();
    if before.len() > 4096 || after.len() > 4096 || before.is_empty() || after.is_empty() {
        return Vec::new();
    }
    let same = |i: usize, j: usize| before[i].1.to_lowercase() == after[j].1.to_lowercase();
    let mut prefix = 0;
    while prefix < before.len().min(after.len()) && same(prefix, prefix) {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < (before.len() - prefix).min(after.len() - prefix)
        && same(before.len() - 1 - suffix, after.len() - 1 - suffix)
    {
        suffix += 1;
    }
    let n = before.len() - prefix - suffix;
    let m = after.len() - prefix - suffix;
    // shortcut: skip a changed region over 24 words (and texts over 4096 words), reconsider with a bounded local aligner if users need longer rewrites.
    if n == 0 || m == 0 || n > 24 || m > 24 {
        return Vec::new();
    }
    let mut lcs = vec![vec![0_usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if same(prefix + i, prefix + j) {
                1 + lcs[i + 1][j + 1]
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let common = prefix + suffix + lcs[0][0];
    if before.len().max(after.len()) > 3 && common * 2 < before.len().max(after.len()) {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && same(prefix + i, prefix + j) {
            i += 1;
            j += 1;
            continue;
        }
        let (start_i, start_j) = (i, j);
        while i < n || j < m {
            if i < n && j < m && same(prefix + i, prefix + j) {
                break;
            }
            if j == m || (i < n && lcs[i + 1][j] >= lcs[i][j + 1]) {
                i += 1;
            } else {
                j += 1;
            }
        }
        ranges.push((prefix + start_i, prefix + i, prefix + start_j, prefix + j));
    }
    if ranges.len() > 3 {
        return Vec::new();
    }
    let mut suggestions = Vec::new();
    for (bi, mut be, ai, mut ae) in ranges {
        if bi == be || ai == ae || be - bi > 3 || ae - ai > 3 {
            continue;
        }
        let wrong = &correction.before[before[bi].0..before[be - 1].0 + before[be - 1].1.len()];
        let right = &correction.after[after[ai].0..after[ae - 1].0 + after[ae - 1].1.len()];
        let Some(confidence) = spelling_confidence(wrong, right) else {
            continue;
        };
        // Keep a shared proper-name suffix ("Klod Code"), instead of a broad "Klod" rule.
        while be < before.len()
            && ae < after.len()
            && be - bi < 3
            && ae - ai < 3
            && same(be, ae)
            && before[be].1.chars().next().is_some_and(char::is_uppercase)
            && correction.before[before[be - 1].0 + before[be - 1].1.len()..before[be].0]
                .chars()
                .all(char::is_whitespace)
            && correction.after[after[ae - 1].0 + after[ae - 1].1.len()..after[ae].0]
                .chars()
                .all(char::is_whitespace)
        {
            be += 1;
            ae += 1;
        }
        let wrong =
            correction.before[before[bi].0..before[be - 1].0 + before[be - 1].1.len()].to_string();
        let term =
            correction.after[after[ai].0..after[ae - 1].0 + after[ae - 1].1.len()].to_string();
        if wrong.chars().count() > 60 || term.chars().count() > 60 {
            continue;
        }
        suggestions.push(DictionarySuggestion {
            id: Uuid::new_v4(),
            wrong,
            term,
            sources: vec![LearningSource {
                dictation_id: correction.dictation_id.clone(),
                corrected_at: correction.corrected_at,
                origin: LearningOrigin::History,
            }],
            status: SuggestionStatus::Pending,
            confidence,
            term_only: false,
        });
    }
    suggestions
}

/// Names the user typed in place of what the recognizer misheard ("les mieux"
/// → "Lumio"). Too far from the heard words for [`extract_suggestions`], and a
/// "les mieux → Lumio" rule would rewrite ordinary French: they are only
/// offered as vocabulary terms, which the models get as a spelling reference.
pub fn new_terms(correction: &Correction) -> Vec<String> {
    let heard: std::collections::HashSet<String> = correction
        .before
        .unicode_words()
        .map(str::to_lowercase)
        .collect();
    let mut terms: Vec<String> = Vec::new();
    for word in correction.after.unicode_words() {
        let mut chars = word.chars();
        let name_like = word.chars().count() >= 3
            && (chars.next().is_some_and(char::is_uppercase)
                || chars.any(|c| c.is_uppercase() || c.is_numeric()));
        if name_like
            && !heard.contains(&word.to_lowercase())
            && !terms
                .iter()
                .any(|t| t.to_lowercase() == word.to_lowercase())
        {
            terms.push(word.to_string());
        }
    }
    // More than a few new names is a rewrite, not a misheard term.
    if terms.len() > 3 { Vec::new() } else { terms }
}

/// Words whose replacement changes what a sentence means (negation,
/// quantity, condition): never part of a rule learned outside History.
const MEANING_WORDS: &[&str] = &[
    "ne", "n", "pas", "non", "nom", "jamais", "rien", "aucun", "aucune", "sans", "et", "est", "ou",
    "où", "si", "un", "une", "le", "la", "les", "des", "plus", "moins", "oui", "not", "no",
    "never", "and", "or", "if", "a", "an", "the",
];

/// Suggestions from a correction made in another application. Stricter than
/// History: the edit was seen, not reviewed, so a rule is offered only for a
/// lexical fix that cannot change meaning; anything else at most as a term.
pub fn app_suggestions(correction: &Correction) -> Vec<DictionarySuggestion> {
    let risky = |text: &str| {
        text.chars().any(|c| c.is_numeric())
            || text
                .unicode_words()
                .any(|w| MEANING_WORDS.contains(&w.to_lowercase().as_str()))
    };
    let name_like = |text: &str| text.chars().any(|c| c.is_uppercase() || c.is_numeric());
    let mut found: Vec<_> = extract_suggestions(correction)
        .into_iter()
        .filter(|s| !risky(&s.wrong) && !risky(&s.term))
        // "cafe → café" is fine in History; seen in passing, a short common
        // word is too likely to be a rewrite.
        .filter(|s| name_like(&s.term) || s.term.chars().count() >= 5)
        .collect();
    if found.is_empty()
        && let Some((before, after)) = changed_words(correction)
        && !risky(&before)
    {
        // "unburn → Unbound": no spelling evidence, but a typed-in name.
        let region = Correction {
            before,
            after,
            ..correction.clone()
        };
        for term in new_terms(&region) {
            if term.chars().any(|c| c.is_numeric()) {
                continue;
            }
            found.push(DictionarySuggestion {
                id: Uuid::new_v4(),
                wrong: region.before.clone(),
                term,
                sources: Vec::new(),
                status: SuggestionStatus::Pending,
                confidence: 0,
                term_only: true,
            });
        }
    }
    for suggestion in &mut found {
        suggestion.sources = vec![LearningSource {
            dictation_id: correction.dictation_id.clone(),
            corrected_at: correction.corrected_at,
            origin: LearningOrigin::Application,
        }];
    }
    found
}

/// The one replaced run of words, both sides non-empty and short: additions
/// at the end, deletions and rewrites return None.
fn changed_words(correction: &Correction) -> Option<(String, String)> {
    let before: Vec<&str> = correction.before.unicode_words().collect();
    let after: Vec<&str> = correction.after.unicode_words().collect();
    let same = |a: &str, b: &str| a.to_lowercase() == b.to_lowercase();
    let prefix = before
        .iter()
        .zip(&after)
        .take_while(|(a, b)| same(a, b))
        .count();
    let suffix = before[prefix..]
        .iter()
        .rev()
        .zip(after[prefix..].iter().rev())
        .take_while(|(a, b)| same(a, b))
        .count();
    let wrong = &before[prefix..before.len() - suffix];
    let right = &after[prefix..after.len() - suffix];
    (!wrong.is_empty() && !right.is_empty() && wrong.len() <= 3 && right.len() <= 3)
        .then(|| (wrong.join(" "), right.join(" ")))
}

/// "LOostream": two capitals then lower case, the trace of a slipped Shift.
/// Shown as a warning, never fixed: "OAuth" looks the same.
pub fn suspicious_case(term: &str) -> bool {
    term.unicode_words().any(|word| {
        let chars: Vec<char> = word.chars().collect();
        chars.len() >= 4
            && chars[0].is_uppercase()
            && chars[1].is_uppercase()
            && chars[2..].iter().all(|c| c.is_lowercase())
    })
}

fn spelling_confidence(wrong: &str, right: &str) -> Option<u8> {
    if wrong
        .chars()
        .chain(right.chars())
        .any(|c| !(c.is_alphabetic() || c.is_numeric() || c.is_whitespace() || is_mark(c)))
    {
        return None;
    }
    let wrong = wrong.to_lowercase();
    let right = right.to_lowercase();
    if wrong == right {
        return None;
    }
    let left: Vec<_> = wrong
        .graphemes(true)
        .filter(|s| !s.trim().is_empty())
        .collect();
    let target: Vec<_> = right
        .graphemes(true)
        .filter(|s| !s.trim().is_empty())
        .collect();
    let length = left.len().max(target.len());
    if left.len().min(target.len()) < 3 || length > 60 {
        return None;
    }
    for prefix in ["in", "un", "non", "dé", "dés", "dis", "anti"] {
        if wrong.strip_prefix(prefix) == Some(right.as_str())
            || right.strip_prefix(prefix) == Some(wrong.as_str())
        {
            return None;
        }
    }
    let mut row: Vec<usize> = (0..=target.len()).collect();
    for (i, a) in left.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, b) in target.iter().enumerate() {
            let prior = row[j + 1];
            row[j + 1] = (row[j] + 1)
                .min(prior + 1)
                .min(diagonal + usize::from(a != b));
            diagonal = prior;
        }
    }
    let distance = row[target.len()];
    if distance <= (length / 3).max(1) {
        return Some((100 - 100 * distance / length) as u8);
    }
    let skeleton = |text: &str| {
        text.chars()
            .filter(|c| !"aeiouyàâäéèêëîïôöùûüÿ".contains(*c) && !c.is_whitespace() && !is_mark(*c))
            .map(|c| if c == 'c' { 'k' } else { c })
            .collect::<String>()
    };
    let skeleton_left = skeleton(&wrong);
    (skeleton_left.len() >= 3 && skeleton_left == skeleton(&right)).then_some(60)
}

fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036f | 0x1ab0..=0x1aff | 0x1dc0..=0x1dff | 0x20d0..=0x20ff | 0xfe20..=0xfe2f)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn correction(before: &str, after: &str) -> Correction {
        Correction {
            dictation_id: "synthetic".into(),
            before: before.into(),
            after: after.into(),
            corrected_at: OffsetDateTime::UNIX_EPOCH,
        }
    }
    #[test]
    fn isolated_and_compound_lexical_changes() {
        let one = extract_suggestions(&correction(
            "J’ai installé Proxmocks.",
            "J’ai installé Proxmox.",
        ));
        assert_eq!(one.len(), 1);
        assert_eq!((&*one[0].wrong, &*one[0].term), ("Proxmocks", "Proxmox"));
        let compound = extract_suggestions(&correction(
            "J’utilise Klod Code chaque jour.",
            "J’utilise Claude Code chaque jour.",
        ));
        assert_eq!(compound.len(), 1);
        assert_eq!(
            (&*compound[0].wrong, &*compound[0].term),
            ("Klod Code", "Claude Code")
        );
        assert_eq!(
            extract_suggestions(&correction("Je vais au cafe.", "Je vais au café.")).len(),
            1
        );
    }
    #[test]
    fn rejects_rewrites_meaning_changes_and_nonlexical_edits() {
        for (before, after) in [
            ("Je viens demain matin.", "Finalement rendez-vous vendredi."),
            ("Active ce service", "Inactive ce service"),
            ("Le serveur fonctionne", "Le service fonctionne"),
            ("hello", "HELLO"),
            ("Oui.", "Oui !"),
            ("une chose", "une belle chose"),
            ("une belle chose", "une chose"),
            ("identique", "identique"),
        ] {
            assert!(
                extract_suggestions(&correction(before, after)).is_empty(),
                "{before} -> {after}"
            );
        }
    }
    /// The reported case: a phonetic mishearing is no spelling evidence, so no
    /// replacement rule is suggested, only the term.
    #[test]
    fn a_misheard_name_is_offered_as_a_term_never_as_a_rule() {
        let lumio = correction(
            "J’ai testé les mieux hier soir.",
            "J’ai testé Lumio hier soir.",
        );
        assert!(extract_suggestions(&lumio).is_empty());
        assert_eq!(new_terms(&lumio), ["Lumio"]);
        let proxmox = correction("J’ai installé Proxmocks.", "J’ai installé Proxmox.");
        assert_eq!(extract_suggestions(&proxmox).len(), 1);
        assert_eq!(new_terms(&proxmox), ["Proxmox"]);
        for (before, after) in [
            ("le contrôle qualité", "le contrôle qualité est fait"),
            ("Bonjour Claire", "bonjour claire"),
            ("un deux trois", "Alpha Beta Gamma Delta"),
        ] {
            assert!(new_terms(&correction(before, after)).is_empty(), "{after}");
        }
        assert_eq!(
            new_terms(&correction("appuie sur contrôle", "appuie sur Ctrl")),
            ["Ctrl"]
        );
    }

    #[test]
    fn multiple_spelling_changes_and_unicode_are_separate() {
        let found = extract_suggestions(&correction(
            "Proxmocks utilise un cafe.",
            "Proxmox utilise un café.",
        ));
        assert_eq!(found.len(), 2);
        assert!(
            extract_suggestions(&correction("Le résume est ici.", "Le résumé est ici.")).len() <= 1
        );
        assert!(extract_suggestions(&correction("🐈 hello", "🐈 HELLO")).is_empty());
    }

    #[test]
    fn app_corrections_are_stricter_than_history() {
        let pairs = |before: &str, after: &str| -> Vec<(String, String, bool)> {
            app_suggestions(&correction(before, after))
                .into_iter()
                .map(|s| (s.wrong, s.term, s.term_only))
                .collect()
        };
        let rule = |w: &str, t: &str| vec![(w.to_string(), t.to_string(), false)];
        assert_eq!(
            pairs("Je vais ouvrir Loustream.", "Je vais ouvrir Loostream."),
            rule("Loustream", "Loostream")
        );
        assert_eq!(
            pairs("Le projet jott avance.", "Le projet Jot avance."),
            rule("jott", "Jot")
        );
        // Proposed as typed, flagged, never repaired.
        assert_eq!(
            pairs("ouvrir Loustream", "ouvrir LOostream"),
            rule("Loustream", "LOostream")
        );
        assert!(suspicious_case("LOostream") && !suspicious_case("Loostream"));
        assert!(!suspicious_case("PowerShell") && !suspicious_case("gRPC"));
        assert!(!suspicious_case("API"));
        // No spelling evidence: the name only.
        assert_eq!(
            pairs("Le service unburn répond.", "Le service Unbound répond."),
            vec![("unburn".into(), "Unbound".into(), true)]
        );
        // Accents in a multi-line text.
        assert_eq!(
            pairs(
                "Le réseau\nKubernetès et PowerShell",
                "Le réseau\nKubernetes et PowerShell"
            ),
            rule("Kubernetès", "Kubernetes")
        );
        for (before, after) in [
            ("Il est là.", "Il et là."),
            ("un serveur", "Unbound serveur"),
            ("Mon non est Paul.", "Mon nom est Paul."),
            ("Le service ne répond pas.", "Le service répond."),
            ("Il faut 20 serveurs.", "Il faut 30 serveurs."),
            ("Prix vingt euros.", "Prix trente euros."),
            ("Ping 10.0.0.1 ok.", "Ping 10.0.0.2 ok."),
            ("si le service tombe", "quand le service tombe"),
            ("Je vais au cafe.", "Je vais au café."),
            (
                "Je vais ouvrir Loustream.",
                "Je vais ouvrir Loustream. Puis Jira et Confluence.",
            ),
            (
                "Je vais ouvrir Loustream.",
                "Finalement on annule tout demain matin.",
            ),
            ("Je vais ouvrir Loustream.", "Je vais ouvrir Loustream !"),
            ("Je vais ouvrir Loustream.", ""),
        ] {
            assert!(pairs(before, after).is_empty(), "{before} -> {after}");
        }
    }
}
