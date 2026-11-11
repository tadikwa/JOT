use jot_core::{
    file_layout,
    history::{ApplicationFilter, HistoryFilter, HistoryStore, StatusFilter, period_start},
    meta::{SessionMeta, SessionStatus},
};
use std::path::PathBuf;
use time::{Duration, OffsetDateTime, macros::datetime};
use uuid::Uuid;

struct Fixture {
    store: HistoryStore,
    temp: tempfile::TempDir,
    _guard: parking_lot::MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let guard = file_layout::TEST_ROOT_LOCK.lock();
        let temp = tempfile::tempdir().unwrap();
        file_layout::set_override_root(Some(temp.path().to_path_buf()));
        std::fs::create_dir_all(file_layout::recordings_root()).unwrap();
        let store = HistoryStore::open(&file_layout::history_sqlite()).unwrap();
        Self {
            store,
            temp,
            _guard: guard,
        }
    }

    fn add(
        &self,
        number: u128,
        status: SessionStatus,
        text: Option<&str>,
    ) -> (SessionMeta, PathBuf) {
        let id = Uuid::from_u128(number << 96);
        let mut meta = SessionMeta::new(id, datetime!(2026-11-11 12:00 UTC), status);
        meta.raw_transcript = text.map(str::to_owned);
        meta.audio_duration_seconds = Some(60.0);
        let folder = file_layout::make_session_folder(id, meta.started_at).unwrap();
        meta.write(&folder);
        self.store.upsert(&meta, &folder);
        (meta, folder)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        file_layout::set_override_root(None);
    }
}

fn matches(store: &HistoryStore, search: &str) -> Vec<String> {
    store
        .page(
            &HistoryFilter {
                search: search.into(),
                ..Default::default()
            },
            0,
            50,
        )
        .unwrap()
        .items
        .into_iter()
        .map(|item| item.id)
        .collect()
}

#[test]
fn corrections_preserve_engine_text_and_survive_reopen_and_index_rebuild() {
    let f = Fixture::new();
    let (mut meta, folder) = f.add(1, SessionStatus::Inserted, Some("sortie brute"));
    meta.cleaned_transcript = Some("Sortie nettoyée.".into());
    meta.write(&folder);
    f.store.upsert(&meta, &folder);
    let id = meta.id.to_string();
    let before_meta = std::fs::read(file_layout::meta_json(&folder)).unwrap();
    f.store
        .edit(&id, "Sortie nettoyée.", "Première correction.")
        .unwrap();
    f.store
        .edit(&id, "Première correction.", "Version finale relue.")
        .unwrap();
    assert!(
        f.store
            .edit(&id, "Première correction.", "écrasement périmé")
            .is_err()
    );
    assert_eq!(
        std::fs::read(file_layout::meta_json(&folder)).unwrap(),
        before_meta
    );
    let record = f.store.record(&id).unwrap();
    assert_eq!(record.raw_transcript.as_deref(), Some("sortie brute"));
    assert_eq!(
        record.cleaned_transcript.as_deref(),
        Some("Sortie nettoyée.")
    );
    assert_eq!(
        record.user_transcript.as_deref(),
        Some("Version finale relue.")
    );
    assert_eq!(record.display_text(), "Version finale relue.");
    let corrections = f.store.corrections(&id).unwrap();
    assert_eq!(corrections.len(), 2);
    assert_eq!(corrections[0].dictation_id, id);
    assert_eq!(corrections[0].before, "Sortie nettoyée.");
    assert_eq!(corrections[0].after, "Première correction.");
    assert_eq!(corrections[1].before, corrections[0].after);
    assert_eq!(corrections[1].after, "Version finale relue.");
    assert!(corrections[1].corrected_at >= corrections[0].corrected_at);
    let journal = std::fs::read(folder.join("corrections.json")).unwrap();
    // A late engine result may replace its metadata, never the user's correction.
    meta.cleaned_transcript = Some("Nouvelle sortie moteur.".into());
    meta.write(&folder);
    f.store.upsert(&meta, &folder);
    assert_eq!(
        f.store.record(&id).unwrap().display_text(),
        "Version finale relue."
    );
    assert_eq!(
        std::fs::read(folder.join("corrections.json")).unwrap(),
        journal
    );
    let reopened = HistoryStore::open(&file_layout::history_sqlite()).unwrap();
    assert_eq!(
        reopened.record(&id).unwrap().display_text(),
        "Version finale relue."
    );
    let rebuilt = HistoryStore::open(&f.temp.path().join("rebuilt.sqlite")).unwrap();
    rebuilt.reindex();
    assert_eq!(
        rebuilt.record(&id).unwrap().display_text(),
        "Version finale relue."
    );
    assert_eq!(rebuilt.corrections(&id).unwrap().len(), 2);
    assert_eq!(matches(&rebuilt, "finale"), vec![id]);
}

