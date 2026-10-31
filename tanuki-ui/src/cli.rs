use crate::app::ConnectionConfig;
use tanuki_protocol::{ClientName, ClientNameParseError};

pub const HELP: &str = "Usage: tanuki-ui [--url ws://HOST:PORT/v1/ws] [--client NAME]\nRead-only terminal inspector. Defaults to local server and a unique local name.\nKeys: q/Ctrl-C quit, r reconnect, ? help, / filter, Tab switch pane.";
#[derive(Debug)]
pub enum Cli {
    Help,
    Run(ConnectionConfig),
}
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("unknown option: {0}")]
    Unknown(String),
    #[error("missing value for {0}")]
    Missing(String),
    #[error("repeated option: {0}")]
    Duplicate(String),
    #[error("command-line arguments must be UTF-8")]
    NonUnicode,
    #[error(transparent)]
    Client(#[from] ClientNameParseError),
}
impl Cli {
    pub fn parse(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Self, CliError> {
        let mut args = args.into_iter();
        let mut url = None;
        let mut client = None;
        while let Some(arg) = args.next() {
            let arg = arg.into_string().map_err(|_| CliError::NonUnicode)?;
            if matches!(arg.as_str(), "--help" | "-h") {
                return Ok(Self::Help);
            }
            let target = match arg.as_str() {
                "--url" => &mut url,
                "--client" => &mut client,
                _ => return Err(CliError::Unknown(arg)),
            };
            if target.is_some() {
                return Err(CliError::Duplicate(arg));
            }
            *target = Some(
                args.next()
                    .ok_or_else(|| CliError::Missing(arg.clone()))?
                    .into_string()
                    .map_err(|_| CliError::NonUnicode)?,
            );
        }
        let default = ConnectionConfig::default();
        let client = client
            .map(|s| ClientName::parse(&s))
            .transpose()?
            .unwrap_or_else(|| default.client().clone());
        Ok(Self::Run(ConnectionConfig::new(
            url.unwrap_or_else(|| default.url().into()),
            client,
        )))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Cli, CliError> {
        Cli::parse(args.iter().map(std::ffi::OsString::from))
    }
    #[test]
    fn default_and_explicit_configuration_and_help() {
        let Cli::Run(default) = parse(&[]).unwrap() else {
            panic!("expected run")
        };
        assert_eq!(default.url(), crate::app::DEFAULT_URL);
        let Cli::Run(config) =
            parse(&["--url", "ws://localhost:9999/v1/ws", "--client", "observer"]).unwrap()
        else {
            panic!("expected config")
        };
        assert_eq!(config.url(), "ws://localhost:9999/v1/ws");
        assert_eq!(config.client().as_str(), "observer");
        assert!(matches!(parse(&["--help"]).unwrap(), Cli::Help));
    }
    #[test]
    fn invalid_arguments_are_errors_before_terminal_setup() {
        for args in [
            vec!["--url"],
            vec!["--client"],
            vec!["--client", "bad/name"],
            vec!["--unknown"],
            vec!["--url", "a", "--url", "b"],
        ] {
            assert!(parse(&args).is_err(), "accepted {args:?}");
        }
    }
}
