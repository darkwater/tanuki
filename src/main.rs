#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let address = std::env::var("TANUKI_LISTEN").unwrap_or_else(|_| "127.0.0.1:3000".to_owned());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!(listen = %listener.local_addr()?, "Tanuki HTTP server listening");
    tanuki::server::serve_until(listener, async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to listen for Ctrl-C");
        }
    })
    .await?;
    Ok(())
}
