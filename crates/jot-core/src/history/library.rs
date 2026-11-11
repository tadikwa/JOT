//! Session corrections are authoritative; every SQLite field here is rebuildable.
use super::*;
use anyhow::{bail, ensure};
use rusqlite::types::Value;
use serde::{Deserialize, Serialize};
use std::io::Write;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ApplicationFilter {
    #[default]
    All,
    Unknown,
    Named(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StatusFilter {
    #[default]
    All,
    Completed,
    Pending,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Default)]
pub struct HistoryFilter {
    pub search: String,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub application: ApplicationFilter,
    pub status: StatusFilter,
}

#[derive(Debug, Clone)]
pub struct HistoryItem {
    pub id: String,
    pub started_at: OffsetDateTime,
    pub status: String,
    pub target_app_name: Option<String>,
    pub duration_seconds: Option<f64>,
    pub preview: String,
    pub word_count: usize,
    pub corrected: bool,
}

#[derive(Debug)]
pub struct HistoryPage {
    pub items: Vec<HistoryItem>,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Correction {
    pub dictation_id: String,
    pub before: String,
    pub after: String,
    #[serde(with = "time::serde::rfc3339")]
    pub corrected_at: OffsetDateTime,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CorrectionFile {
    version: u32,
    corrections: Vec<Correction>,
}

pub(super) fn read_corrections(folder: &Path, id: &str) -> Result<Vec<Correction>> {
    let bytes = match std::fs::read(folder.join("corrections.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("reading corrections"),
    };
    let file: CorrectionFile =
        serde_json::from_slice(&bytes).context("invalid corrections file")?;
    ensure!(file.version == 1, "unsupported corrections version");
    ensure!(
        file.corrections.iter().all(|c| c.dictation_id == id),
        "correction identity mismatch"
    );
    ensure!(
        file.corrections
            .windows(2)
            .all(|w| w[0].after == w[1].before),
        "broken correction history"
    );
    Ok(file.corrections)
}

pub(super) fn migrate(connection: &Connection) -> Result<()> {
    let has_user: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('dictation') WHERE name='user_transcript')",
        [],
        |r| r.get(0),
    )?;
    if !has_user {
        connection.execute_batch("ALTER TABLE dictation ADD COLUMN user_transcript TEXT;")?;
    }
    let has_fts: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='dictation_fts')",
        [],
        |r| r.get(0),
    )?;
    // Creating the real table, rather than inspecting a compile option, proves availability.
    connection.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS dictation_fts USING fts5(
            raw_transcript, cleaned_transcript, user_transcript,
            content='dictation', content_rowid='rowid', tokenize='unicode61 remove_diacritics 2', prefix='2 3 4');
         CREATE TRIGGER IF NOT EXISTS dictation_fts_insert AFTER INSERT ON dictation BEGIN
            INSERT INTO dictation_fts(rowid,raw_transcript,cleaned_transcript,user_transcript)
            VALUES(new.rowid,new.raw_transcript,new.cleaned_transcript,new.user_transcript);
         END;
         CREATE TRIGGER IF NOT EXISTS dictation_fts_delete AFTER DELETE ON dictation BEGIN
            INSERT INTO dictation_fts(dictation_fts,rowid,raw_transcript,cleaned_transcript,user_transcript)
            VALUES('delete',old.rowid,old.raw_transcript,old.cleaned_transcript,old.user_transcript);
         END;
         CREATE TRIGGER IF NOT EXISTS dictation_fts_update AFTER UPDATE OF raw_transcript,cleaned_transcript,user_transcript ON dictation BEGIN
            INSERT INTO dictation_fts(dictation_fts,rowid,raw_transcript,cleaned_transcript,user_transcript)
            VALUES('delete',old.rowid,old.raw_transcript,old.cleaned_transcript,old.user_transcript);
            INSERT INTO dictation_fts(rowid,raw_transcript,cleaned_transcript,user_transcript)
            VALUES(new.rowid,new.raw_transcript,new.cleaned_transcript,new.user_transcript);
         END;
         CREATE INDEX IF NOT EXISTS dictation_chronology ON dictation(started_at DESC,id DESC);
         CREATE INDEX IF NOT EXISTS dictation_app_chronology ON dictation(target_app_name,started_at DESC,id DESC);
         CREATE INDEX IF NOT EXISTS dictation_status_chronology ON dictation(status,started_at DESC,id DESC);"
    )?;
    // A count/page walk must never fetch full transcript payloads for every row.
    connection.execute_batch(&format!(
        "CREATE INDEX IF NOT EXISTS dictation_library ON dictation(started_at DESC,id DESC,target_app_name,status) WHERE ({VISIBLE}) AND status NOT IN ('recording','recorded','transcribing');"
    ))?;
    if !has_fts {
        connection.execute(
            "INSERT INTO dictation_fts(dictation_fts) VALUES('rebuild')",
            [],
        )?;
    }
    Ok(())
}

