//! Reading, downloading and sending mail for Tern, the GNOME client.
//!
//! Not part of upstream Tuta. Ported from the TypeScript client's MailFacade, BlobFacade,
//! BlobAccessTokenFacade, CryptoFacade and LoginFacade at tutao/tutanota@73a3e0e9, which the rest
//! of this SDK copy also comes from. See vendor/tuta-sdk/PATCHES.md.

use std::collections::HashMap;
use std::sync::Arc;

use base64::prelude::BASE64_URL_SAFE_NO_PAD;
use base64::Engine;
use crypto_primitives::aes::{Aes256Key, InitializationVector};
use crypto_primitives::key::GenericAesKey;
use crypto_primitives::randomizer_facade::RandomizerFacade;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::bindings::rest_client::{encode_query_params, HttpMethod, RestClient, RestClientOptions};
use crate::blobs::blob_facade::FileData;
#[cfg_attr(test, mockall_double::double)]
use crate::crypto::asymmetric_crypto_facade::AsymmetricCryptoFacade;
use crate::crypto::crypto_facade::create_auth_verifier;
#[cfg_attr(test, mockall_double::double)]
use crate::crypto::crypto_facade::CryptoFacade;
use crate::crypto::crypto_facade::ResolvedSessionKey;
#[cfg_attr(test, mockall_double::double)]
use crate::crypto::public_key_provider::PublicKeyProvider;
use crate::crypto::asymmetric_crypto_facade::PubEncSymKey;
use crate::crypto::public_key_provider::{PublicKeyIdentifier, PublicKeyLoadingError};
use crate::element_value::{ElementValue, ParsedEntity};
use crate::entities::entity_facade::{EntityFacade, EntityFacadeImpl};
use crate::entities::generated::storage::{
	BlobAccessTokenPostIn, BlobGetIn, BlobId, BlobReadData, BlobServerAccessInfo, InstanceId,
};
use crate::entities::generated::sys::{
	BucketKey, CreateSessionData, GroupInfo, SaltData,
};
use crate::entities::generated::tutanota::{
	AttachmentKeyData, ConversationEntry, DraftAttachment, DraftCreateData, DraftCreateReturn,
	DraftData, DraftRecipient, InternalRecipientKeyData, Mail, MailDetails, MailDetailsBlob,
	MailDetailsDraft, MailSet, MailSetEntry, NewDraftAttachment, SendDraftData,
	SendDraftParameters, SendDraftReturn, TutanotaFile,
};
use crate::entities::Entity;
#[cfg_attr(test, mockall_double::double)]
use crate::key_loader_facade::KeyLoaderFacade;
use crate::login::login_facade::{derive_user_passphrase_key, parse_session_id, KdfType};
use crate::login::{CredentialType, Credentials, LoginError};
use crate::rest_error::HttpError;
use crate::services::generated::storage::BlobAccessTokenService;
use crate::services::generated::sys::{SaltService, SessionService};
use crate::services::generated::tutanota::{DraftService, SendDraftService};
use crate::services::{ExtraServiceParams, Service};
use crate::tutanota_constants::{
	ArchiveDataType, CryptoProtocolVersion, PublicKeyIdentifierType,
};
use crate::json_serializer::InstanceMapperError;
use crate::groups::GroupType;
use crate::metamodel::TypeModel;
use crate::{
	ApiCallError, CustomId, GeneratedId, HeadersProvider, IdTupleCustom, IdTupleGenerated,
	ListLoadDirection, LoggedInSdk, Sdk, TypeRef,
};

/// What Tern keeps from login() that the SDK otherwise drops.
pub(crate) struct TernParts {
	pub key_loader_facade: Arc<KeyLoaderFacade>,
	pub asymmetric_crypto_facade: Arc<AsymmetricCryptoFacade>,
	pub crypto_facade: Arc<CryptoFacade>,
	pub public_key_provider: Arc<PublicKeyProvider>,
	pub rest_client: Arc<dyn RestClient>,
	pub base_url: String,
	pub auth_headers_provider: Arc<HeadersProvider>,
	pub access_token: String,
}

#[derive(Debug, thiserror::Error)]
pub enum TernError {
	#[error("wrong email address or password")]
	BadLogin,
	#[error("two-factor authentication required")]
	SecondFactorRequired,
	#[error("this account still uses the old password format")]
	OldPasswordFormat,
	#[error("session expired")]
	SessionExpired,
	#[error("network error")]
	Offline,
	#[error("recipients not found: {0:?}")]
	RecipientsNotFound(Vec<String>),
	#[error("{0}")]
	Other(String),
}

impl From<ApiCallError> for TernError {
	fn from(error: ApiCallError) -> Self {
		match error {
			ApiCallError::RestClient { .. } => TernError::Offline,
			ApiCallError::ServerResponseError {
				source: HttpError::NotAuthenticatedError | HttpError::SessionExpiredError,
			} => TernError::SessionExpired,
			ApiCallError::ServerResponseError {
				source: HttpError::ConnectionError | HttpError::ServiceUnavailableError { .. },
			} => TernError::Offline,
			other => TernError::Other(other.to_string()),
		}
	}
}

impl From<InstanceMapperError> for TernError {
	fn from(error: InstanceMapperError) -> Self {
		ApiCallError::from(error).into()
	}
}

impl From<LoginError> for TernError {
	fn from(error: LoginError) -> Self {
		match error {
			LoginError::ApiCall { source } => source.into(),
			LoginError::InvalidAccessToken { .. } => TernError::SessionExpired,
			LoginError::InvalidPassphrase { .. } => TernError::BadLogin,
			other => TernError::Other(other.to_string()),
		}
	}
}

/// The domains of Tuta's own addresses. A recipient here with no public key does not exist,
/// rather than being someone outside Tuta.
pub const TUTA_MAIL_ADDRESS_DOMAINS: [&str; 6] = [
	"tuta.com",
	"tutamail.com",
	"tuta.io",
	"tutanota.com",
	"tutanota.de",
	"keemail.me",
];

/// TypeScript's ConversationType.
pub const CONVERSATION_NEW: i64 = 0;
pub const CONVERSATION_REPLY: i64 = 1;
pub const CONVERSATION_FORWARD: i64 = 2;

/// EncryptionAuthStatus.TUTACRYPT_SENDER: the mail was sent by us with TutaCrypt.
const ENCRYPTION_AUTH_STATUS_TUTACRYPT_SENDER: &str = "4";

/// Ids per request; more and the server answers 414.
const MAX_IDS_PER_REQUEST: usize = 100;

/// The body a draft may carry before Tuta refuses it.
const UNCOMPRESSED_MAX_SIZE: usize = 1024 * 1024;

