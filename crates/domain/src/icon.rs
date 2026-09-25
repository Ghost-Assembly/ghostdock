//! Which software a stack runs, so the UI can show its icon.
//!
//! The icons are Simple Icons' monochrome marks, vendored in
//! `crates/web/brand-icons/`, and this is the one list of them: a slug here
//! is a file there, which a test checks. Nothing is ever fetched, so a label
//! naming an icon GhostDock does not have is simply not used.

use std::collections::HashMap;

/// Chooses a stack's icon outright, by slug or by any name in [`ALIASES`].
pub const LABEL: &str = "ghostdock.icon";

/// Labels other tools use to point at an icon file. Used only when the
/// file's name is an icon GhostDock has; the file itself is never fetched.
const HINT_LABELS: &[&str] = &["net.unraid.docker.icon", "homepage.icon"];

/// Every bundled icon, sorted. Each is `crates/web/brand-icons/<slug>.svg`,
/// named as Simple Icons names it. Only icons Simple Icons offers under
/// CC0 are bundled; one under its brand's own licence is left out.
pub const BUNDLED: &[&str] = &[
    "actualbudget",
    "adguard",
    "adminer",
    "affine",
    "appsmith",
    "appwrite",
    "arangodb",
    "audiobookshelf",
    "authentik",
    "baserow",
    "bitwarden",
    "bookstack",
    "borgbackup",
    "budibase",
    "caddy",
    "calibreweb",
    "changedetection",
    "checkmk",
    "clickhouse",
    "cloudflare",
    "cockroachlabs",
    "coder",
    "collaboraonline",
    "consul",
    "cryptpad",
    "diagramsdotnet",
    "directus",
    "discourse",
    "docker",
    "drone",
    "drupal",
    "duplicati",
    "eclipsemosquitto",
    "elasticsearch",
    "element",
    "emby",
    "envoyproxy",
    "esphome",
    "etcd",
    "excalidraw",
    "fireflyiii",
    "fluentbit",
    "freshrss",
    "frigate",
    "ghost",
    "gitea",
    "gitlab",
    "grafana",
    "graylog",
    "grocy",
    "homarr",
    "homeassistant",
    "homebridge",
    "homepage",
    "hoppscotch",
    "icinga",
    "immich",
    "influxdb",
    "invidious",
    "invoiceninja",
    "jaeger",
    "jellyfin",
    "jitsi",
    "joomla",
    "joplin",
    "jupyter",
    "kibana",
    "kodi",
    "kong",
    "libreoffice",
    "listmonk",
    "logstash",
    "mariadb",
    "mastodon",
    "matomo",
    "matrix",
    "mattermost",
    "mealie",
    "meilisearch",
    "metabase",
    "minio",
    "mlflow",
    "mongodb",
    "monica",
    "mysql",
    "n8n",
    "natsdotio",
    "neo4j",
    "netdata",
    "nextcloud",
    "nginx",
    "nginxproxymanager",
    "nodered",
    "ntfy",
    "obsidian",
    "odoo",
    "ollama",
    "onlyoffice",
    "openhab",
    "openproject",
    "opensearch",
    "openvpn",
    "outline",
    "owncloud",
    "paperlessngx",
    "passbolt",
    "phpmyadmin",
    "pihole",
    "piped",
    "piwigo",
    "pixelfed",
    "plane",
    "plausibleanalytics",
    "plex",
    "pocketbase",
    "portainer",
    "postgresql",
    "posthog",
    "prometheus",
    "qbittorrent",
    "rabbitmq",
    "radarr",
    "rancher",
    "rclone",
    "redis",
    "rocketdotchat",
    "roundcube",
    "rustdesk",
    "scylladb",
    "seafile",
    "searxng",
    "sentry",
    "sonarr",
    "sonatype",
    "strapi",
    "supabase",
    "surrealdb",
    "syncthing",
    "tailscale",
    "teamspeak",
    "timescale",
    "traefikproxy",
    "transmission",
    "trilium",
    "uptimekuma",
    "vault",
    "vaultwarden",
    "verdaccio",
    "victoriametrics",
    "vikunja",
    "wallabag",
    "watchtower",
    "wikidotjs",
    "wireguard",
    "wordpress",
    "zerotier",
    "zigbee2mqtt",
    "zulip",
];

