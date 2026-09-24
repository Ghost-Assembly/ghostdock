//! Health fallback parsing for older daemons.

use domain::health::from_status;
use shared::container::Health;

#[test]
fn reads_health_from_a_status_string() {
    assert_eq!(from_status("Up 2 hours (healthy)"), Some(Health::Healthy));
    assert_eq!(
        from_status("Up 5 minutes (unhealthy)"),
        Some(Health::Unhealthy)
    );
    assert_eq!(
        from_status("Up 3 seconds (health: starting)"),
        Some(Health::Starting)
    );
}

#[test]
fn absent_health_is_none() {
    assert_eq!(from_status("Up 2 hours"), None);
    assert_eq!(from_status("Exited (0) 3 days ago"), None);
    assert_eq!(from_status(""), None);
}

#[test]
fn unhealthy_is_not_mistaken_for_healthy() {
    // "unhealthy" contains "healthy"; a naive substring check gets this wrong,
    // and getting it wrong shows a broken container as green.
    assert_eq!(
        from_status("Up 5 minutes (unhealthy)"),
        Some(Health::Unhealthy)
    );
}

#[test]
fn matching_is_case_insensitive() {
    assert_eq!(from_status("Up 2 hours (Healthy)"), Some(Health::Healthy));
}
