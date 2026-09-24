//! Image reference parsing.
//!
//! The defaults here are not obvious, and getting one wrong means asking the
//! wrong registry about the wrong repository and reporting "no update"
//! forever, which is silent and therefore worse than an error.

use registry::reference::{DEFAULT_REGISTRY, ParseError, parse};

fn parsed(image: &str) -> (String, String, Option<String>, Option<String>) {
    let r = parse(image).unwrap_or_else(|e| panic!("{image:?}: {e}"));
    (r.registry, r.repository, r.tag, r.digest)
}

#[test]
fn a_bare_name_is_docker_hubs_library_namespace() {
    let (registry, repository, tag, digest) = parsed("nginx");
    assert_eq!(registry, DEFAULT_REGISTRY);
    assert_eq!(repository, "library/nginx", "Docker Hub hides `library/`");
    assert_eq!(tag.as_deref(), Some("latest"));
    assert_eq!(digest, None);
}

#[test]
fn a_user_image_is_docker_hub_not_a_host_called_user() {
    // The trap: `myteam/app` looks like host/path but is a Docker Hub user.
    let (registry, repository, ..) = parsed("myteam/app");
    assert_eq!(registry, DEFAULT_REGISTRY);
    assert_eq!(repository, "myteam/app");
}

#[test]
fn a_dotted_first_component_is_a_registry() {
    let (registry, repository, tag, _) = parsed("ghcr.io/user/app:1.2");
    assert_eq!(registry, "ghcr.io");
    assert_eq!(repository, "user/app");
    assert_eq!(tag.as_deref(), Some("1.2"));
}

#[test]
fn localhost_is_a_registry_even_without_a_dot() {
    assert_eq!(parsed("localhost/app").0, "localhost");
    assert_eq!(parsed("localhost:5000/app").0, "localhost:5000");
}

#[test]
fn a_port_is_part_of_the_host_not_a_tag() {
    let (registry, repository, tag, _) = parsed("registry.example:5000/team/app");
    assert_eq!(registry, "registry.example:5000");
    assert_eq!(repository, "team/app");
    assert_eq!(tag.as_deref(), Some("latest"), "the colon was a port");
}

#[test]
fn a_port_and_a_tag_can_appear_together() {
    let (registry, repository, tag, _) = parsed("registry.example:5000/team/app:v2");
    assert_eq!(registry, "registry.example:5000");
    assert_eq!(repository, "team/app");
    assert_eq!(tag.as_deref(), Some("v2"));
}

#[test]
fn a_deep_repository_path_survives() {
    let (registry, repository, ..) = parsed("ghcr.io/org/team/service/app:edge");
    assert_eq!(registry, "ghcr.io");
    assert_eq!(repository, "org/team/service/app");
}

#[test]
fn a_digest_is_recognised_and_pins_the_image() {
    const SHA: &str = "sha256:abc123";
    let reference = parse(&format!("nginx@{SHA}")).unwrap();

    assert_eq!(reference.repository, "library/nginx");
    assert_eq!(reference.digest.as_deref(), Some(SHA));
    assert_eq!(reference.tag, None, "a digest replaces the implied latest");
    assert!(reference.is_pinned());
    assert_eq!(reference.manifest_target(), SHA);
}

#[test]
fn a_tag_and_a_digest_together_keep_both() {
    let reference = parse("nginx:1.27@sha256:abc123").unwrap();
    assert_eq!(reference.tag.as_deref(), Some("1.27"));
    assert_eq!(reference.digest.as_deref(), Some("sha256:abc123"));
    assert!(reference.is_pinned());
}

#[test]
fn an_unpinned_image_asks_about_its_tag() {
    assert_eq!(parse("nginx:alpine").unwrap().manifest_target(), "alpine");
    assert_eq!(parse("nginx").unwrap().manifest_target(), "latest");
    assert!(!parse("nginx").unwrap().is_pinned());
}

#[test]
fn nonsense_is_an_error_rather_than_a_wrong_guess() {
    assert_eq!(parse(""), Err(ParseError::Empty));
    assert_eq!(parse("   "), Err(ParseError::Empty));
    assert!(parse("nginx@").is_err());
    assert!(parse("ghcr.io/").is_err());
}
