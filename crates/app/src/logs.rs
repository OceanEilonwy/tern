//! One log, wherever it is read: stderr on the desktop, the system's log on Android. Only Tern's
//! own warnings by default, so the SDK cannot write anything about mail where it may be kept.
//! `TERN_LOG` asks for all of Tern's and the SDK's general information.

use log::{Level, LevelFilter, Log, Metadata, Record};

struct Logger<S> {
	sink: S,
	verbose: bool,
}

fn wanted(target: &str, level: Level, verbose: bool) -> bool {
	let ours = target == "tern" || target.starts_with("tern_") || target.starts_with("tern::");
	match (ours, verbose) {
		(true, true) => true,
		(true, false) => level <= Level::Warn,
		(false, true) => level <= Level::Info,
		(false, false) => false,
	}
}

impl<S: Fn(String) + Send + Sync> Log for Logger<S> {
	fn enabled(&self, metadata: &Metadata) -> bool {
		wanted(metadata.target(), metadata.level(), self.verbose)
	}

	fn log(&self, record: &Record) {
		if self.enabled(record.metadata()) {
			(self.sink)(record.args().to_string());
		}
	}

	fn flush(&self) {}
}

/// Sends the log to `sink`, for the life of the process. A second call changes nothing.
pub fn init(sink: impl Fn(String) + Send + Sync + 'static) {
	let verbose = std::env::var_os("TERN_LOG").is_some();
	if log::set_boxed_logger(Box::new(Logger { sink, verbose })).is_ok() {
		log::set_max_level(if verbose { LevelFilter::Trace } else { LevelFilter::Warn });
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn only_tern_warnings_by_default() {
		assert!(wanted("tern_sync::fetcher", Level::Warn, false));
		assert!(wanted("tern::window", Level::Error, false));
		assert!(!wanted("tern_sync::fetcher", Level::Debug, false));
		assert!(!wanted("tutasdk::login", Level::Error, false));
		assert!(!wanted("ternary", Level::Warn, false));
	}

	#[test]
	fn asking_for_more_stops_short_of_the_sdk_details() {
		assert!(wanted("tern_tuta", Level::Trace, true));
		assert!(wanted("tutasdk::login", Level::Info, true));
		assert!(!wanted("tutasdk::login", Level::Debug, true));
	}
}
