//! Alert channels, resource rules, and the record of deliveries.
//!
//! A channel's URL and token are sealed, each under its own purpose, and
//! only [`Store::channels_to_send`] opens them, for sending.

use shared::alerts::{
    AlertChannel, AlertDelivery, AlertRule, ChannelKind, Metric, NewAlertChannel, NewAlertRule,
};

use crate::secrets::purpose;
use crate::{Error, Result, Store, timestamp, unique_or};

/// Deliveries kept.
const DELIVERIES_KEPT: i64 = 200;

/// A channel opened for sending. Never serialized, and its `Debug` shows
/// neither secret.
#[derive(Clone)]
pub struct ChannelSecret {
    pub id: i64,
    pub name: String,
    pub kind: ChannelKind,
    pub url: String,
    pub token: Option<String>,
}

impl std::fmt::Debug for ChannelSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelSecret")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

fn kind(text: &str) -> ChannelKind {
    // The table's CHECK allows only the two.
    ChannelKind::parse(text).unwrap_or(ChannelKind::Webhook)
}

type RuleTuple = (i64, String, String, f64, i64, bool);

fn to_rule((id, subject, metric, above_pct, for_min, enabled): RuleTuple) -> AlertRule {
    AlertRule {
        id,
        subject,
        metric: Metric::parse(&metric).unwrap_or(Metric::Cpu),
        above_pct,
        for_min: u32::try_from(for_min).unwrap_or(1),
        enabled,
        // Whether it fires is known to the evaluator, not the table.
        firing: false,
    }
}

impl Store {
    // ---- channels -------------------------------------------------------

