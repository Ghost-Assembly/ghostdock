//! Registry authentication challenges.

use registry::challenge::{Challenge, parse};

#[test]
fn parses_a_docker_hub_challenge() {
    let challenge = parse(
        r#"Bearer realm="https://auth.docker.io/token",service="registry.docker.io",scope="repository:library/nginx:pull""#,
    )
    .expect("a bearer challenge");

    assert_eq!(challenge.realm, "https://auth.docker.io/token");
    assert_eq!(challenge.service.as_deref(), Some("registry.docker.io"));
    assert_eq!(
        challenge.scope.as_deref(),
        Some("repository:library/nginx:pull")
    );
}

#[test]
fn a_scope_containing_commas_survives() {
    // `pull,push` is ordinary, and a naive split on commas loses half of it,
    // producing a token without the access that was requested.
    let challenge =
        parse(r#"Bearer realm="https://auth.example/token",scope="repository:team/app:pull,push""#)
            .unwrap();

    assert_eq!(
        challenge.scope.as_deref(),
        Some("repository:team/app:pull,push")
    );
}

#[test]
fn a_challenge_without_a_realm_is_unusable() {
    assert_eq!(parse(r#"Bearer service="registry.example""#), None);
}

#[test]
fn a_non_bearer_scheme_is_declined() {
    assert_eq!(parse("Basic realm=\"registry\""), None);
    assert_eq!(parse("nonsense"), None);
}

#[test]
fn a_token_url_carries_service_and_scope() {
    let url = Challenge {
        realm: "https://auth.docker.io/token".to_owned(),
        service: Some("registry.docker.io".to_owned()),
        scope: Some("repository:library/nginx:pull".to_owned()),
    }
    .token_url();

    assert_eq!(
        url,
        "https://auth.docker.io/token?service=registry.docker.io&scope=repository:library/nginx:pull"
    );
}

#[test]
fn a_realm_that_already_has_a_query_gets_an_ampersand() {
    let url = Challenge {
        realm: "https://auth.example/token?account=x".to_owned(),
        service: Some("reg".to_owned()),
        scope: None,
    }
    .token_url();

    assert_eq!(url, "https://auth.example/token?account=x&service=reg");
}

#[test]
fn awkward_characters_in_a_scope_are_encoded() {
    let url = Challenge {
        realm: "https://auth.example/token".to_owned(),
        service: None,
        scope: Some("repository:team/app:pull,push".to_owned()),
    }
    .token_url();

    assert!(
        url.ends_with("scope=repository:team/app:pull%2Cpush"),
        "the comma must not end the parameter: {url}"
    );
}
