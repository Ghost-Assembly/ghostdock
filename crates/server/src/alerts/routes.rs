//! Alert channels, resource rules and deliveries over HTTP.
//!
//! A channel's URL and token go in and never come back out: the list
//! shows its name, kind and the host the URL points at, and the audit
//! trail records its name alone.

use axum::Json;
use axum::extract::{Path, State};
use shared::alerts::{AlertChannel, AlertDelivery, AlertRule, NewAlertChannel, NewAlertRule};
use shared::reference::Access;
use shared::token::Permission;

use crate::auth::{Authorized, perm};
use crate::error::ApiError;
use crate::reference::Routes;
use crate::state::AppState;

/// Deliveries listed.
const DELIVERIES: i64 = 200;

pub fn routes() -> Routes {
    let view = Access::Token(Permission::HostView);
    let manage = Access::Token(Permission::AlertsManage);
    Routes::new("Alerts")
        .get(
            "/alerts/channels",
            view,
            "Where alerts go: each channel's name, kind and host; never its URL or token",
            channels,
        )
        .post(
            "/alerts/channels",
            manage,
            "Adds a webhook or ntfy channel; its URL and token cannot be read back",
            add_channel,
        )
        .delete(
            "/alerts/channels/{id}",
            manage,
            "Removes an alert channel",
            remove_channel,
        )
        .post(
            "/alerts/channels/{id}/test",
            manage,
            "Sends a test alert to one channel and says how it went",
            test_channel,
        )
        .get(
            "/alerts/rules",
            view,
            "Resource rules, and whether each is firing now",
            rules,
        )
        .post(
            "/alerts/rules",
            manage,
            "Adds a rule: alert when CPU, memory or disk stays over a share for some minutes",
            add_rule,
        )
        .put(
            "/alerts/rules/{id}",
            manage,
            "Changes a resource rule",
            change_rule,
        )
        .delete(
            "/alerts/rules/{id}",
            manage,
            "Removes a resource rule",
            remove_rule,
        )
        .get(
            "/alerts/deliveries",
            view,
            "The latest 200 alerts sent, newest first, and how each went",
            deliveries,
        )
}

async fn channels(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
) -> Result<Json<Vec<AlertChannel>>, ApiError> {
    Ok(Json(state.store.channels_list().await?))
}

async fn add_channel(
    principal: Authorized<perm::AlertsManage>,
    State(state): State<AppState>,
    Json(new): Json<NewAlertChannel>,
) -> Result<Json<AlertChannel>, ApiError> {
    domain::alerts::check_channel(&new).map_err(ApiError::BadRequest)?;
    let made = state
        .store
        .channel_create(&new)
        .await
        .map_err(|e| match e {
            store::Error::NameTaken => ApiError::Conflict(format!(
                "A channel named {} already exists.",
                new.name.trim()
            )),
            other => ApiError::from(other),
        })?;
    // The name and kind, never the URL or token.
    crate::audit::record(
        &state,
        &principal,
        "add alert channel",
        &made.name,
        Some(made.kind.as_str()),
    )
    .await;
    Ok(Json(made))
}

async fn remove_channel(
    principal: Authorized<perm::AlertsManage>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<axum::http::StatusCode, ApiError> {
    let name = state
        .store
        .channels_list()
        .await?
        .into_iter()
        .find(|c| c.id == id)
        .ok_or(ApiError::NotFound)?
        .name;
    state.store.channel_delete(id).await?;
    crate::audit::record(&state, &principal, "remove alert channel", &name, None).await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn test_channel(
    principal: Authorized<perm::AlertsManage>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<AlertDelivery>, ApiError> {
    let delivery = state.alerts.test(id).await?.ok_or(ApiError::NotFound)?;
    crate::audit::record(
        &state,
        &principal,
        "test alert channel",
        &delivery.channel,
        Some(if delivery.ok { "delivered" } else { "failed" }),
    )
    .await;
    Ok(Json(delivery))
}

async fn rules(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
) -> Result<Json<Vec<AlertRule>>, ApiError> {
    let mut rules = state.store.rules_list().await?;
    for rule in &mut rules {
        rule.firing = state.alerts.firing(rule.id);
    }
    Ok(Json(rules))
}

/// A rule in words for the audit trail: `host cpu above 90%`.
fn described(rule: &NewAlertRule) -> String {
    format!(
        "{} {} above {}% for {} min",
        rule.subject.trim(),
        rule.metric.as_str(),
        rule.above_pct,
        rule.for_min
    )
}

async fn add_rule(
    principal: Authorized<perm::AlertsManage>,
    State(state): State<AppState>,
    Json(new): Json<NewAlertRule>,
) -> Result<Json<AlertRule>, ApiError> {
    checked(&state, &new).await?;
    let made = state.store.rule_create(&new).await?;
    crate::audit::record(&state, &principal, "add alert rule", &described(&new), None).await;
    Ok(Json(made))
}

async fn change_rule(
    principal: Authorized<perm::AlertsManage>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(new): Json<NewAlertRule>,
) -> Result<Json<AlertRule>, ApiError> {
    checked(&state, &new).await?;
    let saved = state
        .store
        .rule_update(id, &new)
        .await?
        .ok_or(ApiError::NotFound)?;
    // Its streak was against the old threshold.
    state.alerts.reset(id);
    crate::audit::record(
        &state,
        &principal,
        "change alert rule",
        &described(&new),
        None,
    )
    .await;
    Ok(Json(saved))
}

async fn remove_rule(
    principal: Authorized<perm::AlertsManage>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<axum::http::StatusCode, ApiError> {
    if !state.store.rule_delete(id).await? {
        return Err(ApiError::NotFound);
    }
    state.alerts.reset(id);
    crate::audit::record(
        &state,
        &principal,
        "remove alert rule",
        &id.to_string(),
        None,
    )
    .await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Refuses a rule GhostDock cannot evaluate, or one on a stack that does
/// not exist.
async fn checked(state: &AppState, rule: &NewAlertRule) -> Result<(), ApiError> {
    let subject = domain::alerts::check_rule(rule).map_err(ApiError::BadRequest)?;
    if let domain::alerts::Subject::Stack(id) = subject
        && state.store.stack_by_id(id).await?.is_none()
    {
        return Err(ApiError::BadRequest(
            "That stack does not exist.".to_owned(),
        ));
    }
    Ok(())
}

async fn deliveries(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
) -> Result<Json<Vec<AlertDelivery>>, ApiError> {
    Ok(Json(state.store.deliveries_recent(DELIVERIES).await?))
}
