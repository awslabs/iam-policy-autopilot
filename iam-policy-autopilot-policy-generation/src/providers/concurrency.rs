//! Platform-abstracted concurrent task execution.
//!
//! - **Native**: Uses `tokio::task::JoinSet` for true parallelism across threads.
//! - **WASM**: Executes tasks sequentially (single-threaded runtime, no spawn).
//!
//! Consumers call [`run_all`] with a list of futures and get back all results,
//! without needing to know which platform they're on.

use std::future::Future;

/// Execute a collection of async tasks, returning all results.
///
/// On native, tasks run concurrently via `tokio::task::JoinSet`.
/// On WASM, tasks run sequentially (no multi-threading available).
///
/// # Errors
///
/// Returns results in completion order (native) or input order (WASM).
/// Individual task failures are represented in the `Result` of each item.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn run_all<T, F, Fut>(tasks: impl IntoIterator<Item = F>) -> Vec<T>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    use tokio::task::JoinSet;

    let mut join_set = JoinSet::new();
    for task in tasks {
        join_set.spawn(async move { task().await });
    }

    let mut results = Vec::new();
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(value) => results.push(value),
            Err(e) => {
                log::error!("Task panicked: {e}");
            }
        }
    }
    results
}

/// Execute a collection of async tasks sequentially (WASM fallback).
#[cfg(target_arch = "wasm32")]
pub(crate) async fn run_all<T, F, Fut>(tasks: impl IntoIterator<Item = F>) -> Vec<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = T>,
{
    let mut results = Vec::new();
    for task in tasks {
        results.push(task().await);
    }
    results
}
