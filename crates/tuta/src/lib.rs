//! Tern's [`Backend`] for a Tuta account, on the vendored Tuta SDK.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use tern_core::{
	Account, Address, Attachment, Backend, Draft, Error, Folder, FolderId, FolderKind, Mail, MailId, MailPage, MailSummary, ReplyKind, Result,
};
use tutasdk::bindings::file_client::{FileClient, FileClientError};
use tutasdk::bindings::rest_client::{HttpMethod, RestClient, RestClientError, RestClientOptions, RestResponse};
use tutasdk::entities::generated::tutanota::{Mail as TutaMail, MailAddress, MailSet};
use tutasdk::folder_system::MailSetKind;
use tutasdk::login::{CredentialType, Credentials};
use tutasdk::net::native_rest_client::NativeRestClient;
use tutasdk::tern::{
	OutgoingAttachment, OutgoingMail, OutgoingRecipient, PageItem, TernAccount, TernError, TernMail, CONVERSATION_FORWARD,
	CONVERSATION_REPLY,
};
use tutasdk::{CustomId, GeneratedId, IdTupleGenerated, Sdk};

const BASE_URL: &str = "https://app.tuta.com";
const CLIENT_IDENTIFIER: &str = "Tern on Linux";

/// What the keyring keeps so the next start needs no password.
#[derive(Serialize, Deserialize)]
struct SavedSession {
	login: String,
	user_id: String,
	access_token: String,
	encrypted_passphrase_key: String,
}

/// Signs in with an email address and password. Returns the account and a saved session
/// to keep in the keyring, which [`resume`] turns back into the account without the password.
pub async fn login(email: &str, password: &str) -> Result<(Arc<dyn Backend>, String)> {
	let (email, password) = (email.trim().to_lowercase(), password.to_string());
	guarded(async move { login_inner(&email, &password).await }).await
}

async fn login_inner(email: &str, password: &str) -> Result<(Arc<dyn Backend>, String)> {
	let (sdk, credentials) = sdk()?.tern_sign_in(email, password, CLIENT_IDENTIFIER).await.map_err(to_error)?;
	trace("session open, loading the account");
	let saved = serde_json::to_string(&SavedSession {
		login: credentials.login.clone(),
		user_id: credentials.user_id.to_string(),
		access_token: credentials.access_token.clone(),
		encrypted_passphrase_key: BASE64_STANDARD.encode(&credentials.encrypted_passphrase_key),
	})
	.map_err(|e| Error::Other(e.to_string()))?;
	let backend = TutaBackend::new(sdk.tern(), &credentials.login).await?;
	trace("signed in");
	Ok((Arc::new(backend), saved))
}

/// A step on stderr when `TERN_LOG` is set.
pub fn trace(step: &str) {
	if std::env::var_os("TERN_LOG").is_some() {
		eprintln!("tern: {step}");
	}
}

/// Reopens a session saved by [`login`].
pub async fn resume(saved: &str) -> Result<Arc<dyn Backend>> {
	let saved = saved.to_string();
	guarded(async move { resume_inner(&saved).await }).await
}

async fn resume_inner(saved: &str) -> Result<Arc<dyn Backend>> {
	let saved: SavedSession = serde_json::from_str(saved).map_err(|_| Error::SessionExpired)?;
	let credentials = Credentials {
		login: saved.login.clone(),
		user_id: GeneratedId(saved.user_id),
		access_token: saved.access_token,
		encrypted_passphrase_key: BASE64_STANDARD.decode(saved.encrypted_passphrase_key).map_err(|_| Error::SessionExpired)?,
		credential_type: CredentialType::Internal,
	};
	let sdk = sdk()?.login(credentials).await.map_err(|e| to_error(e.into()))?;
	Ok(Arc::new(TutaBackend::new(sdk.tern(), &saved.login).await?))
}

fn sdk() -> Result<Sdk> {
	let rest = NativeRestClient::try_new().map_err(|e| Error::Other(format!("cannot set up TLS: {e}")))?;
	Ok(Sdk::new(BASE_URL.to_string(), Arc::new(TimedRestClient::new(rest)), Arc::new(CacheFileClient::new())))
}

