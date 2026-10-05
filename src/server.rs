use std::future::Future;

/// Runs the server lifecycle until the supplied shutdown signal completes.
///
/// Network and core tasks will be attached here as their implementation cards
/// land. Keeping shutdown injectable gives tests the same lifecycle path as the
/// binary without installing process signal handlers.
pub async fn run_until<S, E>(shutdown: S) -> Result<(), E>
where
    S: Future<Output = Result<(), E>>,
{
    shutdown.await
}