impl Sdk {
	/// Signs in with a password and returns the credentials to resume the session later.
	///
	/// Like [Sdk::create_session], but checks the password format and second factor first and
	/// hands back the credentials instead of dropping them.
	pub async fn tern_sign_in(
		&self,
		mail_address: &str,
		passphrase: &str,
		client_identifier: &str,
	) -> Result<(Arc<LoggedInSdk>, Credentials), TernError> {
		let headers_provider = Arc::new(HeadersProvider::new(None));
		let entity_facade = Arc::new(EntityFacadeImpl::new(
			self.type_model_provider.clone(),
			RandomizerFacade::from_core(rand_core::OsRng),
		));
		let service_executor = crate::services::service_executor::ServiceExecutor::new(
			headers_provider,
			None,
			entity_facade,
			self.instance_mapper.clone(),
			self.json_serializer.clone(),
			self.rest_client.clone(),
			self.type_model_provider.clone(),
			self.base_url.to_string(),
		);
		let salt = service_executor
			.get::<SaltService>(
				SaltData {
					_format: 0,
					mailAddress: mail_address.to_string(),
				},
				ExtraServiceParams::default(),
			)
			.await
			.map_err(|e| match e {
				ApiCallError::ServerResponseError {
					source: HttpError::NotAuthenticatedError | HttpError::NotFoundError,
				} => TernError::BadLogin,
				e => e.into(),
			})?;
		match KdfType::try_from(salt.kdfVersion) {
			Ok(KdfType::Argon2id) => {},
			Ok(KdfType::Bcrypt) => return Err(TernError::OldPasswordFormat),
			Err(_) => return Err(TernError::Other(format!("unknown password format {}", salt.kdfVersion))),
		}
		let salt: [u8; 16] = salt
			.salt
			.try_into()
			.map_err(|_| TernError::Other("salt has wrong length".into()))?;

		let randomizer = RandomizerFacade::from_core(rand_core::OsRng);
		let access_key = Aes256Key::generate(&randomizer);
		let user_passphrase_key = derive_user_passphrase_key(KdfType::Argon2id, passphrase, salt);
		let session = service_executor
			.post::<SessionService>(
				CreateSessionData {
					_format: 0,
					accessKey: Some(access_key.as_bytes().to_vec()),
					authToken: None,
					authVerifier: Some(create_auth_verifier(user_passphrase_key.clone())),
					clientIdentifier: client_identifier.to_string(),
					mailAddress: Some(mail_address.to_string()),
					recoverCodeVerifier: None,
					user: None,
				},
				ExtraServiceParams::default(),
			)
			.await
			.map_err(|e| match e {
				ApiCallError::ServerResponseError {
					source: HttpError::NotAuthenticatedError,
				} => TernError::BadLogin,
				e => e.into(),
			})?;
		if !session.challenges.is_empty() {
			// The session waits for a second factor we cannot give; drop it again.
			delete_session(self.rest_client.as_ref(), &self.base_url, &session.accessToken).await;
			return Err(TernError::SecondFactorRequired);
		}

		let credentials = Credentials {
			login: mail_address.to_string(),
			user_id: session.user,
			access_token: session.accessToken,
			encrypted_passphrase_key: GenericAesKey::Aes256(access_key).encrypt_key(
				&GenericAesKey::Aes256(user_passphrase_key),
				InitializationVector::generate(&randomizer),
			),
			credential_type: CredentialType::Internal,
		};
		let sdk = self.login(credentials.clone()).await?;
		Ok((sdk, credentials))
	}
}

/// Deletes the session behind `access_token` on the server. Best effort: a failure leaves a
/// session that expires on its own.
async fn delete_session(rest_client: &dyn RestClient, base_url: &str, access_token: &str) {
	let Ok(IdTupleCustom {
		list_id,
		element_id,
	}) = parse_session_id(access_token)
	else {
		return;
	};
	let headers = HeadersProvider::new(Some(access_token.to_string()))
		.provide_headers(SESSION_MODEL_VERSION);
	let url = format!("{base_url}/rest/sys/session/{list_id}/{element_id}");
	let _ = rest_client
		.request_binary(
			url,
			HttpMethod::DELETE,
			RestClientOptions {
				headers,
				body: None,
				suspension_behavior: None,
			},
		)
		.await;
}

/// The sys model version the session service and Session type belong to.
const SESSION_MODEL_VERSION: u64 = <SessionService as Service>::VERSION;

/// The signed-in account, as Tern shows it.
pub struct TernAccount {
	pub mail_address: String,
	pub name: String,
	pub mail_group: GeneratedId,
}

/// A mail opened for reading: the mail, its details and its attachments, all decrypted.
pub struct OpenedMail {
	pub mail: Mail,
	pub details: MailDetails,
	pub attachments: Vec<TutanotaFile>,
}

/// One row of a folder page.
pub enum PageItem {
	Mail(Mail),
	/// A mail Tern cannot decrypt, e.g. from a password-protected external mailbox.
	Unreadable { id: IdTupleGenerated, received_ms: u64 },
}

pub struct OutgoingAttachment {
	pub name: String,
	pub mime_type: String,
	pub data: Vec<u8>,
}

pub struct OutgoingRecipient {
	pub name: String,
	pub address: String,
}

pub struct OutgoingMail {
	pub to: Vec<OutgoingRecipient>,
	pub cc: Vec<OutgoingRecipient>,
	pub bcc: Vec<OutgoingRecipient>,
	pub subject: String,
	pub body_html: String,
	pub attachments: Vec<OutgoingAttachment>,
	/// The mail this answers or forwards, with CONVERSATION_REPLY or CONVERSATION_FORWARD.
	pub previous: Option<(IdTupleGenerated, i64)>,
}

/// Session keys of the instances a bucket key covers, by instance element id.
type InstanceKeys = HashMap<GeneratedId, GenericAesKey>;

pub struct TernMail {
	sdk: Arc<LoggedInSdk>,
	entity_facade: EntityFacadeImpl,
	randomizer: RandomizerFacade,
}

impl LoggedInSdk {
	#[must_use]
	pub fn tern(self: &Arc<Self>) -> TernMail {
		TernMail {
			sdk: self.clone(),
			entity_facade: EntityFacadeImpl::new(
				self.type_model_provider.clone(),
				RandomizerFacade::from_core(rand_core::OsRng),
			),
			randomizer: RandomizerFacade::from_core(rand_core::OsRng),
		}
	}
}

impl TernMail {
	fn parts(&self) -> &TernParts {
		&self.sdk.tern
	}

	/// The address and name of the signed-in mailbox.
	pub async fn account(&self, login: &str) -> Result<TernAccount, TernError> {
		let user = self.sdk.user_facade.get_user();
		let membership = user
			.memberships
			.iter()
			.find(|m| m.group_type() == GroupType::Mail)
			.ok_or_else(|| TernError::Other("this account has no mailbox".into()))?;
		let group_info: Option<GroupInfo> = self
			.sdk
			.crypto_entity_client
			.load(&membership.groupInfo)
			.await
			.ok();
		Ok(TernAccount {
			mail_address: group_info
				.as_ref()
				.and_then(|g| g.mailAddress.clone())
				.unwrap_or_else(|| login.to_string()),
			name: group_info.map(|g| g.name).unwrap_or_default(),
			mail_group: membership.group.clone(),
		})
	}

	pub async fn folders(&self) -> Result<Vec<MailSet>, TernError> {
		let facade = self.sdk.mail_facade();
		let mailbox = facade.load_user_mailbox().await?;
		let folders = facade.load_folders_for_mailbox(&mailbox).await?;
		Ok(folders.into_folders())
	}