#[test]
fn malformed_correction_journal_is_never_silently_replaced() {
    let f = Fixture::new();
    let (meta, folder) = f.add(2, SessionStatus::Inserted, Some("original"));
    let id = meta.id.to_string();
    f.store.edit(&id, "original", "correction").unwrap();
    let damaged = b"{ incomplete correction journal";
    std::fs::write(folder.join("corrections.json"), damaged).unwrap();
    assert!(f.store.corrections(&id).is_err());
    assert!(f.store.edit(&id, "correction", "replacement").is_err());
    assert_eq!(
        std::fs::read(folder.join("corrections.json")).unwrap(),
        damaged
    );
    assert_eq!(f.store.record(&id).unwrap().display_text(), "correction");
}

#[test]
fn search_uses_words_prefixes_accents_and_all_transcript_versions() {
    let f = Fixture::new();
    let (mut meta, folder) = f.add(3, SessionStatus::Inserted, Some("ÉLÉPHANT brut café"));
    meta.cleaned_transcript = Some("Bibliothèque finale".into());
    meta.write(&folder);
    f.store.upsert(&meta, &folder);
    let id = meta.id.to_string();
    f.store
        .edit(&id, "Bibliothèque finale", "Révision validée.")
        .unwrap();
    f.add(4, SessionStatus::Inserted, Some("elephant solitaire"));
    assert_eq!(matches(&f.store, "ELEPH café"), vec![id.clone()]);
    assert_eq!(matches(&f.store, "bibliothe"), vec![id.clone()]);
    assert_eq!(matches(&f.store, "revision valid"), vec![id]);
    assert!(matches(&f.store, "phant").is_empty());
    for search in ["' OR 1=1 --", "\" OR *", "café absent", "100%", "_"] {
        assert!(
            matches(&f.store, search).is_empty(),
            "unexpected match for {search:?}"
        );
    }
    assert_eq!(
        f.store
            .page(&HistoryFilter::default(), 0, 50)
            .unwrap()
            .total,
        2
    );
}

#[test]
fn combined_filters_sort_ties_and_paginate_beyond_five_hundred() {
    let f = Fixture::new();
    for n in 1..=605 {
        let (mut meta, folder) = f.add(n, SessionStatus::Inserted, Some("texte commun"));
        meta.target_app_name = Some(if n % 2 == 0 { "Éditeur" } else { "Navigateur" }.into());
        meta.started_at += Duration::seconds((n / 3) as i64);
        meta.write(&folder);
        f.store.upsert(&meta, &folder);
    }
    let first = f.store.page(&HistoryFilter::default(), 0, 50).unwrap();
    assert_eq!(first.total, 605);
    assert_eq!(first.items.len(), 50);
    assert_eq!(
        f.store
            .page(&HistoryFilter::default(), 0, usize::MAX)
            .unwrap()
            .items
            .len(),
        500
    );
    assert!(
        first
            .items
            .windows(2)
            .all(|rows| { (rows[0].started_at, &rows[0].id) > (rows[1].started_at, &rows[1].id) })
    );
    let last = f.store.page(&HistoryFilter::default(), 600, 50).unwrap();
    assert_eq!(last.total, 605);
    assert_eq!(last.items.len(), 5);
    assert!(
        last.items
            .iter()
            .all(|item| !first.items.iter().any(|row| row.id == item.id))
    );
    assert!(
        f.store
            .page(&HistoryFilter::default(), 605, 50)
            .unwrap()
            .items
            .is_empty()
    );
    let since = datetime!(2026-11-11 12:01 UTC).unix_timestamp();
    let filter = HistoryFilter {
        search: "COMM".into(),
        since: Some(since),
        until: Some(since + 30),
        application: ApplicationFilter::Named("Éditeur".into()),
        status: StatusFilter::Completed,
    };
    let page = f.store.page(&filter, 0, 50).unwrap();
    assert!(!page.items.is_empty());
    for item in page.items {
        assert_eq!(item.target_app_name.as_deref(), Some("Éditeur"));
        assert!(item.started_at.unix_timestamp() >= since);
        assert!(item.started_at.unix_timestamp() < since + 30);
        assert_eq!(item.status, "inserted");
    }
    let applications = f.store.applications().unwrap();
    assert_eq!(applications.len(), 2);
    assert!(applications.contains(&"Éditeur".into()));
    assert!(applications.contains(&"Navigateur".into()));
}