impl HistoryFilter {
    fn predicate(&self) -> (String, Vec<Value>) {
        let mut sql =
            format!("({VISIBLE}) AND status NOT IN ('recording','recorded','transcribing')");
        let mut values = Vec::new();
        let mut bind = |clause: &str, value: Value| {
            sql.push_str(" AND ");
            sql.push_str(clause);
            values.push(value);
        };
        // Quote each token: user input can never become FTS syntax (or SQL).
        let query = self
            .search
            .unicode_words()
            .map(|word| format!("\"{}\"*", word.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND ");
        if !self.search.trim().is_empty() {
            if query.is_empty() {
                bind(
                    "instr(coalesce(user_transcript,cleaned_transcript,raw_transcript,''),?)>0",
                    self.search.trim().to_string().into(),
                );
            } else {
                bind(
                    "rowid IN (SELECT rowid FROM dictation_fts WHERE dictation_fts MATCH ?)",
                    query.into(),
                );
            }
        }
        if let Some(since) = self.since {
            bind("started_at >= ?", since.into());
        }
        if let Some(until) = self.until {
            bind("started_at < ?", until.into());
        }
        match &self.application {
            ApplicationFilter::All => {}
            ApplicationFilter::Unknown => {
                sql.push_str(" AND (target_app_name IS NULL OR target_app_name='')")
            }
            ApplicationFilter::Named(name) => {
                sql.push_str(" AND target_app_name = ?");
                values.push(name.clone().into());
            }
        }
        match self.status {
            StatusFilter::All => {},
            StatusFilter::Completed => sql.push_str(" AND status IN ('inserted','copiedToClipboard','awaitingChip','heldSecure','recovered')"),
            StatusFilter::Pending => sql.push_str(" AND status='queuedForRetry'"),
            StatusFilter::Failed => sql.push_str(" AND status='failed'"),
            StatusFilter::Cancelled => sql.push_str(" AND status='cancelled'"),
        }
        (sql, values)
    }
}

impl HistoryStore {
    /// Bounded summaries only: full transcripts are loaded for the selected session.
    pub fn page(&self, filter: &HistoryFilter, offset: usize, limit: usize) -> Result<HistoryPage> {
        let (predicate, mut values) = filter.predicate();
        let connection = self.connection.lock();
        // Force the partial index so count/offset scans do not fetch every transcript.
        let total: usize = connection.query_row(
            &format!(
                "SELECT count(*) FROM dictation INDEXED BY dictation_library WHERE {predicate}"
            ),
            rusqlite::params_from_iter(values.iter()),
            |r| r.get(0),
        )?;
        values.push((limit.min(500) as i64).into());
        values.push((offset.min(i64::MAX as usize) as i64).into());
        let mut stmt = connection.prepare(&format!(
            "SELECT id,started_at,status,target_app_name,duration_seconds,
                substr(coalesce(user_transcript,cleaned_transcript,raw_transcript,''),1,240),
                word_count,user_transcript IS NOT NULL
             FROM dictation INDEXED BY dictation_library WHERE {predicate} ORDER BY started_at DESC,id DESC LIMIT ? OFFSET ?"
        ))?;
        let items = stmt
            .query_map(rusqlite::params_from_iter(values.iter()), |r| {
                Ok(HistoryItem {
                    id: r.get(0)?,
                    started_at: OffsetDateTime::from_unix_timestamp(r.get(1)?)
                        .unwrap_or(OffsetDateTime::UNIX_EPOCH),
                    status: r.get(2)?,
                    target_app_name: r.get(3)?,
                    duration_seconds: r.get(4)?,
                    preview: r.get(5)?,
                    word_count: r.get(6)?,
                    corrected: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(HistoryPage { items, total })
    }

    pub fn applications(&self) -> Result<Vec<String>> {
        let connection = self.connection.lock();
        let mut stmt = connection.prepare(&format!("SELECT DISTINCT target_app_name FROM dictation INDEXED BY dictation_library WHERE ({VISIBLE}) AND status NOT IN ('recording','recorded','transcribing') AND target_app_name IS NOT NULL AND target_app_name != '' ORDER BY target_app_name COLLATE NOCASE"))?;
        Ok(stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Small source inventory; scans load only one correction journal at a time.
    pub fn correction_session_ids(&self) -> Result<Vec<String>> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT id,folder FROM dictation WHERE user_transcript IS NOT NULL ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut ids = Vec::new();
        for row in rows {
            let (id, folder) = row?;
            let folder = Path::new(&folder);
            if folder.join("meta.json").is_file() && folder.join("corrections.json").is_file() {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    pub fn corrections(&self, id: &str) -> Result<Vec<Correction>> {
        let record = self.record(id).context("dictation no longer exists")?;
        read_corrections(&record.folder, id)
    }

    /// Optimistic guard protects two editors; the store lock serializes retry/reindex/delete.
    pub fn edit(&self, id: &str, expected_text: &str, new_text: &str) -> Result<()> {
        ensure!(
            !new_text.trim().is_empty(),
            "corrected text cannot be empty"
        );
        let connection = self.connection.lock();
        let record = connection
            .query_row("SELECT * FROM dictation WHERE id=?", [id], row_to_record)
            .context("dictation no longer exists")?;
        let meta =
            SessionMeta::read(&record.folder).context("session metadata missing or invalid")?;
        ensure!(meta.id.to_string() == id, "session identity mismatch");
        ensure!(!meta.status.is_in_flight(), "dictation still processing");
        ensure!(
            meta.raw_transcript.is_some() || meta.cleaned_transcript.is_some(),
            "no transcript to correct"
        );
        let mut corrections = read_corrections(&record.folder, id)?;
        let current = corrections
            .last()
            .map(|c| c.after.as_str())
            .unwrap_or(meta.display_text());
        ensure!(
            current == expected_text,
            "text changed: reload before saving"
        );
        if current == new_text {
            return Ok(());
        }
        corrections.push(Correction {
            dictation_id: id.to_string(),
            before: current.to_string(),
            after: new_text.to_string(),
            corrected_at: OffsetDateTime::now_utc(),
        });
        let bytes = serde_json::to_vec_pretty(&CorrectionFile {
            version: 1,
            corrections,
        })?;
        let mut temp =
            tempfile::NamedTempFile::new_in(&record.folder).context("creating correction file")?;
        temp.write_all(&bytes)?;
        temp.as_file().sync_all()?;
        temp.persist(record.folder.join("corrections.json"))
            .context("saving correction atomically")?;
        let result = Self::write_record(&connection, &meta, &record.folder);
        drop(connection);
        // Source saved even if the derived index failed; refresh consumers and report error.
        self.notify(true);
        result.context("correction saved; history index needs rebuilding")?;
        Ok(())
    }

    /// Account/configuration errors permit an explicit retry after fixing them, never an automatic drain.
    pub fn can_retry(&self, id: &str) -> bool {
        let Some(record) = self.record(id) else {
            return false;
        };
        record.display_text().is_empty()
            && file_layout::audio_wav(&record.folder).is_file()
            && (matches!(
                record.status(),
                Some(SessionStatus::QueuedForRetry | SessionStatus::Cancelled)
            ) || record.status() == Some(SessionStatus::Failed)
                && record.error_code.as_deref().is_some_and(|code| {
                    RETRYABLE_ERROR_CODES.contains(&code)
                        || matches!(code, "auth" | "quota" | "model" | "offline")
                }))
    }

    /// All identities/paths are validated before any move; audio is included explicitly by the UI.
    pub fn delete_many(&self, ids: &[String]) -> Result<usize> {
        let unique = ids.iter().collect::<std::collections::BTreeSet<_>>();
        if unique.is_empty() {
            return Ok(0);
        }
        let root = file_layout::recordings_root()
            .canonicalize()
            .context("recordings directory unavailable")?;
        let mut connection = self.connection.lock();
        let mut folders = Vec::new();
        for id in unique {
            let record = connection
                .query_row("SELECT * FROM dictation WHERE id=?", [id], row_to_record)
                .context("dictation no longer exists")?;
            let attrs = std::fs::symlink_metadata(&record.folder)?;
            ensure!(
                !attrs.file_type().is_symlink(),
                "linked session folders cannot be deleted"
            );
            let folder = record.folder.canonicalize()?;
            ensure!(
                folder.parent() == Some(root.as_path()),
                "session outside recordings directory"
            );
            let meta = SessionMeta::read(&folder).context("session metadata missing or invalid")?;
            ensure!(meta.id.to_string() == *id, "session identity mismatch");
            ensure!(!meta.status.is_in_flight(), "dictation still processing");
            folders.push((
                id.clone(),
                folder,
                root.join(format!(".deleted-{}", uuid::Uuid::new_v4())),
            ));
        }
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut moved = Vec::new();
        let result = (|| -> Result<()> {
            for (id, folder, tombstone) in &folders {
                std::fs::rename(folder, tombstone).context("moving session before deletion")?;
                moved.push((folder, tombstone));
                transaction.execute("DELETE FROM dictation WHERE id=?", [id])?;
            }
            transaction.commit()?;
            Ok(())
        })();
        if let Err(error) = result {
            for (folder, tombstone) in moved.into_iter().rev() {
                if let Err(rollback) = std::fs::rename(tombstone, folder) {
                    bail!(
                        "{error}; rollback failed: {rollback}; retained at {}",
                        tombstone.display()
                    );
                }
            }
            return Err(error);
        }
        drop(connection);
        self.notify(true);
        for (_, _, tombstone) in &folders {
            std::fs::remove_dir_all(tombstone).with_context(|| {
                format!("entry removed; files remain at {}", tombstone.display())
            })?;
        }
        Ok(folders.len())
    }
}

/// Local calendar days, including DST, rather than a rolling multiple of 24 hours.
pub fn period_start(days: u16, now: OffsetDateTime) -> Result<i64> {
    ensure!(days > 0, "period must contain at least one day");
    let calendar = LocalCalendar::current()?;
    let today = calendar.date(now)?;
    let start = today
        .checked_sub(time::Duration::days(i64::from(days) - 1))
        .context("period outside calendar")?;
    let mut low = start.midnight().assume_utc().unix_timestamp() - 86_400;
    let mut high = low + 3 * 86_400;
    while low < high {
        let middle = low + (high - low) / 2;
        if calendar.date(OffsetDateTime::from_unix_timestamp(middle)?)? < start {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    Ok(low)
}