	/// A page of a folder, newest first, starting after `after` (an entry id from a previous page).
	/// Returns the mails and the entry id to continue from, if there may be more.
	pub async fn mail_page(
		&self,
		entries: &GeneratedId,
		after: Option<CustomId>,
		count: usize,
	) -> Result<(Vec<PageItem>, Option<CustomId>), TernError> {
		let start = after.unwrap_or_else(custom_max_id);
		let entries: Vec<MailSetEntry> = self
			.sdk
			.typed_entity_client
			.load_range(entries, &start, count, ListLoadDirection::DESC)
			.await?;
		let next = if entries.len() == count {
			entries.last().and_then(|e| e._id.as_ref()).map(|id| id.element_id.clone())
		} else {
			None
		};

		// Entries come newest first; mails may sit in different lists (mail bags).
		let mut by_list: Vec<(GeneratedId, Vec<GeneratedId>)> = Vec::new();
		for entry in &entries {
			match by_list.iter_mut().find(|(list, _)| *list == entry.mail.list_id) {
				Some((_, ids)) => ids.push(entry.mail.element_id.clone()),
				None => by_list.push((entry.mail.list_id.clone(), vec![entry.mail.element_id.clone()])),
			}
		}
		let mut loaded: HashMap<GeneratedId, PageItem> = HashMap::new();
		let mail_model = self.sdk.entity_client.resolve_server_type_ref(&Mail::type_ref())?;
		for (list, ids) in by_list {
			for chunk in ids.chunks(MAX_IDS_PER_REQUEST) {
				for raw in self.load_multiple_raw(&Mail::type_ref(), &list, chunk).await? {
					let Some(element) = element_id(&mail_model, &raw) else { continue };
					let received = received_ms(&mail_model, &raw);
					// A mail we cannot decrypt still gets a row, rather than vanishing or failing the folder.
					let item = match self.decrypt::<Mail>(raw, None).await {
						Ok(mail) => PageItem::Mail(mail),
						Err(e) => {
							log::warn!("mail {element} does not decrypt: {e}");
							PageItem::Unreadable {
								id: IdTupleGenerated { list_id: list.clone(), element_id: element.clone() },
								received_ms: received,
							}
						},
					};
					loaded.insert(element, item);
				}
			}
		}
		let mails = entries
			.iter()
			.filter_map(|e| loaded.remove(&e.mail.element_id))
			.collect();
		Ok((mails, next))
	}

	/// Loads a mail with its body and attachment list.
	pub async fn open(&self, id: &IdTupleGenerated) -> Result<OpenedMail, TernError> {
		let raw = self.sdk.entity_client.load(&Mail::type_ref(), id).await?;
		let instance_keys = self.bucket_instance_keys(&Mail::type_ref(), &raw).await?;
		let mail_key = self.session_key(&Mail::type_ref(), &raw, instance_keys.as_ref()).await?;
		let mail: Mail = self.decrypt_with(raw, mail_key.clone())?;

		let details = if let Some(details_id) = &mail.mailDetails {
			let raw = self.load_mail_details_blob(details_id).await?;
			let key = instance_keys
				.as_ref()
				.and_then(|keys| keys.get(&details_id.element_id).cloned())
				.unwrap_or_else(|| mail_key.clone());
			self.decrypt_with::<MailDetailsBlob>(raw, key)?.details
		} else if let Some(draft_id) = &mail.mailDetailsDraft {
			let raw = self
				.sdk
				.entity_client
				.load(&MailDetailsDraft::type_ref(), draft_id)
				.await?;
			self.decrypt_with::<MailDetailsDraft>(raw, mail_key.clone())?
				.details
		} else {
			return Err(TernError::Other("mail has no body".into()));
		};

		let attachments = self.load_attachments(&mail, instance_keys.as_ref()).await?;
		Ok(OpenedMail {
			mail,
			details,
			attachments,
		})
	}

	async fn load_attachments(
		&self,
		mail: &Mail,
		instance_keys: Option<&InstanceKeys>,
	) -> Result<Vec<TutanotaFile>, TernError> {
		let Some(first) = mail.attachments.first() else {
			return Ok(Vec::new());
		};
		let ids: Vec<GeneratedId> = mail.attachments.iter().map(|a| a.element_id.clone()).collect();
		let mut files = Vec::with_capacity(ids.len());
		for chunk in ids.chunks(MAX_IDS_PER_REQUEST) {
			for raw in self
				.load_multiple_raw(&TutanotaFile::type_ref(), &first.list_id, chunk)
				.await?
			{
				files.push(self.decrypt::<TutanotaFile>(raw, instance_keys).await?);
			}
		}
		Ok(files)
	}

	/// Downloads and decrypts one attachment of a mail.
	pub async fn download(
		&self,
		mail_id: &IdTupleGenerated,
		file_id: &IdTupleGenerated,
	) -> Result<Vec<u8>, TernError> {
		let raw_file = self.sdk.entity_client.load(&TutanotaFile::type_ref(), file_id).await?;
		let raw_mail = self.sdk.entity_client.load(&Mail::type_ref(), mail_id).await?;
		let instance_keys = self.bucket_instance_keys(&Mail::type_ref(), &raw_mail).await?;
		let key = self
			.session_key(&TutanotaFile::type_ref(), &raw_file, instance_keys.as_ref())
			.await?;
		let file: TutanotaFile = self.decrypt_with(raw_file, key.clone())?;
		let Some(first_blob) = file.blobs.first() else {
			return Ok(Vec::new());
		};
		let access = self
			.read_token(
				Some(ArchiveDataType::Attachments),
				&first_blob.archiveId,
				Some((&file_id.list_id, &file_id.element_id)),
			)
			.await?;

		// The size comes from the server; trust it only as a hint.
		let mut out = Vec::with_capacity((file.size.max(0) as usize).min(64 * 1024 * 1024));
		let blob_ids: Vec<&GeneratedId> = file.blobs.iter().map(|b| &b.blobId).collect();
		for chunk in blob_ids.chunks(MAX_IDS_PER_REQUEST) {
			let get = blob_get_in(&self.randomizer, &first_blob.archiveId, chunk);
			let body = self.to_json(get)?;
			let mut params = self.blob_query_params(&access, BlobGetIn::type_ref())?;
			params.push(("_body".to_string(), body));
			let bytes = self
				.try_servers(&access, |server| {
					format!("{server}/rest/storage/blobservice{}", encode_query_params(params.clone()))
				})
				.await?;
			let blobs = parse_multiple_blobs(&bytes)?;
			for id in chunk {
				let encrypted = blobs
					.get(id.as_str())
					.ok_or_else(|| TernError::Other(format!("server did not return blob {id}")))?;
				let plain = key
					.decrypt_data(encrypted)
					.map_err(|e| TernError::Other(format!("cannot decrypt attachment: {e}")))?;
				out.extend_from_slice(&plain);
			}
		}
		Ok(out)
	}

	pub async fn set_unread(&self, mails: Vec<IdTupleGenerated>, unread: bool) -> Result<(), TernError> {
		Ok(self.sdk.mail_facade().set_unread_status_for_mails(mails, unread).await?)
	}

