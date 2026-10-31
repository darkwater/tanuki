//! Concrete executor/time adapters; protocol and session behavior stay shared.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use tokio::{
    spawn,
    task::{AbortHandle, JoinError, JoinHandle},
    time::{Instant, sleep_until, timeout},
};

#[cfg(target_arch = "wasm32")]
mod browser {
    pub(crate) use futures_util::future::AbortHandle;
    use futures_util::future::{Abortable, Aborted};
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll},
        time::Duration,
    };
    use tokio::sync::oneshot;
    pub(crate) use web_time::Instant;

    // Browser panics abort WASM; cancellation is the only recoverable task failure.
    #[derive(Debug, thiserror::Error)]
    #[error("browser task cancelled")]
    pub struct JoinError;
    impl JoinError {
        pub fn is_cancelled(&self) -> bool {
            true
        }
    }
    pub(crate) struct JoinHandle<T> {
        result: oneshot::Receiver<Result<T, Aborted>>,
        abort: AbortHandle,
    }
    impl<T> JoinHandle<T> {
        pub fn abort(&self) {
            self.abort.abort();
        }
        pub fn abort_handle(&self) -> AbortHandle {
            self.abort.clone()
        }
        pub fn is_finished(&self) -> bool {
            !self.result.is_empty() || self.result.is_terminated()
        }
    }
    impl<T> Future for JoinHandle<T> {
        type Output = Result<T, JoinError>;
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            Pin::new(&mut self.result)
                .poll(cx)
                .map(|result| match result {
                    Ok(Ok(value)) => Ok(value),
                    Ok(Err(_)) | Err(_) => Err(JoinError),
                })
        }
    }
    pub(crate) fn spawn<T: 'static>(future: impl Future<Output = T> + 'static) -> JoinHandle<T> {
        let (abort, registration) = AbortHandle::new_pair();
        let (send, result) = oneshot::channel();
        wasm_bindgen_futures::spawn_local(async move {
            let result = Abortable::new(future, registration).await;
            let _ = send.send(result);
        });
        JoinHandle { result, abort }
    }

    pub(crate) async fn sleep_until(deadline: Instant) {
        // setTimeout accepts signed 32-bit milliseconds. Chunk large durations and
        // round up sub-millisecond remainders so a timer never intentionally fires early.
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return;
            }
            let millis = remaining.as_millis().saturating_add(u128::from(
                !remaining.subsec_nanos().is_multiple_of(1_000_000),
            ));
            gloo_timers::future::TimeoutFuture::new(millis.min(i32::MAX as u128) as u32).await;
        }
    }
    pub(crate) async fn timeout<T>(
        duration: Duration,
        future: impl Future<Output = T>,
    ) -> Result<T, ()> {
        let timer = sleep_until(Instant::now() + duration);
        tokio::pin!(future, timer);
        tokio::select! { biased;
            result = &mut future => Ok(result),
            _ = &mut timer => Err(()),
        }
    }
}
#[cfg(target_arch = "wasm32")]
pub(crate) use browser::*;

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use std::{future::pending, time::Duration};
    use tokio::sync::oneshot;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    async fn browser_tasks_join_and_abort_without_a_tokio_runtime() {
        assert_eq!(spawn(async { 42 }).await.unwrap(), 42);
        let (started, ready) = oneshot::channel();
        let (dropped, stopped) = oneshot::channel();
        struct OnDrop(Option<oneshot::Sender<()>>);
        impl Drop for OnDrop {
            fn drop(&mut self) {
                let _ = self.0.take().unwrap().send(());
            }
        }
        let task = spawn(async move {
            let _guard = OnDrop(Some(dropped));
            started.send(()).unwrap();
            pending::<()>().await;
        });
        ready.await.unwrap();
        assert!(!task.is_finished());
        task.abort_handle().abort();
        assert!(task.await.unwrap_err().is_cancelled());
        stopped.await.unwrap();
    }

    #[wasm_bindgen_test]
    async fn browser_timers_round_up_and_cancel_pending_work_at_deadline() {
        let start = Instant::now();
        let deadline = start + Duration::from_nanos(1);
        sleep_until(deadline).await;
        assert!(Instant::now() >= deadline);
        assert_eq!(timeout(Duration::from_secs(1), async { 7 }).await, Ok(7));
        assert!(
            timeout(Duration::from_millis(1), pending::<()>())
                .await
                .is_err()
        );
    }
}