#[test]
fn status_filters_unknown_app_and_unicode_preview_are_bounded() {
    let f = Fixture::new();
    let long = "école 🎙️ 東京 ".repeat(150);
    let (meta, _) = f.add(20, SessionStatus::Inserted, Some(&long));
    f.add(21, SessionStatus::QueuedForRetry, None);
    f.add(22, SessionStatus::Failed, None);
    f.add(23, SessionStatus::Cancelled, None);
    for (filter, expected) in [
        (StatusFilter::Completed, "inserted"),
        (StatusFilter::Pending, "queuedForRetry"),
        (StatusFilter::Failed, "failed"),
        (StatusFilter::Cancelled, "cancelled"),
    ] {
        let result = f
            .store
            .page(
                &HistoryFilter {
                    status: filter,
                    application: ApplicationFilter::Unknown,
                    ..Default::default()
                },
                0,
                50,
            )
            .unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.items[0].status, expected);
    }
    let page = f
        .store
        .page(
            &HistoryFilter {
                status: StatusFilter::Completed,
                ..Default::default()
            },
            0,
            50,
        )
        .unwrap();
    let row = &page.items[0];
    assert_eq!(row.id, meta.id.to_string());
    assert!(row.preview.chars().count() <= 240);
    assert!(row.preview.starts_with("école"));
    assert!(row.word_count > 100);
    assert!(!row.corrected);
    let (mut live, folder) = f.add(24, SessionStatus::Recording, Some("not published"));
    live.target_app_name = Some("Live application".into());
    live.write(&folder);
    f.store.upsert(&live, &folder);
    assert!(f.store.applications().unwrap().is_empty());
}

#[test]
fn edit_updates_counts_and_notifications_once_and_rejects_blank_or_stale_text() {
    let f = Fixture::new();
    let (meta, _) = f.add(30, SessionStatus::Inserted, Some("un deux"));
    let id = meta.id.to_string();
    let mut changes = f.store.subscribe();
    f.store
        .edit(&id, "un deux", "un deux trois quatre")
        .unwrap();
    assert_eq!(changes.try_recv(), Ok(true));
    assert!(changes.try_recv().is_err());
    assert_eq!(f.store.stats().total_words, 4);
    assert_eq!(f.store.record(&id).unwrap().word_count(), 4);
    assert_eq!(f.store.stats().total_dictations, 1);
    let activity = f.store.activity(datetime!(2026-11-11 20:00 UTC)).unwrap();
    assert_eq!(activity.days.values().map(|day| day.words).sum::<u64>(), 4);
    let page = f.store.page(&HistoryFilter::default(), 0, 50).unwrap();
    assert_eq!(page.items[0].word_count, 4);
    assert!(page.items[0].corrected);
    assert!(f.store.edit(&id, "un deux", "stale").is_err());
    assert!(
        f.store
            .edit(&id, "un deux trois quatre", "  \n\t ")
            .is_err()
    );
    assert!(changes.try_recv().is_err());
    f.store.reindex();
    assert_eq!(f.store.stats().total_words, 4);
    assert_eq!(f.store.corrections(&id).unwrap().len(), 1);
    assert_eq!(f.store.delete_many(&[id]).unwrap(), 1);
    assert_eq!(f.store.stats().total_words, 0);
    assert_eq!(f.store.stats().total_dictations, 0);
    assert!(
        f.store
            .activity(datetime!(2026-11-11 20:00 UTC))
            .unwrap()
            .days
            .is_empty()
    );
}

