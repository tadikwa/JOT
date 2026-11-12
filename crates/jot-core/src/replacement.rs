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

//! Deterministic post-model replacement layer: the dictionary's guarantee.
//!
//! The cleanup prompt *suggests* spellings to the model; this layer *enforces*
//! the explicit wrong→right rules afterward. Longest-match-first, word-boundary,
//! case-preserving (ALL-CAPS / Title / lower propagation).

use regex::RegexBuilder;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub wrong: String,
    pub right: String,
}

impl Rule {
    pub fn new(wrong: impl Into<String>, right: impl Into<String>) -> Self {
        Self {
            wrong: wrong.into(),
            right: right.into(),
        }
    }
}

pub fn apply(rules: &[Rule], text: &str) -> String {
    if rules.is_empty() {
        return text.to_string();
    }
    // Longest wrong-form first so "gemini api" wins over "gemini".
    let mut ordered: Vec<&Rule> = rules.iter().filter(|rule| !rule.wrong.is_empty()).collect();
    ordered.sort_by_key(|rule| std::cmp::Reverse(rule.wrong.chars().count()));

    // Match only the original text: generated replacements cannot trigger another rule.
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    for rule in ordered {
        // ' and ’ are the same apostrophe to the user: a rule learned with one matches both.
        let pattern: String = regex::escape(&rule.wrong)
            .chars()
            .map(|c| match c {
                '\'' | '’' => "['’]".to_string(),
                c => c.to_string(),
            })
            .collect();
        let Ok(regex) = RegexBuilder::new(&pattern).case_insensitive(true).build() else {
            continue;
        };
        for found in regex.find_iter(text) {
            if !is_standalone(text, found.start(), found.end())
                || edits
                    .iter()
                    .any(|(range, _)| found.start() < range.end && range.start < found.end())
            {
                continue;
            }
            edits.push((
                found.range(),
                propagate_case(found.as_str(), &rule.right, &rule.wrong),
            ));
        }
    }
    edits.sort_by_key(|(range, _)| range.start);
    let mut result = text.to_string();
    for (range, replacement) in edits.into_iter().rev() {
        result.replace_range(range, &replacement);
    }

    result
}

/// French elides only before a vowel or a mute h, so a term's own first
/// consonant elided in front of it ("l'Loostream") is never French: the
/// recognizer heard that consonant as an article. "n'" is left out: dropping it
/// could drop a negation.
pub(crate) fn elision_letter(term: &str) -> Option<char> {
    let first = term.chars().next()?.to_lowercase().next()?;
    "cdjlmst".contains(first).then_some(first)
}

/// "l'Loostream" → "Loostream", only for the given dictionary terms. Runs after
/// [`apply`], on its output, because a rule or the model may be what put the
/// term behind the stray elision. Every other apostrophe is left as it is.
pub fn repair_elisions<'a>(terms: impl IntoIterator<Item = &'a str>, text: &str) -> String {
    let rules: Vec<Rule> = terms
        .into_iter()
        .filter_map(|term| Some(Rule::new(format!("{}'{term}", elision_letter(term)?), term)))
        .collect();
    apply(&rules, text)
}

/// True when the match is not glued to a word character on either side.
fn is_standalone(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
}

fn is_word_char(c: char) -> bool {
    static WORD: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\A[\p{L}\p{M}\p{N}\p{Pc}\u{200c}\u{200d}]\z")
            .expect("constant Unicode word regex")
    });
    let mut buffer = [0; 4];
    WORD.is_match(c.encode_utf8(&mut buffer))
}