	pub async fn trash(&self, mails: Vec<IdTupleGenerated>) -> Result<(), TernError> {
		Ok(self.sdk.mail_facade().trash_mails(mails).await?)
	}

	pub async fn sign_out(&self) {
		let parts = self.parts();
		delete_session(parts.rest_client.as_ref(), &parts.base_url, &parts.access_token).await;
	}

	/// Creates a draft with the attachments uploaded, then sends it. End-to-end encrypted when every
	/// recipient is on Tuta; otherwise sent as ordinary mail, like the official client does when
	/// "confidential" is off.
	pub async fn send(&self, sender: &TernAccount, outgoing: OutgoingMail) -> Result<SendDraftReturn, TernError> {
		if outgoing.body_html.len() > UNCOMPRESSED_MAX_SIZE {
			return Err(TernError::Other("the message is too long".into()));
		}
		// One key per address, however often it appears across To, Cc and Bcc.
		let mut all: Vec<&OutgoingRecipient> = Vec::new();
		for recipient in outgoing.to.iter().chain(&outgoing.cc).chain(&outgoing.bcc) {
			if !all.iter().any(|r| r.address.eq_ignore_ascii_case(&recipient.address)) {
				all.push(recipient);
			}
		}
		if all.is_empty() {
			return Err(TernError::Other("no recipients".into()));
		}

		// Who is on Tuta decides whether the mail can be end-to-end encrypted.
		let mut public_keys = Vec::with_capacity(all.len());
		let mut not_found = Vec::new();
		for recipient in &all {
			let identifier = PublicKeyIdentifier {
				identifier: recipient.address.clone(),
				identifier_type: PublicKeyIdentifierType::MailAddress,
			};
			match self.parts().public_key_provider.load_current_pub_key(&identifier).await {
				Ok(key) => public_keys.push(Some(key)),
				Err(PublicKeyLoadingError::KeyLoadingError(ApiCallError::ServerResponseError {
					source: HttpError::NotFoundError,
				})) => {
					if is_tuta_address(&recipient.address) {
						not_found.push(recipient.address.clone());
					}
					public_keys.push(None);
				},
				Err(PublicKeyLoadingError::KeyLoadingError(e)) => return Err(e.into()),
				Err(e) => return Err(TernError::Other(format!("cannot use the recipient's key: {e:?}"))),
			}
		}
		if !not_found.is_empty() {
			return Err(TernError::RecipientsNotFound(not_found));
		}
		let confidential = public_keys.iter().all(Option::is_some);

		let mail_group = sender.mail_group.clone();
		let group_key = self
			.parts()
			.key_loader_facade
			.get_current_sym_group_key(&mail_group)
			.await
			.map_err(|e| TernError::Other(format!("cannot load the mailbox key: {e}")))?;
		let iv = || InitializationVector::generate(&self.randomizer);

		let mail_key = GenericAesKey::Aes256(Aes256Key::generate(&self.randomizer));
		let owner_enc_mail_key = group_key.object.encrypt_key(&mail_key, iv());
		let encrypt_string = |key: &GenericAesKey, value: &str| {
			key.encrypt_data(value.as_bytes(), iv())
				.map_err(|e| TernError::Other(format!("cannot encrypt: {e}")))
		};

		// Upload the attachments, each under its own key.
		let file_keys: Vec<GenericAesKey> = outgoing
			.attachments
			.iter()
			.map(|_| GenericAesKey::Aes256(Aes256Key::generate(&self.randomizer)))
			.collect();
		let file_data: Vec<FileData> = outgoing
			.attachments
			.iter()
			.zip(&file_keys)
			.map(|(a, key)| FileData {
				session_key: key.clone(),
				data: a.data.as_slice(),
			})
			.collect();
		let tokens = if file_data.is_empty() {
			Vec::new()
		} else {
			self.sdk
				.blob_facade()
				.encrypt_and_upload_multiple(ArchiveDataType::Attachments, &mail_group, file_data.iter())
				.await?
		};
		let mut added_attachments = Vec::with_capacity(tokens.len());
		for ((attachment, key), reference_tokens) in outgoing.attachments.iter().zip(&file_keys).zip(tokens) {
			added_attachments.push(DraftAttachment {
				_id: None,
				ownerEncFileSessionKey: group_key.object.encrypt_key(key, iv()),
				ownerKeyVersion: group_key.version as i64,
				existingFile: None,
				newFile: Some(NewDraftAttachment {
					_id: None,
					encFileName: encrypt_string(key, &attachment.name)?,
					encMimeType: encrypt_string(key, &attachment.mime_type)?,
					encCid: None,
					referenceTokens: reference_tokens,
				}),
			});
		}

		let recipient = |r: &OutgoingRecipient| DraftRecipient {
			_id: None,
			name: r.name.clone(),
			mailAddress: r.address.clone(),
			_errors: Default::default(),
		};
		let (previous_message_id, conversation_type) = match &outgoing.previous {
			Some((previous, kind)) => (self.previous_message_id(previous).await, *kind),
			None => (None, CONVERSATION_NEW),
		};
		let draft = DraftCreateData {
			_format: 0,
			previousMessageId: previous_message_id,
			conversationType: conversation_type,
			ownerEncSessionKey: owner_enc_mail_key,
			ownerKeyVersion: group_key.version as i64,
			draftData: DraftData {
				_id: None,
				subject: outgoing.subject.clone(),
				bodyText: String::new(),
				senderMailAddress: sender.mail_address.clone(),
				senderName: sender.name.clone(),
				confidential,
				method: 0,
				compressedBodyText: Some(outgoing.body_html.clone()),
				toRecipients: outgoing.to.iter().map(recipient).collect(),
				ccRecipients: outgoing.cc.iter().map(recipient).collect(),
				bccRecipients: outgoing.bcc.iter().map(recipient).collect(),
				addedAttachments: added_attachments,
				removedAttachments: vec![],
				replyTos: vec![],
				_errors: Default::default(),
			},
			_errors: Default::default(),
		};
		let created: DraftCreateReturn = self
			.sdk
			.get_service_executor()
			.post::<DraftService>(
				draft,
				ExtraServiceParams {
					session_key: Some(mail_key.clone()),
					..Default::default()
				},
			)
			.await?;

		// Every attachment's key goes along, wrapped for the recipients.
		let raw_draft = self.sdk.entity_client.load(&Mail::type_ref(), &created.draft).await?;
		let draft_mail: Mail = self.decrypt_with(raw_draft, mail_key.clone())?;
		let bucket_key = GenericAesKey::Aes256(Aes256Key::generate(&self.randomizer));
		let mut attachment_key_data = Vec::with_capacity(draft_mail.attachments.len());
		for file_id in &draft_mail.attachments {
			let raw_file = self.sdk.entity_client.load(&TutanotaFile::type_ref(), file_id).await?;
			let file_key = self.session_key(&TutanotaFile::type_ref(), &raw_file, None).await?;
			attachment_key_data.push(AttachmentKeys {
				file: file_id.clone(),
				bucket_enc_file_session_key: confidential.then(|| bucket_key.encrypt_key(&file_key, iv())),
				file_session_key: (!confidential).then(|| file_key.as_bytes().to_vec()),
			});
		}

		let mut internal_recipient_key_data = Vec::new();
		let user_group = self.sdk.user_facade.get_user_group_id();
		if confidential {
			for (recipient, key) in all.iter().zip(public_keys) {
				let key = key.expect("confidential means every recipient has a key");
				let encrypted = self
					.parts()
					.asymmetric_crypto_facade
					.asym_encrypt_sym_key(bucket_key.clone(), key, &user_group)
					.await
					.map_err(|e| TernError::Other(format!("cannot encrypt for {}: {e}", recipient.address)))?;
				internal_recipient_key_data.push((recipient.address.clone(), encrypted));
			}
		}
		let tuta_crypt_only = confidential
			&& internal_recipient_key_data
				.iter()
				.all(|(_, key)| key.crypto_protocol_version == CryptoProtocolVersion::TutaCrypt);
		let session_enc_auth_status = tuta_crypt_only.then(|| {
			mail_key
				.encrypt_data(ENCRYPTION_AUTH_STATUS_TUTACRYPT_SENDER.as_bytes(), iv())
				.expect("encrypting a short string cannot fail")
		});
		let bucket_enc_mail_session_key = confidential.then(|| bucket_key.encrypt_key(&mail_key, iv()));
		let mail_session_key = (!confidential).then(|| mail_key.as_bytes().to_vec());

		let send = send_draft_data(
			&self.randomizer,
			created.draft,
			SendKeys {
				mail_session_key,
				bucket_enc_mail_session_key,
				session_enc_auth_status,
				recipients: internal_recipient_key_data,
				attachments: attachment_key_data,
			},
		);
		self.sdk
			.get_service_executor()
			.post::<SendDraftService>(send, ExtraServiceParams::default())
			.await
			.map_err(|e| match e {
				ApiCallError::ServerResponseError {
					source: source @ (HttpError::NotFoundError | HttpError::PreconditionFailedError(_)),
				} => TernError::Other(format!("Tuta refused to send the mail ({source})")),
				e => e.into(),
			})
	}