#[test]
fn bulk_delete_removes_audio_and_corrections_and_prevalidates_all_targets() {
    let f = Fixture::new();
    let (one, first) = f.add(40, SessionStatus::Inserted, Some("premier"));
    let (two, second) = f.add(41, SessionStatus::Failed, None);
    std::fs::write(file_layout::audio_wav(&first), b"fixture audio").unwrap();
    std::fs::write(file_layout::audio_wav(&second), b"fixture audio").unwrap();
    let one_id = one.id.to_string();
    let two_id = two.id.to_string();
    f.store.edit(&one_id, "premier", "corrigé").unwrap();
    let outside = tempfile::tempdir().unwrap();
    let unsafe_meta = SessionMeta::new(Uuid::new_v4(), one.started_at, SessionStatus::Inserted);
    unsafe_meta.write(outside.path());
    f.store.upsert(&unsafe_meta, outside.path());
    assert!(
        f.store
            .delete_many(&[one_id.clone(), unsafe_meta.id.to_string()])
            .is_err()
    );
    assert!(first.exists());
    assert!(outside.path().exists());
    assert!(f.store.record(&one_id).is_some());
    let mut changes = f.store.subscribe();
    assert_eq!(
        f.store
            .delete_many(&[one_id.clone(), one_id.clone(), two_id.clone()])
            .unwrap(),
        2
    );
    assert!(!first.exists());
    assert!(!second.exists());
    assert!(f.store.record(&one_id).is_none());
    assert!(f.store.record(&two_id).is_none());
    assert!(matches(&f.store, "corrig").is_empty());
    assert_eq!(changes.try_recv(), Ok(true));
    assert!(changes.try_recv().is_err());
    f.store.reindex();
    assert!(f.store.record(&one_id).is_none());
}

#[test]
fn destructive_operations_reject_wrong_identity_and_live_sessions() {
    let f = Fixture::new();
    let (meta, folder) = f.add(50, SessionStatus::Inserted, Some("intact"));
    let id = meta.id.to_string();
    let mut substituted = meta.clone();
    substituted.id = Uuid::new_v4();
    substituted.write(&folder);
    assert!(f.store.delete_many(std::slice::from_ref(&id)).is_err());
    assert!(f.store.edit(&id, "intact", "wrong session").is_err());
    assert!(folder.exists());
    meta.write(&folder);
    for status in [
        SessionStatus::Recording,
        SessionStatus::Recorded,
        SessionStatus::Transcribing,
    ] {
        let mut live = meta.clone();
        live.status = status;
        live.write(&folder);
        f.store.upsert(&live, &folder);
        assert!(f.store.delete_many(std::slice::from_ref(&id)).is_err());
        assert!(f.store.edit(&id, "intact", "busy").is_err());
        assert!(folder.exists());
    }
}

#[test]
fn retry_requires_a_retained_audio_file_and_terminal_retryable_status() {
    let f = Fixture::new();
    let (mut meta, folder) = f.add(60, SessionStatus::Failed, None);
    meta.error_code = Some("network".into());
    meta.write(&folder);
    f.store.upsert(&meta, &folder);
    let id = meta.id.to_string();
    assert!(!f.store.can_retry(&id));
    std::fs::write(file_layout::audio_wav(&folder), b"fixture audio").unwrap();
    assert!(f.store.can_retry(&id));
    for code in ["auth", "quota", "model", "offline"] {
        meta.error_code = Some(code.into());
        meta.write(&folder);
        f.store.upsert(&meta, &folder);
        assert!(
            f.store.can_retry(&id),
            "{code} can be resolved before a manual retry"
        );
        assert!(
            f.store.retryable_records().is_empty(),
            "account errors must not auto-retry"
        );
    }
    meta.error_code = Some("safety".into());
    meta.write(&folder);
    f.store.upsert(&meta, &folder);
    assert!(!f.store.can_retry(&id));
    meta.status = SessionStatus::Transcribing;
    meta.write(&folder);
    f.store.upsert(&meta, &folder);
    assert!(!f.store.can_retry(&id));
    assert!(!f.store.can_retry("missing"));
}

#[test]
fn opening_a_legacy_index_backfills_search_without_altering_transcripts() {
    let f = Fixture::new();
    let path = f.temp.path().join("legacy.sqlite");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch(
        "CREATE TABLE dictation (
            id TEXT PRIMARY KEY, folder TEXT NOT NULL, started_at INTEGER NOT NULL, status TEXT NOT NULL,
            target_app_name TEXT, target_app_exe TEXT, duration_seconds REAL, raw_transcript TEXT,
            cleaned_transcript TEXT, error_code TEXT, error_message TEXT, pipeline_seconds REAL
         );
         INSERT INTO dictation (id, folder, started_at, status, raw_transcript, cleaned_transcript)
         VALUES ('legacy', 'fixture', 1794398400, 'inserted', 'ancienne sortie', 'Déjà nettoyée');"
    ).unwrap();
    drop(connection);
    for _ in 0..2 {
        let store = HistoryStore::open(&path).unwrap();
        assert_eq!(matches(&store, "deja nettoy"), vec!["legacy"]);
        assert_eq!(store.stats().total_words, 2);
        let record = store.record("legacy").unwrap();
        assert_eq!(record.raw_transcript.as_deref(), Some("ancienne sortie"));
        assert_eq!(record.cleaned_transcript.as_deref(), Some("Déjà nettoyée"));
        assert!(record.user_transcript.is_none());
    }
}

