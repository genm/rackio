use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;

use crate::{MetricSample, NetworkMetric};

const RAW_RETENTION_MS: i64 = 24 * 60 * 60 * 1_000;
const MINUTE_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
const MINUTE_MS: i64 = 60 * 1_000;
const SIZE_CAP_BYTES: i64 = 64 * 1_024 * 1_024;
const SAMPLE_INTERVAL_MS: i64 = 2_000;

/// The most rows any resolution can hold under the retention contract:
/// 24 hours of two-second raw samples. Minute history retains seven days,
/// which is far fewer rows, so this bounds both without truncating a
/// legitimate range.
pub const MAX_QUERY_ROWS: usize = 24 * 60 * 60 * 1_000 / 2_000;

// Keep the literal above tied to the retention contract it is derived from.
const _: () = assert!(
    (RAW_RETENTION_MS / SAMPLE_INTERVAL_MS) == 43_200 && MAX_QUERY_ROWS == 43_200,
    "MAX_QUERY_ROWS must stay equal to the raw retention divided by the sample interval"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryResolution {
    Raw,
    Minute,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("stored metric JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
}

pub struct MetricStore {
    connection: Connection,
}

impl MetricStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        Self::from_connection(connection)
    }

    pub fn in_memory() -> Result<Self, StoreError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, StoreError> {
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS metric_samples (
                timestamp_ms INTEGER PRIMARY KEY,
                sample_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS minute_metrics (
                minute_ms INTEGER PRIMARY KEY,
                sample_count INTEGER NOT NULL,
                cpu_count INTEGER NOT NULL,
                cpu_sum REAL,
                memory_used_count INTEGER NOT NULL,
                memory_used_sum INTEGER,
                rx_count INTEGER NOT NULL,
                rx_sum INTEGER,
                tx_count INTEGER NOT NULL,
                tx_sum INTEGER,
                last_sample_json TEXT NOT NULL
            );
            ",
        )?;
        Ok(Self { connection })
    }

    pub fn insert_batch(&mut self, samples: &[MetricSample]) -> Result<(), StoreError> {
        let transaction = self.connection.transaction()?;
        for sample in samples {
            let json = serde_json::to_string(sample)?;
            // `INSERT OR REPLACE` silently overwrote an existing row, so a
            // backward clock step could destroy already-persisted history.
            // Keep the first sample recorded for a millisecond instead.
            let inserted = transaction.execute(
                "INSERT INTO metric_samples(timestamp_ms, sample_json) VALUES (?1, ?2)
                 ON CONFLICT(timestamp_ms) DO NOTHING",
                params![sample.timestamp_ms, json],
            )?;
            if inserted == 0 {
                // Counting it in the minute aggregate anyway would double-count
                // a sample that raw history stores once.
                continue;
            }

            let minute_ms = sample.timestamp_ms.div_euclid(MINUTE_MS) * MINUTE_MS;
            let cpu = sample.cpu_percent.map(f64::from);
            let memory = sample
                .memory_used_bytes
                .and_then(|value| i64::try_from(value).ok());
            let rx = sample
                .network
                .as_ref()
                .and_then(|value| i64::try_from(value.received_bytes_per_second).ok());
            let tx = sample
                .network
                .as_ref()
                .and_then(|value| i64::try_from(value.sent_bytes_per_second).ok());
            let json = serde_json::to_string(sample)?;

            transaction.execute(
                "
                INSERT INTO minute_metrics(
                    minute_ms, sample_count, cpu_count, cpu_sum,
                    memory_used_count, memory_used_sum, rx_count, rx_sum,
                    tx_count, tx_sum, last_sample_json
                ) VALUES (
                    ?1, 1, CASE WHEN ?2 IS NULL THEN 0 ELSE 1 END, ?2,
                    CASE WHEN ?3 IS NULL THEN 0 ELSE 1 END, ?3,
                    CASE WHEN ?4 IS NULL THEN 0 ELSE 1 END, ?4,
                    CASE WHEN ?5 IS NULL THEN 0 ELSE 1 END, ?5, ?6
                )
                ON CONFLICT(minute_ms) DO UPDATE SET
                    sample_count = sample_count + 1,
                    cpu_count = cpu_count + excluded.cpu_count,
                    cpu_sum = COALESCE(cpu_sum, 0) + COALESCE(excluded.cpu_sum, 0),
                    memory_used_count = memory_used_count + excluded.memory_used_count,
                    memory_used_sum = COALESCE(memory_used_sum, 0) + COALESCE(excluded.memory_used_sum, 0),
                    rx_count = rx_count + excluded.rx_count,
                    rx_sum = COALESCE(rx_sum, 0) + COALESCE(excluded.rx_sum, 0),
                    tx_count = tx_count + excluded.tx_count,
                    tx_sum = COALESCE(tx_sum, 0) + COALESCE(excluded.tx_sum, 0),
                    last_sample_json = excluded.last_sample_json
                ",
                params![minute_ms, cpu, memory, rx, tx, json],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Query without an explicit page size.
    ///
    /// Still bounded: the previous `usize::MAX` form compiled to `LIMIT
    /// i64::MAX`, so a single request could materialise the whole store in one
    /// `Vec` and one IPC line. [`MAX_QUERY_ROWS`] is what retention can
    /// actually hold, so no legitimate range loses a row.
    pub fn query(
        &self,
        from_ms: i64,
        to_ms: i64,
        resolution: HistoryResolution,
    ) -> Result<Vec<MetricSample>, StoreError> {
        self.query_page(from_ms, to_ms, resolution, MAX_QUERY_ROWS)
    }

    pub fn query_page(
        &self,
        from_ms: i64,
        to_ms: i64,
        resolution: HistoryResolution,
        limit: usize,
    ) -> Result<Vec<MetricSample>, StoreError> {
        match resolution {
            HistoryResolution::Raw => self.query_raw(from_ms, to_ms, limit),
            HistoryResolution::Minute => self.query_minute(from_ms, to_ms, limit),
        }
    }

    fn query_raw(
        &self,
        from_ms: i64,
        to_ms: i64,
        limit: usize,
    ) -> Result<Vec<MetricSample>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT sample_json FROM metric_samples
             WHERE timestamp_ms BETWEEN ?1 AND ?2 ORDER BY timestamp_ms LIMIT ?3",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = statement.query_map(params![from_ms, to_ms, limit], |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    fn query_minute(
        &self,
        from_ms: i64,
        to_ms: i64,
        limit: usize,
    ) -> Result<Vec<MetricSample>, StoreError> {
        let mut statement = self.connection.prepare(
            "
            SELECT minute_ms, cpu_count, cpu_sum, memory_used_count, memory_used_sum,
                   rx_count, rx_sum, tx_count, tx_sum, last_sample_json
            FROM minute_metrics
            WHERE minute_ms BETWEEN ?1 AND ?2 ORDER BY minute_ms LIMIT ?3
            ",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = statement.query_map(params![from_ms, to_ms, limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<f64>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, String>(9)?,
            ))
        })?;

        rows.map(|row| {
            let (
                timestamp_ms,
                cpu_count,
                cpu,
                memory_count,
                memory,
                rx_count,
                rx,
                tx_count,
                tx,
                json,
            ) = row?;
            let mut sample: MetricSample = serde_json::from_str(&json)?;
            sample.timestamp_ms = timestamp_ms;
            sample.cpu_percent = average_f64(cpu, cpu_count).map(f64_to_f32);
            sample.memory_used_bytes = average_i64(memory, memory_count);
            if rx_count > 0 || tx_count > 0 {
                sample.network = Some(NetworkMetric {
                    received_bytes_per_second: average_i64(rx, rx_count).unwrap_or_default(),
                    sent_bytes_per_second: average_i64(tx, tx_count).unwrap_or_default(),
                });
            } else {
                sample.network = None;
            }
            Ok(sample)
        })
        .collect()
    }

    /// Delete history outside the retention window.
    ///
    /// `now_ms` must come from a monotonic source (see [`crate::Clock`]).
    /// Passing raw wall-clock time let a forward NTP step or a restored VM
    /// snapshot delete the entire history in one call, silently and
    /// irreversibly.
    pub fn prune(&mut self, now_ms: i64) -> Result<(), StoreError> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "DELETE FROM metric_samples WHERE timestamp_ms < ?1",
            [now_ms - RAW_RETENTION_MS],
        )?;
        transaction.execute(
            "DELETE FROM minute_metrics WHERE minute_ms < ?1",
            [now_ms - MINUTE_RETENTION_MS],
        )?;
        transaction.commit()?;
        self.enforce_size_cap()
    }

    fn enforce_size_cap(&mut self) -> Result<(), StoreError> {
        self.enforce_size_cap_bytes(SIZE_CAP_BYTES)
    }

    fn enforce_size_cap_bytes(&mut self, size_cap_bytes: i64) -> Result<(), StoreError> {
        let mut deleted_any = false;
        while self.database_live_size_bytes()? > size_cap_bytes {
            let deleted = self.connection.execute(
                "
                DELETE FROM metric_samples WHERE timestamp_ms IN (
                    SELECT timestamp_ms FROM metric_samples ORDER BY timestamp_ms LIMIT 1000
                )
                ",
                [],
            )?;
            if deleted == 0 {
                let deleted = self.connection.execute(
                    "
                    DELETE FROM minute_metrics WHERE minute_ms IN (
                        SELECT minute_ms FROM minute_metrics ORDER BY minute_ms LIMIT 1000
                    )
                    ",
                    [],
                )?;
                if deleted == 0 {
                    break;
                }
            }
            deleted_any = true;
        }
        if deleted_any || self.database_allocated_size_bytes()? > size_cap_bytes {
            // Row deletion alone leaves freelist pages behind, so the physical
            // 64 MiB contract requires a checkpoint and compaction.
            self.connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
        }
        Ok(())
    }

    fn database_allocated_size_bytes(&self) -> Result<i64, StoreError> {
        let page_count: i64 = self
            .connection
            .query_row("PRAGMA page_count", [], |row| row.get(0))?;
        let page_size: i64 = self
            .connection
            .query_row("PRAGMA page_size", [], |row| row.get(0))?;
        Ok(page_count.saturating_mul(page_size))
    }

    fn database_live_size_bytes(&self) -> Result<i64, StoreError> {
        let allocated = self.database_allocated_size_bytes()?;
        let free_pages: i64 = self
            .connection
            .query_row("PRAGMA freelist_count", [], |row| row.get(0))?;
        let page_size: i64 = self
            .connection
            .query_row("PRAGMA page_size", [], |row| row.get(0))?;
        Ok(allocated.saturating_sub(free_pages.saturating_mul(page_size)))
    }

    pub fn latest(&self) -> Result<Option<MetricSample>, StoreError> {
        let json = self
            .connection
            .query_row(
                "SELECT sample_json FROM metric_samples ORDER BY timestamp_ms DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        json.map(|value| serde_json::from_str(&value).map_err(StoreError::from))
            .transpose()
    }
}

