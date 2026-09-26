//! Slowing down password guessing.

use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

use server::limiter::{Limits, LoginLimiter};

const WINDOW: Duration = Duration::from_secs(60);

fn limiter() -> LoginLimiter {
    LoginLimiter::new(Limits {
        per_user: 3,
        per_address: 5,
        window: WINDOW,
        capacity: 100,
    })
}

fn address(last: u8) -> Option<IpAddr> {
    Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, last)))
}

#[test]
fn a_username_is_refused_after_its_limit_until_the_window_ends() {
    let l = limiter();
    let t = Instant::now();
    for _ in 0..3 {
        assert!(l.allows("admin", address(1), t));
        l.failed("admin", address(1), t);
    }
    assert!(!l.allows("admin", address(1), t));
    assert!(
        !l.allows("ADMIN", address(2), t),
        "neither case nor a new address gets around it"
    );
    assert!(l.allows("someone-else", address(2), t));
    assert!(l.allows("admin", address(1), t + WINDOW), "the window ends");
}

#[test]
fn an_address_is_refused_after_its_limit_whatever_the_username() {
    let l = limiter();
    let t = Instant::now();
    for i in 0..5 {
        l.failed(&format!("guess{i}"), address(9), t);
    }
    assert!(!l.allows("admin", address(9), t));
    assert!(l.allows("admin", address(10), t));
    assert!(l.allows("admin", None, t));
}

#[test]
fn signing_in_clears_the_usernames_count() {
    let l = limiter();
    let t = Instant::now();
    l.failed("admin", address(1), t);
    l.failed("admin", address(1), t);
    l.succeeded("Admin");
    l.failed("admin", address(1), t);
    l.failed("admin", address(1), t);
    assert!(l.allows("admin", address(1), t));
}

#[test]
fn memory_stays_bounded_however_many_names_are_tried() {
    let l = limiter();
    let t = Instant::now();
    for i in 0..1000 {
        l.failed(&format!("made-up-{i}"), None, t);
    }
    assert!(l.remembered() <= 100, "{}", l.remembered());
}

#[test]
fn a_flood_of_made_up_names_cannot_clear_a_lockout() {
    // At capacity the oldest entry makes room; a locked-out username is
    // usually the oldest, so evicting it would hand the attacker a fresh
    // allowance for the price of a burst of nonsense names.
    let l = limiter();
    let t = Instant::now();
    for _ in 0..3 {
        l.failed("admin", address(1), t);
    }
    assert!(!l.allows("admin", address(2), t));

    let later = t + Duration::from_secs(1);
    for i in 0..1000 {
        l.failed(&format!("made-up-{i}"), None, later);
    }
    assert!(
        !l.allows("admin", address(2), later),
        "the lockout outlives the flood"
    );
    // Usernames and addresses are each bounded; one address is remembered.
    assert!(l.remembered() <= 100 + 1, "{}", l.remembered());
}

#[test]
fn when_every_entry_is_a_lockout_memory_still_stays_bounded() {
    let l = limiter();
    let t = Instant::now();
    for i in 0..100 {
        for _ in 0..3 {
            l.failed(&format!("locked-{i}"), None, t);
        }
    }
    for i in 0..10 {
        l.failed(&format!("new-{i}"), None, t);
    }
    assert_eq!(l.remembered(), 100);
    assert!(!l.allows("locked-0", None, t), "no lockout was dropped");
}

/// Locks out `count` made-up usernames, none from any address.
fn lock_out_usernames(l: &LoginLimiter, count: usize, t: Instant) {
    for i in 0..count {
        for _ in 0..3 {
            l.failed(&format!("locked-{i}"), None, t);
        }
    }
}

/// Locks out `count` addresses, each trying a different made-up username.
fn lock_out_addresses(l: &LoginLimiter, count: u8, t: Instant) {
    for i in 0..count {
        for n in 0..5 {
            l.failed(&format!("guess-{i}-{n}"), address(i), t);
        }
    }
}

#[test]
fn a_full_table_of_username_lockouts_still_counts_addresses() {
    // Usernames and addresses are bounded separately, so filling one table
    // cannot stop the other counting.
    let l = limiter();
    let t = Instant::now();
    lock_out_usernames(&l, 100, t);

    let later = t + WINDOW - Duration::from_secs(1);
    for i in 0..5 {
        l.failed(&format!("fresh-{i}"), address(200), later);
    }

    // The username lockouts are over; the address's count is not.
    let after = t + WINDOW + Duration::from_secs(1);
    assert!(
        !l.allows("anyone", address(200), after),
        "the address was counted"
    );
    assert!(l.allows("anyone", address(201), after));
}

#[test]
fn a_new_address_is_refused_while_every_remembered_one_is_locked_out() {
    // Admitting it uncounted would give every fresh address unlimited
    // guesses; refusing it costs new addresses a wait until lockouts end.
    let l = limiter();
    let t = Instant::now();
    lock_out_addresses(&l, 100, t);

    assert!(!l.allows("admin", address(200), t));
    for _ in 0..1000 {
        l.failed("admin", address(200), t);
    }
    assert!(!l.allows("someone", address(201), t));
}

#[test]
fn a_new_username_is_refused_while_every_remembered_one_is_locked_out() {
    let l = limiter();
    let t = Instant::now();
    lock_out_usernames(&l, 100, t);
    assert!(!l.allows("admin", address(1), t));
}

#[test]
fn lockouts_ending_make_room_again() {
    let l = limiter();
    let t = Instant::now();
    lock_out_addresses(&l, 100, t);
    lock_out_usernames(&l, 100, t);

    let after = t + WINDOW;
    assert!(l.allows("admin", address(200), after));
    l.failed("admin", address(200), after);
    assert!(
        l.allows("admin", address(200), after),
        "counted, and one failure is under both limits"
    );
    assert!(l.remembered() <= 200, "{}", l.remembered());
}
