//! Uptime checks and their incidents.

use shared::checks::{Check, CheckKind, Incident};

use crate::{Error, Result, Store, timestamp, unique_or};

type CheckTuple = (
    i64,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    i64,
    i64,
    Option<String>,
    Option<i64>,
    Option<i64>,
    bool,
    bool,
    i64,
);

/// A query for checks, given its clause after `FROM checks`. Literals
/// only, so every statement is static.
macro_rules! select {
    ($rest:literal) => {
        concat!(
            "SELECT id, name, kind, target, interval_s, timeout_s, retries,
                    expect_status_min, expect_status_max, keyword, latency_warn_ms, stack_id,
                    enabled, notify, created_at FROM checks ",
            $rest
        )
    };
}

/// A stored whole number as the width the wire uses; one out of range reads
/// as the nearest end rather than failing the row.
fn narrow<T: TryFrom<i64> + Default>(v: i64) -> T {
    T::try_from(v).unwrap_or_default()
}

fn to_check(row: CheckTuple) -> Check {
    let (
        id,
        name,
        kind,
        target,
        interval_s,
        timeout_s,
        retries,
        expect_status_min,
        expect_status_max,
        keyword,
        latency_warn_ms,
        stack_id,
        enabled,
        notify,
        created_at,
    ) = row;
    Check {
        id,
        name,
        // The table's CHECK allows only the three.
        kind: CheckKind::parse(&kind).unwrap_or(CheckKind::Http),
        target,
        interval_s: narrow(interval_s),
        timeout_s: narrow(timeout_s),
        retries: narrow(retries),
        expect_status_min: narrow(expect_status_min),
        expect_status_max: narrow(expect_status_max),
        keyword,
        latency_warn_ms: latency_warn_ms.map(narrow),
        stack_id,
        enabled,
        notify,
        created_at: timestamp(created_at),
    }
}

impl Store {
    /// Every check on `host_id`, by name.
    pub async fn checks_list(&self, host_id: i64) -> Result<Vec<Check>> {
        let rows = sqlx::query_as::<_, CheckTuple>(select!("WHERE host_id = ?1 ORDER BY name"))
            .bind(host_id)
            .fetch_all(self.pool())
            .await?;
        Ok(rows.into_iter().map(to_check).collect())
    }

    /// Every check on every host, for the scheduler.
    pub async fn checks_all(&self) -> Result<Vec<Check>> {
        let rows = sqlx::query_as::<_, CheckTuple>(select!("ORDER BY id"))
            .fetch_all(self.pool())
            .await?;
        Ok(rows.into_iter().map(to_check).collect())
    }

    pub async fn check_by_id(&self, id: i64) -> Result<Option<Check>> {
        let row = sqlx::query_as::<_, CheckTuple>(select!("WHERE id = ?1"))
            .bind(id)
            .fetch_optional(self.pool())
            .await?;
        Ok(row.map(to_check))
    }

    /// The host a check belongs to.
    pub async fn check_host(&self, id: i64) -> Result<Option<i64>> {
        Ok(
            sqlx::query_as::<_, (i64,)>("SELECT host_id FROM checks WHERE id = ?1")
                .bind(id)
                .fetch_optional(self.pool())
                .await?
                .map(|(h,)| h),
        )
    }

