use tanuki_tv_volume::{
    Config, Error,
    discovery::{automatic_callback, discover, select_device},
    run,
};
use tokio::net::TcpListener;

const USAGE: &str = "Usage: tanuki-tv-volume [--device UDN_OR_NAME] [--listen ADDR] [--callback-url URL] [--tanuki-url URL]\n       tanuki-tv-volume --discover\n       tanuki-tv-volume DESCRIPTION_URL CALLBACK_URL [LISTEN_ADDR] [TANUKI_WS_URL]\n\nWith no arguments, discovers one RenderingControl device and chooses the callback address automatically.\n--discover only lists devices using SSDP and HTTP GET; it never connects to Tanuki or sends volume commands.\nWhen multiple devices are found, choose a stable UDN using --device. Discovery uses the default IPv4 multicast route.";

#[derive(Debug, Eq, PartialEq)]
enum Mode {
    Bridge,
    Discover,
}
#[derive(Debug, Eq, PartialEq)]
enum Target {
    Automatic(Option<String>),
    Description(String),
}
#[derive(Debug)]
struct Options {
    mode: Mode,
    target: Target,
    callback: Option<String>,
    listen: String,
    tanuki: String,
}
impl Options {
    fn parse(args: &[String]) -> Result<Self, Error> {
        let mut options = Self {
            mode: Mode::Bridge,
            target: Target::Automatic(None),
            callback: None,
            listen: "0.0.0.0:9198".into(),
            tanuki: "ws://127.0.0.1:5167/v1/ws".into(),
        };
        if args.first().is_some_and(|s| !s.starts_with('-')) {
            if !(2..=4).contains(&args.len()) {
                return Err(Error::Protocol(
                    "legacy configuration needs two to four arguments",
                ));
            }
            options.target = Target::Description(args[0].clone());
            options.callback = Some(args[1].clone());
            if let Some(listen) = args.get(2) {
                options.listen = listen.clone();
            }
            if let Some(tanuki) = args.get(3) {
                options.tanuki = tanuki.clone();
            }
            return Ok(options);
        }
        if args == ["--discover"] {
            options.mode = Mode::Discover;
            return Ok(options);
        }
        let mut args = args.iter();
        while let Some(flag) = args.next() {
            if !matches!(
                flag.as_str(),
                "--device" | "--listen" | "--callback-url" | "--tanuki-url"
            ) {
                return Err(Error::Protocol("unknown option; see --help"));
            }
            let value = args
                .next()
                .filter(|s| !s.starts_with("--"))
                .ok_or(Error::Protocol("option requires a value; see --help"))?
                .clone();
            match flag.as_str() {
                "--device" => options.target = Target::Automatic(Some(value)),
                "--listen" => options.listen = value,
                "--callback-url" => options.callback = Some(value),
                "--tanuki-url" => options.tanuki = value,
                _ => unreachable!("validated flag"),
            }
        }
        Ok(options)
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt::init();
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| s == "--help" || s == "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    let options = Options::parse(&args)?;
    // This branch deliberately returns before listener setup, SDK connection,
    // input claiming or any path to SOAP SetVolume (including retained intent).
    if options.mode == Mode::Discover {
        let devices = discover().await?;
        for device in &devices {
            println!(
                "{:?}\t{}\t{}",
                device.name(),
                device.udn(),
                device.description_url()
            );
        }
        if devices.is_empty() {
            return Err(tanuki_tv_volume::discovery::SelectionError::NoneFound.into());
        }
        return Ok(());
    }
    let description_url = match options.target {
        Target::Description(location) => url::Url::parse(&location)?,
        Target::Automatic(selector) => {
            let devices = discover().await?;
            let device = select_device(&devices, selector.as_deref())?;
            tracing::info!(
                name = device.name(),
                udn = device.udn(),
                "selected UPnP device"
            );
            device.description_url().clone()
        }
    };
    let listener = TcpListener::bind(&options.listen).await?;
    let callback_url = match options.callback {
        Some(url) => url,
        None => automatic_callback(&description_url, listener.local_addr()?)
            .await?
            .to_string(),
    };
    tracing::info!(%description_url, %callback_url, "starting TV volume bridge");
    run(
        Config {
            description_url: description_url.to_string(),
            callback_url,
            tanuki_url: options.tanuki,
        },
        listener,
        shutdown(),
    )
    .await
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

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).into()).collect()
    }
    #[test]
    fn default_discovers_and_read_only_listing_is_a_distinct_mode() {
        let defaults = Options::parse(&[]).unwrap();
        assert_eq!(defaults.target, Target::Automatic(None));
        assert!(defaults.callback.is_none());
        assert_eq!(defaults.listen, "0.0.0.0:9198");
        assert_eq!(
            Options::parse(&args(&["--discover"])).unwrap().mode,
            Mode::Discover
        );
    }
    #[test]
    fn selection_and_overrides_work_and_legacy_addresses_remain_supported() {
        let parsed = Options::parse(&args(&[
            "--device",
            "uuid:tv",
            "--listen",
            "0.0.0.0:0",
            "--tanuki-url",
            "ws://server/v1/ws",
        ]))
        .unwrap();
        assert_eq!(parsed.target, Target::Automatic(Some("uuid:tv".into())));
        assert_eq!(parsed.listen, "0.0.0.0:0");
        assert_eq!(parsed.tanuki, "ws://server/v1/ws");
        let legacy =
            Options::parse(&args(&["http://tv/tv.xml", "http://host:9198/notify"])).unwrap();
        assert_eq!(
            legacy.target,
            Target::Description("http://tv/tv.xml".into())
        );
        assert_eq!(legacy.callback.as_deref(), Some("http://host:9198/notify"));
        assert!(Options::parse(&args(&["--device"])).is_err());
        assert!(Options::parse(&args(&["--unknown"])).is_err());
        assert!(Options::parse(&args(&["--discover", "--device", "uuid:tv"])).is_err());
    }
}
