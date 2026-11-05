//! How the window's panes were left, in `~/.config/tern/layout.toml`. Only the desktop has panes to
//! drag, so this is not one of the settings the core knows about.

use std::path::Path;

/// Each width is in pixels, as last dragged, with 0 for the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Layout {
	pub folders_width: u32,
	/// The folder sidebar shows only icons.
	pub folders_rail: bool,
	pub list_width: u32,
}

impl Layout {
	/// As saved; or, the first time, as `settings.toml` kept it before it moved here.
	pub fn load(config_dir: &Path) -> Layout {
		let read = |name: &str| std::fs::read_to_string(config_dir.join(name)).ok();
		read("layout.toml").or_else(|| read("settings.toml")).map(|raw| Layout::parse(&raw)).unwrap_or_default()
	}

	fn parse(raw: &str) -> Layout {
		let table: toml::Table = raw.parse().unwrap_or_default();
		let width = |key: &str| table.get(key).and_then(|v| v.as_integer()).and_then(|w| u32::try_from(w).ok()).unwrap_or(0);
		Layout { folders_width: width("folders_width"), folders_rail: table.get("folders_rail").and_then(|v| v.as_bool()).unwrap_or(false), list_width: width("list_width") }
	}

	pub fn save(self, config_dir: &Path) {
		let _ = std::fs::create_dir_all(config_dir);
		let text = format!("folders_width = {}\nfolders_rail = {}\nlist_width = {}\n", self.folders_width, self.folders_rail, self.list_width);
		if let Err(e) = std::fs::write(config_dir.join("layout.toml"), text) {
			log::warn!("could not save the window's layout: {e}");
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn folder(name: &str) -> std::path::PathBuf {
		let dir = std::env::temp_dir().join(format!("tern-layout-{name}-{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&dir);
		std::fs::create_dir_all(&dir).unwrap();
		dir
	}

	#[test]
	fn a_layout_round_trips_and_bad_values_fall_back() {
		let dir = folder("trip");
		let layout = Layout { folders_width: 240, folders_rail: true, list_width: 480 };
		layout.save(&dir);
		assert_eq!(Layout::load(&dir), layout);
		assert_eq!(Layout::parse("folders_width = -5\nlist_width = \"wide\""), Layout::default());
		assert_eq!(Layout::parse("not = [toml"), Layout::default());
		let _ = std::fs::remove_dir_all(dir);
	}

	#[test]
	fn widths_left_in_the_settings_carry_over_once() {
		let dir = folder("old");
		std::fs::write(dir.join("settings.toml"), "offline = \"low\"\nfolders_width = 240\nfolders_rail = true\nlist_width = 480\n").unwrap();
		let carried = Layout::load(&dir);
		assert_eq!(carried, Layout { folders_width: 240, folders_rail: true, list_width: 480 });
		Layout { list_width: 300, ..carried }.save(&dir);
		assert_eq!(Layout::load(&dir).list_width, 300, "what was saved since wins");
		let _ = std::fs::remove_dir_all(dir);
	}
}
