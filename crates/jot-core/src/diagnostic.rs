//! Opt-in local diagnostic of refused rewrites (Settings → Privacy, off by
//! default). When the fidelity check refuses the local model's rewrite, Jot
//! still inserts the raw transcript; with this on, it also keeps the refused
//! text and why in `diagnostic.json` beside the dictation, so a test session
//! can compare raw / proposed / inserted. A local file only: never sent, never
//! logged, deleted after the retention chosen in Settings or when turned off.

use crate::file_layout;
use serde::{Deserialize, Serialize};
use std::path::Path;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectedReformulation {
    #[serde(with = "time::serde::rfc3339")]
    pub recorded_at: OffsetDateTime,
    /// Same value as the dictation's `modelId`, reason included.
    pub model_id: String,
    pub reason: String,
    /// The dictated words the check found missing or added (`fidelity::explain`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub raw_transcript: String,
    /// What the check judged: the model's answer after artifact clean-up.
    pub proposed: String,
    /// The model's answer before that clean-up, only when it differs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_response: Option<String>,
    /// What Jot inserted instead: the raw transcript after dictionary replacements.
    pub inserted: String,
    pub reformulation_ms: u64,
    /// The partial rescue's pieces, when it ran.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pieces: Vec<crate::rescue::PieceReport>,
}

impl RejectedReformulation {
    /// Only into a real session folder: benches transcribe loose files.
    pub fn write(&self, folder: &Path) {
        if !file_layout::meta_json(folder).exists() {
            return;
        }
        let temp = folder.join("diagnostic.json.tmp");
        let result = serde_json::to_vec_pretty(self)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&temp, bytes))
            .and_then(|()| std::fs::rename(&temp, file_layout::diagnostic_json(folder)));
        if let Err(error) = result {
            tracing::warn!(%error, "rejected-rewrite diagnostic not written");
        }
    }

    pub fn read(folder: &Path) -> Option<Self> {
        let data = std::fs::read(file_layout::diagnostic_json(folder)).ok()?;
        serde_json::from_slice(&data).ok()
    }
}

/// The sweep for the current settings: at startup and whenever they change.
pub fn sweep(settings: &crate::settings::Settings) -> usize {
    purge(
        &file_layout::recordings_root(),
        settings
            .keep_rejected_reformulations
            .then_some(settings.rejected_reformulation_retention_days),
        OffsetDateTime::now_utc(),
    )
}

/// Deletes every diagnostic when `keep_days` is `None` (the setting is off),
/// else those older than `keep_days`. The dictations themselves stay.
pub fn purge(recordings_root: &Path, keep_days: Option<i64>, now: OffsetDateTime) -> usize {
    let Ok(entries) = std::fs::read_dir(recordings_root) else {
        return 0;
    };
    let mut purged = 0;
    for folder in entries.flatten().map(|entry| entry.path()) {
        let path = file_layout::diagnostic_json(&folder);
        if !path.exists() {
            continue;
        }
        let expired = keep_days.is_none_or(|days| {
            RejectedReformulation::read(&folder)
                .is_none_or(|d| d.recorded_at < now - time::Duration::days(days))
        });
        if expired && std::fs::remove_file(&path).is_ok() {
            purged += 1;
        }
    }
    if purged > 0 {
        tracing::info!(purged, "purged rejected-rewrite diagnostics");
    }
    purged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{SessionMeta, SessionStatus};
    use uuid::Uuid;

    fn session(root: &Path, name: &str, age_days: i64) -> std::path::PathBuf {
        let folder = root.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        SessionMeta::new(
            Uuid::new_v4(),
            OffsetDateTime::now_utc(),
            SessionStatus::Inserted,
        )
        .write(&folder);
        RejectedReformulation {
            recorded_at: OffsetDateTime::now_utc() - time::Duration::days(age_days),
            model_id: "local/qwen3-asr-1.7b+qwen3.5-4b:rejected/unit_added".into(),
            reason: "unit_added".into(),
            detail: Some("unité absente du brut : « minutes »".into()),
            raw_transcript: "vingt de temps".into(),
            proposed: "vingt minutes".into(),
            model_response: None,
            inserted: "vingt de temps".into(),
            reformulation_ms: 364,
            pieces: Vec::new(),
        }
        .write(&folder);
        folder
    }

    #[test]
    fn written_beside_a_dictation_only() {
        let temp = tempfile::tempdir().unwrap();
        let folder = session(temp.path(), "a", 0);
        let read = RejectedReformulation::read(&folder).unwrap();
        assert_eq!(read.proposed, "vingt minutes");
        assert_eq!(read.inserted, "vingt de temps");

        let loose = temp.path().join("loose");
        std::fs::create_dir_all(&loose).unwrap();
        RejectedReformulation { ..read }.write(&loose);
        assert!(!file_layout::diagnostic_json(&loose).exists());
    }

    #[test]
    fn purge_honours_retention_and_turning_it_off() {
        let temp = tempfile::tempdir().unwrap();
        let fresh = session(temp.path(), "fresh", 1);
        let old = session(temp.path(), "old", 10);
        let now = OffsetDateTime::now_utc();

        assert_eq!(purge(temp.path(), Some(7), now), 1);
        assert!(file_layout::diagnostic_json(&fresh).exists());
        assert!(!file_layout::diagnostic_json(&old).exists());
        assert!(file_layout::meta_json(&old).exists(), "the dictation stays");

        assert_eq!(purge(temp.path(), None, now), 1);
        assert!(!file_layout::diagnostic_json(&fresh).exists());
    }
}
