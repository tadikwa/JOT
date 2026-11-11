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

//! A queryable index over the session folders.
//!
//! The folders remain the source of truth — one `meta.json` per dictation. The
//! database only makes History fast to search and stats cheap, so a corrupt
//! index is a rebuild rather than a data loss.

mod library;
pub use library::*;

use crate::activity::{Activity, LocalCalendar, Totals};
use crate::file_layout;
use crate::meta::{SessionMeta, SessionStatus};
use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{Connection, OpenFlags, functions::FunctionFlags, params};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use time::{Date, OffsetDateTime};
use tokio::sync::broadcast;
use unicode_segmentation::UnicodeSegmentation;

/// History is a library of words plus things needing attention — never an event
/// log. Visible: anything with a transcript; retryable failures and
/// offline-queued items; long cancelled recordings (recoverable). Silent rows
/// and short cancels are discarded at the source and filtered here for legacy
/// data. In-flight statuses are NOT visible: the live session would surface in
/// the attention shelf with Retry/Discard controls that double-upload or
/// destroy it mid-flight. Crash recovery reads those separately.
const VISIBLE: &str = "(raw_transcript IS NOT NULL OR cleaned_transcript IS NOT NULL \
     OR status IN ('failed','queuedForRetry') \
     OR (status = 'cancelled' AND duration_seconds >= 10)) \
     AND status != 'silent'";

/// Error codes that a retry could plausibly clear.
const RETRYABLE_ERROR_CODES: [&str; 3] = ["network", "timeout", "rate_limit"];
const SUCCESS: &str =
    "status IN ('inserted','copiedToClipboard','awaitingChip','heldSecure','recovered')
    AND word_count > 0";
const RETAINED_CANCEL: &str = "status = 'cancelled' AND duration_seconds >= 10";
const HAS_DURATION: &str = "duration_seconds > 0 AND duration_seconds <= 1.7976931348623157e308";
const HAS_PIPELINE: &str = "status != 'recovered' AND pipeline_seconds >= 0
    AND pipeline_seconds <= 1.7976931348623157e308";

fn activity_filter() -> String {
    format!("({SUCCESS}) OR status IN ('failed','queuedForRetry') OR ({RETAINED_CANCEL})")
}

fn count_words(text: &str) -> i64 {
    text.unicode_words().count() as i64
}

#[derive(Debug, Clone, PartialEq)]
pub struct DictationRecord {
    pub id: String,
    pub folder: PathBuf,
    pub started_at: OffsetDateTime,
    pub status: String,
    pub target_app_name: Option<String>,
    pub target_app_exe: Option<String>,
    pub duration_seconds: Option<f64>,
    pub raw_transcript: Option<String>,
    pub cleaned_transcript: Option<String>,
    pub user_transcript: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub pipeline_seconds: Option<f64>,
}

impl DictationRecord {
    pub fn from_meta(meta: &SessionMeta, folder: &Path) -> Self {
        Self {
            id: meta.id.to_string(),
            folder: folder.to_path_buf(),
            started_at: meta.started_at,
            status: meta.status.as_str().to_string(),
            target_app_name: meta.target_app_name.clone(),
            target_app_exe: meta.target_app_exe.clone(),
            duration_seconds: meta.audio_duration_seconds,
            raw_transcript: meta.raw_transcript.clone(),
            cleaned_transcript: meta.cleaned_transcript.clone(),
            user_transcript: None,
            error_code: meta.error_code.clone(),
            error_message: meta.error_message.clone(),
            pipeline_seconds: meta.pipeline_seconds,
        }
    }

    pub fn display_text(&self) -> &str {
        self.user_transcript
            .as_deref()
            .or(self.cleaned_transcript.as_deref())
            .or(self.raw_transcript.as_deref())
            .unwrap_or("")
    }

    pub fn status(&self) -> Option<SessionStatus> {
        SessionStatus::parse(&self.status)
    }

    pub fn word_count(&self) -> usize {
        count_words(self.display_text()) as usize
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub total_words: usize,
    pub total_dictations: usize,
    pub average_wpm: u32,
}

pub struct HistoryStore {
    path: PathBuf,
    connection: Mutex<Connection>,
    changes: broadcast::Sender<bool>,
}

impl HistoryStore {
    pub fn standard() -> Result<Self> {
        Self::open(&file_layout::history_sqlite())
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("creating the history directory")?;
        }
        let connection = Self::open_or_recreate(path)?;
        let (changes, _) = broadcast::channel(16);
        let store = Self {
            path: path.to_path_buf(),
            connection: Mutex::new(connection),
            changes,
        };
        store.migrate()?;
        Ok(store)
    }