	/// The Message-ID of the mail being answered, so the reply joins its conversation.
	async fn previous_message_id(&self, previous: &IdTupleGenerated) -> Option<String> {
		let raw = self.sdk.entity_client.load(&Mail::type_ref(), previous).await.ok()?;
		let mail: Mail = self.decrypt(raw, None).await.ok()?;
		let entry: ConversationEntry = self
			.sdk
			.typed_entity_client
			.load(&mail.conversationEntry)
			.await
			.ok()?;
		Some(entry.messageId)
	}

	/// For an instance carrying a bucket key (mail as received), decrypts the bucket key and returns
	/// the session keys of every instance it covers: the mail, its details and its attachments.
	async fn bucket_instance_keys(&self, type_ref: &TypeRef, raw: &ParsedEntity) -> Result<Option<InstanceKeys>, TernError> {
		let type_model = self.sdk.entity_client.resolve_server_type_ref(type_ref)?;
		let Ok(attribute) = type_model.get_attribute_id_by_attribute_name("bucketKey") else {
			return Ok(None);
		};
		let Some(ElementValue::Array(values)) = raw.get(&attribute) else {
			return Ok(None);
		};
		let Some(ElementValue::Dict(bucket_key)) = values.first() else {
			return Ok(None);
		};
		let bucket_key: BucketKey = self
			.sdk
			.instance_mapper
			.parse_entity(bucket_key.clone())
			.map_err(|e| TernError::Other(format!("bad bucket key: {e}")))?;
		let (Some(key_group), Some(pub_enc_bucket_key)) = (&bucket_key.keyGroup, &bucket_key.pubEncBucketKey) else {
			// Symmetrically wrapped bucket keys come from secure external mailboxes.
			return Err(TernError::Other("Tern cannot open mail from password-protected external mailboxes yet".into()));
		};
		let protocol = CryptoProtocolVersion::try_from(bucket_key.protocolVersion)
			.map_err(|_| TernError::Other(format!("unknown protocol {}", bucket_key.protocolVersion)))?;
		let decapsulated = self
			.parts()
			.asymmetric_crypto_facade
			.load_key_pair_and_decrypt_sym_key(
				key_group,
				bucket_key.recipientKeyVersion as u64,
				&protocol,
				pub_enc_bucket_key,
			)
			.await
			.map_err(|e| TernError::Other(format!("cannot decrypt the mail key: {e}")))?;
		let mut keys = HashMap::new();
		for instance in bucket_key.bucketEncSessionKeys {
			let key = decapsulated
				.decrypted_aes_key
				.decrypt_aes_key(&instance.symEncSessionKey)
				.map_err(|e| TernError::Other(format!("cannot decrypt a session key: {e}")))?;
			keys.insert(instance.instanceId, key);
		}
		Ok(Some(keys))
	}

	/// The session key of `raw`: from the bucket keys if they cover it, else from its owner group.
	async fn session_key(&self, type_ref: &TypeRef, raw: &ParsedEntity, instance_keys: Option<&InstanceKeys>) -> Result<GenericAesKey, TernError> {
		// Resolve bucket keys here rather than in the SDK, which panics on secure external ones.
		let computed;
		let instance_keys = match instance_keys {
			Some(keys) => Some(keys),
			None => {
				computed = self.bucket_instance_keys(type_ref, raw).await?;
				computed.as_ref()
			},
		};
		let type_model = self.sdk.entity_client.resolve_server_type_ref(type_ref)?;
		if let (Some(keys), Some(id)) = (instance_keys, element_id(&type_model, raw)) {
			if let Some(key) = keys.get(&id) {
				return Ok(key.clone());
			}
		}
		self.parts()
			.crypto_facade
			.resolve_session_key(raw, &type_model)
			.await
			.map_err(|e| TernError::Other(format!("cannot resolve the session key: {e:?}")))?
			.map(|resolved| resolved.session_key)
			.ok_or_else(|| TernError::Other(format!("{type_ref} is not encrypted")))
	}

	async fn decrypt<T: Entity + DeserializeOwned>(&self, raw: ParsedEntity, instance_keys: Option<&InstanceKeys>) -> Result<T, TernError> {
		let type_ref = T::type_ref();
		let key = self.session_key(&type_ref, &raw, instance_keys).await?;
		self.decrypt_with(raw, key)
	}

	fn decrypt_with<T: Entity + DeserializeOwned>(&self, raw: ParsedEntity, session_key: GenericAesKey) -> Result<T, TernError> {
		let type_ref = T::type_ref();
		let type_model = self.sdk.entity_client.resolve_server_type_ref(&type_ref)?;
		let decrypted = self.entity_facade.decrypt_and_map(
			&type_model,
			raw,
			ResolvedSessionKey {
				session_key,
				owner_enc_session_key: Vec::new(),
				owner_key_version: 0,
				sender_identity_pub_key: None,
			},
		)?;
		self.sdk
			.instance_mapper
			.parse_entity::<T>(decrypted)
			.map_err(|e| TernError::Other(format!("cannot read {type_ref}: {e}")))
	}

