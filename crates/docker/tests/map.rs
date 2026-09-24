//! Mapping daemon container summaries onto wire types.

use bollard::models::{
    ContainerSummary, ContainerSummaryHealth, ContainerSummaryHealthStatusEnum,
    ContainerSummaryStateEnum, PortSummary, PortSummaryTypeEnum,
};
use docker::map::{LABEL_PROJECT, LABEL_SERVICE, to_container};
use shared::container::{ContainerState, Health};
use std::collections::HashMap;

fn summary() -> ContainerSummary {
    ContainerSummary {
        id: Some("abcdef1234567890".to_owned()),
        names: Some(vec!["/web".to_owned()]),
        image: Some("nginx:alpine".to_owned()),
        state: Some(ContainerSummaryStateEnum::RUNNING),
        status: Some("Up 2 hours".to_owned()),
        created: Some(1_700_000_000),
        ..Default::default()
    }
}

#[test]
fn strips_the_leading_slash_from_the_name() {
    assert_eq!(to_container(summary()).name, "web");
}

#[test]
fn falls_back_to_the_id_when_a_container_is_unnamed() {
    let mut s = summary();
    s.names = None;
    assert_eq!(to_container(s.clone()).name, "abcdef1234567890");

    s.names = Some(Vec::new());
    assert_eq!(to_container(s).name, "abcdef1234567890");
}

#[test]
fn maps_state_and_timestamps() {
    let c = to_container(summary());
    assert_eq!(c.state, ContainerState::Running);
    assert_eq!(c.image, "nginx:alpine");
    assert_eq!(c.created.unwrap().timestamp(), 1_700_000_000);
}

#[test]
fn an_unknown_state_does_not_panic() {
    let mut s = summary();
    s.state = None;
    assert_eq!(to_container(s).state, ContainerState::Unknown);
}

#[test]
fn reads_compose_membership_from_labels() {
    let mut s = summary();
    s.labels = Some(HashMap::from([
        (LABEL_PROJECT.to_owned(), "blog".to_owned()),
        (LABEL_SERVICE.to_owned(), "web".to_owned()),
    ]));

    let membership = to_container(s).compose.expect("compose labels present");
    assert_eq!(membership.project, "blog");
    assert_eq!(membership.service, "web");
}

#[test]
fn a_container_outside_compose_has_no_membership() {
    assert!(to_container(summary()).compose.is_none());

    // A project label without a service label is not a usable membership.
    let mut s = summary();
    s.labels = Some(HashMap::from([(
        LABEL_PROJECT.to_owned(),
        "blog".to_owned(),
    )]));
    assert!(to_container(s).compose.is_none());
}

#[test]
fn maps_published_and_unpublished_ports() {
    let mut s = summary();
    s.ports = Some(vec![
        PortSummary {
            ip: Some("0.0.0.0".to_owned()),
            private_port: 80,
            public_port: Some(8080),
            typ: Some(PortSummaryTypeEnum::TCP),
        },
        PortSummary {
            ip: None,
            private_port: 9000,
            public_port: None,
            typ: None,
        },
    ]);

    let ports = to_container(s).ports;
    assert_eq!(ports.len(), 2);
    assert_eq!(ports[0].container_port, 80);
    assert_eq!(ports[0].host_port, Some(8080));
    assert_eq!(ports[0].protocol, "tcp");
    assert_eq!(ports[1].host_port, None, "exposed but not published");
    assert_eq!(ports[1].protocol, "tcp", "protocol defaults to tcp");
}

#[test]
fn prefers_the_structured_health_field() {
    let mut s = summary();
    s.health = Some(ContainerSummaryHealth {
        status: Some(ContainerSummaryHealthStatusEnum::UNHEALTHY),
        failing_streak: Some(3),
    });
    s.status = Some("Up 2 hours (healthy)".to_owned());

    assert_eq!(
        to_container(s).health,
        Some(Health::Unhealthy),
        "the structured field wins over the status string"
    );
}

#[test]
fn falls_back_to_the_status_string_on_older_daemons() {
    let mut s = summary();
    s.health = None;
    s.status = Some("Up 2 hours (unhealthy)".to_owned());
    assert_eq!(to_container(s).health, Some(Health::Unhealthy));
}