// CPU samples are bounded percentages, so the f64 SQLite aggregate is always
// representable at the precision used by the f32 wire/domain field.
#[allow(clippy::cast_possible_truncation)]
fn f64_to_f32(value: f64) -> f32 {
    value as f32
}

fn average_f64(sum: Option<f64>, count: i64) -> Option<f64> {
    let count = u32::try_from(count).ok()?;
    (count > 0)
        .then(|| sum.map(|value| value / f64::from(count)))
        .flatten()
}

fn average_i64(sum: Option<i64>, count: i64) -> Option<u64> {
    (count > 0)
        .then(|| sum.and_then(|value| u64::try_from(value / count).ok()))
        .flatten()
}

#[cfg(test)]
mod tests {
    use crate::{
        CapabilityState, CollectorError, HistoryResolution, MetricSample, MetricStore,
        NetworkMetric,
    };

    use super::{
        MAX_QUERY_ROWS, MINUTE_MS, MINUTE_RETENTION_MS, RAW_RETENTION_MS, SAMPLE_INTERVAL_MS,
        SIZE_CAP_BYTES,
    };

    #[test]
    fn retention_constants_match_the_published_contract() {
        // README and docs promise 24-hour raw history, 7-day minute history and
        // a 64 MiB cap. These are the only place those numbers are expressed as
        // code, so a mistyped factor would silently change a user-facing
        // guarantee that nothing else checks.
        assert_eq!(RAW_RETENTION_MS, 86_400_000, "24 hours in milliseconds");
        assert_eq!(MINUTE_RETENTION_MS, 604_800_000, "7 days in milliseconds");
        assert_eq!(MINUTE_MS, 60_000, "one minute in milliseconds");
        assert_eq!(SIZE_CAP_BYTES, 67_108_864, "64 MiB in bytes");
        assert_eq!(SAMPLE_INTERVAL_MS, 2_000, "two-second live sampling");
        assert_eq!(MAX_QUERY_ROWS, 43_200, "24 hours of two-second samples");
        assert_eq!(MINUTE_RETENTION_MS, RAW_RETENTION_MS * 7);
    }

