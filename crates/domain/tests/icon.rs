//! Which icon a stack gets.

use std::collections::HashMap;
use std::path::Path;

use domain::icon::{BUNDLED, Service, for_stack, from_image, from_name};

fn labels(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn service<'a>(image: &'a str, labels: Option<&'a HashMap<String, String>>) -> Service<'a> {
    Service { image, labels }
}

// ---- images -------------------------------------------------------------

#[test]
fn an_official_image_is_known_by_its_name() {
    assert_eq!(from_image("nginx"), Some("nginx"));
    assert_eq!(from_image("redis:7-alpine"), Some("redis"));
    assert_eq!(from_image("mariadb:11"), Some("mariadb"));
    assert_eq!(from_image("nextcloud:apache"), Some("nextcloud"));
}

#[test]
fn a_name_that_differs_from_the_icon_goes_through_an_alias() {
    assert_eq!(from_image("postgres:16"), Some("postgresql"));
    assert_eq!(from_image("mongo:7"), Some("mongodb"));
    assert_eq!(from_image("traefik:v3.1"), Some("traefikproxy"));
    assert_eq!(
        from_image("homeassistant/home-assistant:stable"),
        Some("homeassistant")
    );
    assert_eq!(from_image("requarks/wiki:2"), Some("wikidotjs"));
}

#[test]
fn the_registry_is_ignored_even_with_a_port() {
    assert_eq!(from_image("registry.local:5000/x/nginx:1.2"), Some("nginx"));
    assert_eq!(from_image("localhost:5000/nginx"), Some("nginx"));
    assert_eq!(from_image("docker.io/library/redis:7"), Some("redis"));
    assert_eq!(
        from_image("ghcr.io/home-assistant/home-assistant:2026.9"),
        Some("homeassistant")
    );
}

#[test]
fn a_digest_is_ignored() {
    let digest = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    assert_eq!(from_image(&format!("nginx@{digest}")), Some("nginx"));
    assert_eq!(
        from_image(&format!("grafana/grafana:11@{digest}")),
        Some("grafana")
    );
    // An image id rather than a name, as the daemon reports a container
    // whose image has since been retagged, names nothing.
    assert_eq!(from_image(digest), None);
}

#[test]
fn library_and_republishers_are_stripped() {
    assert_eq!(from_image("library/postgres"), Some("postgresql"));
    assert_eq!(from_image("linuxserver/jellyfin:latest"), Some("jellyfin"));
    assert_eq!(from_image("lscr.io/linuxserver/sonarr:4"), Some("sonarr"));
    assert_eq!(from_image("bitnami/redis:7.2"), Some("redis"));
}

#[test]
fn case_and_separators_do_not_matter() {
    assert_eq!(from_image("Jellyfin/Jellyfin"), Some("jellyfin"));
    assert_eq!(from_image("louislam/uptime-kuma:1"), Some("uptimekuma"));
    assert_eq!(from_image("nodered/node-red"), Some("nodered"));
    assert_eq!(
        from_image("ghcr.io/paperless-ngx/paperless-ngx"),
        Some("paperlessngx")
    );
}

#[test]
fn an_edition_suffix_is_dropped() {
    assert_eq!(from_image("portainer/portainer-ce:2.21"), Some("portainer"));
    assert_eq!(from_image("gitlab/gitlab-ce"), Some("gitlab"));
    assert_eq!(from_image("grafana/grafana-oss"), Some("grafana"));
    assert_eq!(
        from_image("ghcr.io/immich-app/immich-server:release"),
        Some("immich")
    );
}

#[test]
fn a_generic_image_name_falls_back_to_its_publisher() {
    assert_eq!(from_image("vaultwarden/server:latest"), Some("vaultwarden"));
    assert_eq!(from_image("onlyoffice/documentserver"), Some("onlyoffice"));
}

#[test]
fn an_unknown_image_has_no_icon() {
    for image in [
        "alpine:3.22",
        "busybox",
        "example/thing:1",
        "",
        ":",
        "/",
        "@",
    ] {
        assert_eq!(from_image(image), None, "{image}");
    }
}

// ---- a stack --------------------------------------------------------------

#[test]
fn the_ghostdock_label_wins_over_everything() {
    let chosen = labels(&[("ghostdock.icon", "redis")]);
    let hinted = labels(&[(
        "net.unraid.docker.icon",
        "https://example.invalid/grafana.png",
    )]);
    let services = [
        service("nginx", Some(&hinted)),
        service("postgres", None),
        service("alpine", Some(&chosen)),
    ];
    assert_eq!(for_stack(services, "anything"), Some("redis"));
}

#[test]
fn a_ghostdock_label_naming_no_bundled_icon_is_ignored() {
    let chosen = labels(&[("ghostdock.icon", "no-such-icon")]);
    let services = [service("nginx", Some(&chosen))];
    assert_eq!(for_stack(services, "anything"), Some("nginx"));
}

#[test]
fn an_icon_label_counts_only_when_its_file_is_bundled() {
    let hinted = labels(&[(
        "net.unraid.docker.icon",
        "https://example.invalid/templates/img/sonarr-icon.png?v=2",
    )]);
    assert_eq!(
        for_stack([service("alpine", Some(&hinted))], "x"),
        Some("sonarr")
    );

    let homepage = labels(&[("homepage.icon", "si-nextcloud")]);
    assert_eq!(
        for_stack([service("alpine", Some(&homepage))], "x"),
        Some("nextcloud")
    );

    let unknown = labels(&[("net.unraid.docker.icon", "https://example.invalid/mine.png")]);
    assert_eq!(
        for_stack([service("redis", Some(&unknown))], "x"),
        Some("redis")
    );
}

#[test]
fn an_icon_label_wins_over_images() {
    let hinted = labels(&[("net.unraid.docker.icon", "/icons/jellyfin.svg")]);
    let services = [service("postgres", None), service("alpine", Some(&hinted))];
    assert_eq!(for_stack(services, "x"), Some("jellyfin"));
}

#[test]
fn the_first_service_with_a_known_image_wins() {
    let services = [
        service("alpine", None),
        service("grafana/grafana", None),
        service("prom/prometheus", None),
    ];
    assert_eq!(for_stack(services, "x"), Some("grafana"));
}

#[test]
fn a_database_or_proxy_gives_way_to_the_software_it_serves() {
    let services = [
        service("mariadb:11", None),
        service("nextcloud:apache", None),
        service("redis", None),
    ];
    assert_eq!(for_stack(services, "x"), Some("nextcloud"));
    // Alone, it is what the stack runs.
    assert_eq!(for_stack([service("redis", None)], "x"), Some("redis"));
}

#[test]
fn with_nothing_known_the_name_is_tried_and_then_nothing() {
    assert_eq!(
        for_stack([service("alpine", None)], "jellyfin"),
        Some("jellyfin")
    );
    assert_eq!(for_stack([], "home-assistant"), Some("homeassistant"));
    assert_eq!(for_stack([service("alpine", None)], "my-blog"), None);
    assert_eq!(from_name("Plex"), Some("plex"));
    assert_eq!(from_name("notes"), None);
}

// ---- the bundle -----------------------------------------------------------

fn bundle_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../web/brand-icons"))
}