    /// Stores a channel, sealing its URL and token. Fails with
    /// [`Error::NameTaken`] on a duplicate name.
    pub async fn channel_create(&self, new: &NewAlertChannel) -> Result<AlertChannel> {
        let url = new.url.trim();
        let host = domain::checks::url_host(url).unwrap_or_default().to_owned();
        let url_enc = self.cipher().seal(purpose::ALERT_URL, url)?;
        let token_enc = match new
            .token
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            Some(token) => Some(self.cipher().seal(purpose::ALERT_TOKEN, token)?),
            None => None,
        };
        let (id, created_at) = sqlx::query_as::<_, (i64, i64)>(
            "INSERT INTO alert_channels (name, kind, url_enc, token_enc, host, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, unixepoch())
             RETURNING id, created_at",
        )
        .bind(new.name.trim())
        .bind(new.kind.as_str())
        .bind(&url_enc)
        .bind(&token_enc)
        .bind(&host)
        .fetch_one(self.pool())
        .await
        .map_err(unique_or(Error::NameTaken))?;
        Ok(AlertChannel {
            id,
            name: new.name.trim().to_owned(),
            kind: new.kind,
            host,
            created_at: timestamp(created_at),
        })
    }

    /// Every channel, by name: never a URL or token.
    pub async fn channels_list(&self) -> Result<Vec<AlertChannel>> {
        let rows = sqlx::query_as::<_, (i64, String, String, String, i64)>(
            "SELECT id, name, kind, host, created_at FROM alert_channels ORDER BY name",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, name, k, host, created_at)| AlertChannel {
                id,
                name,
                kind: kind(&k),
                host,
                created_at: timestamp(created_at),
            })
            .collect())
    }

    /// Every channel with its URL and token opened, for sending and
    /// nothing else.
    pub async fn channels_to_send(&self) -> Result<Vec<ChannelSecret>> {
        let rows = sqlx::query_as::<_, (i64, String, String, String, Option<String>)>(
            "SELECT id, name, kind, url_enc, token_enc FROM alert_channels ORDER BY id",
        )
        .fetch_all(self.pool())
        .await?;
        rows.into_iter()
            .map(|(id, name, k, url_enc, token_enc)| {
                Ok(ChannelSecret {
                    id,
                    name,
                    kind: kind(&k),
                    url: self.cipher().open(purpose::ALERT_URL, &url_enc)?,
                    token: match token_enc {
                        Some(sealed) => Some(self.cipher().open(purpose::ALERT_TOKEN, &sealed)?),
                        None => None,
                    },
                })
            })
            .collect()
    }

    /// Removes a channel. False if there was none.
    pub async fn channel_delete(&self, id: i64) -> Result<bool> {
        let done = sqlx::query("DELETE FROM alert_channels WHERE id = ?1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(done.rows_affected() > 0)
    }

    // ---- rules ----------------------------------------------------------

    pub async fn rule_create(&self, rule: &NewAlertRule) -> Result<AlertRule> {
        let row = sqlx::query_as::<_, RuleTuple>(
            "INSERT INTO alert_rules (subject, metric, above_pct, for_min, enabled, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, unixepoch())
             RETURNING id, subject, metric, above_pct, for_min, enabled",
        )
        .bind(rule.subject.trim())
        .bind(rule.metric.as_str())
        .bind(rule.above_pct)
        .bind(rule.for_min)
        .bind(rule.enabled)
        .fetch_one(self.pool())
        .await?;
        Ok(to_rule(row))
    }

    /// Replaces a rule's settings; `None` if there is no such rule.
    pub async fn rule_update(&self, id: i64, rule: &NewAlertRule) -> Result<Option<AlertRule>> {
        let row = sqlx::query_as::<_, RuleTuple>(
            "UPDATE alert_rules SET subject = ?2, metric = ?3, above_pct = ?4, for_min = ?5,
                 enabled = ?6
             WHERE id = ?1
             RETURNING id, subject, metric, above_pct, for_min, enabled",
        )
        .bind(id)
        .bind(rule.subject.trim())
        .bind(rule.metric.as_str())
        .bind(rule.above_pct)
        .bind(rule.for_min)
        .bind(rule.enabled)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(to_rule))
    }

    pub async fn rules_list(&self) -> Result<Vec<AlertRule>> {
        let rows = sqlx::query_as::<_, RuleTuple>(
            "SELECT id, subject, metric, above_pct, for_min, enabled FROM alert_rules ORDER BY id",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(to_rule).collect())
    }

    pub async fn rule_delete(&self, id: i64) -> Result<bool> {
        let done = sqlx::query("DELETE FROM alert_rules WHERE id = ?1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(done.rows_affected() > 0)
    }

    // ---- deliveries -----------------------------------------------------

    /// Records a delivery, keeping only the latest 200.
    pub async fn delivery_record(
        &self,
        channel: &str,
        subject: &str,
        state: &str,
        ok: bool,
        attempts: u32,
        error: Option<&str>,
    ) -> Result<AlertDelivery> {
        let mut tx = self.pool().begin().await?;
        let (id, at) = sqlx::query_as::<_, (i64, i64)>(
            "INSERT INTO alert_deliveries (channel, subject, state, at, ok, attempts, error)
             VALUES (?1, ?2, ?3, unixepoch(), ?4, ?5, ?6)
             RETURNING id, at",
        )
        .bind(channel)
        .bind(subject)
        .bind(state)
        .bind(ok)
        .bind(attempts)
        .bind(error)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM alert_deliveries WHERE id <= ?1 - ?2")
            .bind(id)
            .bind(DELIVERIES_KEPT)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(AlertDelivery {
            id,
            channel: channel.to_owned(),
            subject: subject.to_owned(),
            state: state.to_owned(),
            at: timestamp(at),
            ok,
            attempts,
            error: error.map(str::to_owned),
        })
    }

    /// Newest first.
    pub async fn deliveries_recent(&self, limit: i64) -> Result<Vec<AlertDelivery>> {
        let rows =
            sqlx::query_as::<_, (i64, String, String, String, i64, bool, i64, Option<String>)>(
                "SELECT id, channel, subject, state, at, ok, attempts, error
             FROM alert_deliveries ORDER BY id DESC LIMIT ?1",
            )
            .bind(limit)
            .fetch_all(self.pool())
            .await?;
        Ok(rows
            .into_iter()
            .map(
                |(id, channel, subject, state, at, ok, attempts, error)| AlertDelivery {
                    id,
                    channel,
                    subject,
                    state,
                    at: timestamp(at),
                    ok,
                    attempts: u32::try_from(attempts).unwrap_or(0),
                    error,
                },
            )
            .collect())
    }
}
