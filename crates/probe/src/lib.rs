//! Reaching services from GhostDock's own network: HTTP(S) requests and TCP
//! connections, timed, and plain POSTs for alert deliveries.
//!
//! It knows nothing about checks, alerts, the database or Docker. It
//! reports what happened; deciding whether that is good is `domain`'s job.
//!
//! No error it returns repeats the URL it was given. A webhook URL often
//! carries its secret in the path, and these messages are shown and stored.

use std::time::{Duration, Instant};

/// How much of a body is searched for a keyword.
pub const KEYWORD_WINDOW: usize = 1 << 20;

/// What an HTTP probe found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpAnswer {
    pub status: u16,
    /// Until the response's headers arrived.
    pub latency: Duration,
    /// Whether the keyword is in the body's first MiB; `None` when none
    /// was asked for.
    pub keyword_found: Option<bool>,
    /// Whole days until the server's certificate expires, for HTTPS;
    /// negative once it has.
    pub tls_days_left: Option<i64>,
}

/// An HTTP client for probes and deliveries. Cheap to clone: connections
/// are pooled and shared.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
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
                .user_agent(concat!("GhostDock/", env!("CARGO_PKG_VERSION")))
                // The peer certificate, for its expiry.
                .tls_info(true)
                .build()
                .unwrap_or_default(),
        }
    }

    /// Requests `url` and reports the answer, looking for `keyword` in the
    /// first [`KEYWORD_WINDOW`] bytes of the body. Any answer is a success
    /// here, whatever its status; failing to get one is the error.
    pub async fn http(
        &self,
        url: &str,
        timeout: Duration,
        keyword: Option<&str>,
    ) -> Result<HttpAnswer, String> {
        let started = Instant::now();
        let work = async {
            let mut response = self.http.get(url).send().await.map_err(|e| describe(&e))?;
            let latency = started.elapsed();
            let status = response.status().as_u16();
            let tls_days_left = response
                .extensions()
                .get::<reqwest::tls::TlsInfo>()
                .and_then(reqwest::tls::TlsInfo::peer_certificate)
                .and_then(not_after)
                .map(|expires| (expires - now()).div_euclid(86_400));
            let keyword_found = match keyword {
                Some(word) => {
                    let mut body = Vec::new();
                    while body.len() < KEYWORD_WINDOW {
                        match response.chunk().await.map_err(|e| describe(&e))? {
                            Some(chunk) => body.extend_from_slice(&chunk),
                            None => break,
                        }
                    }
                    body.truncate(KEYWORD_WINDOW);
                    Some(contains(&body, word.as_bytes()))
                }
                None => None,
            };
            Ok(HttpAnswer {
                status,
                latency,
                keyword_found,
                tls_days_left,
            })
        };
        // One deadline for the whole exchange, body included.
        tokio::time::timeout(timeout, work)
            .await
            .unwrap_or_else(|_| Err(timed_out(timeout)))
    }

    /// POSTs `body` to `url` with `headers`, and returns the status. A
    /// status outside 2xx is an error, as is getting no answer.
    pub async fn post(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
        timeout: Duration,
    ) -> Result<u16, String> {
        let mut request = self.http.post(url).body(body);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = tokio::time::timeout(timeout, request.send())
            .await
            .map_err(|_| timed_out(timeout))?
            .map_err(|e| describe(&e))?;
        let status = response.status();
        if status.is_success() {
            Ok(status.as_u16())
        } else {
            Err(format!("answered {}", status.as_u16()))
        }
    }
}

/// Connects to `target` (`host:port`) and reports how long it took.
pub async fn tcp(target: &str, timeout: Duration) -> Result<Duration, String> {
    let started = Instant::now();
    match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(target)).await {
        Ok(Ok(_stream)) => Ok(started.elapsed()),
        Ok(Err(e)) => Err(format!("could not connect: {e}")),
        Err(_) => Err(timed_out(timeout)),
    }
}

fn timed_out(timeout: Duration) -> String {
    format!("timed out after {} s", timeout.as_secs_f64())
}

/// A request error in words, without its URL: the deepest cause says what
/// actually went wrong ("Connection refused"), where reqwest's own message
/// says only that sending failed.
fn describe(e: &reqwest::Error) -> String {
    let what = if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "could not connect"
    } else if e.is_redirect() {
        "too many redirects"
    } else {
        "request failed"
    };
    // Never reqwest's own message, which names the URL.
    let mut deepest = None;
    let mut cause = std::error::Error::source(e);
    while let Some(next) = cause {
        deepest = Some(next.to_string());
        cause = next.source();
    }
    match deepest {
        Some(cause) if !cause.contains("://") => format!("{what}: {cause}"),
        _ => what.to_owned(),
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

// ---- certificates -------------------------------------------------------

/// When a DER-encoded X.509 certificate expires, as Unix seconds: its
/// `notAfter`. `None` for anything that does not read as one.
///
/// A few dozen lines of DER walking rather than an X.509 crate: this is
/// the one field wanted, and its place in the structure has not moved
/// since 1988.
#[must_use]
pub fn not_after(der: &[u8]) -> Option<i64> {
    // Certificate ::= SEQUENCE { tbsCertificate, signatureAlgorithm, signature }
    let (certificate, _) = element(der, 0x30)?;
    let (tbs, _) = element(certificate, 0x30)?;
    // [0] EXPLICIT version, absent from a v1 certificate.
    let rest = match element(tbs, 0xa0) {
        Some((_, rest)) => rest,
        None => tbs,
    };
    let (_serial, rest) = element(rest, 0x02)?;
    let (_signature, rest) = element(rest, 0x30)?;
    let (_issuer, rest) = element(rest, 0x30)?;
    let (validity, _) = element(rest, 0x30)?;
    let (_, after) = time(validity)?;
    time(after).map(|(t, _)| t)
}

/// The content of the element at the start of `input` if it has `tag`,
/// and what follows it.
fn element(input: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    let (&found, rest) = input.split_first()?;
    if found != tag {
        return None;
    }
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 {
            return None;
        }
        let bytes = rest.get(..count)?;
        let len = bytes
            .iter()
            .fold(0_usize, |acc, b| (acc << 8) | usize::from(*b));
        (len, rest.get(count..)?)
    };
    Some((rest.get(..len)?, rest.get(len..)?))
}

/// A UTCTime or GeneralizedTime at the start of `input`, as Unix seconds,
/// and what follows it.
fn time(input: &[u8]) -> Option<(i64, &[u8])> {
    let (text, rest, short) = match element(input, 0x17) {
        Some((text, rest)) => (text, rest, true),
        None => {
            let (text, rest) = element(input, 0x18)?;
            (text, rest, false)
        }
    };
    let text = std::str::from_utf8(text).ok()?;
    let digits = text.strip_suffix('Z')?;
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let full = if short {
        // RFC 5280: YY of 50 or more is 19YY, below that 20YY.
        let yy: u32 = digits.get(..2)?.parse().ok()?;
        format!("{}{digits}", if yy >= 50 { "19" } else { "20" })
    } else {
        digits.to_owned()
    };
    let at = chrono::NaiveDateTime::parse_from_str(&full, "%Y%m%d%H%M%S").ok()?;
    Some((at.and_utc().timestamp(), rest))
}