/// How long one request may take before Tern gives up on it. Generous, since attachments
/// ride on single requests, but a stalled connection no longer leaves the window waiting forever.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// The SDK's HTTP client with a time limit on every request, and a log line per request on
/// stderr when `TERN_LOG` is set. The log shows method, path, status and time: never the query
/// string, headers or bodies, which carry tokens and mail.
struct TimedRestClient {
	inner: NativeRestClient,
	log: bool,
}

impl TimedRestClient {
	fn new(inner: NativeRestClient) -> Self {
		Self { inner, log: std::env::var_os("TERN_LOG").is_some() }
	}
}

#[async_trait]
impl RestClient for TimedRestClient {
	async fn request_binary(&self, url: String, method: HttpMethod, options: RestClientOptions) -> std::result::Result<RestResponse, RestClientError> {
		let what = format!("{method:?} {}", log_path(&url));
		if self.log {
			eprintln!("tern: {what} ...");
		}
		let started = std::time::Instant::now();
		let result = tokio::time::timeout(REQUEST_TIMEOUT, self.inner.request_binary(url, method, options)).await;
		let ms = started.elapsed().as_millis();
		if self.log {
			match &result {
				Ok(Ok(response)) if response.status >= 400 => {
					// Error answers carry no tokens; their headers may say why.
					let mut headers: Vec<_> = response.headers.iter().filter(|(k, _)| !k.contains("cookie")).collect();
					headers.sort();
					let body = response.body.as_deref().map(|b| String::from_utf8_lossy(&b[..b.len().min(300)]).into_owned());
					eprintln!("tern: {what} -> {} ({ms} ms) headers {headers:?} body {body:?}", response.status);
				},
				Ok(Ok(response)) => eprintln!("tern: {what} -> {} ({ms} ms)", response.status),
				Ok(Err(e)) => eprintln!("tern: {what} -> {e:?} ({ms} ms)"),
				Err(_) => eprintln!("tern: {what} -> timed out ({ms} ms)"),
			}
		}
		result.unwrap_or(Err(RestClientError::NetworkError))
	}
}

/// The URL without host and query string.
fn log_path(url: &str) -> &str {
	let path = url.find("/rest/").map_or(url, |i| &url[i..]);
	path.split('?').next().unwrap_or(path)
}

/// Runs SDK work in its own task, so a panic on an answer the SDK does not expect becomes an error
/// message instead of taking the window down.
async fn guarded<T: Send + 'static>(work: impl std::future::Future<Output = Result<T>> + Send + 'static) -> Result<T> {
	match tokio::spawn(work).await {
		Ok(Err(e)) => {
			trace(&format!("failed: {e:?}"));
			Err(e)
		},
		Ok(result) => result,
		Err(e) if e.is_panic() => Err(Error::Other("Tuta sent an answer Tern does not understand".into())),
		Err(_) => Err(Error::Other("Cancelled".into())),
	}
}

fn to_error(error: TernError) -> Error {
	match error {
		TernError::BadLogin => Error::BadLogin,
		TernError::SecondFactorRequired => Error::SecondFactorRequired,
		TernError::OldPasswordFormat => {
			Error::Other("This account still uses Tuta's old password format. Sign in once at app.tuta.com, then try again.".into())
		},
		TernError::SessionExpired => Error::SessionExpired,
		TernError::Offline => Error::Offline,
		TernError::RecipientsNotFound(addresses) => Error::RecipientsNotFound(addresses),
		TernError::Other(message) => Error::Other(message),
	}
}

struct TutaBackend {
	mail: Arc<TernMail>,
	account: Arc<TernAccount>,
}

impl TutaBackend {
	async fn new(mail: TernMail, login: &str) -> Result<Self> {
		let account = mail.account(login).await.map_err(to_error)?;
		trace("account loaded");
		Ok(Self { mail: Arc::new(mail), account: Arc::new(account) })
	}
}

fn id_string(id: &IdTupleGenerated) -> MailId {
	MailId { list: id.list_id.to_string(), element: id.element_id.to_string() }
}

