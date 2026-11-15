//! Non-panicking async-to-sync bridge for driving futures from the TUI loop.
//!
//! Callers: synchronous TUI controllers and views (`views::policy_approval`,
//! `views::settings`) that need to run a short future on the shared Tokio
//! runtime handle regardless of whether the caller is outside Tokio, on a
//! multi-threaded worker, or inside a current-thread test runtime.
//!
//! Must NOT contain RPC client logic, signer/wallet domain types, or UI code.

use std::fmt;
use std::future::Future;

use tokio::runtime::{Handle, RuntimeFlavor};

/// Error returned when a scoped worker thread panics while driving a future.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockingError {
    /// Helper thread terminated with a panic before returning a result.
    WorkerPanicked,
}

impl fmt::Display for BlockingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkerPanicked => write!(f, "blocking_worker_panicked"),
        }
    }
}

impl std::error::Error for BlockingError {}

/// Drive `fut` to completion on `handle` from synchronous code without panicking.
///
/// Uses `tokio::task::block_in_place` on a multi-threaded runtime, a scoped OS
/// thread on a current-thread runtime, and direct `handle.block_on` outside a
/// Tokio runtime context.
pub fn block_on_handle<F, T>(handle: &Handle, fut: F) -> Result<T, BlockingError>
where
    F: Future<Output = T> + Send,
    T: Send,
{
    match Handle::try_current() {
        Ok(cur) if cur.runtime_flavor() == RuntimeFlavor::MultiThread => {
            Ok(tokio::task::block_in_place(|| handle.block_on(fut)))
        }
        Ok(_) => std::thread::scope(|s| {
            s.spawn(|| handle.block_on(fut))
                .join()
                .map_err(|_| BlockingError::WorkerPanicked)
        }),
        Err(_) => Ok(handle.block_on(fut)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_on_handle_returns_value_outside_and_inside_current_thread_runtime() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let handle = rt.handle().clone();

        assert_eq!(block_on_handle(&handle, async { 7usize }), Ok(7));
        rt.block_on(async {
            assert_eq!(block_on_handle(&handle, async { 13usize }), Ok(13));
        });
    }

    #[test]
    fn block_on_handle_maps_worker_panic_to_error_without_panicking() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let handle = rt.handle().clone();

        let err = rt.block_on(async {
            block_on_handle(&handle, async {
                panic!("simulated worker panic");
            })
        });
        assert_eq!(err, Err(BlockingError::WorkerPanicked));
    }
}
