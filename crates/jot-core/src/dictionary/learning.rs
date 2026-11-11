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
            }],
            status: SuggestionStatus::Pending,
            confidence,
        });
    }
    suggestions
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
}
