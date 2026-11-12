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

//! The personal dictionary: terms (spelling hints fed to the recogniser and the
//! cleanup prompt) and explicit wrong→right rules (enforced deterministically
//! post-model).

mod learning;
pub use learning::{
    DictionarySuggestion, LearningOrigin, LearningSource, SuggestionStatus, app_suggestions,
    extract_suggestions, new_terms, suspicious_case,
};

/// Pending suggestions learned in another application expire after this.
pub const APP_SUGGESTION_RETENTION_DAYS: i64 = 30;

use crate::file_layout;
use crate::history::Correction;
use crate::replacement::Rule;
use anyhow::{Context, Result, ensure};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use time::OffsetDateTime;
use uuid::Uuid;

/// Terms outside this length are rejected on entry and on import.
pub const TERM_LENGTH: std::ops::RangeInclusive<usize> = 1..=60;
/// Ceiling on a single CSV import, so a pasted spreadsheet can't wedge the app.
pub const MAX_IMPORT_ROWS: usize = 1000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DictionaryEntry {
    pub id: Uuid,
    /// The correct term ("Kubernetes", "Ammaar", "gRPC"). 1–60 chars.
    pub term: String,
    /// Optional misspelling the model tends to produce ("cooper netties").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub misspelling: Option<String>,
    #[serde(default)]
    pub starred: bool,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<LearningSource>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl DictionaryEntry {
    pub fn new(term: impl Into<String>, misspelling: Option<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            term: term.into(),
            misspelling: misspelling.filter(|m| !m.is_empty()),
            starred: false,
            enabled: true,
            provenance: None,
            created_at: OffsetDateTime::now_utc(),
        }
    }
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DictionaryFile {
    version: u32,
    entries: Vec<DictionaryEntry>,
    #[serde(default)]
    suggestions: Vec<DictionarySuggestion>,
}

impl Default for DictionaryFile {
    fn default() -> Self {
        Self {
            version: 2,
            entries: Vec::new(),
            suggestions: Vec::new(),
        }
    }
}

pub struct DictionaryStore {
    path: PathBuf,
    state: RwLock<DictionaryFile>,
    load_error: Option<String>,
}

static GLOBAL: LazyLock<Arc<DictionaryStore>> =
    LazyLock::new(|| Arc::new(DictionaryStore::open(file_layout::dictionary_json())));

impl DictionaryStore {
    pub fn global() -> Arc<DictionaryStore> {
        GLOBAL.clone()
    }

    pub fn open(path: PathBuf) -> Self {
        let loaded = (|| -> Result<DictionaryFile> {
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(DictionaryFile::default());
                }
                Err(error) => return Err(error).context("cannot read dictionary"),
            };
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).context("invalid dictionary JSON")?;
            if value.is_array() {
                Ok(DictionaryFile {
                    entries: serde_json::from_value(value)?,
                    ..Default::default()
                })
            } else {
                let file: DictionaryFile = serde_json::from_value(value)?;
                ensure!(file.version == 2, "unsupported dictionary version");
                Ok(file)
            }
        })();
        // Do not expose JSON parser snippets: the input may contain private vocabulary.
        let (state, load_error) = match loaded {
            Ok(state) => (state, None),
            Err(_) => (DictionaryFile::default(), Some("Dictionary cannot be read; existing file is preserved. Restore a valid backup before editing.".to_string())),
        };
        Self {
            path,
            state: RwLock::new(state),
            load_error,
        }
    }

    pub fn load_error(&self) -> Option<String> {
        self.load_error.clone()
    }

    pub fn entries(&self) -> Vec<DictionaryEntry> {
        self.state.read().entries.clone()
    }

    pub fn suggestions(&self) -> Vec<DictionarySuggestion> {
        self.state.read().suggestions.clone()
    }

    fn mutate<T>(&self, edit: impl FnOnce(&mut DictionaryFile) -> Result<T>) -> Result<T> {
        ensure!(
            self.load_error.is_none(),
            "dictionary is unreadable; existing file preserved"
        );
        let mut state = self.state.write();
        let mut next = state.clone();
        let result = edit(&mut next)?;
        if next == *state {
            return Ok(result);
        }
        let parent = self
            .path
            .parent()
            .context("dictionary has no parent directory")?;
        std::fs::create_dir_all(parent).context("creating dictionary directory")?;
        let encoded = serde_json::to_vec_pretty(&next)?;
        let mut temp =
            tempfile::NamedTempFile::new_in(parent).context("creating dictionary save file")?;
        temp.write_all(&encoded).context("writing dictionary")?;
        temp.as_file().sync_all().context("syncing dictionary")?;
        temp.persist(&self.path)
            .context("saving dictionary atomically")?;
        *state = next;
        Ok(result)
    }

    pub fn save(&self, entries: Vec<DictionaryEntry>) -> Result<()> {
        self.mutate(|file| {
            for entry in &entries {
                validate_entry(&entries, entry, Some(entry.id))?;
            }
            file.entries = entries;
            Ok(())
        })
    }

    pub fn add_checked(&self, term: &str, misspelling: Option<&str>) -> Result<()> {
        let entry = DictionaryEntry::new(term.trim(), misspelling.map(|m| m.trim().to_string()));
        self.mutate(|file| {
            validate_entry(&file.entries, &entry, None)?;
            file.entries.push(entry);
            Ok(())
        })
    }

    pub fn add(&self, term: &str, misspelling: Option<&str>) -> bool {
        self.add_checked(term, misspelling).is_ok()
    }

    pub fn remove_checked(&self, id: Uuid) -> Result<()> {
        self.mutate(|file| {
            file.entries.retain(|entry| entry.id != id);
            Ok(())
        })
    }

    pub fn remove(&self, id: Uuid) {
        let _ = self.remove_checked(id);
    }

    pub fn toggle_star_checked(&self, id: Uuid) -> Result<()> {
        self.mutate(|file| {
            let entry = file
                .entries
                .iter_mut()
                .find(|entry| entry.id == id)
                .context("entry no longer exists")?;
            entry.starred = !entry.starred;
            Ok(())
        })
    }

    pub fn toggle_star(&self, id: Uuid) {
        let _ = self.toggle_star_checked(id);
    }

    pub fn update_entry(&self, id: Uuid, term: &str, misspelling: Option<&str>) -> Result<()> {
        self.mutate(|file| {
            let index = file
                .entries
                .iter()
                .position(|entry| entry.id == id)
                .context("entry no longer exists")?;
            let mut entry = file.entries[index].clone();
            entry.term = term.trim().to_string();
            entry.misspelling = misspelling
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty());
            validate_entry(&file.entries, &entry, Some(id))?;
            file.entries[index] = entry;
            Ok(())
        })
    }

    pub fn set_enabled(&self, id: Uuid, enabled: bool) -> Result<()> {
        self.mutate(|file| {
            let entry = file
                .entries
                .iter_mut()
                .find(|entry| entry.id == id)
                .context("entry no longer exists")?;
            entry.enabled = enabled;
            Ok(())
        })
    }

    pub fn learn_from_corrections(&self, corrections: &[Correction]) -> Result<usize> {
        self.merge(corrections.iter().flat_map(extract_suggestions).collect())
    }

    /// A correction seen in another application right after an insertion.
    pub fn learn_from_app_correction(&self, correction: &Correction) -> Result<usize> {
        self.merge(app_suggestions(correction))
    }

    /// The same pair seen again (from History or an application) is one
    /// suggestion with one more source. Reviewed pairs stay reviewed.
    fn merge(&self, candidates: Vec<DictionarySuggestion>) -> Result<usize> {
        if candidates.is_empty() {
            return Ok(0);
        }
        self.mutate(|file| {
            let mut added = 0;
            for candidate in candidates {
                if let Some(existing) = file.suggestions.iter_mut().find(|s| {
                    case_key(&s.wrong) == case_key(&candidate.wrong)
                        && case_key(&s.term) == case_key(&candidate.term)
                }) {
                    if existing.status == SuggestionStatus::Pending {
                        for source in candidate.sources {
                            if !existing.sources.contains(&source) {
                                existing.sources.push(source);
                            }
                        }
                    }
                    continue;
                }
                if validate_entry(&file.entries, &suggested_entry(&candidate), None).is_err() {
                    continue;
                }
                file.suggestions.push(candidate);
                added += 1;
            }
            Ok(added)
        })
    }

    pub fn accept_suggestion(&self, id: Uuid) -> Result<()> {
        self.review(id, None)
    }

    /// "Modifier": the user's own term and misspelling (None or empty: term
    /// only) instead of the suggested ones.
    pub fn accept_suggestion_edited(
        &self,
        id: Uuid,
        term: &str,
        misspelling: Option<&str>,
    ) -> Result<()> {
        self.review(id, Some((term, misspelling)))
    }

    fn review(&self, id: Uuid, edited: Option<(&str, Option<&str>)>) -> Result<()> {
        self.mutate(|file| {
            let index = file
                .suggestions
                .iter()
                .position(|s| s.id == id)
                .context("suggestion no longer exists")?;
            let suggestion = &file.suggestions[index];
            ensure!(
                suggestion.status == SuggestionStatus::Pending,
                "suggestion already reviewed"
            );
            let mut entry = match edited {
                Some((term, misspelling)) => {
                    DictionaryEntry::new(term.trim(), misspelling.map(|m| m.trim().to_string()))
                }
                None => suggested_entry(suggestion),
            };
            entry.provenance = suggestion.sources.first().cloned();
            validate_entry(&file.entries, &entry, None)?;
            file.entries.push(entry);
            file.suggestions[index].status = SuggestionStatus::Accepted;
            Ok(())
        })
    }

    pub fn reject_suggestion(&self, id: Uuid) -> Result<()> {
        self.mutate(|file| {
            let suggestion = file
                .suggestions
                .iter_mut()
                .find(|s| s.id == id)
                .context("suggestion no longer exists")?;
            ensure!(
                suggestion.status == SuggestionStatus::Pending,
                "suggestion already reviewed"
            );
            suggestion.status = SuggestionStatus::Rejected;
            Ok(())
        })
    }

    /// Accepted entries are voluntary knowledge, independent of later session
    /// deletion. A pending suggestion keeps a History source while that
    /// dictation still has a correction (`corrected`), an application source
    /// while the dictation itself exists (`dictations`) and is younger than
    /// [`APP_SUGGESTION_RETENTION_DAYS`]; with no source left it goes.
    pub fn purge_orphan_suggestions(
        &self,
        corrected: &HashSet<String>,
        dictations: &HashSet<String>,
    ) -> Result<usize> {
        let expiry =
            OffsetDateTime::now_utc() - time::Duration::days(APP_SUGGESTION_RETENTION_DAYS);
        self.purge(|source| match source.origin {
            LearningOrigin::History => corrected.contains(&source.dictation_id),
            LearningOrigin::Application => {
                dictations.contains(&source.dictation_id) && source.corrected_at > expiry
            }
        })
    }

    /// "Delete suggestions from applications": pending ones only; accepted
    /// entries and refusals stay.
    pub fn purge_app_suggestions(&self) -> Result<usize> {
        self.purge(|source| source.origin != LearningOrigin::Application)
    }

    fn purge(&self, keep: impl Fn(&LearningSource) -> bool) -> Result<usize> {
        self.mutate(|file| {
            let before = file.suggestions.len();
            for suggestion in &mut file.suggestions {
                if suggestion.status == SuggestionStatus::Pending {
                    suggestion.sources.retain(&keep);
                }
            }
            file.suggestions
                .retain(|s| s.status != SuggestionStatus::Pending || !s.sources.is_empty());
            Ok(before - file.suggestions.len())
        })
    }

    // ----- Pipeline inputs

    /// Starred first, then newest.
    fn ranked(&self) -> Vec<DictionaryEntry> {
        let mut entries: Vec<_> = self
            .entries()
            .into_iter()
            .filter(|entry| entry.enabled && TERM_LENGTH.contains(&entry.term.chars().count()))
            .collect();
        entries.sort_by(|a, b| {
            b.starred
                .cmp(&a.starred)
                .then(b.created_at.cmp(&a.created_at))
        });
        entries
    }

    /// Vocabulary for the cleanup prompt: starred first, then newest. Cap 100.
    pub fn vocabulary(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.ranked()
            .into_iter()
            .filter(|entry| seen.insert(case_key(&entry.term)))
            .take(100)
            .map(|entry| entry.term)
            .collect()
    }

    /// Vocabulary as it goes over the wire — the SHARED sanitizer for both the
    /// cleanup prompt and the transcription request's `custom_vocabulary`.
    ///
    /// Dictionary entries are user/CSV data, so newlines are stripped and each
    /// term capped: a crafted entry must not be able to smuggle its own
    /// instruction line into the prompt. Both consumers call this so they cannot
    /// drift apart, and a total-byte ceiling bounds the request whatever the
    /// per-term caps allow. Only CORRECT terms — never misspellings; biasing a
    /// recogniser toward "cooper netties" is actively harmful.
    pub fn sanitized_vocabulary(&self, max_bytes: usize) -> Vec<String> {
        let mut used = 0;
        let mut out = Vec::new();
        for term in self.vocabulary() {
            let clean: String = term
                .replace(['\n', '\r'], " ")
                .chars()
                .take(60)
                .collect::<String>()
                .trim()
                .to_string();
            if clean.is_empty() {
                continue;
            }
            let cost = clean.len() + 1;
            if used + cost > max_bytes {
                break; // truncate from the END: starred survive
            }
            used += cost;
            out.push(clean);
        }
        out
    }

    /// Spelling hints for the prompt (top 10 with misspellings) — starred first,
    /// matching `vocabulary()`: "starred words are prioritized" must be true for
    /// both prompt inputs, not just one.
    pub fn spellings(&self) -> Vec<(String, String)> {
        let safe: HashSet<_> = self
            .replacement_rules()
            .into_iter()
            .map(|rule| (rule.wrong, rule.right))
            .collect();
        self.ranked()
            .into_iter()
            .filter_map(|entry| {
                entry
                    .misspelling
                    .filter(|m| !m.is_empty())
                    .map(|wrong| (wrong, entry.term))
            })
            .filter(|pair| safe.contains(pair))
            .take(10)
            .collect()
    }

    /// Deterministic rules for the replacement engine (ALL entries with misspellings).
    pub fn replacement_rules(&self) -> Vec<Rule> {
        let mut entries = self.ranked();
        // Legacy files may contain conflicts predating validation. Manual knowledge wins.
        entries.sort_by_key(|entry| entry.provenance.is_some());
        let canonical: HashSet<_> = entries.iter().map(|entry| case_key(&entry.term)).collect();
        let mut seen = HashSet::new();
        entries
            .into_iter()
            .filter_map(|entry| {
                let wrong = entry
                    .misspelling
                    .filter(|m| !m.is_empty() && m.chars().count() <= 60)?;
                let key = case_key(&wrong);
                if key != case_key(&entry.term) && canonical.contains(&key) || !seen.insert(key) {
                    return None;
                }
                Some(Rule::new(wrong, entry.term))
            })
            .collect()
    }

    // ----- CSV (data portability)

    pub fn export_csv(&self) -> String {
        let mut lines = vec!["term,misspelling,enabled".to_string()];
        for entry in self.entries() {
            let term = entry.term.replace('"', "\"\"");
            let misspelling = entry.misspelling.unwrap_or_default().replace('"', "\"\"");
            lines.push(format!("\"{term}\",\"{misspelling}\",{}", entry.enabled));
        }
        lines.join("\n")
    }

    pub fn import_csv(&self, csv: &str) -> usize {
        self.import_csv_checked(csv).unwrap_or(0)
    }

    pub fn import_csv_checked(&self, csv: &str) -> Result<usize> {
        let mut records = split_records(csv);
        if let Some(first) = records.first() {
            let first_cell = parse_csv_line(first)
                .first()
                .map(|cell| cell.to_lowercase())
                .unwrap_or_default();
            if ["term", "word", "phrase"].contains(&first_cell.as_str()) {
                records.remove(0);
            }
        }
        self.mutate(|file| {
            let mut imported = 0;
            for record in records.into_iter().take(MAX_IMPORT_ROWS) {
                let columns = parse_csv_line(&record);
                let Some(term) = columns.first() else {
                    continue;
                };
                let misspelling = columns.get(1).filter(|c| !c.is_empty()).cloned();
                let mut entry = DictionaryEntry::new(term.trim(), misspelling);
                entry.enabled = match columns.get(2).map(|v| v.to_lowercase()) {
                    None => true,
                    Some(value) if value.is_empty() || value == "true" => true,
                    Some(value) if value == "false" => false,
                    _ => continue,
                };
                if validate_entry(&file.entries, &entry, None).is_err() {
                    continue;
                }
                file.entries.push(entry);
                imported += 1;
            }
            Ok(imported)
        })
    }
}

