//! Resource history in `metrics.db`.

use domain::sizing::{Summary, rank};
use std::collections::HashMap;

use shared::metrics::{ContainerFigures, Reading, Resolution, SubjectKind};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

use crate::Result;

pub const MINUTE_DAYS: i64 = 30;
pub const QUARTER_DAYS: i64 = 365;
const DAY: i64 = 86_400;

#[derive(Debug, Clone)]
pub struct MetricsStore {
    pool: SqlitePool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectRow {
    pub id: i64,
    pub kind: SubjectKind,
    pub key: String,
    pub project: Option<String>,
    pub service: Option<String>,
    pub last_seen: i64,
}

type Row = (
    i64,
    Option<f64>,
    Option<f64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
);

/// Folds rows yielding reading columns into buckets starting at `from`
/// (`?2`), `?4` seconds wide. Literals only, so every statement is static.
macro_rules! bucketed {
    ($source:expr) => {
        concat!(
            "SELECT ?2 + ((t - ?2) / ?4) * ?4 AS b,
                    AVG(cpu), MAX(cpu_max), CAST(AVG(mem) AS INTEGER), MAX(mem_max), MAX(mem_limit),
                    AVG(net_rx), AVG(net_tx), AVG(io_read), AVG(io_write), AVG(throttled), AVG(load)
             FROM (",
            $source,
            ") GROUP BY b ORDER BY b"
        )
    };
}

macro_rules! subject_series {
    ($table:literal) => {
        bucketed!(concat!(
            "SELECT t, cpu, cpu_max, mem, mem_max, mem_limit, net_rx, net_tx, io_read, io_write, throttled, load
             FROM ", $table, " WHERE subject_id = ?1 AND t >= ?2 AND t < ?3"
        ))
    };
}

macro_rules! stack_series {
    ($table:literal) => {
        bucketed!(concat!(
            "SELECT s.t AS t, SUM(s.cpu) AS cpu, SUM(s.cpu_max) AS cpu_max, SUM(s.mem) AS mem, SUM(s.mem_max) AS mem_max,
                    CASE WHEN COUNT(s.mem_limit) = COUNT(*) THEN SUM(s.mem_limit) END AS mem_limit,
                    SUM(s.net_rx) AS net_rx, SUM(s.net_tx) AS net_tx, SUM(s.io_read) AS io_read, SUM(s.io_write) AS io_write,
                    NULL AS throttled, NULL AS load
             FROM ", $table, " s JOIN subjects j ON j.id = s.subject_id
             WHERE j.kind = 'container' AND j.project = ?1 AND s.t >= ?2 AND s.t < ?3
             GROUP BY s.t"
        ))
    };
}

/// A stack's containers in `[?2, ?3)` of `$table`: rows for `figures`.
macro_rules! stack_rows {
    ($table:literal, $($extra:tt)*) => {
        concat!(
            " FROM ",
            $table,
            " s JOIN subjects j ON j.id = s.subject_id
             WHERE j.kind = 'container' AND j.project = ?1 AND s.t >= ?2 AND s.t < ?3",
            $($extra)*
        )
    };
}

/// Each container's peaks, and its service.
macro_rules! figures_peaks {
    ($table:literal) => {
        concat!(
            "SELECT j.key, MAX(j.service), MAX(COALESCE(s.cpu_max, s.cpu)), MAX(COALESCE(s.mem_max, s.mem))",
            stack_rows!($table, " GROUP BY j.key ORDER BY j.key")
        )
    };
}

/// Each container's nearest-rank 95th percentile of `$column`, the same
/// rank as `domain::sizing::rank`: SQLite and Rust both round half away
/// from zero.
macro_rules! figures_p95 {
    ($table:literal, $column:literal) => {
        concat!(
            "SELECT key, v FROM (SELECT j.key AS key, s.",
            $column,
            " AS v,
                    ROW_NUMBER() OVER (PARTITION BY j.key ORDER BY s.",
            $column,
            ") - 1 AS i,
                    COUNT(*) OVER (PARTITION BY j.key) AS n",
            stack_rows!($table, " AND s.", $column, " IS NOT NULL"),
            ") WHERE i = CAST(ROUND((n - 1) * 0.95) AS INTEGER)"
        )
    };
}

#[allow(clippy::cast_sign_loss)]
fn from_row(r: Row) -> Reading {
    let (
        t,
        cpu,
        cpu_max,
        mem,
        mem_max,
        mem_limit,
        net_rx,
        net_tx,
        io_read,
        io_write,
        throttled,
        load,
    ) = r;
    let u = |v: Option<i64>| v.map(|v| v.max(0) as u64);
    Reading {
        t,
        cpu,
        cpu_max,
        mem: u(mem),
        mem_max: u(mem_max),
        mem_limit: u(mem_limit),
        net_rx,
        net_tx,
        io_read,
        io_write,
        throttled,
        load,
    }
}

#[allow(clippy::cast_possible_wrap)]
fn signed(v: Option<u64>) -> Option<i64> {
    v.map(|v| v.min(i64::MAX as u64) as i64)
}

impl MetricsStore {
    pub async fn open(path: &str) -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal);
        Self::connect(opts, 4).await
    }

    pub async fn open_in_memory() -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true);
        Self::connect(opts, 1).await
    }

    async fn connect(opts: SqliteConnectOptions, max: u32) -> Result<Self> {
        let mut pool = SqlitePoolOptions::new().max_connections(max);
        if max == 1 {
            pool = pool
                .min_connections(1)
                .idle_timeout(None)
                .max_lifetime(None);
        }
        let pool = pool.connect_with(opts).await?;
        sqlx::migrate!("./migrations_metrics").run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn subject(
        &self,
        kind: SubjectKind,
        key: &str,
        project: Option<&str>,
        service: Option<&str>,
        now: i64,
    ) -> Result<i64> {
        let (id,) = sqlx::query_as::<_, (i64,)>(
            "INSERT INTO subjects (kind, key, project, service, first_seen, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT (kind, key) DO UPDATE SET
                 project = COALESCE(excluded.project, subjects.project),
                 service = COALESCE(excluded.service, subjects.service),
                 last_seen = MAX(subjects.last_seen, excluded.last_seen)
             RETURNING id",
        )
        .bind(kind.as_str())
        .bind(key)
        .bind(project)
        .bind(service)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn find(&self, kind: SubjectKind, key: &str) -> Result<Option<SubjectRow>> {
        let row = sqlx::query_as::<_, (i64, String, String, Option<String>, Option<String>, i64)>(
            "SELECT id, kind, key, project, service, last_seen FROM subjects WHERE kind = ?1 AND key = ?2",
        )
        .bind(kind.as_str())
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(subject_row))
    }

    pub async fn containers_seen_since(&self, since: i64) -> Result<Vec<SubjectRow>> {
        let rows = sqlx::query_as::<_, (i64, String, String, Option<String>, Option<String>, i64)>(
            "SELECT id, kind, key, project, service, last_seen FROM subjects
             WHERE kind = 'container' AND last_seen >= ?1 ORDER BY key",
        )
        .bind(since)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().filter_map(subject_row).collect())
    }

    pub async fn write_minute(&self, rows: &[(i64, Reading)]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (id, r) in rows {
            sqlx::query(
                "INSERT OR REPLACE INTO samples_1m
                 (subject_id, t, cpu, cpu_max, mem, mem_max, mem_limit, net_rx, net_tx, io_read, io_write, throttled, load)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )
            .bind(id).bind(r.t).bind(r.cpu).bind(r.cpu_max)
            .bind(signed(r.mem)).bind(signed(r.mem_max)).bind(signed(r.mem_limit))
            .bind(r.net_rx).bind(r.net_tx).bind(r.io_read).bind(r.io_write)
            .bind(r.throttled).bind(r.load)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Folds the 1-minute rows in `[from, to)` into 15-minute rows: averages
    /// of averages, maxima of peaks, the latest limit.
    pub async fn rollup(&self, from: i64, to: i64) -> Result<usize> {
        let done = sqlx::query(
            "INSERT OR REPLACE INTO samples_15m
             (subject_id, t, cpu, cpu_max, mem, mem_max, mem_limit, net_rx, net_tx, io_read, io_write, throttled, load)
             SELECT subject_id, (t / 900) * 900,
                    AVG(cpu), MAX(cpu_max), CAST(AVG(mem) AS INTEGER), MAX(mem_max), MAX(mem_limit),
                    AVG(net_rx), AVG(net_tx), AVG(io_read), AVG(io_write), AVG(throttled), AVG(load)
             FROM samples_1m WHERE t >= ?1 AND t < ?2
             GROUP BY subject_id, t / 900",
        )
        .bind(from)
        .bind(to)
        .execute(&self.pool)
        .await?;
        Ok(usize::try_from(done.rows_affected()).unwrap_or(0))
    }

    pub async fn read(
        &self,
        subject_id: i64,
        resolution: Resolution,
        from: i64,
        to: i64,
    ) -> Result<Vec<Reading>> {
        let sql = match resolution {
            Resolution::Quarter => "SELECT t, cpu, cpu_max, mem, mem_max, mem_limit, net_rx, net_tx, io_read, io_write, throttled, load
                 FROM samples_15m WHERE subject_id = ?1 AND t >= ?2 AND t < ?3 ORDER BY t",
            _ => "SELECT t, cpu, cpu_max, mem, mem_max, mem_limit, net_rx, net_tx, io_read, io_write, throttled, load
                 FROM samples_1m WHERE subject_id = ?1 AND t >= ?2 AND t < ?3 ORDER BY t",
        };
        let rows = sqlx::query_as::<_, Row>(sql)
            .bind(subject_id)
            .bind(from)
            .bind(to)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(from_row).collect())
    }

    pub async fn read_stack(
        &self,
        project: &str,
        resolution: Resolution,
        from: i64,
        to: i64,
    ) -> Result<Vec<Reading>> {
        let sql = match resolution {
            Resolution::Quarter => {
                "SELECT s.t, SUM(s.cpu), SUM(s.cpu_max), SUM(s.mem), SUM(s.mem_max),
                    CASE WHEN COUNT(s.mem_limit) = COUNT(*) THEN SUM(s.mem_limit) END,
                    SUM(s.net_rx), SUM(s.net_tx), SUM(s.io_read), SUM(s.io_write), NULL, NULL
                 FROM samples_15m s JOIN subjects j ON j.id = s.subject_id
                 WHERE j.kind = 'container' AND j.project = ?1 AND s.t >= ?2 AND s.t < ?3
                 GROUP BY s.t ORDER BY s.t"
            }
            _ => {
                "SELECT s.t, SUM(s.cpu), SUM(s.cpu_max), SUM(s.mem), SUM(s.mem_max),
                    CASE WHEN COUNT(s.mem_limit) = COUNT(*) THEN SUM(s.mem_limit) END,
                    SUM(s.net_rx), SUM(s.net_tx), SUM(s.io_read), SUM(s.io_write), NULL, NULL
                 FROM samples_1m s JOIN subjects j ON j.id = s.subject_id
                 WHERE j.kind = 'container' AND j.project = ?1 AND s.t >= ?2 AND s.t < ?3
                 GROUP BY s.t ORDER BY s.t"
            }
        };
        let rows = sqlx::query_as::<_, Row>(sql)
            .bind(project)
            .bind(from)
            .bind(to)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(from_row).collect())
    }

    /// A chart's worth of `[from, to)`: at most `points` buckets of time,
    /// each the average of its rows' averages and the maximum of their
    /// peaks, so a spike inside a bucket survives. SQLite does the folding;
    /// decoding a month of minutes row by row costs ~15x more.
    pub async fn read_series(
        &self,
        subject_id: i64,
        resolution: Resolution,
        from: i64,
        to: i64,
        points: usize,
    ) -> Result<Vec<Reading>> {
        let sql = match resolution {
            Resolution::Quarter => subject_series!("samples_15m"),
            _ => subject_series!("samples_1m"),
        };
        let rows = sqlx::query_as::<_, Row>(sql)
            .bind(subject_id)
            .bind(from)
            .bind(to)
            .bind(bucket(resolution, from, to, points))
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(from_row).collect())
    }

    /// [`Self::read_series`] for a stack: its containers summed at each
    /// moment, then bucketed.
    pub async fn read_stack_series(
        &self,
        project: &str,
        resolution: Resolution,
        from: i64,
        to: i64,
        points: usize,
    ) -> Result<Vec<Reading>> {
        let sql = match resolution {
            Resolution::Quarter => stack_series!("samples_15m"),
            _ => stack_series!("samples_1m"),
        };
        let rows = sqlx::query_as::<_, Row>(sql)
            .bind(project)
            .bind(from)
            .bind(to)
            .bind(bucket(resolution, from, to, points))
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(from_row).collect())
    }

    /// What sizing needs from a container's minutes in `[from, to)`, worked
    /// out by SQLite: aggregates and nearest-rank percentiles, the same
    /// definitions as `domain::sizing::summarise`, without decoding a month
    /// of rows.
    pub async fn sizing_summary(&self, subject_id: i64, from: i64, to: i64) -> Result<Summary> {
        let (running, mem_peak, cpu_burst, throttled, n_cpu, n_mem) =
            sqlx::query_as::<_, (i64, Option<i64>, Option<f64>, Option<f64>, i64, i64)>(
                "SELECT COUNT(*), MAX(COALESCE(mem_max, mem)), MAX(COALESCE(cpu_max, cpu)),
                        AVG(throttled), COUNT(cpu), COUNT(mem)
                 FROM samples_1m
                 WHERE subject_id = ?1 AND t >= ?2 AND t < ?3
                   AND (cpu IS NOT NULL OR mem IS NOT NULL)",
            )
            .bind(subject_id)
            .bind(from)
            .bind(to)
            .fetch_one(&self.pool)
            .await?;

        // The limit in force at the last running minute; failing that, the
        // last minute at all. Both walk the key backwards and stop at once.
        let limit = if running > 0 {
            sqlx::query_as::<_, (Option<i64>,)>(
                "SELECT mem_limit FROM samples_1m
                 WHERE subject_id = ?1 AND t >= ?2 AND t < ?3
                   AND (cpu IS NOT NULL OR mem IS NOT NULL)
                 ORDER BY t DESC LIMIT 1",
            )
        } else {
            sqlx::query_as::<_, (Option<i64>,)>(
                "SELECT mem_limit FROM samples_1m
                 WHERE subject_id = ?1 AND t >= ?2 AND t < ?3
                 ORDER BY t DESC LIMIT 1",
            )
        }
        .bind(subject_id)
        .bind(from)
        .bind(to)
        .fetch_optional(&self.pool)
        .await?
        .and_then(|(l,)| l);

        let count = |n: i64| u64::try_from(n).unwrap_or(0);
        let (p50, p95) = (rank(count(n_cpu), 0.5), rank(count(n_cpu), 0.95));
        let cpu = sqlx::query_as::<_, (i64, f64)>(
            "SELECT i, cpu FROM (
                 SELECT cpu, ROW_NUMBER() OVER (ORDER BY cpu) - 1 AS i FROM samples_1m
                 WHERE subject_id = ?1 AND t >= ?2 AND t < ?3 AND cpu IS NOT NULL
             ) WHERE i IN (?4, ?5)",
        )
        .bind(subject_id)
        .bind(from)
        .bind(to)
        .bind(signed(Some(p50)))
        .bind(signed(Some(p95)))
        .fetch_all(&self.pool)
        .await?;
        let at = |rank: u64| {
            cpu.iter()
                .find(|(i, _)| signed(Some(rank)) == Some(*i))
                .map_or(0.0, |(_, v)| *v)
        };

        let mem_typical = sqlx::query_as::<_, (i64,)>(
            "SELECT mem FROM samples_1m
             WHERE subject_id = ?1 AND t >= ?2 AND t < ?3 AND mem IS NOT NULL
             ORDER BY mem LIMIT 1 OFFSET ?4",
        )
        .bind(subject_id)
        .bind(from)
        .bind(to)
        .bind(signed(Some(rank(count(n_mem), 0.5))))
        .fetch_optional(&self.pool)
        .await?
        .map_or(0, |(m,)| count(m));

        Ok(Summary {
            running: count(running),
            mem_peak: mem_peak.map_or(0, count),
            mem_typical,
            mem_limit: limit.map(count),
            cpu_typical: at(p50),
            cpu_p95: at(p95),
            cpu_burst: cpu_burst.unwrap_or(0.0).max(0.0),
            throttled,
        })
    }

    /// Each of a stack's containers over `[from, to)`: typical and peak CPU
    /// and memory, as `domain::metrics::figures` defines them.
    pub async fn container_figures(
        &self,
        project: &str,
        resolution: Resolution,
        from: i64,
        to: i64,
    ) -> Result<Vec<ContainerFigures>> {
        let (peaks, cpu, mem) = match resolution {
            Resolution::Quarter => (
                figures_peaks!("samples_15m"),
                figures_p95!("samples_15m", "cpu"),
                figures_p95!("samples_15m", "mem"),
            ),
            _ => (
                figures_peaks!("samples_1m"),
                figures_p95!("samples_1m", "cpu"),
                figures_p95!("samples_1m", "mem"),
            ),
        };
        let peaks = sqlx::query_as::<_, (String, Option<String>, Option<f64>, Option<i64>)>(peaks)
            .bind(project)
            .bind(from)
            .bind(to)
            .fetch_all(&self.pool)
            .await?;
        let cpu: HashMap<String, f64> = sqlx::query_as::<_, (String, f64)>(cpu)
            .bind(project)
            .bind(from)
            .bind(to)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .collect();
        let mem: HashMap<String, i64> = sqlx::query_as::<_, (String, i64)>(mem)
            .bind(project)
            .bind(from)
            .bind(to)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .collect();
        let bytes = |v: i64| u64::try_from(v).unwrap_or(0);
        Ok(peaks
            .into_iter()
            .map(|(key, service, cpu_peak, mem_peak)| ContainerFigures {
                cpu_typical: cpu.get(&key).copied(),
                mem_typical: mem.get(&key).copied().map(bytes),
                key,
                service,
                cpu_peak,
                mem_peak: mem_peak.map(bytes),
            })
            .collect())
    }

    pub async fn record_event(&self, subject_id: i64, t: i64, kind: &str) -> Result<()> {
        sqlx::query("INSERT INTO events (subject_id, t, kind) VALUES (?1, ?2, ?3)")
            .bind(subject_id)
            .bind(t)
            .bind(kind)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn count_events(&self, subject_id: i64, kind: &str, since: i64) -> Result<u32> {
        let (n,) = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM events WHERE subject_id = ?1 AND kind = ?2 AND t >= ?3",
        )
        .bind(subject_id)
        .bind(kind)
        .bind(since)
        .fetch_one(&self.pool)
        .await?;
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    }

    pub async fn prune(&self, now: i64) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM samples_1m WHERE t < ?1")
            .bind(now - MINUTE_DAYS * DAY)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM samples_15m WHERE t < ?1")
            .bind(now - QUARTER_DAYS * DAY)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM events WHERE t < ?1")
            .bind(now - QUARTER_DAYS * DAY)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM subjects WHERE last_seen < ?1")
            .bind(now - QUARTER_DAYS * DAY)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