    fn sample(timestamp_ms: i64, cpu: f32) -> MetricSample {
        MetricSample {
            timestamp_ms,
            sequence: u64::try_from(timestamp_ms).unwrap_or_default(),
            cpu_percent: Some(cpu),
            memory_used_bytes: Some(100),
            memory_total_bytes: Some(200),
            swap_used_bytes: Some(0),
            swap_total_bytes: Some(0),
            disks: Vec::new(),
            network: Some(NetworkMetric {
                received_bytes_per_second: 10,
                sent_bytes_per_second: 20,
            }),
            uptime_seconds: 1,
            errors: Vec::new(),
        }
    }

    #[test]
    fn persists_raw_and_minute_history() {
        let mut store = MetricStore::in_memory().unwrap_or_else(|error| panic!("{error}"));
        store
            .insert_batch(&[sample(1_000, 10.0), sample(2_000, 30.0)])
            .unwrap_or_else(|error| panic!("{error}"));

        let raw = store
            .query(0, 10_000, HistoryResolution::Raw)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(raw.len(), 2);

        let minute = store
            .query(0, 60_000, HistoryResolution::Minute)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(minute.len(), 1);
        assert_eq!(minute[0].cpu_percent, Some(20.0));
    }

    #[test]
    fn a_repeated_timestamp_does_not_overwrite_history_or_double_count_the_minute() {
        let mut store = MetricStore::in_memory().unwrap_or_else(|error| panic!("{error}"));
        let first = sample(1_000, 10.0);
        let repeat = sample(1_000, 90.0);

        store
            .insert_batch(&[first])
            .unwrap_or_else(|error| panic!("{error}"));
        store
            .insert_batch(&[repeat])
            .unwrap_or_else(|error| panic!("{error}"));

        let raw = store
            .query(0, 10_000, HistoryResolution::Raw)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(raw.len(), 1);
        assert_eq!(
            raw[0].cpu_percent,
            Some(10.0),
            "the persisted sample must not be replaced"
        );

        let minute = store
            .query(0, 60_000, HistoryResolution::Minute)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(minute.len(), 1);
        assert_eq!(
            minute[0].cpu_percent,
            Some(10.0),
            "the discarded duplicate must not enter the minute average"
        );
    }

