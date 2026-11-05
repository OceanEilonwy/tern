//! Secrets in the Secret Service, as the core asks for them: the session, so the password is never
//! stored, and each account's cache key. Items keep the attributes they always had, so existing
//! sign-ins survive.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;

use tern_app::{SecretError, SecretStore, CACHE_KEY_SECRET, SESSION_SECRET};

use crate::APP_ID;

/// The core calls a store off its own task, so waiting here, on an unlock prompt perhaps, holds up
/// nothing else. A thread of the runtime's blocking pool may wait on the runtime.
fn wait<T>(work: impl Future<Output = T>) -> T {
	tern_app::runtime().handle().block_on(work)
}

fn session_attrs(email: Option<&str>) -> HashMap<&str, &str> {
	let mut a = HashMap::from([("application", APP_ID)]);
	if let Some(e) = email {
		a.insert("account", e);
	}
	a
}

/// Kept apart from the session items, which are replaced as a whole.
const CACHE_KEY_APP: &str = "xyz.hollingworth.Tern.CacheKey";

fn cache_attrs(email: &str) -> HashMap<&str, &str> {
	HashMap::from([("application", CACHE_KEY_APP), ("account", email)])
}

fn unavailable(what: &str, e: impl std::fmt::Display) -> SecretError {
	log::warn!("could not {what}: {e}");
	SecretError::Unavailable
}

async fn opened() -> Result<oo7::Keyring, SecretError> {
	let keyring = oo7::Keyring::new().await.map_err(|e| unavailable("reach the keyring", e))?;
	let _ = keyring.unlock().await;
	Ok(keyring)
}

/// The session saved last, as the address and session the core saved. `None` when there is none.
async fn saved_session() -> Result<Option<Vec<u8>>, SecretError> {
	let keyring = opened().await?;
	let found = keyring.search_items(&session_attrs(None)).await.map_err(|e| unavailable("read the keyring", e))?;
	let Some(item) = found.into_iter().next() else { return Ok(None) };
	let email = item.attributes().await.map_err(|e| unavailable("read the keyring", e))?.remove("account").unwrap_or_default();
	let secret = item.secret().await.map_err(|e| unavailable("read the keyring", e))?;
	let session = String::from_utf8(secret.to_vec()).map_err(|e| unavailable("read the session", e))?;
	serde_json::to_vec(&(email, session)).map(Some).map_err(|e| unavailable("read the session", e))
}

/// The Secret Service.
pub struct Keyring;

impl SecretStore for Keyring {
	fn get(&self, name: String) -> Result<Option<Vec<u8>>, SecretError> {
		wait(async {
			if name == SESSION_SECRET {
				// With no keyring there is nothing to sign in with, which is no failure worth a word: the
				// sign-in page is what anyone without one has always seen.
				let limit = std::time::Duration::from_secs(10);
				return Ok(tokio::time::timeout(limit, saved_session()).await.ok().and_then(Result::ok).flatten());
			}
			let email = name.strip_prefix(CACHE_KEY_SECRET).ok_or(SecretError::Unavailable)?;
			let keyring = opened().await?;
			let found = keyring.search_items(&cache_attrs(email)).await.map_err(|e| unavailable("read the keyring", e))?;
			let Some(item) = found.into_iter().next() else { return Ok(None) };
			let secret = item.secret().await.map_err(|e| unavailable("read the keyring", e))?;
			Ok(Some(secret.to_vec()))
		})
	}

	fn put(&self, name: String, value: Vec<u8>) -> Result<(), SecretError> {
		wait(async {
			let keyring = oo7::Keyring::new().await.map_err(|e| unavailable("reach the keyring", e))?;
			if name == SESSION_SECRET {
				let (email, session): (String, String) = serde_json::from_slice(&value).map_err(|e| unavailable("save the session", e))?;
				let _ = keyring.delete(&session_attrs(None)).await;
				let label = format!("Tern session for {email}");
				return keyring.create_item(&label, &session_attrs(Some(&email)), session, true).await.map_err(|e| unavailable("save the session", e));
			}
			let email = name.strip_prefix(CACHE_KEY_SECRET).ok_or(SecretError::Unavailable)?;
			let _ = keyring.unlock().await;
			keyring.create_item(&format!("Tern mail cache key for {email}"), &cache_attrs(email), value, true).await.map_err(|e| unavailable("save a cache key", e))
		})
	}

	fn remove(&self, name: String) {
		wait(async {
			let Ok(keyring) = oo7::Keyring::new().await else { return };
			if name == SESSION_SECRET {
				let _ = keyring.delete(&session_attrs(None)).await;
			} else if let Some(email) = name.strip_prefix(CACHE_KEY_SECRET) {
				let _ = keyring.delete(&cache_attrs(email)).await;
			}
		});
	}
}

/// Secrets for as long as the process, for the scenes, which must not touch the real keyring.
#[derive(Default)]
pub struct InMemory(Mutex<HashMap<String, Vec<u8>>>);

impl SecretStore for InMemory {
	fn get(&self, name: String) -> Result<Option<Vec<u8>>, SecretError> {
		Ok(self.0.lock().unwrap_or_else(|p| p.into_inner()).get(&name).cloned())
	}
	fn put(&self, name: String, value: Vec<u8>) -> Result<(), SecretError> {
		self.0.lock().unwrap_or_else(|p| p.into_inner()).insert(name, value);
		Ok(())
	}
	fn remove(&self, name: String) {
		self.0.lock().unwrap_or_else(|p| p.into_inner()).remove(&name);
	}
}
