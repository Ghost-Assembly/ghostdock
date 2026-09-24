//! Parsing a registry's `WWW-Authenticate` challenge.
//!
//! Registries do not hand out anonymous tokens at a fixed address; they tell
//! you where to ask, in a header, and the address differs per registry and
//! per repository. Parsing it is pure, so it is tested on its own rather
//! than only through the network.

/// Where to ask for a token, and what to ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub realm: String,
    pub service: Option<String>,
    pub scope: Option<String>,
}

impl Challenge {
    /// The URL to request a token from.
    #[must_use]
    pub fn token_url(&self) -> String {
        let mut url = self.realm.clone();
        let mut separator = if url.contains('?') { '&' } else { '?' };

        if let Some(service) = &self.service {
            url.push(separator);
            url.push_str(&format!("service={}", urlencode(service)));
            separator = '&';
        }
        if let Some(scope) = &self.scope {
            url.push(separator);
            url.push_str(&format!("scope={}", urlencode(scope)));
        }
        url
    }
}

/// Parses a `Bearer` challenge. Returns `None` for any other scheme.
#[must_use]
pub fn parse(header: &str) -> Option<Challenge> {
    let rest = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))?;

    let mut realm = None;
    let mut service = None;
    let mut scope = None;

    for part in split_params(rest) {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_owned();
        match key.trim() {
            "realm" => realm = Some(value),
            "service" => service = Some(value),
            "scope" => scope = Some(value),
            _ => {}
        }
    }

    Some(Challenge {
        realm: realm?,
        service,
        scope,
    })
}

/// Splits on commas that are not inside quotes.
///
/// A scope routinely contains commas -- `repository:x:pull,push` -- so a
/// naive split loses half of it and the token comes back without the access
/// that was asked for.
fn split_params(input: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in input.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            ',' if !in_quotes => {
                parts.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
}

/// Percent-encodes the characters that appear in registry scopes.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
