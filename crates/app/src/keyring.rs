//! The saved session, kept in the Secret Service so the password is never stored.

use std::collections::HashMap;

use crate::APP_ID;

pub struct Saved {
	pub email: String,
	pub session: String,
}

fn attrs(email: Option<&str>) -> HashMap<&str, &str> {
	let mut a = HashMap::from([("application", APP_ID)]);
	if let Some(e) = email {
		a.insert("account", e);
	}
	a
}

/// The session saved last, if the keyring has one. Must run on the Tokio runtime.
pub async fn load() -> Option<Saved> {
	tokio::time::timeout(std::time::Duration::from_secs(10), load_inner()).await.ok().flatten()
}

async fn load_inner() -> Option<Saved> {
	let keyring = oo7::Keyring::new().await.ok()?;
	let _ = keyring.unlock().await;
	let item = keyring.search_items(&attrs(None)).await.ok()?.into_iter().next()?;
	let email = item.attributes().await.ok()?.remove("account").unwrap_or_default();
	let secret = item.secret().await.ok()?;
	let session = String::from_utf8(secret.to_vec()).ok()?;
	Some(Saved { email, session })
}

/// Remembers a session, replacing any other. Errors are ignored: Tern then just does not remember.
pub async fn store(email: &str, session: &str) {
	let Ok(keyring) = oo7::Keyring::new().await else { return };
	let _ = keyring.delete(&attrs(None)).await;
	let label = format!("Tern session for {email}");
	if let Err(e) = keyring.create_item(&label, &attrs(Some(email)), session, true).await {
		eprintln!("tern: could not save the session: {e}");
	}
}

pub async fn forget() {
	if let Ok(keyring) = oo7::Keyring::new().await {
		let _ = keyring.delete(&attrs(None)).await;
	}
}

/// Kept apart from the session items, which are replaced as a whole.
const CACHE_KEY_APP: &str = "xyz.hollingworth.Tern.CacheKey";

fn cache_attrs(email: &str) -> HashMap<&str, &str> {
	HashMap::from([("application", CACHE_KEY_APP), ("account", email)])
}

/// The key of `email`'s mail cache, made and stored on first use. None when there is no keyring:
/// then nothing is cached on disk.
pub async fn cache_key(email: &str) -> Option<[u8; 32]> {
	let keyring = oo7::Keyring::new().await.ok()?;
	let _ = keyring.unlock().await;
	if let Some(item) = keyring.search_items(&cache_attrs(email)).await.ok()?.into_iter().next() {
		if let Ok(key) = <[u8; 32]>::try_from(item.secret().await.ok()?.as_bytes()) {
			return Some(key);
		}
	}
	let mut key = [0u8; 32];
	std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut key)).ok()?;
	keyring.create_item(&format!("Tern mail cache key for {email}"), &cache_attrs(email), key.to_vec(), true).await.ok()?;
	Some(key)
}

pub async fn forget_cache_key(email: &str) {
	if let Ok(keyring) = oo7::Keyring::new().await {
		let _ = keyring.delete(&cache_attrs(email)).await;
	}
}
