use log::{Metadata, Record};
pub use log::{debug, error, info, trace, warn};

static LOGGER: Logger = Logger;

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let level = match record.level() {
                log::Level::Error => crate::logging::Level::Error,
                log::Level::Warn => crate::logging::Level::Warn,
                log::Level::Info => crate::logging::Level::Info,
                log::Level::Debug => crate::logging::Level::Debug,
                log::Level::Trace => crate::logging::Level::Trace,
            };

            crate::logging::log(level, record.target(), &record.args().to_string());
        }
    }

    fn flush(&self) {}
}

pub fn init() {
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(log::LevelFilter::Debug);
}