fn id_tuple(id: &MailId) -> IdTupleGenerated {
	IdTupleGenerated { list_id: GeneratedId(id.list.clone()), element_id: GeneratedId(id.element.clone()) }
}

fn folder_kind(set: &MailSet) -> Option<FolderKind> {
	match MailSetKind::try_from(set.folderType as u64).ok()? {
		MailSetKind::Inbox => Some(FolderKind::Inbox),
		MailSetKind::Draft => Some(FolderKind::Drafts),
		MailSetKind::Sent => Some(FolderKind::Sent),
		MailSetKind::Archive => Some(FolderKind::Archive),
		MailSetKind::Spam => Some(FolderKind::Spam),
		MailSetKind::Trash => Some(FolderKind::Trash),
		MailSetKind::Custom => Some(FolderKind::Custom),
		// "All" and labels are views across folders; Tern shows folders only.
		MailSetKind::All | MailSetKind::Unknown => None,
	}
}

const FOLDER_ORDER: [FolderKind; 7] =
	[FolderKind::Inbox, FolderKind::Drafts, FolderKind::Sent, FolderKind::Archive, FolderKind::Spam, FolderKind::Trash, FolderKind::Custom];

fn folder(set: &MailSet) -> Option<Folder> {
	let kind = folder_kind(set)?;
	let name = match kind {
		FolderKind::Inbox => "Inbox".to_string(),
		FolderKind::Drafts => "Drafts".to_string(),
		FolderKind::Sent => "Sent".to_string(),
		FolderKind::Archive => "Archive".to_string(),
		FolderKind::Spam => "Spam".to_string(),
		FolderKind::Trash => "Trash".to_string(),
		FolderKind::Custom => set.name.clone(),
	};
	// The folder id carries the entry list (to page through) and the folder's own id (for links).
	let element = set._id.as_ref().map(|id| id.element_id.to_string()).unwrap_or_default();
	Some(Folder { id: FolderId(format!("{}/{}", set.entries, element)), name, kind, unread: 0 })
}

fn summary(mail: &TutaMail, me: &str) -> Option<MailSummary> {
	Some(MailSummary {
		to: mail.firstRecipient.as_ref().map(|a| Address::new(a.name.clone(), a.address.clone())),
		outgoing: mail.sender.address.eq_ignore_ascii_case(me),
		id: id_string(mail._id.as_ref()?),
		from: Address::new(mail.sender.name.clone(), mail.sender.address.clone()),
		subject: mail.subject.clone(),
		received_ms: mail.receivedDate.as_millis() as i64,
		unread: mail.unread,
		has_attachments: !mail.attachments.is_empty(),
	})
}

#[async_trait]
impl Backend for TutaBackend {
	fn account(&self) -> Account {
		Account { address: self.account.mail_address.clone(), name: self.account.name.clone() }
	}