/// "KUBERNETES"→"GRPC" stays caps; "Kubernetes"→"GRPC"… follows the rule's
/// canonical casing unless the match was ALL-CAPS or the rule carries EXPLICIT
/// casing. A rule is explicitly cased when its right side contains uppercase
/// (gRPC, iPhone) — or when its WRONG side does ("NPM"→"npm" is a deliberate
/// lowercase rule; ALL-CAPS propagation would silently undo it).
pub fn propagate_case(original: &str, replacement: &str, wrong: &str) -> String {
    let has_explicit_casing = replacement.chars().any(char::is_uppercase)
        || wrong.chars().any(char::is_uppercase)
        || (!wrong.is_empty() && wrong.to_lowercase() == replacement.to_lowercase());
    if has_explicit_casing {
        // The dictionary term carries its own casing (gRPC, iPhone).
        return replacement.to_string();
    }
    if original == original.to_uppercase() && original.chars().count() > 1 {
        return replacement.to_uppercase();
    }
    if original.chars().next().is_some_and(char::is_uppercase) {
        let mut chars = replacement.chars();
        return match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        };
    }
    replacement.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacements_do_not_cascade_or_rewrite_canonical_terms() {
        assert_eq!(
            apply(
                &[Rule::new("alpha", "beta"), Rule::new("beta", "gamma")],
                "alpha beta"
            ),
            "beta gamma"
        );
    }

    #[test]
    fn combining_marks_are_word_boundaries_only_with_their_base() {
        assert_eq!(
            apply(&[Rule::new("cafe", "coffee")], "cafe\u{301} cafe"),
            "cafe\u{301} coffee"
        );
    }

    #[test]
    fn empty_rules_pass_text_through() {
        assert_eq!(apply(&[], "unchanged"), "unchanged");
    }

    #[test]
    fn longest_wrong_form_wins() {
        let rules = [
            Rule::new("gemini", "Gemini"),
            Rule::new("gemini api", "Gemini API"),
        ];
        assert_eq!(apply(&rules, "the gemini api docs"), "the Gemini API docs");
    }

    #[test]
    fn matches_respect_word_boundaries() {
        let rules = [Rule::new("cat", "dog")];
        assert_eq!(apply(&rules, "concatenate the cat"), "concatenate the dog");
    }

    #[test]
    fn punctuated_wrong_forms_still_match() {
        // `\b` would never fire on either end of these.
        let rules = [Rule::new("c++", "C++"), Rule::new("e.g.", "for example")];
        assert_eq!(apply(&rules, "write c++ code"), "write C++ code");
        assert_eq!(apply(&rules, "e.g. this"), "for example this");
    }

    #[test]
    fn all_caps_propagates_when_the_rule_is_uncased() {
        assert_eq!(propagate_case("HELLO", "goodbye", "hello"), "GOODBYE");
    }

    #[test]
    fn title_case_propagates_when_the_rule_is_uncased() {
        assert_eq!(propagate_case("Hello", "goodbye", "hello"), "Goodbye");
    }

    #[test]
    fn explicit_rule_casing_beats_propagation() {
        assert_eq!(
            propagate_case("COOPER NETTIES", "gRPC", "cooper netties"),
            "gRPC"
        );
        // A deliberate lowercase rule must not be undone by ALL-CAPS input.
        assert_eq!(propagate_case("NPM", "npm", "NPM"), "npm");
    }

    #[test]
    fn single_uppercase_letter_is_not_treated_as_all_caps() {
        assert_eq!(propagate_case("A", "b", "a"), "B");
    }

    #[test]
    fn replacements_apply_case_insensitively_across_a_sentence() {
        let rules = [Rule::new("cooper netties", "Kubernetes")];
        assert_eq!(
            apply(&rules, "Cooper netties and cooper netties."),
            "Kubernetes and Kubernetes."
        );
    }

    #[test]
    fn rules_match_either_apostrophe() {
        let rules = [Rule::new("l'oustream", "Loostream")];
        assert_eq!(apply(&rules, "Ouvre l’oustream."), "Ouvre Loostream.");
        assert_eq!(apply(&rules, "de l'oustream"), "de Loostream");
        assert_eq!(apply(&rules, "l'oustreamer"), "l'oustreamer");
    }

    #[test]
    fn stray_elisions_before_a_term_are_repaired() {
        let fix = |text| repair_elisions(["Loostream", "Docker", "jot"], text);
        assert_eq!(
            fix("Ouvre l'Loostream maintenant."),
            "Ouvre Loostream maintenant."
        );
        assert_eq!(fix("L’Loostream est prêt."), "Loostream est prêt.");
        assert_eq!(fix("de l'Loostream"), "de Loostream");
        assert_eq!(fix("l'image d'Docker"), "l'image Docker");
        assert_eq!(fix("J'jot vite"), "Jot vite");
    }

    #[test]
    fn genuine_elisions_and_other_words_are_left_alone() {
        let fix = |text| repair_elisions(["Loostream", "Ollama", "Netflix", "Lyon"], text);
        for text in [
            "l'application, l'heure, l'ours, l'ostéopathe et la loutre.",
            "Lance l'Ollama local.",
            "Je n'Netflix pas.",
            "l'Loostreamer et xl'Loostream",
            "Je vais à Lyon, l'ion est chargé.",
            "Le streaming de Loostream.",
        ] {
            assert_eq!(fix(text), text);
        }
        assert_eq!(repair_elisions([], "l'Loostream"), "l'Loostream");
    }

    #[test]
    fn multibyte_text_is_not_corrupted() {
        let rules = [Rule::new("cafe", "café")];
        assert_eq!(apply(&rules, "a cafe — naïve"), "a café — naïve");
    }
}