/// Names software goes by in images and labels, where they are not simply
/// its slug with the punctuation taken out: an image's last segment, its
/// whole repository, or its publisher.
pub const ALIASES: &[(&str, &str)] = &[
    ("adguardhome", "adguard"),
    ("cloudflared", "cloudflare"),
    ("cockroach", "cockroachlabs"),
    ("cockroachdb", "cockroachlabs"),
    ("code-server", "coder"),
    ("codercom", "coder"),
    ("collabora/code", "collaboraonline"),
    ("drawio", "diagramsdotnet"),
    ("element-web", "element"),
    ("goauthentik/server", "authentik"),
    ("jaegertracing", "jaeger"),
    ("kodi-headless", "kodi"),
    ("matrixdotorg", "matrix"),
    ("mongo", "mongodb"),
    ("mosquitto", "eclipsemosquitto"),
    ("nats", "natsdotio"),
    ("nginx-unprivileged", "nginx"),
    ("nginxinc", "nginx"),
    ("plausible", "plausibleanalytics"),
    ("plexinc", "plex"),
    ("pms-docker", "plex"),
    ("postgres", "postgresql"),
    ("qbittorrent-nox", "qbittorrent"),
    ("requarks/wiki", "wikidotjs"),
    ("rocketchat", "rocketdotchat"),
    ("seafile-mc", "seafile"),
    ("timescaledb", "timescale"),
    ("traefik", "traefikproxy"),
    ("triliumnext", "trilium"),
    ("wikijs", "wikidotjs"),
];

/// What other software is built on or sits in front of: a database, a
/// cache, a broker, a proxy, a tool for one of those. A stack running one
/// beside something else is that something else, so these give way.
pub const SUPPORTING: &[&str] = &[
    "adminer",
    "arangodb",
    "caddy",
    "clickhouse",
    "cloudflare",
    "cockroachlabs",
    "docker",
    "eclipsemosquitto",
    "elasticsearch",
    "envoyproxy",
    "etcd",
    "influxdb",
    "kong",
    "mariadb",
    "meilisearch",
    "minio",
    "mongodb",
    "mysql",
    "natsdotio",
    "neo4j",
    "nginx",
    "opensearch",
    "phpmyadmin",
    "postgresql",
    "rabbitmq",
    "redis",
    "scylladb",
    "surrealdb",
    "tailscale",
    "timescale",
    "traefikproxy",
    "watchtower",
];

/// Publishers that repackage other people's software: their name says
/// nothing about what an image runs.
const REPUBLISHERS: &[&str] = &["library", "linuxserver", "bitnami", "hotio"];

/// Endings that name an edition or a part of the software, not other
/// software, and the ones icon file names carry.
const SUFFIXES: &[&str] = &["-ce", "-ee", "-oss", "-server", "-icon", "-logo"];

/// Prefixes dashboards put before an icon name to say which set it is from.
const PREFIXES: &[&str] = &["si-", "sh-"];

/// One service of a stack, as the daemon reports its container.
#[derive(Debug, Clone, Copy)]
pub struct Service<'a> {
    pub image: &'a str,
    pub labels: Option<&'a HashMap<String, String>>,
}

impl<'a> Service<'a> {
    fn label(&self, key: &str) -> Option<&'a str> {
        self.labels?.get(key).map(String::as_str)
    }
}

