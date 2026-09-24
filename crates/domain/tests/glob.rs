//! Matching repository paths against a pattern.

use domain::glob::Pattern;

fn matches(pattern: &str, path: &str) -> bool {
    Pattern::parse(pattern)
        .expect("valid pattern")
        .matches(path)
}

#[test]
fn a_star_stays_within_one_directory() {
    assert!(matches("compose/*.yml", "compose/blog.yml"));
    assert!(!matches("compose/*.yml", "compose/old/blog.yml"));
    assert!(!matches("*.yml", "compose/blog.yml"));
}

#[test]
fn a_double_star_crosses_directories_including_none() {
    assert!(matches("**/compose.yaml", "compose.yaml"));
    assert!(matches("**/compose.yaml", "apps/blog/compose.yaml"));
    assert!(matches("apps/**/compose.yaml", "apps/compose.yaml"));
    assert!(matches("apps/**/compose.yaml", "apps/a/b/compose.yaml"));
    assert!(!matches("apps/**/compose.yaml", "other/compose.yaml"));
}

#[test]
fn braces_offer_alternatives() {
    let p = "**/{compose,docker-compose}.{yml,yaml}";
    for path in [
        "compose.yml",
        "a/compose.yaml",
        "a/b/docker-compose.yml",
        "docker-compose.yaml",
    ] {
        assert!(matches(p, path), "{path}");
    }
    assert!(!matches(p, "a/compose.json"));
    assert!(!matches(p, "a/xcompose.yml"));
}

#[test]
fn a_question_mark_is_one_character_but_never_a_slash() {
    assert!(matches("v?.yml", "v1.yml"));
    assert!(!matches("v?.yml", "v10.yml"));
    assert!(!matches("a?b", "a/b"));
}

#[test]
fn several_patterns_can_be_given_at_once() {
    let p = Pattern::parse("compose/*.yml, **/compose.yaml").unwrap();
    assert!(p.matches("compose/blog.yml"));
    assert!(p.matches("apps/wiki/compose.yaml"));
    assert!(!p.matches("README.md"));
}

#[test]
fn matching_is_literal_apart_from_the_wildcards() {
    assert!(matches("a.b/c+d.yml", "a.b/c+d.yml"));
    assert!(!matches("a.b/c+d.yml", "aXb/c+d.yml"));
}

#[test]
fn unbalanced_braces_and_empty_patterns_are_refused() {
    assert!(Pattern::parse("{a,b").is_err());
    assert!(Pattern::parse("a}").is_err());
    assert!(Pattern::parse("  ").is_err());
    assert!(Pattern::parse("a,,b").is_err());
}

#[test]
fn a_pathological_pattern_finishes_quickly() {
    // Backtracking matchers go exponential on stars against a near miss.
    let p = Pattern::parse("*a*a*a*a*a*a*a*a*b").unwrap();
    let start = std::time::Instant::now();
    assert!(!p.matches(&"a".repeat(60)));
    assert!(start.elapsed() < std::time::Duration::from_millis(100));
}
