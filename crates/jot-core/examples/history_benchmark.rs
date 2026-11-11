//! Run with `cargo run --release -p jot-core --example history_benchmark`.
//! Isolated generated indexes; setup excluded; 31 warm samples per operation.
//! Measures database work, not GPUI layout, rendering, clipboard or disk reindexing.

use anyhow::Result;
use jot_core::history::{ApplicationFilter, HistoryFilter, HistoryStore, StatusFilter};
use rusqlite::{Connection, params};
use std::{hint::black_box, time::Instant};
use time::OffsetDateTime;
use unicode_segmentation::UnicodeSegmentation;

fn measure(rows: usize, name: &str, mut operation: impl FnMut() -> Result<()>) -> Result<()> {
    operation()?;
    let mut samples = Vec::with_capacity(31);
    for _ in 0..31 {
        let started = Instant::now();
        operation()?;
        samples.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    samples.sort_by(f64::total_cmp);
    println!("{rows},{name},31,{:.3},{:.3}", samples[15], samples[29]);
    Ok(())
}

fn seed(path: &std::path::Path, rows: usize, now: OffsetDateTime) -> Result<()> {
    let mut connection = Connection::open(path)?;
    let transaction = connection.transaction()?;
    {
        // Production schema and FTS triggers are created by HistoryStore::open.
        // One transaction excludes per-session persistence from query timings.
        let mut insert = transaction.prepare(
            "INSERT INTO dictation (id, folder, started_at, status, target_app_name,
                 duration_seconds, raw_transcript, cleaned_transcript, pipeline_seconds, word_count)
             VALUES (?1, 'synthetic-benchmark', ?2, 'inserted', ?3, 10, ?4, ?5, 1, ?6)",
        )?;
        let ordinary =
            "Dictée de démonstration avec quelques mots pour mesurer la bibliothèque locale. "
                .repeat(8);
        let matching = format!("{ordinary} Café spécifique.");
        let ordinary_words = ordinary.unicode_words().count() as i64;
        let matching_words = matching.unicode_words().count() as i64;
        for n in 1..=rows {
            let (text, words) = if n % 10 == 0 {
                (&matching, matching_words)
            } else {
                (&ordinary, ordinary_words)
            };
            insert.execute(params![
                format!("fixture-{n:08}"),
                now.unix_timestamp() - (n as i64) * 60,
                if n % 2 == 0 { "Éditeur" } else { "Navigateur" },
                &ordinary,
                text,
                words,
            ])?;
        }
    }
    transaction.commit()?;
    connection.execute_batch("ANALYZE;")?;
    Ok(())
}

fn explain(path: &std::path::Path, rows: usize) -> Result<()> {
    let connection = Connection::open(path)?;
    let predicate = "(raw_transcript IS NOT NULL OR cleaned_transcript IS NOT NULL
         OR status IN ('failed','queuedForRetry') OR (status='cancelled' AND duration_seconds>=10))
         AND status!='silent' AND status NOT IN ('recording','recorded','transcribing')";
    for (name, query) in [
        ("count", format!("SELECT count(*) FROM dictation INDEXED BY dictation_library WHERE {predicate}")),
        ("page", format!("SELECT id,substr(coalesce(user_transcript,cleaned_transcript,raw_transcript,''),1,240)
             FROM dictation INDEXED BY dictation_library WHERE {predicate} ORDER BY started_at DESC,id DESC LIMIT 50 OFFSET {}", rows-50)),
        ("applications", format!("SELECT DISTINCT target_app_name FROM dictation INDEXED BY dictation_library WHERE {predicate}
             AND target_app_name IS NOT NULL AND target_app_name!='' ORDER BY target_app_name COLLATE NOCASE")),
        ("search_count", format!("SELECT count(*) FROM dictation INDEXED BY dictation_library WHERE {predicate}
             AND rowid IN (SELECT rowid FROM dictation_fts WHERE dictation_fts MATCH '\"CAFE\"* AND \"biblioth\"*')")),
    ] {
        let mut statement = connection.prepare(&format!("EXPLAIN QUERY PLAN {query}"))?;
        let plan = statement.query_map([], |row| row.get::<_, String>(3))?.collect::<rusqlite::Result<Vec<_>>>()?;
        eprintln!("plan {rows} {name}: {}", plan.join(" | "));
    }
    Ok(())
}

fn main() -> Result<()> {
    println!("rows,operation,samples,median_ms,p95_ms");
    for rows in [100, 1_000, 10_000, 100_000] {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("history.sqlite");
        let now = OffsetDateTime::now_utc();
        let setup = Instant::now();
        let store = HistoryStore::open(&path)?;
        seed(&path, rows, now)?;
        explain(&path, rows)?;
        eprintln!(
            "setup: {rows} synthetic rows in {:.3}s (excluded)",
            setup.elapsed().as_secs_f64()
        );
        let all = HistoryFilter::default();
        let search = HistoryFilter {
            search: "CAFE biblioth".into(),
            ..Default::default()
        };
        let combined = HistoryFilter {
            search: search.search.clone(),
            status: StatusFilter::Completed,
            application: ApplicationFilter::Named("Éditeur".into()),
            since: Some(now.unix_timestamp() - 7 * 24 * 60 * 60),
            ..Default::default()
        };
        assert_eq!(store.page(&all, 0, 50)?.total, rows);
        assert_eq!(store.page(&search, 0, 50)?.total, rows / 10);
        assert_eq!(store.page(&all, rows - 50, 50)?.items.len(), 50);
        assert!(
            store
                .page(&all, 0, 50)?
                .items
                .iter()
                .all(|item| item.preview.chars().count() <= 240)
        );
        measure(rows, "first_page_50_with_total", || {
            black_box(store.page(&all, 0, 50)?);
            Ok(())
        })?;
        measure(rows, "search_prefix_accent_with_total", || {
            black_box(store.page(&search, 0, 50)?);
            Ok(())
        })?;
        measure(rows, "combined_search_app_status_period", || {
            black_box(store.page(&combined, 0, 50)?);
            Ok(())
        })?;
        assert_eq!(store.applications()?.len(), 2);
        measure(rows, "application_filter_choices", || {
            black_box(store.applications()?);
            Ok(())
        })?;
        measure(rows, "deep_page_last_50_with_total", || {
            black_box(store.page(&all, rows - 50, 50)?);
            Ok(())
        })?;
        assert_eq!(
            store
                .activity(now)?
                .days
                .values()
                .map(|day| day.dictations)
                .sum::<u64>(),
            rows as u64
        );
        measure(rows, "activity_local_calendar", || {
            black_box(store.activity(now)?);
            Ok(())
        })?;
    }
    Ok(())
}
