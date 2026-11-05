//! The saved mail rules, in `~/.config/tern/rules.toml`. What rules mean is `tern-rules`.

use tern_rules::{from_toml, to_toml, Rule};

use crate::paths::Paths;

/// The saved rules; none when there are none or the file does not read.
pub fn load(paths: &Paths) -> Vec<Rule> {
	std::fs::read_to_string(paths.rules_file()).map(|raw| from_toml(&raw)).unwrap_or_default()
}

pub fn save(paths: &Paths, rules: &[Rule]) {
	let path = paths.rules_file();
	if let Some(dir) = path.parent() {
		let _ = std::fs::create_dir_all(dir);
	}
	if let Err(e) = std::fs::write(path, to_toml(rules)) {
		log::warn!("could not save rules: {e}");
	}
}
