//! Naming stacks found in a repository.

use domain::discovery::{DEFAULT_PATTERN, stack_name};
use domain::glob::Pattern;

#[test]
fn a_file_named_for_its_stack_gives_the_name() {
    assert_eq!(stack_name("compose/blog.yml").as_deref(), Some("blog"));
    assert_eq!(stack_name("stacks/wiki.yaml").as_deref(), Some("wiki"));
}

#[test]
fn a_generic_file_is_named_for_its_directory() {
    assert_eq!(
        stack_name("apps/blog/compose.yaml").as_deref(),
        Some("blog")
    );
    assert_eq!(
        stack_name("wiki/docker-compose.yml").as_deref(),
        Some("wiki")
    );
}

#[test]
fn a_generic_file_at_the_root_has_no_name_of_its_own() {
    assert_eq!(stack_name("compose.yaml"), None);
    assert_eq!(stack_name("docker-compose.yml"), None);
}

#[test]
fn the_default_pattern_finds_the_common_layouts() {
    let p = Pattern::parse(DEFAULT_PATTERN).unwrap();
    for path in [
        "compose.yaml",
        "apps/blog/compose.yml",
        "wiki/docker-compose.yaml",
        "compose/blog.yml",
        "compose/wiki.yaml",
    ] {
        assert!(p.matches(path), "{path}");
    }
    for path in [
        "README.md",
        ".github/workflows/ci.yml",
        "compose/old/blog.yml",
    ] {
        assert!(!p.matches(path), "{path}");
    }
}
