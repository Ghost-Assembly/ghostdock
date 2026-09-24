//! `.env` rendering, an injection boundary.

use compose::env::{EnvError, render};

fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[test]
fn renders_ordinary_variables() {
    let out = render(&vars(&[("PUID", "1000"), ("TZ", "Europe/London")])).unwrap();
    assert_eq!(out, "PUID='1000'\nTZ='Europe/London'\n");
}

#[test]
fn quotes_values_so_compose_does_not_expand_or_split_them() {
    let out = render(&vars(&[("MSG", "hello world"), ("COST", "$100")])).unwrap();
    assert!(
        out.contains("MSG='hello world'"),
        "a space must not truncate a value"
    );
    assert!(
        out.contains("COST='$100'"),
        "an unquoted $ would be substituted away: {out}"
    );
}

#[test]
fn escapes_quotes_within_a_value() {
    let out = render(&vars(&[("Q", "it's")])).unwrap();
    assert_eq!(out, "Q='it'\\''s'\n");
}

#[test]
fn a_newline_in_a_value_is_refused() {
    // Otherwise setting one variable sets any variable.
    for evil in ["a\nADMIN=true", "a\r\nADMIN=true", "\n"] {
        assert_eq!(
            render(&vars(&[("X", evil)])),
            Err(EnvError::NewlineInValue("X".to_owned())),
            "{evil:?} must not be renderable"
        );
    }
}

#[test]
fn keys_are_restricted() {
    assert_eq!(render(&vars(&[("", "v")])), Err(EnvError::EmptyKey));
    assert!(matches!(
        render(&vars(&[("A B", "v")])),
        Err(EnvError::BadKey(_))
    ));
    assert!(matches!(
        render(&vars(&[("1ST", "v")])),
        Err(EnvError::BadKey(_))
    ));
    assert!(matches!(
        render(&vars(&[("A=B", "v")])),
        Err(EnvError::BadKey(_))
    ));
    assert!(render(&vars(&[("_OK9", "v")])).is_ok());
}