// Uppercase first also unifies final sigma and long-s before conservative deduplication.
/// The entry that accepting `suggestion` as is would create.
fn suggested_entry(suggestion: &DictionarySuggestion) -> DictionaryEntry {
    let misspelling = (!suggestion.term_only).then(|| suggestion.wrong.clone());
    DictionaryEntry::new(&suggestion.term, misspelling)
}

fn case_key(text: &str) -> String {
    text.to_uppercase().to_lowercase()
}

fn validate_entry(
    entries: &[DictionaryEntry],
    entry: &DictionaryEntry,
    except: Option<Uuid>,
) -> Result<()> {
    ensure!(
        TERM_LENGTH.contains(&entry.term.chars().count()),
        "term must contain 1 to 60 characters"
    );
    let term = case_key(&entry.term);
    let wrong = case_key(entry.misspelling.as_deref().unwrap_or(""));
    ensure!(
        wrong.chars().count() <= 60,
        "misspelling must contain at most 60 characters"
    );
    for other in entries.iter().filter(|e| Some(e.id) != except) {
        let other_term = case_key(&other.term);
        let other_wrong = case_key(other.misspelling.as_deref().unwrap_or(""));
        ensure!(
            term != other_term
                || (!wrong.is_empty() && wrong != other_wrong && entry.term == other.term),
            "term or spelling variant already exists"
        );
        ensure!(
            wrong.is_empty() || other_wrong.is_empty() || wrong != other_wrong,
            "misspelling already has a rule"
        );
        // Disallow chains as well as cycles: correctly spelled vocabulary must remain stable.
        ensure!(
            wrong.is_empty() || wrong != other_term,
            "rule would replace an existing correct term"
        );
        ensure!(
            other_wrong.is_empty() || term != other_wrong,
            "rule would form a replacement chain"
        );
    }
    Ok(())
}

