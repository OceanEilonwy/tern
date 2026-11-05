//! Where Tern keeps things. Worked out once, at the top, and handed down: a process can hold apps
//! on different folders, as the tests do, and nothing has to rewrite its own environment.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
	config: PathBuf,
	cache: PathBuf,
}

impl Paths {
	/// Under the given base folders, each of which gets a `tern` folder of its own.
	pub fn new(config_base: impl AsRef<Path>, cache_base: impl AsRef<Path>) -> Paths {
		Paths { config: config_base.as_ref().join("tern"), cache: cache_base.as_ref().join("tern") }
	}

	/// Where a Linux desktop says to put things.
	pub fn from_env() -> Paths {
		let base = |xdg: &str, under_home: &str| {
			std::env::var_os(xdg)
				.map(PathBuf::from)
				.or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(under_home)))
				.unwrap_or_else(std::env::temp_dir)
		};
		Paths::new(base("XDG_CONFIG_HOME", ".config"), base("XDG_CACHE_HOME", ".cache"))
	}

	pub fn config_dir(&self) -> &Path {
		&self.config
	}

	pub fn settings_file(&self) -> PathBuf {
		self.config.join("settings.toml")
	}

	pub fn rules_file(&self) -> PathBuf {
		self.config.join("rules.toml")
	}

	/// An account's mail cache.
	pub fn mail_cache(&self, email: &str) -> PathBuf {
		let name: String = email.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '@' { c } else { '_' }).collect();
		self.cache.join("mail").join(format!("{name}.db"))
	}

	/// The language model search by meaning uses.
	pub fn model_dir(&self) -> PathBuf {
		self.cache.join("models").join("potion-base-8M")
	}

	/// The Tuta SDK's own cache.
	pub fn sdk_dir(&self) -> PathBuf {
		self.cache.join("sdk")
	}

	/// Files handed to other apps to open.
	pub fn opened_dir(&self) -> PathBuf {
		self.cache.join("open")
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn everything_stays_where_it_has_always_been() {
		let paths = Paths::new("/c", "/k");
		assert_eq!(paths.settings_file(), Path::new("/c/tern/settings.toml"));
		assert_eq!(paths.rules_file(), Path::new("/c/tern/rules.toml"));
		assert_eq!(paths.model_dir(), Path::new("/k/tern/models/potion-base-8M"));
		assert_eq!(paths.sdk_dir(), Path::new("/k/tern/sdk"));
	}

	#[test]
	fn an_address_cannot_lead_out_of_the_mail_folder() {
		let paths = Paths::new("/c", "/k");
		assert_eq!(paths.mail_cache("me@tuta.com"), Path::new("/k/tern/mail/me@tuta.com.db"));
		assert_eq!(paths.mail_cache("../a/b@x"), Path::new("/k/tern/mail/.._a_b@x.db"));
	}
}
