//! What a screen knows about something it reads from the server.
//!
//! One value rather than an `Option` beside an error: with two, a failed
//! read showed its error above a "Loading" that never ended, and an empty
//! list said "nothing here yet" before the answer had arrived.

use crate::api;

#[derive(Clone, Debug, PartialEq)]
pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> From<api::Result<T>> for Load<T> {
    fn from(result: api::Result<T>) -> Self {
        match result {
            Ok(value) => Self::Ready(value),
            Err(e) => Self::Failed(e.message),
        }
    }
}

impl<T> Load<T> {
    /// The value, once there is one.
    #[must_use]
    pub fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_becomes_a_load() {
        let ok: api::Result<u8> = Ok(3);
        assert_eq!(Load::from(ok), Load::Ready(3));
        let failed: api::Result<u8> = Err(api::Error {
            message: "no".to_owned(),
            status: Some(500),
        });
        assert_eq!(Load::from(failed), Load::Failed("no".to_owned()));
        assert_eq!(Load::<u8>::Loading.ready(), None);
    }
}