#[test]
fn a_container_without_a_healthcheck_reports_no_health() {
    assert_eq!(to_container(summary()).health, None);

    let mut s = summary();
    s.health = Some(ContainerSummaryHealth {
        status: Some(ContainerSummaryHealthStatusEnum::NONE),
        failing_streak: None,
    });
    assert_eq!(to_container(s).health, None);
}

#[test]
fn a_dual_stack_binding_is_reported_once() {
    // A published port on a dual-stack host comes back from the daemon twice,
    // once for 0.0.0.0 and once for ::. Passing both through shows the user
    // the same port listed twice. Captured from Docker 29.7.2.
    let mut s = summary();
    s.ports = Some(vec![
        PortSummary {
            ip: Some("0.0.0.0".to_owned()),
            private_port: 80,
            public_port: Some(18080),
            typ: Some(PortSummaryTypeEnum::TCP),
        },
        PortSummary {
            ip: Some("::".to_owned()),
            private_port: 80,
            public_port: Some(18080),
            typ: Some(PortSummaryTypeEnum::TCP),
        },
    ]);

    let ports = to_container(s).ports;
    assert_eq!(ports.len(), 1, "one mapping, not one per address family");
    assert_eq!(ports[0].host_ip.as_deref(), Some("0.0.0.0"));
    assert_eq!(ports[0].host_port, Some(18080));
}

#[test]
fn distinct_mappings_are_preserved_and_sorted() {
    let mut s = summary();
    s.ports = Some(vec![
        PortSummary {
            ip: None,
            private_port: 443,
            public_port: Some(8443),
            typ: Some(PortSummaryTypeEnum::TCP),
        },
        PortSummary {
            ip: None,
            private_port: 53,
            public_port: Some(53),
            typ: Some(PortSummaryTypeEnum::UDP),
        },
        PortSummary {
            ip: None,
            private_port: 53,
            public_port: Some(53),
            typ: Some(PortSummaryTypeEnum::TCP),
        },
    ]);

    let ports = to_container(s).ports;
    assert_eq!(
        ports.len(),
        3,
        "same port on tcp and udp are different mappings"
    );

    // Sorted, so the UI does not reorder rows between polls.
    let shown: Vec<_> = ports
        .iter()
        .map(|p| format!("{}/{}", p.container_port, p.protocol))
        .collect();
    assert_eq!(shown, ["53/tcp", "53/udp", "443/tcp"]);
}

mod digests {
    use docker::map::repo_digest;

    #[test]
    fn picks_the_entry_whose_repository_matches() {
        // An image pulled under several names carries several entries, and
        // taking the first would compare one repository against another.
        let digests = [
            "ghcr.io/team/app@sha256:aaa".to_owned(),
            "docker.io/library/nginx@sha256:bbb".to_owned(),
        ];
        assert_eq!(
            repo_digest(Some(&digests), "docker.io/library/nginx:alpine").as_deref(),
            Some("sha256:bbb")
        );
        assert_eq!(
            repo_digest(Some(&digests), "ghcr.io/team/app:1.0").as_deref(),
            Some("sha256:aaa")
        );
    }

    #[test]
    fn a_single_entry_is_unambiguous_however_the_name_is_written() {
        // `nginx:alpine` against `docker.io/library/nginx@...` is the
        // ordinary case and must not be treated as a mismatch.
        let digests = ["docker.io/library/nginx@sha256:bbb".to_owned()];
        assert_eq!(
            repo_digest(Some(&digests), "nginx:alpine").as_deref(),
            Some("sha256:bbb")
        );
    }

    #[test]
    fn an_image_with_no_registry_digest_has_none() {
        // Built locally or loaded from a file: nothing to drift against.
        assert_eq!(repo_digest(None, "myapp:dev"), None);
        assert_eq!(repo_digest(Some(&[]), "myapp:dev"), None);
    }

    #[test]
    fn several_entries_that_all_mismatch_are_not_guessed_at() {
        let digests = [
            "ghcr.io/a/x@sha256:aaa".to_owned(),
            "ghcr.io/b/y@sha256:bbb".to_owned(),
        ];
        assert_eq!(repo_digest(Some(&digests), "docker.io/library/nginx"), None);
    }
}