/// The bundled icon for a stack, from its services in order and then its
/// name; `None` for none, which the UI shows as the name's initials.
///
/// A `ghostdock.icon` label on any service decides it. Then another tool's
/// icon label, if its file is one GhostDock has. Then the images: the first
/// that is known wins, except that a database or proxy gives way to the
/// software it serves.
#[must_use]
pub fn for_stack<'a>(
    services: impl IntoIterator<Item = Service<'a>>,
    project: &str,
) -> Option<&'static str> {
    let services: Vec<Service<'a>> = services.into_iter().collect();

    let chosen = services
        .iter()
        .find_map(|s| s.label(LABEL).and_then(resolve));
    let hinted = || {
        services.iter().find_map(|s| {
            HINT_LABELS
                .iter()
                .find_map(|key| s.label(key).and_then(from_icon_file))
        })
    };
    let imaged = || {
        let known: Vec<&'static str> = services
            .iter()
            .filter_map(|s| from_image(s.image))
            .collect();
        known
            .iter()
            .find(|slug| !SUPPORTING.contains(slug))
            .or_else(|| known.first())
            .copied()
    };

    chosen
        .or_else(hinted)
        .or_else(imaged)
        .or_else(|| from_name(project))
}

/// The bundled icon an image runs, if it names one.
///
/// The registry, `library/`, a republisher such as `linuxserver/`, the tag
/// and the digest say nothing about the software, so they are dropped. Then
/// the whole repository is looked up, then its last segment, then its
/// publisher.
#[must_use]
pub fn from_image(image: &str) -> Option<&'static str> {
    let image = image.trim().to_ascii_lowercase();
    // An image id: the daemon reports one when the name has moved on.
    if image.starts_with("sha256:") {
        return None;
    }
    let named = image.split('@').next().unwrap_or_default();
    let repository = match named.rsplit_once(':') {
        Some((name, tag)) if !tag.contains('/') => name,
        _ => named,
    };

    let mut segments: Vec<&str> = repository.split('/').filter(|s| !s.is_empty()).collect();
    if segments.len() > 1
        && segments
            .first()
            .is_some_and(|first| first.contains(['.', ':']) || *first == "localhost")
    {
        segments.remove(0);
    }
    if segments.len() > 1
        && segments
            .first()
            .is_some_and(|first| REPUBLISHERS.contains(first))
    {
        segments.remove(0);
    }

    let whole = segments.join("/");
    alias(&whole)
        .or_else(|| segments.last().and_then(|last| resolve(last)))
        .or_else(|| {
            (segments.len() > 1)
                .then(|| segments.first().and_then(|publisher| resolve(publisher)))
                .flatten()
        })
}

/// The bundled icon a stack's name names, if any: a stack called
/// `jellyfin` is Jellyfin even before anything of it runs.
#[must_use]
pub fn from_name(name: &str) -> Option<&'static str> {
    resolve(&name.trim().to_ascii_lowercase())
}

/// An icon file's name, as another tool's label gives it (a path, a URL, or
/// a name from a dashboard's icon set), matched against the bundle.
fn from_icon_file(value: &str) -> Option<&'static str> {
    let value = value.trim().to_ascii_lowercase();
    let path = value.split(['?', '#']).next().unwrap_or_default();
    let file = path.rsplit('/').next().unwrap_or_default();
    let stem = match file.rsplit_once('.') {
        Some((stem, "png" | "svg" | "webp" | "jpg" | "jpeg" | "ico" | "gif")) => stem,
        _ => file,
    };
    let stem = PREFIXES
        .iter()
        .find_map(|p| stem.strip_prefix(p))
        .unwrap_or(stem);
    resolve(stem)
}

/// One lowercase name to a bundled slug: as an alias, as a slug once its
/// punctuation is gone, and then again without an edition suffix.
fn resolve(name: &str) -> Option<&'static str> {
    let name = name.trim();
    let direct = |name: &str| {
        let compact: String = name
            .chars()
            .filter(|c| !matches!(c, '-' | '_' | '.'))
            .collect();
        alias(name)
            .or_else(|| bundled(&compact))
            .or_else(|| alias(&compact))
    };
    direct(name).or_else(|| {
        SUFFIXES
            .iter()
            .find_map(|suffix| name.strip_suffix(suffix))
            .filter(|rest| !rest.is_empty())
            .and_then(direct)
    })
}

fn alias(name: &str) -> Option<&'static str> {
    ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map(|(_, slug)| *slug)
}

/// `slug` itself, when it is a bundled icon.
#[must_use]
pub fn bundled(slug: &str) -> Option<&'static str> {
    BUNDLED.iter().find(|s| **s == slug).copied()
}