	fn to_json<T: Entity + Serialize>(&self, data: T) -> Result<String, TernError> {
		let type_ref = T::type_ref();
		let parsed = self
			.sdk
			.instance_mapper
			.serialize_entity(data)
			.map_err(|e| TernError::Other(format!("cannot serialize {type_ref}: {e}")))?;
		let raw = self.sdk.json_serializer.serialize(&type_ref, parsed)?;
		serde_json::to_string(&raw).map_err(|e| TernError::Other(e.to_string()))
	}

	/// Loads several list elements in one request, still encrypted.
	async fn load_multiple_raw(&self, type_ref: &TypeRef, list: &GeneratedId, ids: &[GeneratedId]) -> Result<Vec<ParsedEntity>, TernError> {
		let type_model = self.sdk.entity_client.resolve_client_type_ref(type_ref)?;
		let parts = self.parts();
		let joined = ids.iter().map(GeneratedId::as_str).collect::<Vec<_>>().join(",");
		let url = format!(
			"{}/rest/{}/{}/{}{}",
			parts.base_url,
			type_ref.app,
			type_model.name.to_lowercase(),
			list,
			encode_query_params([("ids", joined.as_str())])
		);
		let headers = parts.auth_headers_provider.provide_headers(type_model.version);
		let response = parts
			.rest_client
			.request_binary(
				url,
				HttpMethod::GET,
				RestClientOptions {
					headers,
					body: None,
					suspension_behavior: None,
				},
			)
			.await
			.map_err(ApiCallError::from)?;
		self.parse_list_response(type_ref, response.status, &response.headers, response.body)
	}

	fn parse_list_response(
		&self,
		type_ref: &TypeRef,
		status: u32,
		headers: &HashMap<String, String>,
		body: Option<Vec<u8>>,
	) -> Result<Vec<ParsedEntity>, TernError> {
		if !(200..300).contains(&status) {
			let error = HttpError::from_http_response(status, headers)?;
			return Err(ApiCallError::ServerResponseError { source: error }.into());
		}
		let raws: Vec<crate::json_element::RawEntity> = serde_json::from_slice(&body.unwrap_or_default())
			.map_err(|e| TernError::Other(format!("bad response: {e}")))?;
		raws.into_iter()
			.map(|raw| self.sdk.json_serializer.parse(type_ref, raw).map_err(TernError::from))
			.collect()
	}

	/// Loads a mail body, which lives on the blob storage servers.
	async fn load_mail_details_blob(&self, id: &IdTupleGenerated) -> Result<ParsedEntity, TernError> {
		let type_ref = MailDetailsBlob::type_ref();
		let access = self.read_token(None, &id.list_id, None).await?;
		let mut params = self.blob_query_params(&access, type_ref.clone())?;
		params.push(("ids".to_string(), id.element_id.to_string()));
		let type_model = self.sdk.entity_client.resolve_client_type_ref(&type_ref)?;
		let path = format!("/rest/{}/{}/{}", type_ref.app, type_model.name.to_lowercase(), id.list_id);
		let bytes = self
			.try_servers(&access, |server| format!("{server}{path}{}", encode_query_params(params.clone())))
			.await?;
		self.parse_list_response(&type_ref, 200, &HashMap::new(), Some(bytes))?
			.into_iter()
			.next()
			.ok_or_else(|| TernError::Other("the mail body is missing".into()))
	}

	async fn read_token(
		&self,
		archive_data_type: Option<ArchiveDataType>,
		archive: &GeneratedId,
		instance: Option<(&GeneratedId, &GeneratedId)>,
	) -> Result<BlobServerAccessInfo, TernError> {
		let request = read_token_request(&self.randomizer, archive_data_type, archive, instance);
		Ok(self
			.sdk
			.get_service_executor()
			.post::<BlobAccessTokenService>(request, ExtraServiceParams::default())
			.await?
			.blobAccessInfo)
	}

	/// Blob servers take the credentials as query parameters, as the TypeScript client sends them.
	fn blob_query_params(&self, access: &BlobServerAccessInfo, type_ref: TypeRef) -> Result<Vec<(String, String)>, TernError> {
		let version = self.sdk.entity_client.resolve_client_type_ref(&type_ref)?.version;
		let mut params: Vec<(String, String)> = self
			.parts()
			.auth_headers_provider
			.provide_headers(version)
			.into_iter()
			.collect();
		params.sort();
		params.push(("blobAccessToken".to_string(), access.blobAccessToken.clone()));
		Ok(params)
	}

	/// GETs from each blob server in turn until one answers.
	async fn try_servers(&self, access: &BlobServerAccessInfo, url: impl Fn(&str) -> String) -> Result<Vec<u8>, TernError> {
		let mut last_error = TernError::Other("no blob server to load from".into());
		for server in &access.servers {
			let response = self
				.parts()
				.rest_client
				.request_binary(
					url(&server.url),
					HttpMethod::GET,
					RestClientOptions {
						headers: HashMap::new(),
						body: None,
						suspension_behavior: None,
					},
				)
				.await;
			match response {
				Ok(response) if (200..300).contains(&response.status) => return Ok(response.body.unwrap_or_default()),
				Ok(response) => {
					let error = HttpError::from_http_response(response.status, &response.headers)?;
					last_error = ApiCallError::ServerResponseError { source: error }.into();
				},
				Err(e) => last_error = ApiCallError::from(e).into(),
			}
		}
		Err(last_error)
	}
}

/// A fresh id for an aggregate. The SDK fills these in only for encrypted requests, so
/// unencrypted ones must carry their own or they do not serialize.
fn aggregate_id(random: &RandomizerFacade) -> Option<CustomId> {
	Some(CustomId(BASE64_URL_SAFE_NO_PAD.encode(random.generate_random_array::<4>())))
}

fn read_token_request(
	random: &RandomizerFacade,
	archive_data_type: Option<ArchiveDataType>,
	archive: &GeneratedId,
	instance: Option<(&GeneratedId, &GeneratedId)>,
) -> BlobAccessTokenPostIn {
	BlobAccessTokenPostIn {
		_format: 0,
		archiveDataType: archive_data_type.map(|t| t.discriminant()),
		write: None,
		read: Some(BlobReadData {
			_id: aggregate_id(random),
			archiveId: archive.clone(),
			instanceListId: instance.map(|(list, _)| list.clone()),
			instanceIds: instance
				.map(|(_, element)| {
					vec![InstanceId {
						_id: aggregate_id(random),
						instanceId: Some(element.clone()),
					}]
				})
				.unwrap_or_default(),
		}),
	}
}

fn blob_get_in(random: &RandomizerFacade, archive: &GeneratedId, blobs: &[&GeneratedId]) -> BlobGetIn {
	BlobGetIn {
		_format: 0,
		archiveId: archive.clone(),
		blobId: None,
		blobIds: blobs
			.iter()
			.map(|id| BlobId {
				_id: aggregate_id(random),
				blobId: (*id).clone(),
			})
			.collect(),
	}
}

struct AttachmentKeys {
	file: IdTupleGenerated,
	bucket_enc_file_session_key: Option<Vec<u8>>,
	file_session_key: Option<Vec<u8>>,
}

