use tanuki_tv_volume::{Config, Error, run};
use tokio::net::TcpListener;

const USAGE: &str = "Usage: tanuki-tv-volume DESCRIPTION_URL CALLBACK_URL [LISTEN_ADDR] [TANUKI_WS_URL]\nExample: tanuki-tv-volume http://192.168.1.50:9197/description.xml http://192.168.1.20:9198/notify 0.0.0.0:9198 ws://127.0.0.1:5167/v1/ws\nUse the TV's actual advertised device-description URL; the example URL is a placeholder.";

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt::init();
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| s == "--help" || s == "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    if !(2..=4).contains(&args.len()) {
        eprintln!("{USAGE}");
        return Err(Error::Protocol("expected two to four arguments"));
    }
    let config = Config {
        description_url: args[0].clone(),
        callback_url: args[1].clone(),
        tanuki_url: args
            .get(3)
            .cloned()
            .unwrap_or_else(|| "ws://127.0.0.1:5167/v1/ws".into()),
    };
    let listener =
        TcpListener::bind(args.get(2).map(String::as_str).unwrap_or("0.0.0.0:9198")).await?;
    run(config, listener, shutdown()).await
}

async fn shutdown() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut term) => tokio::select! {
                result = tokio::signal::ctrl_c() => if let Err(error) = result { tracing::error!(%error, "signal handler failed"); },
                _ = term.recv() => {},
            },
            Err(error) => tracing::error!(%error, "SIGTERM handler failed"),
        }
    }
    #[cfg(not(unix))]
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "signal handler failed");
    }
}
