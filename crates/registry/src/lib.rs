//! Container registry manifest polling for image-digest drift.
//!
//! Must stay behind a shared rate limiter: Docker Hub imposes strict
//! anonymous pull limits and this runs on a timer across every service.

pub mod challenge;
pub mod reference;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use reference::Reference;
use tokio::time::Instant;

/// Header a registry returns the canonical digest in.
const DIGEST_HEADER: &str = "docker-content-digest";

/// Manifest types to accept.
///
/// The index types matter: without them a registry serving a multi-platform
/// image returns the digest of one platform's manifest, which differs from
/// the digest a client pulling the same tag would record, and every poll
/// would then report an update that does not exist.
const ACCEPT: &str = "application/vnd.oci.image.index.v1+json, \
     application/vnd.docker.distribution.manifest.list.v2+json, \
     application/vnd.oci.image.manifest.v1+json, \
     application/vnd.docker.distribution.manifest.v2+json";

/// Minimum spacing between requests to one registry.
///
/// Anonymous Docker Hub allows a limited number of manifest requests per IP
/// per six hours, shared with actual pulls. Spacing requests keeps a poll of
/// many stacks from spending that budget in one burst and breaking deploys.
pub const DEFAULT_MIN_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Reference(#[from] reference::ParseError),
    #[error("could not reach {registry}: {source}")]
    Unreachable {
        registry: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("{registry} wants credentials for {repository}")]
    Unauthorized {
        registry: String,
        repository: String,
    },
    #[error("{repository}:{target} is not on {registry}")]
    NotFound {
        registry: String,
        repository: String,
        target: String,
    },
    #[error("{0} is rate limiting GhostDock; the next check will try again")]
    RateLimited(String),
    #[error("{registry} answered {status} for {repository}")]
    Unexpected {
        registry: String,
        repository: String,
        status: u16,
    },
    #[error("{0} did not return a digest for an image it says exists")]
    NoDigest(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Asks registries what a tag currently points at.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    min_interval: Duration,
    /// When each registry may next be asked, so one host cannot be flooded.
    next_request: Arc<Mutex<HashMap<String, Instant>>>,
    /// Overrides the scheme and host, for testing against a local server.
    base_override: Option<String>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    #[must_use]
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent("ghostdock")
                .build()
                .unwrap_or_default(),
            min_interval: DEFAULT_MIN_INTERVAL,
            next_request: Arc::default(),
            base_override: None,
        }
    }

    /// Points every request at `base` instead of the reference's registry.
    ///
    /// For tests against a local server; the reference is still parsed and
    /// used for the path, so the real code path is exercised.
    #[must_use]
    pub fn with_base_override(mut self, base: impl Into<String>) -> Self {
        self.base_override = Some(base.into());
        self
    }

    #[must_use]
    pub fn with_min_interval(mut self, interval: Duration) -> Self {
        self.min_interval = interval;
        self
    }

    /// The digest a tag currently resolves to.
    pub async fn digest(&self, reference: &Reference) -> Result<String> {
        let base = self
            .base_override
            .clone()
            .unwrap_or_else(|| format!("https://{}", reference.registry));
        let url = format!(
            "{base}/v2/{}/manifests/{}",
            reference.repository,
            reference.manifest_target()
        );

        let response = self.send(&reference.registry, &url, None).await?;

        // A registry says who to ask for a token rather than assuming one
        // address, so the challenge is followed rather than guessed at.
        let response = if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            match response
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok())
                .and_then(challenge::parse)
            {
                Some(challenge) => {
                    let token = self.token(&reference.registry, &challenge).await?;
                    self.send(&reference.registry, &url, Some(&token)).await?
                }
                None => {
                    return Err(Error::Unauthorized {
                        registry: reference.registry.clone(),
                        repository: reference.repository.clone(),
                    });
                }
            }
        } else {
            response
        };

        match response.status().as_u16() {
            200..=299 => response
                .headers()
                .get(DIGEST_HEADER)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
                .ok_or_else(|| Error::NoDigest(reference.registry.clone())),
            401 | 403 => Err(Error::Unauthorized {
                registry: reference.registry.clone(),
                repository: reference.repository.clone(),
            }),
            404 => Err(Error::NotFound {
                registry: reference.registry.clone(),
                repository: reference.repository.clone(),
                target: reference.manifest_target().to_owned(),
            }),
            429 => Err(Error::RateLimited(reference.registry.clone())),
            status => Err(Error::Unexpected {
                registry: reference.registry.clone(),
                repository: reference.repository.clone(),
                status,
            }),
        }
    }

    async fn token(&self, registry: &str, challenge: &challenge::Challenge) -> Result<String> {
        #[derive(serde::Deserialize)]
        struct TokenResponse {
            /// Registries disagree about the field name, and a client that
            /// reads only one of them fails against half of them.
            token: Option<String>,
            access_token: Option<String>,
        }

        self.pace(registry).await;
        let response = self
            .http
            .get(challenge.token_url())
            .send()
            .await
            .map_err(|source| Error::Unreachable {
                registry: registry.to_owned(),
                source,
            })?;

        let body: TokenResponse = response.json().await.map_err(|source| Error::Unreachable {
            registry: registry.to_owned(),
            source,
        })?;

        body.token
            .or(body.access_token)
            .ok_or_else(|| Error::Unauthorized {
                registry: registry.to_owned(),
                repository: String::new(),
            })
    }

    async fn send(
        &self,
        registry: &str,
        url: &str,
        token: Option<&str>,
    ) -> Result<reqwest::Response> {
        self.pace(registry).await;

        // HEAD, not GET: only the digest header is wanted, and manifests are
        // large enough that fetching them for every service on a timer is
        // needless traffic on both ends.
        let mut request = self.http.head(url).header(reqwest::header::ACCEPT, ACCEPT);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }

        request.send().await.map_err(|source| Error::Unreachable {
            registry: registry.to_owned(),
            source,
        })
    }

    /// Waits until this registry may be asked again.
    ///
    /// Takes the next free slot for the registry and sleeps without the
    /// lock, so a wait owed to one registry never holds up another, and
    /// requests to one registry made together still go one interval apart.
    async fn pace(&self, registry: &str) {
        let slot = {
            let mut next = self
                .next_request
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let now = Instant::now();
            let slot = next
                .get(registry)
                .copied()
                .filter(|at| *at > now)
                .unwrap_or(now);
            next.insert(registry.to_owned(), slot + self.min_interval);
            slot
        };
        tokio::time::sleep_until(slot).await;
    }
}