/// The keys a send carries: the mail's, wrapped for each recipient when confidential, else plain.
struct SendKeys {
	mail_session_key: Option<Vec<u8>>,
	bucket_enc_mail_session_key: Option<Vec<u8>>,
	session_enc_auth_status: Option<Vec<u8>>,
	recipients: Vec<(String, PubEncSymKey)>,
	attachments: Vec<AttachmentKeys>,
}

/// SendDraftData, with the parameters mirrored at the top level as the TypeScript client sends them.
fn send_draft_data(random: &RandomizerFacade, mail: IdTupleGenerated, keys: SendKeys) -> SendDraftData {
	let recipients = |keys: &SendKeys| -> Vec<InternalRecipientKeyData> {
		keys.recipients
			.iter()
			.map(|(address, key)| InternalRecipientKeyData {
				_id: aggregate_id(random),
				mailAddress: address.clone(),
				pubEncBucketKey: key.pub_enc_sym_key_bytes.clone(),
				recipientKeyVersion: key.recipient_key_version as i64,
				protocolVersion: key.crypto_protocol_version.clone() as i64,
				senderKeyVersion: key.sender_key_version.map(|v| v as i64),
			})
			.collect()
	};
	let attachments = |keys: &SendKeys| -> Vec<AttachmentKeyData> {
		keys.attachments
			.iter()
			.map(|a| AttachmentKeyData {
				_id: aggregate_id(random),
				file: a.file.clone(),
				bucketEncFileSessionKey: a.bucket_enc_file_session_key.clone(),
				fileSessionKey: a.file_session_key.clone(),
			})
			.collect()
	};
	SendDraftData {
		_format: 0,
		language: "en".to_string(),
		mailSessionKey: keys.mail_session_key.clone(),
		bucketEncMailSessionKey: keys.bucket_enc_mail_session_key.clone(),
		senderNameUnencrypted: None,
		plaintext: false,
		calendarMethod: false,
		sessionEncEncryptionAuthStatus: keys.session_enc_auth_status.clone(),
		sendAt: None,
		allowUndo: false,
		internalRecipientKeyData: recipients(&keys),
		secureExternalRecipientKeyData: vec![],
		attachmentKeyData: attachments(&keys),
		mail: mail.clone(),
		symEncInternalRecipientKeyData: vec![],
		parameters: Some(SendDraftParameters {
			_id: aggregate_id(random),
			language: "en".to_string(),
			mailSessionKey: keys.mail_session_key.clone(),
			bucketEncMailSessionKey: keys.bucket_enc_mail_session_key.clone(),
			senderNameUnencrypted: None,
			plaintext: false,
			calendarMethod: false,
			sessionEncEncryptionAuthStatus: keys.session_enc_auth_status.clone(),
			mail,
			internalRecipientKeyData: recipients(&keys),
			secureExternalRecipientKeyData: vec![],
			symEncInternalRecipientKeyData: vec![],
			attachmentKeyData: attachments(&keys),
		}),
	}
}

/// The received date, which is stored unencrypted, so it is there even when the mail does not decrypt.
fn received_ms(type_model: &TypeModel, raw: &ParsedEntity) -> u64 {
	type_model
		.get_attribute_id_by_attribute_name("receivedDate")
		.ok()
		.and_then(|id| match raw.get(&id) {
			Some(ElementValue::Date(date)) => Some(date.as_millis()),
			_ => None,
		})
		.unwrap_or(0)
}

fn element_id(type_model: &TypeModel, raw: &ParsedEntity) -> Option<GeneratedId> {
	let id_attribute = type_model.get_attribute_id_by_attribute_name("_id").ok()?;
	match raw.get(&id_attribute)? {
		ElementValue::IdTupleGeneratedElementId(IdTupleGenerated { element_id, .. }) => Some(element_id.clone()),
		ElementValue::IdGeneratedId(id) => Some(id.clone()),
		_ => None,
	}
}

/// The highest custom id, to load a custom-id list from its newest end (CUSTOM_MAX_ID).
fn custom_max_id() -> CustomId {
	CustomId("_".repeat(340))
}

fn is_tuta_address(address: &str) -> bool {
	let address = address.to_ascii_lowercase();
	TUTA_MAIL_ADDRESS_DOMAINS
		.iter()
		.any(|domain| address.ends_with(&format!("@{domain}")))
}

/// Splits the BlobService's binary answer into blobs by id, as `parseMultipleBlobsResponse` does:
/// a 4-byte count, then per blob a 9-byte id, a 6-byte hash, a 4-byte length and the data.
fn parse_multiple_blobs(data: &[u8]) -> Result<HashMap<String, Vec<u8>>, TernError> {
	let bad = |what: &str| TernError::Other(format!("bad blob response: {what}"));
	let read_u32 = |at: usize| -> Option<u32> { data.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])) };
	let count = read_u32(0).ok_or_else(|| bad("too short"))? as i32;
	if count < 0 {
		return Err(bad("negative count"));
	}
	let mut blobs = HashMap::new();
	let mut offset = 4;
	while offset < data.len() {
		let id = data.get(offset..offset + 9).ok_or_else(|| bad("truncated id"))?;
		let size = read_u32(offset + 15).ok_or_else(|| bad("truncated length"))? as usize;
		let start = offset + 19;
		let contents = data.get(start..start + size).ok_or_else(|| bad("truncated data"))?;
		blobs.insert(to_base64_ext(id), contents.to_vec());
		offset = start + size;
	}
	if blobs.len() != count as usize {
		return Err(bad("wrong number of blobs"));
	}
	Ok(blobs)
}

