//! The two things the client needs from an executor: spawning its background
//! task and timers. Tokio natively; the browser event loop on WebAssembly.
/// A timer ran out before the future completed.
#[derive(Debug)]
pub(crate) struct Elapsed;

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::Elapsed;
    use std::{future::Future, time::Duration};

    /// Panics outside a Tokio runtime, like `tokio::spawn`.
    pub(crate) fn spawn(future: impl Future<Output = ()> + Send + 'static) {
        drop(tokio::spawn(future));
    }

    pub(crate) async fn sleep(duration: Duration) {
        tokio::time::sleep(duration).await;
    }

    pub(crate) async fn timeout<T>(
        duration: Duration,
        future: impl Future<Output = T>,
    ) -> Result<T, Elapsed> {
        tokio::time::timeout(duration, future)
            .await
            .map_err(|_| Elapsed)
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::Elapsed;
    use std::{future::Future, time::Duration};

    pub(crate) fn spawn(future: impl Future<Output = ()> + 'static) {
        wasm_bindgen_futures::spawn_local(future);
    }

    /// `setTimeout` takes signed 32-bit milliseconds: chunk long durations and
    /// round sub-millisecond remainders up so a timer never fires early.
    pub(crate) async fn sleep(duration: Duration) {
        let mut millis = duration.as_millis().saturating_add(u128::from(
            !duration.subsec_nanos().is_multiple_of(1_000_000),
        ));
        while millis > 0 {
            let chunk = millis.min(i32::MAX as u128);
            millis -= chunk;
            #[expect(clippy::cast_possible_truncation, reason = "chunk is at most i32::MAX")]
            gloo_timers::future::TimeoutFuture::new(chunk as u32).await;
        }
    }

    pub(crate) async fn timeout<T>(
        duration: Duration,
        future: impl Future<Output = T>,
    ) -> Result<T, Elapsed> {
        tokio::select! { biased;
            result = future => Ok(result),
            () = sleep(duration) => Err(Elapsed),
        }
    }
}

pub(crate) use imp::{sleep, spawn, timeout};