	async fn folders(&self) -> Result<Vec<Folder>> {
		let mail = self.mail.clone();
		let sets = guarded(async move { mail.folders().await.map_err(to_error) }).await?;
		let order = |k: FolderKind| FOLDER_ORDER.iter().position(|&o| o == k);
		let mut folders: Vec<Folder> = sets.iter().filter_map(folder).collect();
		folders.sort_by(|a, b| order(a.kind).cmp(&order(b.kind)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
		Ok(folders)
	}

	async fn list(&self, folder: &FolderId, next: Option<String>, count: usize) -> Result<MailPage> {
		let entries = GeneratedId(folder.0.split('/').next().unwrap_or_default().to_string());
		let mail = self.mail.clone();
		let (items, next) = guarded(async move { mail.mail_page(&entries, next.map(CustomId), count).await.map_err(to_error) }).await?;
		let me = &self.account.mail_address;
		let mails = items
			.iter()
			.filter_map(|item| match item {
				PageItem::Mail(mail) => summary(mail, me),
				PageItem::Unreadable { id, received_ms } => Some(MailSummary {
					id: id_string(id),
					from: Address::new("Tuta", ""),
					subject: "Open this message in Tuta".into(),
					received_ms: *received_ms as i64,
					unread: false,
					has_attachments: false,
					to: None,
					outgoing: false,
				}),
			})
			.collect();
		Ok(MailPage { mails, next: next.map(|n| n.0) })
	}

	async fn open(&self, id: &MailId) -> Result<Mail> {
		let (mail, tuple) = (self.mail.clone(), id_tuple(id));
		let opened = guarded(async move { mail.open(&tuple).await.map_err(to_error) }).await?;
		let address = |a: &MailAddress| Address::new(a.name.clone(), a.address.clone());
		let body = &opened.details.body;
		let body_html = body.compressedText.clone().filter(|t| !t.is_empty()).or_else(|| body.text.clone()).unwrap_or_default();
		let folder = opened.mail.sets.first().map(|set| set.element_id.to_string()).unwrap_or_default();
		Ok(Mail {
			summary: summary(&opened.mail, &self.account.mail_address).ok_or_else(|| Error::Other("mail has no id".into()))?,
			to: opened.details.recipients.toRecipients.iter().map(address).collect(),
			cc: opened.details.recipients.ccRecipients.iter().map(address).collect(),
			reply_to: opened.details.replyTos.iter().map(|a| Address::new(a.name.clone(), a.address.clone())).collect(),
			body_html,
			attachments: opened
				.attachments
				.iter()
				.filter_map(|file| {
					let id = file._id.as_ref()?;
					Some(Attachment {
						id: format!("{}/{}", id.list_id, id.element_id),
						name: file.name.clone(),
						mime: file.mimeType.clone().unwrap_or_else(|| "application/octet-stream".into()),
						size: file.size.max(0) as u64,
						cid: file.cid.clone().filter(|c| !c.is_empty()),
					})
				})
				.collect(),
			web_url: format!("{BASE_URL}/mail/{folder}/{}", id.element),
		})
	}

	async fn attachment(&self, mail: &MailId, attachment: &Attachment) -> Result<Vec<u8>> {
		let (list, element) = attachment.id.split_once('/').ok_or_else(|| Error::Other("bad attachment id".into()))?;
		let file = IdTupleGenerated { list_id: GeneratedId(list.into()), element_id: GeneratedId(element.into()) };
		let (tern, mail) = (self.mail.clone(), id_tuple(mail));
		guarded(async move { tern.download(&mail, &file).await.map_err(to_error) }).await
	}

	async fn send(&self, draft: Draft) -> Result<()> {
		let recipients = |list: Vec<Address>| list.into_iter().map(|a| OutgoingRecipient { name: a.name, address: a.address }).collect();
		let previous = draft.previous.and_then(|(id, kind)| match kind {
			ReplyKind::Reply => Some((id_tuple(&id), CONVERSATION_REPLY)),
			ReplyKind::Forward => Some((id_tuple(&id), CONVERSATION_FORWARD)),
			ReplyKind::New => None,
		});
		let outgoing = OutgoingMail {
			to: recipients(draft.to),
			cc: recipients(draft.cc),
			bcc: recipients(draft.bcc),
			subject: draft.subject,
			body_html: draft.body_html,
			attachments: draft.attachments.into_iter().map(|a| OutgoingAttachment { name: a.name, mime_type: a.mime, data: a.data }).collect(),
			previous,
		};
		let (mail, account) = (self.mail.clone(), self.account.clone());
		guarded(async move { mail.send(&account, outgoing).await.map(|_| ()).map_err(to_error) }).await
	}

	async fn set_unread(&self, id: &MailId, unread: bool) -> Result<()> {
		let (mail, id) = (self.mail.clone(), id_tuple(id));
		guarded(async move { mail.set_unread(vec![id], unread).await.map_err(to_error) }).await
	}

	async fn trash(&self, id: &MailId) -> Result<()> {
		let (mail, id) = (self.mail.clone(), id_tuple(id));
		guarded(async move { mail.trash(vec![id]).await.map_err(to_error) }).await
	}

	async fn sign_out(&self) -> Result<()> {
		let mail = self.mail.clone();
		guarded(async move {
			mail.sign_out().await;
			Ok(())
		})
		.await
	}
}

/// Where the SDK caches Tuta's type models between runs, so start-up skips downloading them.
struct CacheFileClient {
	dir: PathBuf,
}

impl CacheFileClient {
	fn new() -> Self {
		let base = std::env::var_os("XDG_CACHE_HOME")
			.map(PathBuf::from)
			.or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
			.unwrap_or_else(std::env::temp_dir);
		Self { dir: base.join("tern").join("sdk") }
	}

	fn path(&self, name: &str) -> PathBuf {
		// Names come from the SDK; keep them inside the cache directory regardless.
		self.dir.join(name.replace(['/', '\\'], "_"))
	}
}

#[async_trait]
impl FileClient for CacheFileClient {
	async fn persist_content(&self, name: String, content: Vec<u8>) -> std::result::Result<(), FileClientError> {
		std::fs::create_dir_all(&self.dir).map_err(|e| FileClientError::from(e.kind()))?;
		std::fs::write(self.path(&name), content).map_err(|e| FileClientError::from(e.kind()))
	}

	async fn read_content(&self, name: String) -> std::result::Result<Vec<u8>, FileClientError> {
		std::fs::read(self.path(&name)).map_err(|e| FileClientError::from(e.kind()))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn ids_round_trip() {
		let id = MailId { list: "abc".into(), element: "def".into() };
		assert_eq!(id_string(&id_tuple(&id)), id);
	}

	#[test]
	fn cache_names_stay_inside_the_cache() {
		let client = CacheFileClient { dir: PathBuf::from("/tmp/x") };
		assert_eq!(client.path("../../etc/passwd"), PathBuf::from("/tmp/x/.._.._etc_passwd"));
	}

	#[tokio::test]
	async fn wrong_password_is_a_bad_login_over_http() {
		// Tuta answers an unknown address with a made-up salt, so this walks the whole sign-in:
		// salt, Argon2, and the session request the server turns down.
		if std::env::var_os("TERN_OFFLINE").is_some() {
			return;
		}
		match login("tern-no-such-user-7f3a@tutamail.com", "wrong").await {
			Err(Error::BadLogin) => {},
			Err(other) => panic!("expected BadLogin, got {other:?}"),
			Ok(_) => panic!("signed in to an account that should not exist"),
		}
	}

	#[tokio::test]
	#[ignore = "talks to app.tuta.com; set TERN_TEST_EMAIL and TERN_TEST_PASSWORD"]
	async fn live_round_trip_with_attachment_over_http() {
		let email = std::env::var("TERN_TEST_EMAIL").unwrap();
		let password = std::env::var("TERN_TEST_PASSWORD").unwrap();
		let (backend, saved) = login(&email, &password).await.unwrap();
		let resumed = resume(&saved).await.unwrap();
		assert_eq!(backend.account(), resumed.account());
		let folders = backend.folders().await.unwrap();
		let inbox = folders.iter().find(|f| f.kind == FolderKind::Inbox).unwrap();
		let sent = folders.iter().find(|f| f.kind == FolderKind::Sent).unwrap();
		backend.list(&inbox.id, None, 20).await.unwrap();

		let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
		let subject = format!("Tern self-test {stamp}");
		let data = b"%PDF-1.4 tern self test".to_vec();
		backend
			.send(Draft {
				to: vec![Address::new("", backend.account().address)],
				subject: subject.clone(),
				body_html: tern_core::text_to_html("Sent by Tern's live test."),
				attachments: vec![tern_core::NewAttachment { name: "test.pdf".into(), mime: "application/pdf".into(), data: data.clone() }],
				..Default::default()
			})
			.await
			.unwrap();

		// The copy in Sent is ours to read back.
		let page = backend.list(&sent.id, None, 20).await.unwrap();
		let mail = page.mails.iter().find(|m| m.subject == subject).expect("sent mail in Sent");
		let opened = backend.open(&mail.id).await.unwrap();
		assert!(opened.body_html.contains("live test"));
		assert_eq!(opened.attachments[0].name, "test.pdf");
		assert_eq!(backend.attachment(&mail.id, &opened.attachments[0]).await.unwrap(), data);
		backend.sign_out().await.unwrap();
	}
}
