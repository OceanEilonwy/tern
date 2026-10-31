//! Search by meaning, when turned on in Preferences: the language model it needs, downloaded once
//! into `~/.cache/tern/models`, and working out each mail's meaning in the background.
//!
//! The model is public and the same for everyone; the vectors worked out from mail stay in the
//! encrypted cache. Nothing about mail leaves the computer.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tern_cache::embed::Model;
use tern_cache::{sync, Cache};
use tern_core::Error;

/// The model's name with each vector, so vectors from another model are worked out again.
pub const NAME: &str = "potion-base-8M@bf8b056";
/// What Preferences says the download is.
pub const SIZE: &str = "30 MB";
/// minishlab/potion-base-8M (MIT licence) at a fixed revision; each file is checked by its SHA-256.
const REPO: &str = "https://huggingface.co/minishlab/potion-base-8M/resolve/bf8b056651a2c21b8d2565580b8569da283cab23";
const FILES: [(&str, &str); 2] = [
	("tokenizer.json", "e67e803f624fb4d67dea1c730d06e1067e1b14d830e2c2202569e3ef0f70bb50"),
	("model.safetensors", "f65d0f325faadc1e121c319e2faa41170d3fa07d8c89abd48ca5358d9a223de2"),
];

static MODEL: Mutex<Option<Arc<Model>>> = Mutex::new(None);
/// Held while the model is read or fetched, so it is not downloaded twice at once.
static LOADING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn dir() -> PathBuf {
	let base = std::env::var_os("XDG_CACHE_HOME")
		.map(PathBuf::from)
		.or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
		.unwrap_or_else(std::env::temp_dir);
	base.join("tern").join("models").join("potion-base-8M")
}

/// The model, if it has been loaded.
pub fn loaded() -> Option<Arc<Model>> {
	MODEL.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Loads the model, downloading it first if it is not on disk (or does not read). Runs on the Tokio runtime.
pub async fn ensure() -> Result<Arc<Model>, Error> {
	let _one_at_a_time = LOADING.lock().await;
	if let Some(model) = loaded() {
		return Ok(model);
	}
	let dir = dir();
	let read = |dir: PathBuf| {
		tokio::task::spawn_blocking(move || {
			let file = |name: &str| std::fs::read(dir.join(name)).map_err(|e| e.to_string());
			Model::load(&file(FILES[0].0)?, &file(FILES[1].0)?)
		})
	};
	let model = match read(dir.clone()).await.map_err(|e| Error::Other(e.to_string()))? {
		Ok(model) => model,
		Err(_) => {
			std::fs::create_dir_all(&dir).map_err(|e| Error::Other(e.to_string()))?;
			for (name, sha256) in FILES {
				let data = tern_tuta::download(&format!("{REPO}/{name}"), sha256).await?;
				// Written whole, then renamed, so a half-written file is never read.
				let part = dir.join(format!("{name}.part"));
				std::fs::write(&part, data).and_then(|_| std::fs::rename(&part, dir.join(name))).map_err(|e| Error::Other(e.to_string()))?;
			}
			read(dir).await.map_err(|e| Error::Other(e.to_string()))?.map_err(Error::Other)?
		},
	};
	let model = Arc::new(model);
	*MODEL.lock().unwrap_or_else(|p| p.into_inner()) = Some(model.clone());
	Ok(model)
}

/// Search by meaning was turned off: the vectors go, the model leaves memory. The download stays
/// on disk, so turning it on again needs no network.
pub fn forget(cache: Option<&Cache>) {
	*MODEL.lock().unwrap_or_else(|p| p.into_inner()) = None;
	if let Some(cache) = cache {
		let _ = cache.clear_vectors();
	}
}

/// Bytes the downloaded model takes on disk; 0 when it is not there.
pub fn model_bytes() -> u64 {
	FILES.iter().filter_map(|(name, _)| std::fs::metadata(dir().join(name)).ok()).map(|m| m.len()).sum()
}

/// Deletes the downloaded model; turning search by meaning on fetches it again.
pub fn remove_model() {
	let _ = std::fs::remove_dir_all(dir());
}

/// Works out the meaning of the mails that lack it, off the UI thread. Returns how many.
pub async fn index(cache: Arc<Cache>) -> Result<usize, Error> {
	let model = ensure().await?;
	tokio::task::spawn_blocking(move || sync::vectors(&cache, &model, NAME))
		.await
		.map_err(|e| Error::Other(e.to_string()))?
		.map_err(|e| Error::Other(e.to_string()))
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Downloads the real model into a temporary cache directory, so it needs the network:
	/// `cargo test -p tern semantic -- --ignored`
	#[tokio::test(flavor = "multi_thread")]
	#[ignore]
	async fn the_model_downloads_and_finds_mail_by_meaning() {
		let dir = std::env::temp_dir().join(format!("tern-model-test-{}", std::process::id()));
		std::env::set_var("XDG_CACHE_HOME", &dir);
		let model = ensure().await.unwrap();
		assert!(dir.join("tern/models/potion-base-8M/model.safetensors").exists());
		let cache = Arc::new(Cache::in_memory());
		let inbox = tern_core::Folder { id: tern_core::FolderId("in".into()), name: "Inbox".into(), kind: tern_core::FolderKind::Inbox, unread: 0 };
		cache.set_folders(std::slice::from_ref(&inbox)).unwrap();
		let mail = |n: u32, subject: &str| tern_core::MailSummary {
			id: tern_core::MailId { list: "l".into(), element: format!("e{n}") },
			from: tern_core::Address::new("Mia", "mia@tuta.com"),
			subject: subject.into(),
			received_ms: n.into(),
			unread: false,
			has_attachments: false,
			to: None,
			outgoing: false,
			draft: false,
		};
		cache.merge_page(&inbox.id, &[mail(1, "Photos from Saturday"), mail(2, "Your invoice for October")], true, true).unwrap();
		assert_eq!(index(cache.clone()).await.unwrap(), 2);
		let query = tern_cache::search::Query::parse("pictures");
		let hits = cache.search(&query, Some((NAME, &model.embed(&query.text()))), 10).unwrap();
		assert_eq!(hits.iter().map(|h| h.row.latest.subject.as_str()).collect::<Vec<_>>(), ["Photos from Saturday"]);
		let _ = std::fs::remove_dir_all(dir);
	}
}
