//! Tern's few settings, in `~/.config/tern/settings.toml`.

use tern_sync::Level;

use crate::paths::Paths;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
	/// How much mail to keep ready offline.
	pub offline: Level,
	/// The folder sidebar's width in pixels, as last dragged; 0 is the default.
	pub folders_width: u32,
	/// The folder sidebar shows only icons.
	pub folders_rail: bool,
	/// The conversation list's width in pixels, as last dragged; 0 is the default.
	pub list_width: u32,
	/// Search finds mail by meaning too, with a model downloaded once.
	pub semantic: bool,
	/// Signing out deletes the mail kept on this computer, and its key; off keeps it for the next sign-in.
	pub forget_on_sign_out: bool,
	/// The sidebar offers to clean up once the cache passes this many megabytes; 0 for never.
	pub cleanup_mb: u32,
	/// Android's dark theme uses true black, for OLED screens.
	pub pure_black: bool,
	/// Android takes its colours from the wallpaper instead of Tern's blue.
	pub match_wallpaper: bool,
	/// Android checks for new mail in the background this often, in minutes; 0 for never.
	pub background_minutes: u32,
	/// Android says when new mail comes to the Inbox. Needs the background checks.
	pub notifications: bool,
}

/// How often the background checks can run, in minutes, with 0 for Off first. Android runs periodic
/// work 15 minutes apart at the closest.
pub const BACKGROUND_MINUTES: [u32; 4] = [0, 15, 30, 60];

/// The sizes "Offer Cleanup Past" can be, in megabytes, with 0 for Never last.
pub const CLEANUP_MB: [u32; 6] = [500, 1000, 2000, 5000, 10000, 0];

impl Default for Settings {
	fn default() -> Settings {
		Settings { offline: Level::default(), folders_width: 0, folders_rail: false, list_width: 0, semantic: false, forget_on_sign_out: false, cleanup_mb: 1000, pure_black: true, match_wallpaper: false, background_minutes: 0, notifications: false }
	}
}

impl Settings {
	/// The saved settings, or the defaults when there are none or they do not read.
	pub fn load(paths: &Paths) -> Settings {
		if let Ok(raw) = std::fs::read_to_string(paths.settings_file()) {
			return Self::parse(&raw);
		}
		// Settings were JSON before; move them over once.
		let old = paths.config_dir().join("settings.json");
		let Ok(raw) = std::fs::read(&old) else { return Settings::default() };
		let settings = Self::from_json(&raw);
		settings.save(paths);
		let _ = std::fs::remove_file(old);
		settings
	}

	/// Each setting on its own: one that does not read falls back to its default.
	fn parse(raw: &str) -> Settings {
		let table: toml::Table = raw.parse().unwrap_or_default();
		let width = |key: &str| table.get(key).and_then(|v| v.as_integer()).and_then(|w| u32::try_from(w).ok()).unwrap_or(0);
		Settings {
			offline: table.get("offline").and_then(|v| v.as_str()).and_then(Level::from_name).unwrap_or_default(),
			folders_width: width("folders_width"),
			folders_rail: table.get("folders_rail").and_then(|v| v.as_bool()).unwrap_or(false),
			list_width: width("list_width"),
			semantic: table.get("semantic").and_then(|v| v.as_bool()).unwrap_or(false),
			forget_on_sign_out: table.get("forget_on_sign_out").and_then(|v| v.as_bool()).unwrap_or(false),
			cleanup_mb: table
				.get("cleanup_mb")
				.and_then(|v| v.as_integer())
				.and_then(|mb| u32::try_from(mb).ok())
				.filter(|mb| CLEANUP_MB.contains(mb))
				.unwrap_or(Settings::default().cleanup_mb),
			pure_black: table.get("pure_black").and_then(|v| v.as_bool()).unwrap_or(true),
			match_wallpaper: table.get("match_wallpaper").and_then(|v| v.as_bool()).unwrap_or(false),
			background_minutes: table.get("background_minutes").and_then(|v| v.as_integer()).and_then(|m| u32::try_from(m).ok()).filter(|m| BACKGROUND_MINUTES.contains(m)).unwrap_or(0),
			notifications: table.get("notifications").and_then(|v| v.as_bool()).unwrap_or(false),
		}
	}