/// Seconds per bucket: a whole number of rows, enough that `[from, to)`
/// needs at most `points` of them.
fn bucket(resolution: Resolution, from: i64, to: i64, points: usize) -> i64 {
    let up = |a: i64, b: i64| (a + b - 1) / b;
    let step = resolution.step().max(1);
    let rows = up((to - from).max(0), step);
    let points = i64::try_from(points).unwrap_or(i64::MAX).max(1);
    up(rows, points).max(1) * step
}

fn subject_row(
    (id, kind, key, project, service, last_seen): (
        i64,
        String,
        String,
        Option<String>,
        Option<String>,
        i64,
    ),
) -> Option<SubjectRow> {
    Some(SubjectRow {
        id,
        kind: SubjectKind::parse(&kind)?,
        key,
        project,
        service,
        last_seen,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// How SQLite would run `sql`, its plan's lines joined.
    async fn plan(m: &MetricsStore, sql: &str) -> String {
        // A test's own literal tables, not input.
        let explain = sqlx::AssertSqlSafe(format!("EXPLAIN QUERY PLAN {sql}"));
        sqlx::query_as::<_, (i64, i64, i64, String)>(explain)
            .bind(0_i64)
            .bind(0_i64)
            .fetch_all(&m.pool)
            .await
            .unwrap()
            .into_iter()
            .map(|(.., detail)| detail)
            .collect::<Vec<_>>()
            .join("; ")
    }

    #[tokio::test]
    async fn rollup_and_pruning_find_rows_by_time_without_a_full_scan() {
        // Both filter on `t` alone, which the (subject_id, t) key cannot
        // serve: without an index of their own they read every row kept.
        let m = MetricsStore::open_in_memory().await.unwrap();
        for table in ["samples_1m", "samples_15m"] {
            for sql in [
                format!("SELECT subject_id FROM {table} WHERE t >= ?1 AND t < ?2"),
                format!("DELETE FROM {table} WHERE t < ?1 AND ?2 = ?2"),
            ] {
                let plan = plan(&m, &sql).await;
                assert!(plan.contains(&format!("idx_{table}_t")), "{sql}: {plan}");
            }
        }
    }
}