/// Quote-aware record splitter: CRLF endings and RFC-4180 quoted newlines both
/// break a naive `split('\n')` (dropped or mangled rows while reporting success).
fn split_records(csv: &str) -> Vec<String> {
    let mut records = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for char in csv.chars() {
        match char {
            '"' => {
                in_quotes = !in_quotes;
                current.push(char);
            }
            '\n' | '\r' if !in_quotes => {
                if !current.is_empty() {
                    records.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(char),
        }
    }
    if !current.is_empty() {
        records.push(current);
    }
    records
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut columns = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let chars: Vec<char> = line.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        match (chars[index], in_quotes) {
            // RFC 4180 escaped quote.
            ('"', true) if chars.get(index + 1) == Some(&'"') => {
                current.push('"');
                index += 1;
            }
            ('"', _) => in_quotes = !in_quotes,
            (',', false) => columns.push(std::mem::take(&mut current)),
            (char, _) => current.push(char),
        }
        index += 1;
    }
    columns.push(current);
    columns
        .into_iter()
        .map(|column| column.trim().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, DictionaryStore) {
        let temp = tempfile::tempdir().unwrap();
        let store = DictionaryStore::open(temp.path().join("dictionary.json"));
        (temp, store)
    }

    fn correction(id: &str, before: &str, after: &str) -> Correction {
        Correction {
            dictation_id: id.into(),
            before: before.into(),
            after: after.into(),
            corrected_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn review_lifecycle_survives_reopen_without_private_transcripts() {
        let (temp, store) = store();
        let source = correction(
            "session-a",
            "Une phrase privée sur Proxmocks.",
            "Une phrase privée sur Proxmox.",
        );
        assert_eq!(
            store
                .learn_from_corrections(std::slice::from_ref(&source))
                .unwrap(),
            1
        );
        assert!(store.replacement_rules().is_empty());
        let suggestion = store.suggestions()[0].clone();
        store.accept_suggestion(suggestion.id).unwrap();
        assert_eq!(
            crate::replacement::apply(&store.replacement_rules(), "Proxmocks ProxmocksExtra"),
            "Proxmox ProxmocksExtra"
        );
        assert_eq!(
            store
                .learn_from_corrections(std::slice::from_ref(&source))
                .unwrap(),
            0
        );
        let reopened = DictionaryStore::open(temp.path().join("dictionary.json"));
        assert_eq!(
            reopened.entries()[0]
                .provenance
                .as_ref()
                .unwrap()
                .dictation_id,
            "session-a"
        );
        assert_eq!(reopened.suggestions()[0].status, SuggestionStatus::Accepted);
        let id = reopened.entries()[0].id;
        reopened.set_enabled(id, false).unwrap();
        assert!(reopened.vocabulary().is_empty());
        assert!(reopened.replacement_rules().is_empty());
        reopened
            .update_entry(id, "Proxmox VE", Some("Proxmocks"))
            .unwrap();
        reopened.set_enabled(id, true).unwrap();
        assert_eq!(reopened.vocabulary(), ["Proxmox VE"]);
        reopened
            .purge_orphan_suggestions(&HashSet::new(), &HashSet::new())
            .unwrap();
        assert_eq!(reopened.entries().len(), 1);
        reopened.remove_checked(id).unwrap();
        assert_eq!(reopened.learn_from_corrections(&[source]).unwrap(), 0);
        assert!(reopened.entries().is_empty());
        let encoded = std::fs::read_to_string(temp.path().join("dictionary.json")).unwrap();
        assert!(!encoded.contains("phrase privée"));
        assert!(!encoded.contains("before"));
        assert!(!encoded.contains("after"));
    }

    #[test]
    fn rejected_suggestion_is_a_durable_tombstone_and_orphans_are_purged() {
        let (temp, store) = store();
        let a = correction("a", "Proxmocks", "Proxmox");
        let b = correction("b", "Proxmocks", "Proxmox");
        store
            .learn_from_corrections(&[a.clone(), b.clone()])
            .unwrap();
        assert_eq!(store.suggestions().len(), 1);
        assert_eq!(store.suggestions()[0].sources.len(), 2);
        assert_eq!(
            store
                .purge_orphan_suggestions(&HashSet::from(["b".into()]), &HashSet::new())
                .unwrap(),
            0
        );
        assert_eq!(store.suggestions()[0].sources[0].dictation_id, "b");
        let id = store.suggestions()[0].id;
        store.reject_suggestion(id).unwrap();
        store
            .purge_orphan_suggestions(&HashSet::new(), &HashSet::new())
            .unwrap();
        let reopened = DictionaryStore::open(temp.path().join("dictionary.json"));
        assert_eq!(reopened.learn_from_corrections(&[a, b]).unwrap(), 0);
        assert_eq!(reopened.suggestions()[0].status, SuggestionStatus::Rejected);
        reopened
            .learn_from_corrections(&[correction("c", "Un cafe", "Un café")])
            .unwrap();
        assert_eq!(
            reopened
                .purge_orphan_suggestions(&HashSet::new(), &HashSet::new())
                .unwrap(),
            1
        );
        assert_eq!(reopened.suggestions().len(), 1);
    }

    fn app_fix(id: &str, before: &str, after: &str) -> Correction {
        Correction {
            corrected_at: OffsetDateTime::now_utc(),
            ..correction(id, before, after)
        }
    }

    #[test]
    fn app_corrections_need_review_then_change_the_next_dictation() {
        let (temp, store) = store();
        let fix = app_fix(
            "d1",
            "Je vais ouvrir Loustream.",
            "Je vais ouvrir Loostream.",
        );
        assert_eq!(store.learn_from_app_correction(&fix).unwrap(), 1);
        // Seen again in another dictation: one suggestion, two sources.
        let again = app_fix("d2", "Loustream démarre.", "Loostream démarre.");
        assert_eq!(store.learn_from_app_correction(&again).unwrap(), 0);
        let suggestion = store.suggestions()[0].clone();
        assert_eq!(suggestion.sources.len(), 2);
        assert_eq!(suggestion.sources[0].origin, LearningOrigin::Application);
        // Nothing active before review.
        assert!(store.replacement_rules().is_empty() && store.vocabulary().is_empty());
        store.accept_suggestion(suggestion.id).unwrap();
        assert_eq!(
            crate::replacement::apply(&store.replacement_rules(), "Ouvre Loustream."),
            "Ouvre Loostream."
        );
        let entry = &store.entries()[0];
        assert_eq!(
            entry.provenance.as_ref().unwrap().origin,
            LearningOrigin::Application
        );

        // A term-only suggestion becomes vocabulary, never a rule.
        let unbound = app_fix(
            "d3",
            "Le service unburn répond.",
            "Le service Unbound répond.",
        );
        store.learn_from_app_correction(&unbound).unwrap();
        let term = store
            .suggestions()
            .into_iter()
            .find(|s| s.term_only)
            .unwrap();
        store.accept_suggestion(term.id).unwrap();
        assert!(store.vocabulary().contains(&"Unbound".to_string()));
        assert_eq!(store.replacement_rules().len(), 1);

        // "Modifier": the user's own spelling wins.
        let jot = app_fix("d4", "Le projet jott avance.", "Le projet JOt avance.");
        store.learn_from_app_correction(&jot).unwrap();
        let edited = store
            .suggestions()
            .into_iter()
            .find(|s| s.wrong == "jott")
            .unwrap();
        store
            .accept_suggestion_edited(edited.id, "Jot", Some("jott"))
            .unwrap();
        assert_eq!(
            crate::replacement::apply(&store.replacement_rules(), "jott"),
            "Jot"
        );

        // Files written before 1.6 carry no origin: History.
        let encoded = std::fs::read_to_string(temp.path().join("dictionary.json")).unwrap();
        assert!(encoded.contains("\"origin\": \"application\""));
        assert!(encoded.contains("\"termOnly\": true"));
        assert!(
            !encoded.contains("Je vais ouvrir"),
            "no surrounding text stored"
        );
        let legacy: LearningSource =
            serde_json::from_str(r#"{"dictationId":"x","correctedAt":"2026-01-01T00:00:00Z"}"#)
                .unwrap();
        assert_eq!(legacy.origin, LearningOrigin::History);
    }

    #[test]
    fn pending_app_suggestions_follow_their_dictation() {
        let (_temp, store) = store();
        store
            .learn_from_app_correction(&app_fix("live", "ouvrir Loustream", "ouvrir Loostream"))
            .unwrap();
        store
            .learn_from_app_correction(&app_fix("gone", "lancer Proxmocks", "lancer Proxmox"))
            .unwrap();
        let mut old = app_fix("old", "voir Kubernètes", "voir Kubernetes");
        old.corrected_at -= time::Duration::days(APP_SUGGESTION_RETENTION_DAYS + 1);
        store.learn_from_app_correction(&old).unwrap();
        // History corrections never vouch for an application source.
        let dictations = HashSet::from(["live".to_string(), "old".to_string()]);
        assert_eq!(
            store
                .purge_orphan_suggestions(&HashSet::new(), &dictations)
                .unwrap(),
            2,
            "deleted dictation and expired suggestion"
        );
        assert_eq!(store.suggestions()[0].term, "Loostream");
        let id = store.suggestions()[0].id;
        store
            .learn_from_corrections(&[correction("h", "Proxmocks", "Proxmox")])
            .unwrap();
        store.reject_suggestion(id).unwrap();
        // Purging application suggestions keeps History ones and refusals.
        assert_eq!(store.purge_app_suggestions().unwrap(), 0);
        assert_eq!(store.suggestions().len(), 2);
        store
            .learn_from_app_correction(&app_fix("live", "voir Kubernètes", "voir Kubernetes"))
            .unwrap();
        assert_eq!(store.purge_app_suggestions().unwrap(), 1);
    }

    #[test]
    fn legacy_migration_and_unknown_version_preserve_data() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("dictionary.json");
        let entry = DictionaryEntry::new("Ansible", Some("Ansibul".into()));
        let mut value = serde_json::to_value(&entry).unwrap();
        value.as_object_mut().unwrap().remove("enabled");
        value.as_object_mut().unwrap().remove("provenance");
        std::fs::write(&path, serde_json::to_vec(&vec![value]).unwrap()).unwrap();
        let legacy = DictionaryStore::open(path.clone());
        assert!(legacy.entries()[0].enabled);
        assert!(legacy.entries()[0].provenance.is_none());
        legacy.add_checked("Kubernetes", None).unwrap();
        assert_eq!(DictionaryStore::open(path.clone()).entries().len(), 2);
        let future = br#"{"version":99,"entries":[],"private":"preserve me"}"#;
        std::fs::write(&path, future).unwrap();
        let unsupported = DictionaryStore::open(path.clone());
        assert!(unsupported.load_error().is_some());
        assert!(unsupported.add_checked("New", None).is_err());
        assert_eq!(std::fs::read(path).unwrap(), future);
    }

    #[test]
    fn save_failure_does_not_change_live_rules() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("parent/dictionary.json");
        let store = DictionaryStore::open(path);
        std::fs::write(temp.path().join("parent"), "block directory creation").unwrap();
        assert!(store.add_checked("Proxmox", Some("Proxmocks")).is_err());
        assert!(store.entries().is_empty());
        assert!(store.replacement_rules().is_empty());
    }

    #[test]
    fn acceptance_cannot_overwrite_a_manual_rule_added_after_extraction() {
        let (_temp, store) = store();
        store
            .learn_from_corrections(&[correction("a", "Proxmocks", "Proxmox")])
            .unwrap();
        let id = store.suggestions()[0].id;
        store.add_checked("ManualName", Some("Proxmocks")).unwrap();
        assert!(store.accept_suggestion(id).is_err());
        assert_eq!(store.suggestions()[0].status, SuggestionStatus::Pending);
        assert_eq!(store.entries()[0].term, "ManualName");
    }

    #[test]
    fn synthetic_bulk_learning_is_idempotent_and_prompt_inputs_remain_bounded() {
        let (_temp, store) = store();
        let corrections: Vec<_> = (0..2000)
            .map(|i| {
                correction(
                    &format!("fictional-{i}"),
                    &format!("Proxmocks{i}"),
                    &format!("Proxmox{i}"),
                )
            })
            .collect();
        let started = std::time::Instant::now();
        assert_eq!(store.learn_from_corrections(&corrections).unwrap(), 2000);
        assert_eq!(store.learn_from_corrections(&corrections).unwrap(), 0);
        eprintln!(
            "fictional 2000 corrections, two scans: {:?}",
            started.elapsed()
        );
        assert_eq!(store.suggestions().len(), 2000);
        let csv = (0..1000)
            .map(|i| format!("Canonical{i},Typoo{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(store.import_csv_checked(&csv).unwrap(), 1000);
        assert_eq!(store.vocabulary().len(), 100);
        assert!(
            store
                .sanitized_vocabulary(2048)
                .iter()
                .map(|s| s.len() + 1)
                .sum::<usize>()
                <= 2048
        );
    }

    #[test]
    fn legacy_conflicts_never_rewrite_another_canonical_term() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("dictionary.json");
        let entries = [
            DictionaryEntry::new("alpha", Some("beta".into())),
            DictionaryEntry::new("beta", Some("gamma".into())),
        ];
        std::fs::write(&path, serde_json::to_vec(&entries).unwrap()).unwrap();
        let store = DictionaryStore::open(path);
        assert_eq!(
            crate::replacement::apply(&store.replacement_rules(), "alpha beta gamma"),
            "alpha beta beta"
        );
    }

    #[test]
    fn csv_roundtrip_keeps_disabled_rules_disabled() {
        let (_temp, store) = store();
        store.add_checked("Proxmox", Some("Proxmocks")).unwrap();
        store.set_enabled(store.entries()[0].id, false).unwrap();
        let (_temp2, target) = self::store();
        target.import_csv_checked(&store.export_csv()).unwrap();
        assert!(target.replacement_rules().is_empty());
        assert!(!target.entries()[0].enabled);
    }

    #[test]
    fn spelling_variants_share_vocabulary_without_overwriting_manual_knowledge() {
        let (_temp, store) = store();
        store.add_checked("Proxmox", None).unwrap();
        assert_eq!(
            store
                .learn_from_corrections(&[correction("a", "Proxmocks", "Proxmox")])
                .unwrap(),
            1
        );
        store.accept_suggestion(store.suggestions()[0].id).unwrap();
        store.add_checked("Proxmox", Some("Proxmoxx")).unwrap();
        assert_eq!(store.entries().len(), 3);
        assert!(store.entries()[0].provenance.is_none());
        assert!(store.entries()[0].misspelling.is_none());
        assert_eq!(store.vocabulary(), ["Proxmox"]);
        assert_eq!(
            crate::replacement::apply(&store.replacement_rules(), "Proxmocks Proxmoxx Proxmox"),
            "Proxmox Proxmox Proxmox"
        );
        assert!(!store.add("Proxmox", Some("Proxmoxx")));
        assert!(!store.add("Proxmox", None));
    }

    #[test]
    fn repeated_session_scans_do_not_write_unchanged_dictionary() {
        let (temp, store) = store();
        let sources: Vec<_> = (0..1000)
            .map(|i| {
                correction(
                    &format!("synthetic-{i}"),
                    &format!("Proxmocks{i}"),
                    &format!("Proxmox{i}"),
                )
            })
            .collect();
        let first = std::time::Instant::now();
        for source in &sources {
            store
                .learn_from_corrections(std::slice::from_ref(source))
                .unwrap();
        }
        let initial = first.elapsed();
        let path = temp.path().join("dictionary.json");
        let metadata = std::fs::metadata(&path).unwrap();
        let modified = metadata.modified().unwrap();
        let repeated = std::time::Instant::now();
        for source in &sources {
            assert_eq!(
                store
                    .learn_from_corrections(std::slice::from_ref(source))
                    .unwrap(),
                0
            );
        }
        eprintln!(
            "fictional 1000 single-session scans: first={initial:?}, repeated={:?}",
            repeated.elapsed()
        );
        assert_eq!(
            std::fs::metadata(path).unwrap().modified().unwrap(),
            modified
        );
        assert_eq!(store.suggestions().len(), 1000);
    }

    #[test]
    fn unicode_case_variants_cannot_hide_a_conflicting_rule() {
        let (_temp, store) = store();
        store.add_checked("Correct", Some("ΟΣ")).unwrap();
        assert!(!store.add("Other", Some("ος")));
        assert!(!store.add("Another", Some("οσ")));
    }

    #[test]
    fn migrated_unsafe_rules_are_excluded_from_both_pipeline_inputs() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("dictionary.json");
        let entries = [
            DictionaryEntry::new("alpha", Some("beta".into())),
            DictionaryEntry::new("beta", Some("gamma".into())),
            DictionaryEntry::new("x".repeat(61), Some("oversized".into())),
        ];
        std::fs::write(&path, serde_json::to_vec(&entries).unwrap()).unwrap();
        let store = DictionaryStore::open(path);
        assert!(!store.spellings().contains(&("beta".into(), "alpha".into())));
        assert!(
            store
                .replacement_rules()
                .iter()
                .all(|rule| rule.right.chars().count() <= 60)
        );
    }

    #[test]
    fn conflicts_and_cycles_are_rejected() {
        let (_temp, store) = store();
        assert!(store.add("Proxmox", Some("Proxmocks")));
        assert!(!store.add("Another", Some("Proxmocks")));
        assert!(!store.add("Proxmocks", Some("Proxmox")));
    }

    #[test]
    fn invalid_existing_file_is_never_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("dictionary.json");
        std::fs::write(&path, b"{broken private file").unwrap();
        let store = DictionaryStore::open(path.clone());
        assert!(!store.add("Proxmox", None));
        assert_eq!(std::fs::read(path).unwrap(), b"{broken private file");
    }

    #[test]
    fn add_rejects_duplicates_and_out_of_range_terms() {
        let (_temp, store) = store();
        assert!(store.add("Kubernetes", Some("cooper netties")));
        assert!(!store.add("kubernetes", None), "case-insensitive duplicate");
        assert!(!store.add("", None));
        assert!(!store.add(&"x".repeat(61), None));
        assert_eq!(store.entries().len(), 1);
    }

    #[test]
    fn entries_persist_across_reopen() {
        let (temp, store) = store();
        store.add("gRPC", None);
        let reopened = DictionaryStore::open(temp.path().join("dictionary.json"));
        assert_eq!(reopened.entries()[0].term, "gRPC");
    }

    #[test]
    fn starred_terms_rank_first_in_both_prompt_inputs() {
        let (_temp, store) = store();
        store.add("First", Some("furst"));
        store.add("Second", Some("secund"));
        let id = store.entries()[0].id;
        store.toggle_star(id);

        assert_eq!(store.vocabulary()[0], "First");
        assert_eq!(store.spellings()[0], ("furst".into(), "First".into()));
    }

    #[test]
    fn sanitized_vocabulary_strips_newlines_and_honours_the_byte_ceiling() {
        let (_temp, store) = store();
        store.add("Alpha\nBeta", None);
        assert_eq!(store.sanitized_vocabulary(2048), vec!["Alpha Beta"]);
        // A ceiling below the first term's cost yields nothing rather than a
        // truncated term.
        assert!(store.sanitized_vocabulary(4).is_empty());
    }

    #[test]
    fn replacement_rules_skip_entries_without_a_misspelling() {
        let (_temp, store) = store();
        store.add("Kubernetes", Some("cooper netties"));
        store.add("Ammaar", None);
        let rules = store.replacement_rules();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].right, "Kubernetes");
    }

    #[test]
    fn csv_round_trips() {
        let (_temp, source) = store();
        source.add("gRPC", Some("g r p c"));
        source.add("Quote\"Term", None);

        let (_temp2, target) = store();
        assert_eq!(target.import_csv(&source.export_csv()), 2);
        let terms: Vec<String> = target.entries().into_iter().map(|e| e.term).collect();
        assert!(terms.contains(&"gRPC".to_string()));
        assert!(terms.contains(&"Quote\"Term".to_string()));
    }

    #[test]
    fn import_handles_crlf_and_headerless_files() {
        let (_temp, store) = store();
        assert_eq!(store.import_csv("terminal,terminul\r\nKubernetes,\r\n"), 2);
        let terms: Vec<String> = store.entries().into_iter().map(|e| e.term).collect();
        assert!(terms.contains(&"terminal".to_string()), "{terms:?}");
    }

    #[test]
    fn import_drops_only_a_real_header_row() {
        let (_temp, store) = store();
        assert_eq!(
            store.import_csv("term,misspelling\nKubernetes,cooper netties"),
            1
        );
        assert_eq!(store.entries().len(), 1);
        assert_eq!(store.entries()[0].term, "Kubernetes");
    }

    #[test]
    fn import_skips_duplicates_within_and_across_the_file() {
        let (_temp, store) = store();
        store.add("gRPC", None);
        assert_eq!(store.import_csv("gRPC,\ngrpc,\nNew,"), 1);
    }

    #[test]
    fn quoted_newlines_do_not_split_a_record() {
        assert_eq!(split_records("\"a\nb\",c\nd,e").len(), 2);
    }
}