#[cfg(windows)]
#[test]
fn locked_correction_file_reports_save_failure_and_preserves_the_previous_version() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = Fixture::new();
    let (meta, folder) = f.add(70, SessionStatus::Inserted, Some("original"));
    let id = meta.id.to_string();
    f.store.edit(&id, "original", "première version").unwrap();
    let journal_path = folder.join("corrections.json");
    let before = std::fs::read(&journal_path).unwrap();
    // Allow reads while denying replacement, as another Windows application can.
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&journal_path)
        .unwrap();
    let mut changes = f.store.subscribe();
    assert!(
        f.store
            .edit(&id, "première version", "seconde version")
            .is_err()
    );
    assert_eq!(std::fs::read(&journal_path).unwrap(), before);
    assert_eq!(
        f.store.record(&id).unwrap().display_text(),
        "première version"
    );
    assert_eq!(f.store.corrections(&id).unwrap().len(), 1);
    assert!(changes.try_recv().is_err());
    drop(lock);
    f.store
        .edit(&id, "première version", "seconde version")
        .unwrap();
    assert_eq!(
        f.store.record(&id).unwrap().display_text(),
        "seconde version"
    );
}

#[test]
fn period_start_keeps_today_and_includes_the_requested_calendar_span() {
    let now = datetime!(2026-11-11 12:00 UTC);
    let today = OffsetDateTime::from_unix_timestamp(period_start(1, now).unwrap()).unwrap();
    let week = OffsetDateTime::from_unix_timestamp(period_start(7, now).unwrap()).unwrap();
    assert!(today <= now);
    assert!(now - today < Duration::hours(26));
    // A local-calendar week may cross a daylight-saving boundary.
    assert!((today - week - Duration::days(6)).abs() <= Duration::hours(2));
}

#[test]
fn dictionary_source_ids_are_small_and_follow_corrections_and_deletion() {
    let f = Fixture::new();
    let (first, _) = f.add(101, SessionStatus::Inserted, Some("J’utilise Proxmocks."));
    let (_second, folder) = f.add(102, SessionStatus::Inserted, Some("Autre dictée."));
    let id = first.id.to_string();
    assert!(f.store.correction_session_ids().unwrap().is_empty());
    f.store
        .edit(&id, "J’utilise Proxmocks.", "J’utilise Proxmox.")
        .unwrap();
    assert_eq!(f.store.correction_session_ids().unwrap(), vec![id.clone()]);
    std::fs::remove_dir_all(folder).unwrap();
    f.store.reindex();
    assert_eq!(f.store.correction_session_ids().unwrap(), vec![id.clone()]);
    f.store.delete_many(&[id]).unwrap();
    assert!(f.store.correction_session_ids().unwrap().is_empty());
}

#[test]
fn removing_a_correction_journal_removes_it_from_dictionary_sources() {
    let f = Fixture::new();
    let (meta, folder) = f.add(103, SessionStatus::Inserted, Some("J’utilise Proxmocks."));
    let id = meta.id.to_string();
    f.store
        .edit(&id, "J’utilise Proxmocks.", "J’utilise Proxmox.")
        .unwrap();
    assert_eq!(f.store.correction_session_ids().unwrap(), vec![id]);
    std::fs::remove_file(folder.join("corrections.json")).unwrap();
    assert!(f.store.correction_session_ids().unwrap().is_empty());
    assert!(folder.join("meta.json").is_file());
}

