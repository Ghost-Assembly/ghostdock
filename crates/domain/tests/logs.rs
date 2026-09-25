//! Reading several containers' output as one.

use domain::logs::{MAX_CONTAINERS, Selection, merge};
use shared::logs::{Stream, TaggedLine};

fn line(container: &str, at: Option<&str>, text: &str) -> TaggedLine {
    TaggedLine {
        container: container.to_owned(),
        stack: None,
        stream: Stream::Stdout,
        at: at.map(str::to_owned),
        text: text.to_owned(),
    }
}

fn texts(lines: &[TaggedLine]) -> Vec<&str> {
    lines.iter().map(|l| l.text.as_str()).collect()
}

#[test]
fn lines_are_merged_by_when_they_were_written() {
    let a = vec![
        line("a", Some("2026-09-25T10:00:01.5Z"), "a1"),
        line("a", Some("2026-09-25T10:00:03Z"), "a2"),
    ];
    let b = vec![
        line("b", Some("2026-09-25T10:00:01.25Z"), "b1"),
        line("b", Some("2026-09-25T10:00:02.000000001Z"), "b2"),
    ];
    let (merged, cut) = merge(vec![a, b], 100);
    // Compared as strings, "…:01.25Z" < "…:01.5Z" happens to hold, but
    // "…:03Z" > "…:02.000000001Z" only when read as times.
    assert_eq!(texts(&merged), ["b1", "a1", "b2", "a2"]);
    assert!(!cut);
}

#[test]
fn a_whole_second_sorts_before_a_fraction_of_it() {
    // RFC 3339 with trailing zeros trimmed: as strings "10Z" > "10.5Z".
    let a = vec![line("a", Some("2026-09-25T10:00:10.5Z"), "later")];
    let b = vec![line("b", Some("2026-09-25T10:00:10Z"), "earlier")];
    let (merged, _) = merge(vec![a, b], 100);
    assert_eq!(texts(&merged), ["earlier", "later"]);
}

#[test]
fn one_containers_order_is_kept_even_when_its_stamps_disagree() {
    // The daemon stamps stdout and stderr as it reads them, so one log's
    // order is not always its timestamps' order; a line without a stamp
    // stays after the line before it.
    let a = vec![
        line("a", Some("2026-09-25T10:00:02Z"), "first"),
        line("a", None, "second"),
        line("a", Some("2026-09-25T10:00:03Z"), "third"),
    ];
    let b = vec![line("b", Some("2026-09-25T10:00:02.5Z"), "other")];
    let (merged, _) = merge(vec![a, b], 100);
    assert_eq!(texts(&merged), ["first", "second", "other", "third"]);

    let a = vec![
        line("a", Some("2026-09-25T10:00:03Z"), "written first"),
        line("a", Some("2026-09-25T10:00:02Z"), "written second"),
    ];
    let b = vec![line("b", Some("2026-09-25T10:00:02.5Z"), "other")];
    let (merged, _) = merge(vec![a, b], 100);
    assert_eq!(texts(&merged), ["other", "written first", "written second"]);
}

#[test]
fn equal_stamps_keep_the_order_the_containers_were_given_in() {
    let at = Some("2026-09-25T10:00:02Z");
    let a = vec![line("a", at, "a")];
    let b = vec![line("b", at, "b")];
    let (merged, _) = merge(vec![a, b], 100);
    assert_eq!(texts(&merged), ["a", "b"]);
}

#[test]
fn past_the_limit_the_newest_lines_are_kept_and_the_cut_is_said() {
    let a: Vec<TaggedLine> = (0..10)
        .map(|i| {
            line(
                "a",
                Some(&format!("2026-09-25T10:00:{i:02}Z")),
                &i.to_string(),
            )
        })
        .collect();
    let (merged, cut) = merge(vec![a], 3);
    assert_eq!(texts(&merged), ["7", "8", "9"]);
    assert!(cut);
}

#[test]
fn a_selection_is_exactly_one_of_all_a_stack_or_containers() {
    assert_eq!(Selection::parse(Some(""), None, None), Ok(Selection::All));
    assert_eq!(
        Selection::parse(Some("true"), None, None),
        Ok(Selection::All)
    );
    assert_eq!(
        Selection::parse(None, Some("7"), None),
        Ok(Selection::Stack(7))
    );
    assert_eq!(
        Selection::parse(None, None, Some("web-1,db_1,web-1")),
        Ok(Selection::Containers(vec![
            "web-1".to_owned(),
            "db_1".to_owned()
        ])),
        "named twice is followed once"
    );
    assert!(
        Selection::parse(None, None, None).is_err(),
        "nothing chosen"
    );
    assert!(
        Selection::parse(Some(""), Some("7"), None).is_err(),
        "two at once"
    );
    assert!(Selection::parse(Some("false"), None, None).is_err());
    assert!(Selection::parse(None, Some("blog"), None).is_err(), "an id");
    assert!(Selection::parse(None, None, Some("")).is_err(), "no names");
}

#[test]
fn a_container_name_cannot_carry_anything_else() {
    for bad in [
        "a&all=1", "../x", "a/b", "a b", "a?x", "a#x", "-lead", ".", "é",
    ] {
        assert!(
            Selection::parse(None, None, Some(bad)).is_err(),
            "{bad} accepted"
        );
    }
    let long = "a".repeat(129);
    assert!(Selection::parse(None, None, Some(&long)).is_err());
}

#[test]
fn more_than_the_cap_is_refused_and_says_the_cap() {
    let names: Vec<String> = (0..=MAX_CONTAINERS).map(|i| format!("c{i}")).collect();
    let err = Selection::parse(None, None, Some(&names.join(","))).expect_err("over the cap");
    assert!(err.contains(&MAX_CONTAINERS.to_string()), "{err}");
    let names: Vec<String> = (0..MAX_CONTAINERS).map(|i| format!("c{i}")).collect();
    assert!(Selection::parse(None, None, Some(&names.join(","))).is_ok());
}
