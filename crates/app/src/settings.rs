//! Tern's few settings, in `~/.config/tern/settings.json`.

use std::path::PathBuf;

use tern_cache::sync::Level;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settings {
	/// How much mail to keep ready offline.
	pub offline: Level,
}

fn path() -> PathBuf {
	let base = std::env::var_os("XDG_CONFIG_HOME")
		.map(PathBuf::from)
		.or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
		.unwrap_or_else(std::env::temp_dir);
	base.join("tern").join("settings.json")
}

impl Settings {
	/// The saved settings, or the defaults when there are none or they do not read.
	pub fn load() -> Settings {
		std::fs::read(path()).ok().map(|raw| Self::parse(&raw)).unwrap_or_default()
	}

	fn parse(raw: &[u8]) -> Settings {
		let json: serde_json::Value = serde_json::from_slice(raw).unwrap_or_default();
		Settings { offline: json["offline"].as_str().and_then(Level::from_name).unwrap_or_default() }
	}

	fn to_json(self) -> String {
		serde_json::json!({ "offline": self.offline.name() }).to_string()
	}

	pub fn save(self) {
		let path = path();
		if let Some(dir) = path.parent() {
			let _ = std::fs::create_dir_all(dir);
		}
		if let Err(e) = std::fs::write(&path, self.to_json()) {
			eprintln!("tern: could not save settings: {e}");
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn settings_round_trip_and_bad_files_fall_back() {
		let s = Settings { offline: Level::High };
		assert_eq!(Settings::parse(s.to_json().as_bytes()), s);
		assert_eq!(Settings::parse(b"not json"), Settings::default());
		assert_eq!(Settings::parse(br#"{"offline":"huge"}"#), Settings::default());
	}
}
