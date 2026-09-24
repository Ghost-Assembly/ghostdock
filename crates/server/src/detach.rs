//! Work that must finish once asked for, whoever stops listening.
//!
//! A handler's future is dropped when its client goes away: a closed tab, a
//! phone losing signal. Work awaited directly in the handler stops at that
//! point, half done. Run through [`finish`], the work is its own task: the
//! handler waits for it, but dropping the handler drops only the wait.

use std::future::Future;

use crate::error::ApiError;

pub async fn finish<T: Send + 'static>(
    work: impl Future<Output = Result<T, ApiError>> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::spawn(work)
        .await
        .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?
}