	fn from_json(raw: &[u8]) -> Settings {
		let json: serde_json::Value = serde_json::from_slice(raw).unwrap_or_default();
		let width = |key: &str| json[key].as_u64().and_then(|w| u32::try_from(w).ok()).unwrap_or(0);
		Settings {
			offline: json["offline"].as_str().and_then(Level::from_name).unwrap_or_default(),
			folders_width: width("folders_width"),
			folders_rail: json["folders_rail"].as_bool().unwrap_or(false),
			list_width: width("list_width"),
			..Settings::default()
		}
	}

	fn to_toml(self) -> String {
		format!(
			"offline = \"{}\"\nfolders_width = {}\nfolders_rail = {}\nlist_width = {}\nsemantic = {}\nforget_on_sign_out = {}\ncleanup_mb = {}\npure_black = {}\nmatch_wallpaper = {}\nbackground_minutes = {}\nnotifications = {}\n",
			self.offline.name(),
			self.folders_width,
			self.folders_rail,
			self.list_width,
			self.semantic,
			self.forget_on_sign_out,
			self.cleanup_mb,
			self.pure_black,
			self.match_wallpaper,
			self.background_minutes,
			self.notifications
		)
	}

	pub fn save(self, paths: &Paths) {
		let _ = std::fs::create_dir_all(paths.config_dir());
		if let Err(e) = std::fs::write(paths.settings_file(), self.to_toml()) {
			log::warn!("could not save settings: {e}");
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn settings_round_trip_and_bad_values_fall_back() {
		let s = Settings { offline: Level::High, folders_width: 240, folders_rail: true, list_width: 480, semantic: true, forget_on_sign_out: true, cleanup_mb: 0, pure_black: false, match_wallpaper: true, background_minutes: 30, notifications: true };
		assert_eq!(Settings::parse(&s.to_toml()), s);
		assert_eq!(s.to_toml(), "offline = \"high\"\nfolders_width = 240\nfolders_rail = true\nlist_width = 480\nsemantic = true\nforget_on_sign_out = true\ncleanup_mb = 0\npure_black = false\nmatch_wallpaper = true\nbackground_minutes = 30\nnotifications = true\n");
		assert_eq!(Settings::parse("not = [toml"), Settings::default());
		assert!(!Settings::parse("").forget_on_sign_out, "signing out keeps the mail unless asked");
		assert_eq!(Settings::parse("").cleanup_mb, 1000);
		assert_eq!(Settings::parse("cleanup_mb = 5000").cleanup_mb, 5000);
		assert_eq!(Settings::parse("cleanup_mb = 3").cleanup_mb, 1000, "only the sizes offered");
		assert_eq!(Settings::parse("background_minutes = 5").background_minutes, 0, "only the times offered");
		assert!(!Settings::parse("").notifications, "off until asked for");
		assert_eq!(Settings::parse("offline = \"huge\""), Settings::default());
		// One bad value leaves the others.
		assert_eq!(Settings::parse("offline = \"low\"\nfolders_width = -5\nlist_width = \"wide\""), Settings { offline: Level::Low, ..Settings::default() });
	}

	#[test]
	fn old_json_settings_carry_over() {
		let s = Settings { offline: Level::High, folders_width: 240, folders_rail: true, list_width: 480, semantic: false, ..Settings::default() };
		assert_eq!(Settings::from_json(br#"{"offline":"high","folders_width":240,"folders_rail":true,"list_width":480}"#), s);
		assert_eq!(Settings::from_json(br#"{"offline":"low"}"#), Settings { offline: Level::Low, ..Settings::default() });
		assert_eq!(Settings::from_json(b"not json"), Settings::default());
	}
}