#[test]
fn dictionary_learning_keeps_history_authoritative_and_only_accepted_rules_reach_pipeline() {
    use jot_core::dictionary::{DictionaryStore, SuggestionStatus};
    use jot_core::replacement;
    let f = Fixture::new();
    let (meta, folder) = f.add(
        110,
        SessionStatus::Inserted,
        Some("J’ai installé Proxmocks."),
    );
    let id = meta.id.to_string();
    let meta_bytes = std::fs::read(folder.join("meta.json")).unwrap();
    f.store
        .edit(&id, "J’ai installé Proxmocks.", "J’ai installé Proxmox.")
        .unwrap();
    let journal_bytes = std::fs::read(folder.join("corrections.json")).unwrap();
    let dictionary_path = f.temp.path().join("dictionary.json");
    let dictionary = DictionaryStore::open(dictionary_path.clone());
    assert_eq!(
        dictionary
            .learn_from_corrections(&f.store.corrections(&id).unwrap())
            .unwrap(),
        1
    );
    assert!(dictionary.replacement_rules().is_empty());
    assert!(dictionary.sanitized_vocabulary(2048).is_empty());
    let suggestion = dictionary.suggestions()[0].clone();
    dictionary.reject_suggestion(suggestion.id).unwrap();
    let reopened = DictionaryStore::open(dictionary_path.clone());
    assert_eq!(
        reopened
            .learn_from_corrections(&f.store.corrections(&id).unwrap())
            .unwrap(),
        0
    );
    assert_eq!(reopened.suggestions()[0].status, SuggestionStatus::Rejected);

    let (second, _) = f.add(111, SessionStatus::Inserted, Some("Voici Klod Code."));
    let second_id = second.id.to_string();
    f.store
        .edit(&second_id, "Voici Klod Code.", "Voici Claude Code.")
        .unwrap();
    assert_eq!(
        reopened
            .learn_from_corrections(&f.store.corrections(&second_id).unwrap())
            .unwrap(),
        1
    );
    let accepted = reopened
        .suggestions()
        .into_iter()
        .find(|s| s.status == SuggestionStatus::Pending)
        .unwrap();
    reopened.accept_suggestion(accepted.id).unwrap();
    assert_eq!(
        replacement::apply(&reopened.replacement_rules(), "Klod Code et Klod Codebase."),
        "Claude Code et Klod Codebase."
    );
    assert_eq!(reopened.sanitized_vocabulary(2048), vec!["Claude Code"]);
    assert_eq!(std::fs::read(folder.join("meta.json")).unwrap(), meta_bytes);
    f.store.reindex();
    assert_eq!(
        std::fs::read(folder.join("corrections.json")).unwrap(),
        journal_bytes
    );
    assert_eq!(
        reopened
            .learn_from_corrections(&f.store.corrections(&id).unwrap())
            .unwrap(),
        0
    );
    f.store.delete_many(&[id, second_id]).unwrap();
    reopened
        .purge_orphan_suggestions(
            &f.store
                .correction_session_ids()
                .unwrap()
                .into_iter()
                .collect(),
        )
        .unwrap();
    let final_store = DictionaryStore::open(dictionary_path);
    assert_eq!(
        final_store.entries().len(),
        1,
        "accepted rules belong to the user after source deletion"
    );
    assert_eq!(final_store.entries()[0].term, "Claude Code");
    assert!(
        final_store
            .suggestions()
            .iter()
            .any(|s| s.status == SuggestionStatus::Rejected)
    );
}

#[test]
fn deleting_a_dictation_purges_its_pending_suggestion_but_keeps_accepted_rules() {
    use jot_core::dictionary::{DictionaryStore, SuggestionStatus};
    let f = Fixture::new();
    let (kept, _) = f.add(120, SessionStatus::Inserted, Some("Voici Klod Code."));
    let (pending, _) = f.add(121, SessionStatus::Inserted, Some("J’ai Proxmocks."));
    let (kept, pending) = (kept.id.to_string(), pending.id.to_string());
    f.store
        .edit(&kept, "Voici Klod Code.", "Voici Claude Code.")
        .unwrap();
    f.store
        .edit(&pending, "J’ai Proxmocks.", "J’ai Proxmox.")
        .unwrap();
    let path = f.temp.path().join("dictionary.json");
    let dictionary = DictionaryStore::open(path.clone());
    for id in [&kept, &pending] {
        dictionary
            .learn_from_corrections(&f.store.corrections(id).unwrap())
            .unwrap();
    }
    let accepted = dictionary
        .suggestions()
        .into_iter()
        .find(|s| s.sources.iter().any(|source| source.dictation_id == kept))
        .unwrap();
    dictionary.accept_suggestion(accepted.id).unwrap();

    // Same sequence as the History view's delete action, without opening Dictionary.
    f.store.delete_many(&[kept, pending]).unwrap();
    let live = f.store.correction_session_ids().unwrap();
    assert_eq!(
        dictionary
            .purge_orphan_suggestions(&live.into_iter().collect())
            .unwrap(),
        1
    );
    let reopened = DictionaryStore::open(path);
    assert!(
        reopened
            .suggestions()
            .iter()
            .all(|s| s.status != SuggestionStatus::Pending)
    );
    assert_eq!(reopened.entries().len(), 1);
    assert_eq!(reopened.entries()[0].term, "Claude Code");
}