    /// Stores a check; its `id` and `created_at` are ignored and assigned.
    /// Fails with [`Error::NameTaken`] when the host has one of that name.
    pub async fn check_create(&self, host_id: i64, check: &Check) -> Result<Check> {
        let (id,) = sqlx::query_as::<_, (i64,)>(
            "INSERT INTO checks (host_id, name, kind, target, interval_s, timeout_s, retries,
                                 expect_status_min, expect_status_max, keyword, latency_warn_ms,
                                 stack_id, enabled, notify, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, unixepoch())
             RETURNING id",
        )
        .bind(host_id)
        .bind(&check.name)
        .bind(check.kind.as_str())
        .bind(&check.target)
        .bind(check.interval_s)
        .bind(check.timeout_s)
        .bind(check.retries)
        .bind(check.expect_status_min)
        .bind(check.expect_status_max)
        .bind(&check.keyword)
        .bind(check.latency_warn_ms)
        .bind(check.stack_id)
        .bind(check.enabled)
        .bind(check.notify)
        .fetch_one(self.pool())
        .await
        .map_err(unique_or(Error::NameTaken))?;
        self.check_by_id(id).await?.ok_or(Error::NotFound)
    }

    /// Replaces a check's settings with `check`'s.
    pub async fn check_update(&self, check: &Check) -> Result<Check> {
        let done = sqlx::query(
            "UPDATE checks SET name = ?2, kind = ?3, target = ?4, interval_s = ?5,
                 timeout_s = ?6, retries = ?7, expect_status_min = ?8, expect_status_max = ?9,
                 keyword = ?10, latency_warn_ms = ?11, stack_id = ?12, enabled = ?13, notify = ?14
             WHERE id = ?1",
        )
        .bind(check.id)
        .bind(&check.name)
        .bind(check.kind.as_str())
        .bind(&check.target)
        .bind(check.interval_s)
        .bind(check.timeout_s)
        .bind(check.retries)
        .bind(check.expect_status_min)
        .bind(check.expect_status_max)
        .bind(&check.keyword)
        .bind(check.latency_warn_ms)
        .bind(check.stack_id)
        .bind(check.enabled)
        .bind(check.notify)
        .execute(self.pool())
        .await
        .map_err(unique_or(Error::NameTaken))?;
        if done.rows_affected() == 0 {
            return Err(Error::NotFound);
        }
        self.check_by_id(check.id).await?.ok_or(Error::NotFound)
    }

    /// Removes a check and its incidents. False if there was none.
    pub async fn check_delete(&self, id: i64) -> Result<bool> {
        let done = sqlx::query("DELETE FROM checks WHERE id = ?1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(done.rows_affected() > 0)
    }

    // ---- incidents ------------------------------------------------------

    pub async fn incident_is_open(&self, check_id: i64) -> Result<bool> {
        let (open,) = sqlx::query_as::<_, (bool,)>(
            "SELECT EXISTS (SELECT 1 FROM incidents WHERE check_id = ?1 AND ended_at IS NULL)",
        )
        .bind(check_id)
        .fetch_one(self.pool())
        .await?;
        Ok(open)
    }

    pub async fn incident_open(&self, check_id: i64, cause: &str, at: i64) -> Result<()> {
        sqlx::query("INSERT INTO incidents (check_id, started_at, cause) VALUES (?1, ?2, ?3)")
            .bind(check_id)
            .bind(at)
            .bind(cause)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Ends whatever incident `check_id` has open.
    pub async fn incident_close(&self, check_id: i64, at: i64) -> Result<()> {
        sqlx::query("UPDATE incidents SET ended_at = ?2 WHERE check_id = ?1 AND ended_at IS NULL")
            .bind(check_id)
            .bind(at)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// The latest incidents across a host's checks, newest first.
    pub async fn incidents_recent(&self, host_id: i64, limit: i64) -> Result<Vec<Incident>> {
        let rows = sqlx::query_as::<_, IncidentTuple>(
            "SELECT i.id, i.check_id, c.name, i.started_at, i.ended_at, i.cause
             FROM incidents i JOIN checks c ON c.id = i.check_id
             WHERE c.host_id = ?1 ORDER BY i.started_at DESC, i.id DESC LIMIT ?2",
        )
        .bind(host_id)
        .bind(limit)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(to_incident).collect())
    }

    /// One check's incidents still open or ended since `since`, newest
    /// first.
    pub async fn incidents_for(&self, check_id: i64, since: i64) -> Result<Vec<Incident>> {
        let rows = sqlx::query_as::<_, IncidentTuple>(
            "SELECT i.id, i.check_id, c.name, i.started_at, i.ended_at, i.cause
             FROM incidents i JOIN checks c ON c.id = i.check_id
             WHERE i.check_id = ?1 AND (i.ended_at IS NULL OR i.ended_at >= ?2)
             ORDER BY i.started_at DESC, i.id DESC LIMIT 200",
        )
        .bind(check_id)
        .bind(since)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(to_incident).collect())
    }
}

type IncidentTuple = (i64, i64, String, i64, Option<i64>, String);

fn to_incident((id, check_id, check_name, started_at, ended_at, cause): IncidentTuple) -> Incident {
    Incident {
        id,
        check_id,
        check_name,
        started_at: timestamp(started_at),
        ended_at: ended_at.map(timestamp),
        cause,
    }
}
