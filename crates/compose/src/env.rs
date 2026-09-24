//! Rendering the `.env` file Compose reads.
//!
//! Pure, and tested, because it is an injection boundary: a value containing
//! a newline would define a second variable, letting whoever can set one
//! variable set any of them.

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EnvError {
    #[error("variable name must not be empty")]
    EmptyKey,
    #[error("variable name {0:?} may contain only letters, digits and underscores")]
    BadKey(String),
    #[error("value for {0} must not contain a line break")]
    NewlineInValue(String),
}

/// Renders key/value pairs into `.env` form.
pub fn render(vars: &[(String, String)]) -> Result<String, EnvError> {
    let mut out = String::new();
    for (key, value) in vars {
        validate_key(key)?;
        if value.contains('\n') || value.contains('\r') {
            return Err(EnvError::NewlineInValue(key.clone()));
        }
        // Single quotes, because Compose does not expand anything inside them.
        // An unquoted value containing `$` would otherwise be substituted, and
        // a value containing a space would be truncated.
        out.push_str(key);
        out.push_str("='");
        out.push_str(&value.replace('\'', r"'\''"));
        out.push_str("'\n");
    }
    Ok(out)
}

fn validate_key(key: &str) -> Result<(), EnvError> {
    if key.is_empty() {
        return Err(EnvError::EmptyKey);
    }
    let valid = key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !key.as_bytes()[0].is_ascii_digit();
    if valid {
        Ok(())
    } else {
        Err(EnvError::BadKey(key.to_owned()))
    }
}