    /// A corrupt index must not disable history and recovery forever — the
    /// folders are the source of truth and `reindex` rebuilds from them, so the
    /// broken file is quarantined and a fresh one takes its place.
    fn open_or_recreate(path: &Path) -> Result<Connection> {
        match Self::open_verified(path) {
            Ok(connection) => Ok(connection),
            Err(error) => {
                tracing::error!(%error, "history index unreadable — quarantining and recreating");
                let stamp = OffsetDateTime::now_utc().unix_timestamp();
                let quarantine = path.with_extension(format!("corrupt-{stamp}.sqlite"));
                let _ = std::fs::rename(path, &quarantine);
                Self::open_verified(path)
            }
        }
    }

    fn open_verified(path: &Path) -> Result<Connection> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        // WAL plus NORMAL: the folders are the source of truth and `reindex`
        // rebuilds the index from them, so a rollback journal's per-write fsync
        // buys nothing but launch stalls.
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        // Probe readability so corruption surfaces here, not at the first query.
        connection.pragma_query(None, "schema_version", |_| Ok(()))?;
        Ok(connection)
    }

    fn migrate(&self) -> Result<()> {
        let mut connection = self.connection.lock();
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS dictation (
                 id TEXT PRIMARY KEY,
                 folder TEXT NOT NULL,
                 started_at INTEGER NOT NULL,
                 status TEXT NOT NULL,
                 target_app_name TEXT,
                 target_app_exe TEXT,
                 duration_seconds REAL,
                 raw_transcript TEXT,
                 cleaned_transcript TEXT,
                 error_code TEXT,
                 error_message TEXT,
                 pipeline_seconds REAL,
                 word_count INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS dictation_started_at ON dictation (started_at);
             CREATE INDEX IF NOT EXISTS dictation_status ON dictation (status);",
        )?;
        let has_count: bool = transaction.query_row(
            "SELECT EXISTS (SELECT 1 FROM pragma_table_info('dictation') WHERE name = 'word_count')",
            [],
            |row| row.get(0),
        )?;
        if !has_count {
            transaction.create_scalar_function(
                "jot_word_count",
                1,
                FunctionFlags::SQLITE_DETERMINISTIC,
                |ctx| Ok(count_words(ctx.get_raw(0).as_str()?)),
            )?;
            transaction.execute_batch(
                "ALTER TABLE dictation ADD COLUMN word_count INTEGER NOT NULL DEFAULT 0;
                 UPDATE dictation SET word_count = jot_word_count(coalesce(cleaned_transcript, raw_transcript, ''));",
            )?;
        }
        transaction.execute_batch(&format!(
            "CREATE INDEX IF NOT EXISTS dictation_activity
             ON dictation (started_at, status, word_count, duration_seconds, pipeline_seconds)
             WHERE {}",
            activity_filter()
        ))?;
        library::migrate(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    /// Fires after any write so an open History pane refreshes as dictations
    /// land, retries drain, or rows are deleted — no polling.
    /// The payload says whether the write can affect dashboard totals.
    pub fn subscribe(&self) -> broadcast::Receiver<bool> {
        self.changes.subscribe()
    }

    fn notify(&self, affects_activity: bool) {
        let _ = self.changes.send(affects_activity);
    }

    // ----- Writes

    pub fn upsert(&self, meta: &SessionMeta, folder: &Path) {
        let connection = self.connection.lock();
        let affects_activity = !meta.status.is_in_flight()
            || connection
                .query_row(
                    &format!(
                        "SELECT EXISTS (SELECT 1 FROM dictation WHERE id = ?1 AND ({}))",
                        activity_filter()
                    ),
                    [meta.id.to_string()],
                    |row| row.get(0),
                )
                .unwrap_or(true);
        let result = Self::write_record(&connection, meta, folder);
        drop(connection);
        self.finish_upsert(result, affects_activity);
    }

    /// A retry may finish while the user deletes its original recording.
    pub(crate) fn save_retry(&self, meta: &SessionMeta, folder: &Path) -> bool {
        let result = (|| {
            let connection = self.connection.lock();
            let exists: bool = connection.query_row(
                "SELECT EXISTS (SELECT 1 FROM dictation WHERE id = ?1)",
                [meta.id.to_string()],
                |row| row.get(0),
            )?;
            if !exists || !folder.is_dir() {
                return Ok(None);
            }
            meta.write(folder);
            Self::write_record(&connection, meta, folder).map(Some)
        })();
        match result {
            Ok(None) => false,
            Ok(Some(changed)) => {
                if changed > 0 {
                    self.notify(true);
                }
                true
            }
            Err(error) => {
                self.finish_upsert(Err(error), true);
                false
            }
        }
    }

    fn write_record(
        connection: &Connection,
        meta: &SessionMeta,
        folder: &Path,
    ) -> rusqlite::Result<usize> {
        let mut record = DictationRecord::from_meta(meta, folder);
        record.user_transcript = library::read_corrections(folder, &record.id)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(error.into()))?
            .last()
            .map(|correction| correction.after.clone());
        connection.execute(
            "INSERT INTO dictation (id, folder, started_at, status, target_app_name,
                 target_app_exe, duration_seconds, raw_transcript, cleaned_transcript,
                 error_code, error_message, pipeline_seconds, word_count, user_transcript)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(id) DO UPDATE SET
                 folder = excluded.folder, started_at = excluded.started_at,
                 status = excluded.status, target_app_name = excluded.target_app_name,
                 target_app_exe = excluded.target_app_exe,
                 duration_seconds = excluded.duration_seconds,
                 raw_transcript = excluded.raw_transcript,
                 cleaned_transcript = excluded.cleaned_transcript,
                 error_code = excluded.error_code, error_message = excluded.error_message,
                 pipeline_seconds = excluded.pipeline_seconds, word_count = excluded.word_count,
                 user_transcript = excluded.user_transcript
             WHERE (dictation.folder, dictation.started_at, dictation.status, dictation.target_app_name,
                    dictation.target_app_exe, dictation.duration_seconds, dictation.raw_transcript,
                    dictation.cleaned_transcript, dictation.error_code, dictation.error_message,
                    dictation.pipeline_seconds, dictation.word_count, dictation.user_transcript)
                IS NOT (excluded.folder, excluded.started_at, excluded.status, excluded.target_app_name,
                        excluded.target_app_exe, excluded.duration_seconds, excluded.raw_transcript,
                        excluded.cleaned_transcript, excluded.error_code, excluded.error_message,
                        excluded.pipeline_seconds, excluded.word_count, excluded.user_transcript)",
            params![
                record.id,
                record.folder.to_string_lossy(),
                record.started_at.unix_timestamp(),
                record.status,
                record.target_app_name,
                record.target_app_exe,
                record.duration_seconds,
                record.raw_transcript,
                record.cleaned_transcript,
                record.error_code,
                record.error_message,
                record.pipeline_seconds,
                count_words(record.display_text()),
                record.user_transcript,
            ],
        )
    }

    fn finish_upsert(&self, result: rusqlite::Result<usize>, affects_activity: bool) {
        match result {
            Ok(changed) if changed > 0 => self.notify(affects_activity),
            Ok(_) => {}
            Err(error) => tracing::error!(%error, "history upsert failed"),
        }
    }

    pub fn delete(&self, id: &str, remove_folder: bool) {
        let folder = self.record(id).map(|record| record.folder);
        let deleted = self
            .connection
            .lock()
            .execute("DELETE FROM dictation WHERE id = ?1", params![id]);
        match deleted {
            Ok(_) => {
                if remove_folder && let Some(folder) = folder {
                    let _ = std::fs::remove_dir_all(folder);
                }
                self.notify(true);
            }
            Err(error) => tracing::error!(%error, "history delete failed"),
        }
    }

    /// Wipes the index and, optionally, the folders — sweeping the DIRECTORY
    /// rather than the query, because the visible-records filter hides cancelled
    /// sessions whose audio would otherwise survive a "delete everything".
    /// The live session's folder is spared.
    pub fn delete_all(&self, remove_folders: bool, sparing: Option<&Path>) {
        if let Err(error) = self.connection.lock().execute("DELETE FROM dictation", []) {
            tracing::error!(%error, "history delete-all failed");
        }
        self.notify(true);
        if !remove_folders {
            return;
        }
        let Ok(entries) = std::fs::read_dir(file_layout::recordings_root()) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if sparing.is_some_and(|spared| same_path(spared, &path)) {
                continue;
            }
            let _ = std::fs::remove_dir_all(path);
        }
    }

    /// Rebuilds the index from the folders on disk (launch reconciliation).
    ///
    /// Folders are the source of truth: rows whose folders vanished (external
    /// cleanup, a manual delete in Explorer) are pruned rather than shown as
    /// ghosts. The prune walks the UNFILTERED table — `records` hides silent and
    /// short-cancelled rows, and exactly those would otherwise linger forever as
    /// invisible ghosts.
    pub fn reindex(&self) {
        let Ok(entries) = std::fs::read_dir(file_layout::recordings_root()) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir()
                && !entry.file_name().to_string_lossy().starts_with(".deleted-")
                && let Some(meta) = SessionMeta::read(&path)
            {
                self.upsert(&meta, &path);
            }
        }
        let orphans: Vec<String> = self
            .query("SELECT id, folder FROM dictation", &[], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .into_iter()
            .filter(|(_, folder)| !Path::new(folder).exists())
            .map(|(id, _)| id)
            .collect();
        for id in &orphans {
            self.delete(id, false);
        }
        if !orphans.is_empty() {
            tracing::info!(count = orphans.len(), "reindex pruned orphaned rows");
        }
    }

    // ----- Reads

    fn query<T>(
        &self,
        sql: &str,
        args: &[&dyn rusqlite::ToSql],
        map: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Vec<T> {
        let connection = self.connection.lock();
        let Ok(mut statement) = connection.prepare(sql) else {
            return Vec::new();
        };
        match statement.query_map(args, map) {
            Ok(rows) => rows.flatten().collect(),
            Err(error) => {
                tracing::error!(%error, "history query failed");
                Vec::new()
            }
        }
    }

    pub fn record(&self, id: &str) -> Option<DictationRecord> {
        self.query(
            "SELECT * FROM dictation WHERE id = ?1",
            &[&id],
            row_to_record,
        )
        .pop()
    }

    pub fn records(&self, search: Option<&str>, limit: usize) -> Vec<DictationRecord> {
        match search.map(str::trim).filter(|query| !query.is_empty()) {
            Some(query) => {
                // Escape LIKE wildcards so a search for "100%" finds "100%".
                let pattern = format!(
                    "%{}%",
                    query
                        .replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_")
                );
                self.query(
                    &format!(
                        "SELECT * FROM dictation WHERE (raw_transcript LIKE ?1 ESCAPE '\\'
                             OR cleaned_transcript LIKE ?1 ESCAPE '\\'
                             OR target_app_name LIKE ?1 ESCAPE '\\') AND {VISIBLE}
                         ORDER BY started_at DESC LIMIT ?2"
                    ),
                    &[&pattern, &(limit as i64)],
                    row_to_record,
                )
            }
            None => self.query(
                &format!(
                    "SELECT * FROM dictation WHERE {VISIBLE} ORDER BY started_at DESC LIMIT ?1"
                ),
                &[&(limit as i64)],
                row_to_record,
            ),
        }
    }

    /// Sessions the retry queue should drain: offline-queued plus transient
    /// failures. A daily quota or an auth wall is deliberately absent — those
    /// are account-level and must not be re-sent on a loop.
    pub fn retryable_records(&self) -> Vec<DictationRecord> {
        self.query(
            "SELECT * FROM dictation
             WHERE status = 'queuedForRetry'
                OR (status = 'failed' AND error_code IN (?1, ?2, ?3))
             ORDER BY started_at ASC",
            &[
                &RETRYABLE_ERROR_CODES[0],
                &RETRYABLE_ERROR_CODES[1],
                &RETRYABLE_ERROR_CODES[2],
            ],
            row_to_record,
        )
    }

    /// Non-terminal sessions found at launch: the app died mid-flight.
    pub fn interrupted_records(&self) -> Vec<DictationRecord> {
        self.query(
            "SELECT * FROM dictation
             WHERE status IN ('recording', 'recorded', 'transcribing')
             ORDER BY started_at DESC",
            &[],
            row_to_record,
        )
    }

    /// Every row id, visibility filter bypassed — observability for tests and
    /// the diagnostics pane.
    pub fn all_ids(&self) -> Vec<String> {
        self.query("SELECT id FROM dictation ORDER BY started_at", &[], |row| {
            row.get(0)
        })
    }

    pub fn stats(&self) -> Stats {
        self.query(
            &format!(
                "SELECT coalesce(sum(word_count), 0), count(*),
                        coalesce(60.0 * sum(word_count) FILTER (WHERE {HAS_DURATION})
                                 / sum(duration_seconds) FILTER (WHERE {HAS_DURATION}), 0)
                 FROM dictation WHERE {SUCCESS}"
            ),
            &[],
            |row| {
                Ok(Stats {
                    total_words: row.get(0)?,
                    total_dictations: row.get(1)?,
                    average_wpm: row.get::<_, f64>(2)?.round() as u32,
                })
            },
        )
        .pop()
        .unwrap_or_default()
    }

    pub fn activity(&self, now: OffsetDateTime) -> Result<Activity> {
        let calendar = LocalCalendar::current()?;
        let today = calendar.date(now)?;
        self.activity_with(now, today, move |utc| calendar.date(utc))
    }

    fn activity_with(
        &self,
        now: OffsetDateTime,
        today: Date,
        local_date: impl Fn(OffsetDateTime) -> Result<Date> + Send + 'static,
    ) -> Result<Activity> {
        // WAL keeps this snapshot off the coordinator's write connection.
        let connection = Connection::open_with_flags(&self.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.create_scalar_function(
            "jot_local_day",
            1,
            FunctionFlags::SQLITE_DETERMINISTIC,
            move |ctx| {
                let timestamp = ctx.get::<i64>(0)?;
                OffsetDateTime::from_unix_timestamp(timestamp)
                    .map_err(anyhow::Error::from)
                    .and_then(&local_date)
                    .map(Date::to_julian_day)
                    .map_err(|error| rusqlite::Error::UserFunctionError(error.into()))
            },
        )?;
        let sql = format!(
            "SELECT jot_local_day(started_at) AS day,
                    coalesce(sum(word_count) FILTER (WHERE success), 0),
                    count(*) FILTER (WHERE success),
                    coalesce(sum(word_count) FILTER (WHERE timed), 0),
                    count(*) FILTER (WHERE timed),
                    coalesce(sum(duration_seconds) FILTER (WHERE timed), 0.0),
                    count(*) FILTER (WHERE timed AND pipeline),
                    coalesce(sum(pipeline_seconds) FILTER (WHERE timed AND pipeline), 0.0),
                    count(*) FILTER (WHERE status = 'failed'),
                    count(*) FILTER (WHERE status = 'cancelled'),
                    count(*) FILTER (WHERE status = 'queuedForRetry')
             FROM (
                 SELECT started_at, word_count, status, duration_seconds, pipeline_seconds,
                        ({SUCCESS}) AS success,
                        ({SUCCESS}) AND ({HAS_DURATION}) AS timed,
                        ({HAS_PIPELINE}) AS pipeline
                 FROM dictation INDEXED BY dictation_activity
                 WHERE ({}) AND started_at <= ?1
             )
             GROUP BY day ORDER BY day",
            activity_filter(),
        );
        let mut statement = connection.prepare(&sql)?;
        let mut rows = statement.query([now.unix_timestamp()])?;
        let mut days = BTreeMap::new();
        while let Some(row) = rows.next()? {
            let date = Date::from_julian_day(row.get(0)?)?;
            if date <= today {
                days.insert(
                    date,
                    Totals {
                        words: row.get(1)?,
                        dictations: row.get(2)?,
                        timed_words: row.get(3)?,
                        timed_dictations: row.get(4)?,
                        audio_seconds: row.get(5)?,
                        measured_pipeline: row.get(6)?,
                        pipeline_seconds: row.get(7)?,
                        failed: row.get(8)?,
                        cancelled: row.get(9)?,
                        queued: row.get(10)?,
                    },
                );
            }
        }
        Ok(Activity { today, days })
    }
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<DictationRecord> {
    Ok(DictationRecord {
        id: row.get("id")?,
        folder: PathBuf::from(row.get::<_, String>("folder")?),
        started_at: OffsetDateTime::from_unix_timestamp(row.get("started_at")?)
            .unwrap_or(OffsetDateTime::UNIX_EPOCH),
        status: row.get("status")?,
        target_app_name: row.get("target_app_name")?,
        target_app_exe: row.get("target_app_exe")?,
        duration_seconds: row.get("duration_seconds")?,
        raw_transcript: row.get("raw_transcript")?,
        cleaned_transcript: row.get("cleaned_transcript")?,
        user_transcript: row.get("user_transcript")?,
        error_code: row.get("error_code")?,
        error_message: row.get("error_message")?,
        pipeline_seconds: row.get("pipeline_seconds")?,
    })
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

// ---------------------------------------------------------------------------
// Retention
// ---------------------------------------------------------------------------

/// Audio retention: purge audio files after N days while KEEPING transcripts
/// and meta forever (until the user deletes them). Audio is never deleted
/// before a transcript exists — that would break Retry.
#[derive(Debug, Clone, Copy)]
pub struct RetentionPolicy {
    /// `0` keeps audio forever; a negative value means "never keep audio" and
    /// purges as soon as a transcript exists.
    pub audio_retention_days: i64,
}

impl RetentionPolicy {
    pub fn purge_expired_audio(&self, recordings_root: &Path, now: OffsetDateTime) -> usize {
        if self.audio_retention_days == 0 {
            return 0;
        }
        let cutoff = if self.audio_retention_days < 0 {
            // Everything eligible, including a dictation that just finished.
            now + time::Duration::seconds(60)
        } else {
            now - time::Duration::days(self.audio_retention_days)
        };

        let Ok(entries) = std::fs::read_dir(recordings_root) else {
            return 0;
        };
        let mut purged = 0;
        for entry in entries.flatten() {
            let folder = entry.path();
            if !folder.is_dir() {
                continue;
            }
            let Some(meta) = SessionMeta::read(&folder) else {
                continue;
            };
            // Transcript-less cancelled rows are retention-IMMUNE: their audio is
            // the only artifact — the "keep long cancels" rule preserved it
            // precisely so Retry works, and History advertises that the audio is
            // kept. Purging it makes that row a lie with a dead-end Retry.
            let has_words = meta.raw_transcript.is_some() || meta.status == SessionStatus::Silent;
            if meta.started_at >= cutoff || !has_words {
                continue;
            }
            let audio = file_layout::audio_wav(&folder);
            if audio.exists() && std::fs::remove_file(&audio).is_ok() {
                purged += 1;
            }
        }
        if purged > 0 {
            tracing::info!(
                purged,
                days = self.audio_retention_days,
                "purged expired audio"
            );
        }
        purged
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::SessionMeta;
    use uuid::Uuid;

    fn store() -> (tempfile::TempDir, HistoryStore) {
        let temp = tempfile::tempdir().unwrap();
        let store = HistoryStore::open(&temp.path().join("history.sqlite")).unwrap();
        (temp, store)
    }

    fn meta(status: SessionStatus) -> SessionMeta {
        SessionMeta::new(Uuid::new_v4(), OffsetDateTime::now_utc(), status)
    }

    #[test]
    fn upsert_updates_rather_than_duplicating() {
        let (temp, store) = store();
        let mut record = meta(SessionStatus::Recorded);
        store.upsert(&record, temp.path());
        record.status = SessionStatus::Inserted;
        record.cleaned_transcript = Some("hello there".into());
        store.upsert(&record, temp.path());

        assert_eq!(store.all_ids().len(), 1);
        let rows = store.records(None, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].display_text(), "hello there");
    }

    #[test]
    fn in_flight_and_silent_rows_stay_out_of_history() {
        let (temp, store) = store();
        for status in [
            SessionStatus::Recording,
            SessionStatus::Recorded,
            SessionStatus::Transcribing,
            SessionStatus::Silent,
        ] {
            store.upsert(&meta(status), temp.path());
        }
        assert!(store.records(None, 10).is_empty());
        assert_eq!(store.all_ids().len(), 4);
    }

    #[test]
    fn a_short_cancel_is_hidden_but_a_long_one_is_recoverable() {
        let (temp, store) = store();
        let mut short = meta(SessionStatus::Cancelled);
        short.audio_duration_seconds = Some(3.0);
        store.upsert(&short, temp.path());
        assert!(store.records(None, 10).is_empty());

        let mut long = meta(SessionStatus::Cancelled);
        long.audio_duration_seconds = Some(42.0);
        store.upsert(&long, temp.path());
        assert_eq!(store.records(None, 10).len(), 1);
    }

    #[test]
    fn search_escapes_like_wildcards() {
        let (temp, store) = store();
        let mut hit = meta(SessionStatus::Inserted);
        hit.cleaned_transcript = Some("we hit 100% of target".into());
        store.upsert(&hit, temp.path());
        let mut miss = meta(SessionStatus::Inserted);
        miss.cleaned_transcript = Some("nothing relevant".into());
        store.upsert(&miss, temp.path());

        assert_eq!(store.records(Some("100%"), 10).len(), 1);
        // A bare "%" searches for a literal percent sign. Unescaped it would be
        // a wildcard and match every row, including "nothing relevant".
        assert_eq!(store.records(Some("%"), 10).len(), 1);
    }

    #[test]
    fn only_transient_failures_are_offered_to_the_retry_queue() {
        let (temp, store) = store();
        for code in ["network", "timeout", "rate_limit", "auth", "quota"] {
            let mut record = meta(SessionStatus::Failed);
            record.error_code = Some(code.into());
            store.upsert(&record, temp.path());
        }
        store.upsert(&meta(SessionStatus::QueuedForRetry), temp.path());
        assert_eq!(store.retryable_records().len(), 4);
    }

    #[test]
    fn interrupted_records_are_the_crash_recovery_set() {
        let (temp, store) = store();
        store.upsert(&meta(SessionStatus::Recording), temp.path());
        store.upsert(&meta(SessionStatus::Transcribing), temp.path());
        store.upsert(&meta(SessionStatus::Inserted), temp.path());
        assert_eq!(store.interrupted_records().len(), 2);
    }

    #[test]
    fn stats_count_words_across_whitespace_and_ignore_untimed_rows_for_wpm() {
        let (temp, store) = store();
        let mut timed = meta(SessionStatus::Inserted);
        timed.cleaned_transcript = Some("one two\nthree four".into());
        timed.audio_duration_seconds = Some(60.0);
        store.upsert(&timed, temp.path());
        let mut untimed = meta(SessionStatus::Inserted);
        untimed.cleaned_transcript = Some("five six".into());
        store.upsert(&untimed, temp.path());

        let stats = store.stats();
        assert_eq!(stats.total_words, 6);
        assert_eq!(stats.total_dictations, 2);
        assert_eq!(stats.average_wpm, 4);
    }

    #[test]
    fn dashboard_uses_final_words_weighted_duration_and_latest_session_status() {
        use time::macros::{date, datetime, offset};
        let (temp, store) = store();
        let now = datetime!(2026-09-02 22:30 UTC);
        let local_day = date!(2026 - 09 - 03);
        let snapshot = || {
            store
                .activity_with(now, local_day, |utc| Ok(utc.to_offset(offset!(+2)).date()))
                .unwrap()
        };
        let mut changes = store.subscribe();
        let mut first = SessionMeta::new(uuid::Uuid::new_v4(), now, SessionStatus::Inserted);
        first.cleaned_transcript = Some("L’heure\tarrive.\nÉcoute-moi ! 🎙".into());
        first.raw_transcript = Some("ignored raw text".into());
        first.audio_duration_seconds = Some(15.0);
        first.pipeline_seconds = Some(3.0);
        store.upsert(&first, temp.path());
        assert_eq!(changes.try_recv(), Ok(true));
        assert_eq!(snapshot().days[&local_day].words, 4);

        let mut retry = SessionMeta::new(uuid::Uuid::new_v4(), now, SessionStatus::Failed);
        retry.audio_duration_seconds = Some(45.0);
        store.upsert(&retry, temp.path());
        assert_eq!(snapshot().days[&local_day].failed, 1);
        retry.status = SessionStatus::Recovered;
        retry.raw_transcript = Some("un deux".into());
        retry.pipeline_seconds = Some(99.0);
        assert!(store.save_retry(&retry, temp.path()));
        assert!(store.save_retry(&retry, temp.path()));
        let mut untimed = SessionMeta::new(uuid::Uuid::new_v4(), now, SessionStatus::HeldSecure);
        untimed.cleaned_transcript = Some("encore".into());
        store.upsert(&untimed, temp.path());
        for status in [
            SessionStatus::Silent,
            SessionStatus::Transcribing,
            SessionStatus::Cancelled,
            SessionStatus::QueuedForRetry,
        ] {
            let mut row = SessionMeta::new(uuid::Uuid::new_v4(), now, status);
            row.raw_transcript = Some("do not count".into());
            row.audio_duration_seconds = Some(20.0);
            store.upsert(&row, temp.path());
        }

        let totals = snapshot().days[&local_day];
        assert_eq!(totals.words, 7);
        assert_eq!(totals.dictations, 3);
        assert_eq!((totals.failed, totals.cancelled, totals.queued), (0, 1, 1));
        assert_eq!(totals.timed_dictations, 2);
        assert_eq!(totals.measured_pipeline, 1);
        assert_eq!(totals.wpm(), Some(6.0));
        assert_eq!(totals.estimated_seconds_saved(30), Some(-51.0));
        assert_eq!(Totals::default().estimated_seconds_saved(30), None);

        first.cleaned_transcript = Some("texte corrigé".into());
        store.upsert(&first, temp.path());
        assert_eq!(snapshot().days[&local_day].words, 5);
        while changes.try_recv().is_ok() {}
        store.delete(&retry.id.to_string(), false);
        assert_eq!(changes.try_recv(), Ok(true));
        assert!(!store.save_retry(&retry, temp.path()));
        assert_eq!(snapshot().days[&local_day].words, 3);
        store.delete_all(false, None);
        assert_eq!(changes.try_recv(), Ok(true));
        assert!(snapshot().days.is_empty());
    }

    #[test]
    fn dashboard_backfills_a_legacy_index_without_altering_transcripts() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("legacy.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(
            "CREATE TABLE dictation (
                id TEXT PRIMARY KEY, folder TEXT NOT NULL, started_at INTEGER NOT NULL, status TEXT NOT NULL,
                target_app_name TEXT, target_app_exe TEXT, duration_seconds REAL, raw_transcript TEXT,
                cleaned_transcript TEXT, error_code TEXT, error_message TEXT, pipeline_seconds REAL
             );
             INSERT INTO dictation (id, folder, started_at, status, raw_transcript)
             VALUES ('old', 'old', 1770000000, 'recovered', 'Déjà là !');",
        ).unwrap();
        drop(connection);
        for _ in 0..2 {
            let store = HistoryStore::open(&path).unwrap();
            assert_eq!(store.stats().total_words, 2);
            assert_eq!(
                store.record("old").unwrap().raw_transcript.as_deref(),
                Some("Déjà là !")
            );
        }
    }

    #[test]
    #[ignore = "performance measurement on an isolated 100,000-row index"]
    fn dashboard_large_history() {
        let (_temp, store) = store();
        let mut connection = store.connection.lock();
        let transaction = connection.transaction().unwrap();
        transaction.execute_batch(
            "WITH RECURSIVE samples(n) AS (VALUES(0) UNION ALL SELECT n + 1 FROM samples WHERE n < 99999)
             INSERT INTO dictation (id, folder, started_at, status, cleaned_transcript, word_count, duration_seconds)
             SELECT n, 'fixture', 1740000000 + n * 360, 'inserted',
                    replace(hex(zeroblob(256)), '0', 'a') || ' words', 2, 4 FROM samples;",
        ).unwrap();
        transaction.commit().unwrap();
        drop(connection);
        for _ in 0..3 {
            let started = std::time::Instant::now();
            let result = store.activity(OffsetDateTime::now_utc()).unwrap();
            let elapsed = started.elapsed();
            assert_eq!(
                result.days.values().map(|day| day.words).sum::<u64>(),
                200_000
            );
            eprintln!(
                "100,000 sessions → {} local days in {elapsed:?}",
                result.days.len()
            );
        }
    }

    #[test]
    fn reindex_prunes_rows_whose_folders_are_gone() {
        let _guard = file_layout::TEST_ROOT_LOCK.lock();
        let temp = tempfile::tempdir().unwrap();
        file_layout::set_override_root(Some(temp.path().to_path_buf()));
        std::fs::create_dir_all(file_layout::recordings_root()).unwrap();
        let store = HistoryStore::open(&temp.path().join("history.sqlite")).unwrap();

        store.upsert(&meta(SessionStatus::Inserted), &temp.path().join("gone"));
        assert_eq!(store.all_ids().len(), 1);
        store.reindex();
        assert!(store.all_ids().is_empty());

        file_layout::set_override_root(None);
    }

    #[test]
    fn a_corrupt_index_is_quarantined_and_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("history.sqlite");
        std::fs::write(&path, b"this is definitely not a database").unwrap();

        let store = HistoryStore::open(&path).unwrap();
        store.upsert(&meta(SessionStatus::Inserted), temp.path());
        assert_eq!(store.all_ids().len(), 1);
        assert!(
            std::fs::read_dir(temp.path())
                .unwrap()
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().contains("corrupt-"))
        );
    }

    #[test]
    fn retention_keeps_audio_for_a_cancel_that_has_no_transcript() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("session");
        std::fs::create_dir_all(&folder).unwrap();
        let mut record = meta(SessionStatus::Cancelled);
        record.started_at = OffsetDateTime::now_utc() - time::Duration::days(30);
        record.write(&folder);
        std::fs::write(file_layout::audio_wav(&folder), b"audio").unwrap();

        let policy = RetentionPolicy {
            audio_retention_days: 7,
        };
        assert_eq!(
            policy.purge_expired_audio(temp.path(), OffsetDateTime::now_utc()),
            0
        );
        assert!(file_layout::audio_wav(&folder).exists());
    }

    #[test]
    fn retention_purges_transcribed_audio_past_the_cutoff() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("session");
        std::fs::create_dir_all(&folder).unwrap();
        let mut record = meta(SessionStatus::Inserted);
        record.started_at = OffsetDateTime::now_utc() - time::Duration::days(30);
        record.raw_transcript = Some("hello".into());
        record.write(&folder);
        std::fs::write(file_layout::audio_wav(&folder), b"audio").unwrap();

        let policy = RetentionPolicy {
            audio_retention_days: 7,
        };
        assert_eq!(
            policy.purge_expired_audio(temp.path(), OffsetDateTime::now_utc()),
            1
        );
        assert!(!file_layout::audio_wav(&folder).exists());
    }

    #[test]
    fn never_keep_audio_purges_a_dictation_that_just_finished() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("session");
        std::fs::create_dir_all(&folder).unwrap();
        let mut record = meta(SessionStatus::Inserted);
        record.raw_transcript = Some("hello".into());
        record.write(&folder);
        std::fs::write(file_layout::audio_wav(&folder), b"audio").unwrap();

        let policy = RetentionPolicy {
            audio_retention_days: -1,
        };
        assert_eq!(
            policy.purge_expired_audio(temp.path(), OffsetDateTime::now_utc()),
            1
        );
    }

    #[test]
    fn keeping_audio_forever_purges_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let policy = RetentionPolicy {
            audio_retention_days: 0,
        };
        assert_eq!(
            policy.purge_expired_audio(temp.path(), OffsetDateTime::now_utc()),
            0
        );
    }
}
#[cfg(test)]
mod library_contract_test {
    use super::*;
    #[test]
    fn fts_is_accent_insensitive_and_preserves_original_correction() {
        let temp = tempfile::tempdir().unwrap();
        let store = HistoryStore::open(&temp.path().join("history.sqlite")).unwrap();
        let folder = temp.path().join("session");
        std::fs::create_dir(&folder).unwrap();
        let mut meta = SessionMeta::new(
            uuid::Uuid::new_v4(),
            OffsetDateTime::now_utc(),
            SessionStatus::Inserted,
        );
        meta.raw_transcript = Some("été Proxmocks".into());
        meta.cleaned_transcript = Some("Été Proxmocks.".into());
        meta.write(&folder);
        store.upsert(&meta, &folder);
        store
            .edit(&meta.id.to_string(), "Été Proxmocks.", "Été Proxmox.")
            .unwrap();
        assert_eq!(
            SessionMeta::read(&folder)
                .unwrap()
                .cleaned_transcript
                .as_deref(),
            Some("Été Proxmocks.")
        );
        let page = store
            .page(
                &HistoryFilter {
                    search: "ETE proxmox".into(),
                    ..Default::default()
                },
                0,
                50,
            )
            .unwrap();
        assert_eq!(page.total, 1);
        assert!(page.items[0].corrected);
    }
}