/// Tuta's "base64ext" id encoding, which sorts like the bytes do.
fn to_base64_ext(bytes: &[u8]) -> String {
	crate::util::BASE64_EXT.encode(bytes)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_the_blob_service_answer() {
		let mut data = 2u32.to_be_bytes().to_vec();
		for (id, payload) in [([1u8; 9], b"hello".as_slice()), ([2u8; 9], b"world!".as_slice())] {
			data.extend_from_slice(&id);
			data.extend_from_slice(&[9; 6]);
			data.extend_from_slice(&(payload.len() as u32).to_be_bytes());
			data.extend_from_slice(payload);
		}
		let blobs = parse_multiple_blobs(&data).unwrap();
		assert_eq!(blobs[&to_base64_ext(&[1; 9])], b"hello");
		assert_eq!(blobs[&to_base64_ext(&[2; 9])], b"world!");
		assert!(parse_multiple_blobs(&data[..data.len() - 1]).is_err());
	}

	#[test]
	fn base64_ext_matches_the_typescript_client() {
		// base64ToBase64Ext(uint8ArrayToBase64(...)) in the TypeScript client
		assert_eq!(to_base64_ext(&[0, 0, 0]), "----");
		assert_eq!(to_base64_ext(&[0xff, 0xff, 0xff]), "zzzz");
		// "Man" is "TWFu" in base64; T=19→I, W=22→L, F=5→4, u=46→i
		assert_eq!(to_base64_ext(b"Man"), "IL4i");
	}

	/// The draft request is encrypted with the mail's session key, except the attachment's name and
	/// type, which go under the attachment's own key so recipients can read them.
	#[test]
	fn draft_request_encrypts_the_mail_and_leaves_attachment_names_to_their_key() {
		use crate::instance_mapper::InstanceMapper;
		use crate::util::test_utils::mock_type_model_provider;

		let provider = Arc::new(mock_type_model_provider());
		let mapper = InstanceMapper::new(provider.clone());
		let random = RandomizerFacade::from_core(rand_core::OsRng);
		let facade = EntityFacadeImpl::new(provider.clone(), random.clone());
		let mail_key = GenericAesKey::Aes256(Aes256Key::generate(&random));
		let file_key = GenericAesKey::Aes256(Aes256Key::generate(&random));
		let enc_name = file_key.encrypt_data(b"report.pdf", InitializationVector::generate(&random)).unwrap();
		let draft = DraftCreateData {
			_format: 0,
			previousMessageId: None,
			conversationType: CONVERSATION_NEW,
			ownerEncSessionKey: vec![1; 48],
			ownerKeyVersion: 0,
			draftData: DraftData {
				_id: None,
				subject: "Hello".into(),
				bodyText: String::new(),
				senderMailAddress: "me@tuta.com".into(),
				senderName: "Me".into(),
				confidential: true,
				method: 0,
				compressedBodyText: Some("<div>Hi</div>".into()),
				toRecipients: vec![DraftRecipient {
					_id: None,
					name: "Ann".into(),
					mailAddress: "ann@tuta.com".into(),
					_errors: Default::default(),
				}],
				ccRecipients: vec![],
				bccRecipients: vec![],
				addedAttachments: vec![DraftAttachment {
					_id: None,
					ownerEncFileSessionKey: vec![2; 48],
					ownerKeyVersion: 0,
					existingFile: None,
					newFile: Some(NewDraftAttachment {
						_id: None,
						encFileName: enc_name.clone(),
						encMimeType: enc_name.clone(),
						encCid: None,
						referenceTokens: vec![],
					}),
				}],
				removedAttachments: vec![],
				replyTos: vec![],
				_errors: Default::default(),
			},
			_errors: Default::default(),
		};

		let parsed = mapper.serialize_entity(draft).unwrap();
		let model = provider.resolve_client_type_ref(&DraftCreateData::type_ref()).unwrap();
		let encrypted = facade.encrypt_and_map(model, &parsed, &mail_key).unwrap();
		crate::json_serializer::JsonSerializer::new(provider.clone())
			.serialize(&DraftCreateData::type_ref(), encrypted.clone())
			.expect("the encrypted draft serializes for the wire");

		let field = |entity: &ParsedEntity, type_ref: TypeRef, name: &str| -> ElementValue {
			let model = provider.resolve_client_type_ref(&type_ref).unwrap();
			entity[&model.get_attribute_id_by_attribute_name(name).unwrap()].clone()
		};
		let first = |value: ElementValue| value.assert_array()[0].assert_dict();
		let decrypt = |key: &GenericAesKey, value: ElementValue| key.decrypt_data(value.assert_bytes());

		let data = first(field(&encrypted, DraftCreateData::type_ref(), "draftData"));
		assert_eq!(decrypt(&mail_key, field(&data, DraftData::type_ref(), "subject")).unwrap(), b"Hello");
		assert!(decrypt(&mail_key, field(&data, DraftData::type_ref(), "compressedBodyText")).is_ok());
		assert_eq!(field(&data, DraftData::type_ref(), "senderMailAddress").assert_string(), "me@tuta.com");
		let to = first(field(&data, DraftData::type_ref(), "toRecipients"));
		assert_eq!(decrypt(&mail_key, field(&to, DraftRecipient::type_ref(), "name")).unwrap(), b"Ann");
		let attachment = first(field(&data, DraftData::type_ref(), "addedAttachments"));
		let new_file = first(field(&attachment, DraftAttachment::type_ref(), "newFile"));
		let name = field(&new_file, NewDraftAttachment::type_ref(), "encFileName");
		assert_eq!(name.assert_bytes(), &enc_name, "the file name is passed through, not re-encrypted");
		assert_eq!(decrypt(&file_key, name).unwrap(), b"report.pdf");
	}

	/// Requests the SDK does not encrypt get no aggregate ids from it; without our own they fail
	/// to serialize and reading bodies, downloading and sending all break.
	#[test]
	fn unencrypted_requests_with_aggregates_serialize() {
		use crate::instance_mapper::InstanceMapper;
		use crate::json_serializer::JsonSerializer;
		use crate::util::test_utils::mock_type_model_provider;

		let provider = Arc::new(mock_type_model_provider());
		let mapper = InstanceMapper::new(provider.clone());
		let json = JsonSerializer::new(provider.clone());
		let random = RandomizerFacade::from_core(rand_core::OsRng);
		let id = |s: &str| GeneratedId(s.to_string());
		let mail = IdTupleGenerated { list_id: id("list"), element_id: id("mail") };
		macro_rules! serializes {
			($value:expr) => {{
				let value = $value;
				let type_ref = entity_type_ref(&value);
				let parsed = mapper.serialize_entity(value).unwrap();
				json.serialize(&type_ref, parsed).unwrap();
			}};
		}
		fn entity_type_ref<T: Entity>(_: &T) -> TypeRef {
			T::type_ref()
		}

		serializes!(read_token_request(&random, None, &id("archive"), None));
		serializes!(read_token_request(&random, Some(ArchiveDataType::Attachments), &id("archive"), Some((&id("files"), &id("file")))));
		serializes!(blob_get_in(&random, &id("archive"), &[&id("blob1"), &id("blob2")]));
		let pub_key = || PubEncSymKey {
			pub_enc_sym_key_bytes: vec![1; 32],
			crypto_protocol_version: CryptoProtocolVersion::TutaCrypt,
			sender_key_version: Some(0),
			recipient_key_version: 0,
		};
		serializes!(send_draft_data(
			&random,
			mail.clone(),
			SendKeys {
				mail_session_key: None,
				bucket_enc_mail_session_key: Some(vec![2; 48]),
				session_enc_auth_status: Some(vec![3; 48]),
				recipients: vec![("ann@tuta.com".into(), pub_key()), ("bob@tuta.com".into(), pub_key())],
				attachments: vec![AttachmentKeys {
					file: IdTupleGenerated { list_id: id("files"), element_id: id("file") },
					bucket_enc_file_session_key: Some(vec![4; 48]),
					file_session_key: None,
				}],
			},
		));
		serializes!(send_draft_data(
			&random,
			mail,
			SendKeys {
				mail_session_key: Some(vec![5; 32]),
				bucket_enc_mail_session_key: None,
				session_enc_auth_status: None,
				recipients: vec![],
				attachments: vec![],
			},
		));
	}

	#[test]
	fn knows_tuta_addresses() {
		assert!(is_tuta_address("Ann@Tuta.com"));
		assert!(is_tuta_address("x@keemail.me"));
		assert!(!is_tuta_address("x@gmail.com"));
		assert!(!is_tuta_address("x@nottuta.com"));
	}
}