    #[test]
    fn prunes_raw_before_minute_history() {
        let mut store = MetricStore::in_memory().unwrap_or_else(|error| panic!("{error}"));
        let now = 8 * 24 * 60 * 60 * 1_000;
        store
            .insert_batch(&[sample(0, 10.0), sample(now, 20.0)])
            .unwrap_or_else(|error| panic!("{error}"));
        store.prune(now).unwrap_or_else(|error| panic!("{error}"));

        let raw = store
            .query(0, now, HistoryResolution::Raw)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].timestamp_ms, now);
    }

    #[test]
    fn minute_history_does_not_average_missing_metrics_as_zero() {
        let mut store = MetricStore::in_memory().unwrap_or_else(|error| panic!("{error}"));
        let mut missing = sample(2_000, 0.0);
        missing.cpu_percent = None;
        missing.memory_used_bytes = None;
        missing.network = None;
        store
            .insert_batch(&[sample(1_000, 20.0), missing])
            .unwrap_or_else(|error| panic!("{error}"));

        let minute = store
            .query(0, 60_000, HistoryResolution::Minute)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(minute[0].cpu_percent, Some(20.0));
        assert_eq!(minute[0].memory_used_bytes, Some(100));
        assert_eq!(
            minute[0]
                .network
                .as_ref()
                .map(|network| network.received_bytes_per_second),
            Some(10)
        );
    }

    #[test]
    fn query_page_bounds_history_allocation() {
        let mut store = MetricStore::in_memory().unwrap_or_else(|error| panic!("{error}"));
        store
            .insert_batch(&[
                sample(1_000, 10.0),
                sample(2_000, 20.0),
                sample(3_000, 30.0),
            ])
            .unwrap_or_else(|error| panic!("{error}"));

        let page = store
            .query_page(0, 10_000, HistoryResolution::Raw, 2)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(page.len(), 2);
        assert_eq!(page[1].timestamp_ms, 2_000);
    }

    #[test]
    fn size_cap_reclaims_sqlite_pages_after_deleting_oldest_history() {
        let mut store = MetricStore::in_memory().unwrap_or_else(|error| panic!("{error}"));
        let samples: Vec<_> = (0..500)
            .map(|timestamp_ms| {
                let mut metric = sample(timestamp_ms, 20.0);
                metric.errors.push(CollectorError {
                    source: String::from("test"),
                    kind: CapabilityState::Unsupported,
                    message: "x".repeat(1_024),
                });
                metric
            })
            .collect();
        store
            .insert_batch(&samples)
            .unwrap_or_else(|error| panic!("{error}"));

        store
            .enforce_size_cap_bytes(32 * 1_024)
            .unwrap_or_else(|error| panic!("{error}"));

        let allocated = store
            .database_allocated_size_bytes()
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(allocated <= 32 * 1_024, "allocated {allocated} bytes");
    }
}