#[test]
fn the_list_is_sorted_and_unique() {
    assert!(
        BUNDLED.windows(2).all(|w| w[0] < w[1]),
        "keep BUNDLED sorted, each slug once"
    );
}

#[test]
fn every_listed_icon_has_a_vendored_file() {
    for slug in BUNDLED {
        let file = bundle_dir().join(format!("{slug}.svg"));
        assert!(file.is_file(), "{} is missing", file.display());
    }
}

#[test]
fn every_vendored_file_is_listed() {
    let mut files: Vec<String> = std::fs::read_dir(bundle_dir())
        .expect("the bundle directory")
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            name.strip_suffix(".svg").map(str::to_owned)
        })
        .collect();
    files.sort();
    let listed: Vec<String> = BUNDLED.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(files, listed, "a file nothing can choose is dead weight");
}

#[test]
fn every_alias_and_supporting_slug_is_bundled() {
    for (name, slug) in domain::icon::ALIASES {
        assert!(
            BUNDLED.contains(slug),
            "alias {name} names {slug}, which is not bundled"
        );
    }
    for slug in domain::icon::SUPPORTING {
        assert!(BUNDLED.contains(slug), "{slug} is not bundled");
    }
}

#[test]
fn every_slug_is_safe_in_a_path() {
    for slug in BUNDLED {
        assert!(
            !slug.is_empty()
                && slug
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
            "{slug}"
        );
    }
}
