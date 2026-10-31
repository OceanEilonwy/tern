//! Reading, downloading, drafting and sending mail, and reading contacts, for Tern, the GNOME client.
//!
//! Not part of upstream Tuta. Ported from the TypeScript client's MailFacade, BlobFacade,
//! BlobAccessTokenFacade, CryptoFacade, LoginFacade and ContactModel at tutao/tutanota@73a3e0e9, which the rest
//! of this SDK copy also comes from. See vendor/tuta-sdk/PATCHES.md.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::prelude::BASE64_URL_SAFE_NO_PAD;
use base64::Engine;
use crypto_primitives::aes::{Aes256Key, InitializationVector};
use crypto_primitives::key::GenericAesKey;
use crypto_primitives::randomizer_facade::RandomizerFacade;
use crypto_primitives::versioned::{Versioned, VersionedAesKey};
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
use crate::crypto::key::{AsymmetricKeyPair, PublicKey};
use crate::crypto::public_key_provider::{PublicKeyIdentifier, PublicKeyLoadingError};
use crate::element_value::{ElementValue, ParsedEntity};
use crate::entities::entity_facade::{EntityFacade, EntityFacadeImpl};
use crate::entities::generated::storage::{
	BlobAccessTokenPostIn, BlobGetIn, BlobId, BlobReadData, BlobServerAccessInfo, InstanceId,
};
use crate::entities::generated::sys::{
	BucketKey, CreateSessionData, GroupInfo, RootInstance, SaltData,
};
use crate::entities::generated::tutanota::{
	AttachmentKeyData, Contact, ContactList, ConversationEntry, DraftAttachment, DraftCreateData,
	DraftCreateReturn, DraftData, DraftRecipient, DraftUpdateData, DraftUpdateReturn,
	EncryptedMailAddress, InternalRecipientKeyData, Mail, MailDetails, MailDetailsBlob,
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
	/// Tuta asks for fewer requests: wait this many seconds (if it says) and try again.
	#[error("too many requests")]
	RateLimited(Option<u64>),
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
				source:
					HttpError::TooManyRequestsError { suspension_time_sec }
					| HttpError::ServiceUnavailableError { suspension_time_sec: suspension_time_sec @ Some(_) },
			} => TernError::RateLimited(suspension_time_sec),
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

/// MailState.DRAFT: a mail not sent yet.
const MAIL_STATE_DRAFT: i64 = 0;

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
	Mail(Box<Mail>),
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
	/// The mail this answers or forwards, with CONVERSATION_REPLY or CONVERSATION_FORWARD. Ignored
	/// with `existing`: a saved draft keeps what it answers.
	pub previous: Option<(IdTupleGenerated, i64)>,
	/// The saved draft to write over instead of creating a new one.
	pub existing: Option<ExistingDraft>,
}

/// A saved draft to update, and which of its attachments stay on it. The others are removed.
pub struct ExistingDraft {
	pub mail: IdTupleGenerated,
	pub kept_attachments: Vec<IdTupleGenerated>,
}

/// A contact as the composer suggests it: the name, and every address it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TernContact {
	pub name: String,
	pub addresses: Vec<String>,
}

/// Each distinct recipient with its public key, None for those outside Tuta, and the Tuta
/// addresses that do not exist.
struct RecipientLookup<'a> {
	recipients: Vec<&'a OutgoingRecipient>,
	keys: Vec<Option<Versioned<PublicKey>>>,
	not_found: Vec<String>,
}

/// A draft on the server and the session key its contents are encrypted with.
struct WrittenDraft {
	id: IdTupleGenerated,
	mail_key: GenericAesKey,
}

/// What a draft request carries beyond the mail as written.
struct DraftParts {
	confidential: bool,
	method: i64,
	reply_tos: Vec<EncryptedMailAddress>,
	added: Vec<DraftAttachment>,
	removed: Vec<IdTupleGenerated>,
}

/// Session keys of the instances a bucket key covers, by instance element id.
type InstanceKeys = HashMap<GeneratedId, GenericAesKey>;

/// BlobAccessTokenKind.Archive: a read token good for the whole archive, not just some instances.
const TOKEN_KIND_ARCHIVE: i64 = 0;

/// A mail of a batch once it is decrypted, waiting for its details and attachments.
struct Decrypted {
	mail: Mail,
	mail_key: GenericAesKey,
	instance_keys: Option<InstanceKeys>,
}

pub struct TernMail {
	sdk: Arc<LoggedInSdk>,
	entity_facade: EntityFacadeImpl,
	randomizer: RandomizerFacade,
	/// Key pairs by group and version. Loading one costs a Group request; every mail with a
	/// bucket key needs one, and they are nearly always the same.
	key_pairs: Mutex<HashMap<(GeneratedId, u64), AsymmetricKeyPair>>,
	/// Read tokens by archive, kept until they expire, as the TypeScript client keeps them.
	read_tokens: Mutex<HashMap<GeneratedId, BlobServerAccessInfo>>,
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
			key_pairs: Mutex::default(),
			read_tokens: Mutex::default(),
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
						Ok(mail) => PageItem::Mail(Box::new(mail)),
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

	/// Loads a mail with its body and attachment list, one request at a time. The fallback when a
	/// batch from [`Self::open_many`] fails.
	pub async fn open(&self, id: &IdTupleGenerated) -> Result<OpenedMail, TernError> {
		let raw = self.sdk.entity_client.load(&Mail::type_ref(), id).await?;
		let decrypted = self.decrypt_mail(raw).await?;
		let mail = &decrypted.mail;
		let details = if let Some(details_id) = &mail.mailDetails {
			let raw = self
				.load_mail_details_blobs(&details_id.list_id, std::slice::from_ref(&details_id.element_id))
				.await?
				.into_iter()
				.next()
				.ok_or_else(|| TernError::Other("the mail body is missing".into()))?;
			self.decrypt_with::<MailDetailsBlob>(raw, details_key(&decrypted, &details_id.element_id))?.details
		} else if let Some(draft_id) = &mail.mailDetailsDraft {
			let raw = self.sdk.entity_client.load(&MailDetailsDraft::type_ref(), draft_id).await?;
			self.decrypt_with::<MailDetailsDraft>(raw, decrypted.mail_key.clone())?.details
		} else {
			return Err(TernError::Other("mail has no body".into()));
		};
		let mut files = Vec::with_capacity(mail.attachments.len());
		for (list, ids) in by_list(&mail.attachments) {
			for chunk in ids.chunks(MAX_IDS_PER_REQUEST) {
				for raw in self.load_multiple_raw(&TutanotaFile::type_ref(), &list, chunk).await? {
					files.push(self.decrypt::<TutanotaFile>(raw, decrypted.instance_keys.as_ref()).await?);
				}
			}
		}
		Ok(OpenedMail { mail: decrypted.mail, details, attachments: files })
	}

	/// Loads several mails with their bodies and attachment lists in as few requests as Tuta takes:
	/// one per mail list for the mails, one per archive for the bodies (with one read token per
	/// archive, kept until it expires), one per list for draft bodies and for attachments, as the
	/// TypeScript client's BulkMailLoader does. A failed request fails the whole batch; a mail
	/// that is missing or does not decrypt fails alone. Results come in the order of `ids`.
	pub async fn open_many(
		&self,
		ids: &[IdTupleGenerated],
	) -> Result<Vec<(IdTupleGenerated, Result<OpenedMail, TernError>)>, TernError> {
		let mail_model = self.sdk.entity_client.resolve_server_type_ref(&Mail::type_ref())?;
		let mut raws: HashMap<GeneratedId, ParsedEntity> = HashMap::new();
		for (list, elements) in by_list(ids) {
			for chunk in elements.chunks(MAX_IDS_PER_REQUEST) {
				for raw in self.load_multiple_raw(&Mail::type_ref(), &list, chunk).await? {
					if let Some(element) = element_id(&mail_model, &raw) {
						raws.insert(element, raw);
					}
				}
			}
		}
		let mut mails: Vec<Result<Decrypted, TernError>> = Vec::with_capacity(ids.len());
		for id in ids {
			mails.push(match raws.remove(&id.element_id) {
				Some(raw) => self.decrypt_mail(raw).await,
				None => Err(TernError::Other("the mail is gone".into())),
			});
		}
		let decrypted = || mails.iter().filter_map(|m| m.as_ref().ok());

		let blob_ids: Vec<IdTupleGenerated> = decrypted().filter_map(|d| d.mail.mailDetails.clone()).collect();
		let mut blobs: HashMap<GeneratedId, ParsedEntity> = HashMap::new();
		let blob_model = self.sdk.entity_client.resolve_server_type_ref(&MailDetailsBlob::type_ref())?;
		for (archive, elements) in by_list(&blob_ids) {
			for chunk in elements.chunks(MAX_IDS_PER_REQUEST) {
				for raw in self.load_mail_details_blobs(&archive, chunk).await? {
					if let Some(element) = element_id(&blob_model, &raw) {
						blobs.insert(element, raw);
					}
				}
			}
		}
		let draft_ids: Vec<IdTupleGenerated> = decrypted().filter_map(|d| d.mail.mailDetailsDraft.clone()).collect();
		let drafts = self.load_all_raw(&MailDetailsDraft::type_ref(), &draft_ids).await?;
		let file_ids: Vec<IdTupleGenerated> = decrypted().flat_map(|d| d.mail.attachments.iter().cloned()).collect();
		let mut files = self.load_all_raw(&TutanotaFile::type_ref(), &file_ids).await?;

		let mut opened = Vec::with_capacity(ids.len());
		for (id, mail) in ids.iter().zip(mails) {
			let result = match mail {
				Ok(decrypted) => self.assemble(decrypted, &mut blobs, &drafts, &mut files).await,
				Err(e) => Err(e),
			};
			opened.push((id.clone(), result));
		}
		Ok(opened)
	}

	/// A mail of a batch with its details and attachments, from what the batch loaded.
	async fn assemble(
		&self,
		decrypted: Decrypted,
		blobs: &mut HashMap<GeneratedId, ParsedEntity>,
		drafts: &HashMap<GeneratedId, ParsedEntity>,
		files: &mut HashMap<GeneratedId, ParsedEntity>,
	) -> Result<OpenedMail, TernError> {
		let mail = &decrypted.mail;
		let details = if let Some(details_id) = &mail.mailDetails {
			let raw = blobs
				.remove(&details_id.element_id)
				.ok_or_else(|| TernError::Other("the mail body is missing".into()))?;
			self.decrypt_with::<MailDetailsBlob>(raw, details_key(&decrypted, &details_id.element_id))?.details
		} else if let Some(draft_id) = &mail.mailDetailsDraft {
			let raw = drafts
				.get(&draft_id.element_id)
				.cloned()
				.ok_or_else(|| TernError::Other("the draft body is missing".into()))?;
			self.decrypt_with::<MailDetailsDraft>(raw, decrypted.mail_key.clone())?.details
		} else {
			return Err(TernError::Other("mail has no body".into()));
		};
		let mut attachments = Vec::with_capacity(mail.attachments.len());
		for file in &mail.attachments {
			// A file the server did not return is left out, as a single open leaves it out.
			if let Some(raw) = files.remove(&file.element_id) {
				attachments.push(self.decrypt::<TutanotaFile>(raw, decrypted.instance_keys.as_ref()).await?);
			}
		}
		Ok(OpenedMail { mail: decrypted.mail, details, attachments })
	}

	/// Decrypts a mail as loaded, keeping the keys its details and attachments need.
	async fn decrypt_mail(&self, raw: ParsedEntity) -> Result<Decrypted, TernError> {
		let instance_keys = self.bucket_instance_keys(&Mail::type_ref(), &raw).await?;
		let mail_key = self.session_key(&Mail::type_ref(), &raw, instance_keys.as_ref()).await?;
		let mail: Mail = self.decrypt_with(raw, mail_key.clone())?;
		Ok(Decrypted { mail, mail_key, instance_keys })
	}

	/// Loads list elements from any number of lists, still encrypted, by element id.
	async fn load_all_raw(&self, type_ref: &TypeRef, ids: &[IdTupleGenerated]) -> Result<HashMap<GeneratedId, ParsedEntity>, TernError> {
		let model = self.sdk.entity_client.resolve_server_type_ref(type_ref)?;
		let mut loaded = HashMap::new();
		for (list, elements) in by_list(ids) {
			for chunk in elements.chunks(MAX_IDS_PER_REQUEST) {
				for raw in self.load_multiple_raw(type_ref, &list, chunk).await? {
					if let Some(element) = element_id(&model, &raw) {
						loaded.insert(element, raw);
					}
				}
			}
		}
		Ok(loaded)
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

	/// Saves `outgoing` to Drafts: a new draft, or over `outgoing.existing`. Returns the draft's id.
	///
	/// Recipients outside Tuta make the draft non-confidential, as [Self::send] would send it.
	pub async fn save_draft(&self, sender: &TernAccount, outgoing: OutgoingMail) -> Result<IdTupleGenerated, TernError> {
		check_body_size(&outgoing)?;
		let lookup = self.recipient_keys(&outgoing).await?;
		let confidential = lookup.keys.iter().all(Option::is_some);
		Ok(self.write_draft(sender, &outgoing, confidential).await?.id)
	}

	/// Writes the draft (creating it, or updating `outgoing.existing`) with the attachments uploaded,
	/// then sends it. End-to-end encrypted when every recipient is on Tuta; otherwise sent as
	/// ordinary mail, like the official client does when "confidential" is off.
	pub async fn send(&self, sender: &TernAccount, outgoing: OutgoingMail) -> Result<SendDraftReturn, TernError> {
		check_body_size(&outgoing)?;
		let RecipientLookup { recipients: all, keys: public_keys, not_found } = self.recipient_keys(&outgoing).await?;
		if all.is_empty() {
			return Err(TernError::Other("no recipients".into()));
		}
		if !not_found.is_empty() {
			return Err(TernError::RecipientsNotFound(not_found));
		}
		let confidential = public_keys.iter().all(Option::is_some);
		let WrittenDraft { id: draft_id, mail_key } = self.write_draft(sender, &outgoing, confidential).await?;
		let iv = || InitializationVector::generate(&self.randomizer);

		// Every attachment's key goes along, wrapped for the recipients, as sendDraft does with
		// getAttachmentIds(draft).
		let raw_draft = self.sdk.entity_client.load(&Mail::type_ref(), &draft_id).await?;
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
			draft_id,
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

	/// Looks up every recipient's public key, once per address however often it appears across
	/// To, Cc and Bcc. Who is on Tuta decides whether the mail can be end-to-end encrypted.
	async fn recipient_keys<'a>(&self, outgoing: &'a OutgoingMail) -> Result<RecipientLookup<'a>, TernError> {
		let mut recipients: Vec<&OutgoingRecipient> = Vec::new();
		for recipient in outgoing.to.iter().chain(&outgoing.cc).chain(&outgoing.bcc) {
			if !recipients.iter().any(|r| r.address.eq_ignore_ascii_case(&recipient.address)) {
				recipients.push(recipient);
			}
		}
		let mut keys = Vec::with_capacity(recipients.len());
		let mut not_found = Vec::new();
		for recipient in &recipients {
			let identifier = PublicKeyIdentifier {
				identifier: recipient.address.clone(),
				identifier_type: PublicKeyIdentifierType::MailAddress,
			};
			match self.parts().public_key_provider.load_current_pub_key(&identifier).await {
				Ok(key) => keys.push(Some(key)),
				Err(PublicKeyLoadingError::KeyLoadingError(ApiCallError::ServerResponseError {
					source: HttpError::NotFoundError,
				})) => {
					if is_tuta_address(&recipient.address) {
						not_found.push(recipient.address.clone());
					}
					keys.push(None);
				},
				Err(PublicKeyLoadingError::KeyLoadingError(e)) => return Err(e.into()),
				Err(e) => return Err(TernError::Other(format!("cannot use the recipient's key: {e:?}"))),
			}
		}
		Ok(RecipientLookup { recipients, keys, not_found })
	}

	/// Creates the draft (TypeScript `createDraft`) or, with `outgoing.existing`, updates it
	/// (`updateDraft`). Returns its id and the session key its contents are encrypted with.
	async fn write_draft(&self, sender: &TernAccount, outgoing: &OutgoingMail, confidential: bool) -> Result<WrittenDraft, TernError> {
		match &outgoing.existing {
			None => self.create_draft(sender, outgoing, confidential).await,
			Some(existing) => self.update_draft(sender, outgoing, existing, confidential).await,
		}
	}

	async fn create_draft(&self, sender: &TernAccount, outgoing: &OutgoingMail, confidential: bool) -> Result<WrittenDraft, TernError> {
		let group_key = self
			.parts()
			.key_loader_facade
			.get_current_sym_group_key(&sender.mail_group)
			.await
			.map_err(|e| TernError::Other(format!("cannot load the mailbox key: {e}")))?;
		let mail_key = GenericAesKey::Aes256(Aes256Key::generate(&self.randomizer));
		let owner_enc_mail_key = group_key.object.encrypt_key(&mail_key, InitializationVector::generate(&self.randomizer));
		let added = self.upload_attachments(&sender.mail_group, &group_key, &outgoing.attachments).await?;
		let (previous_message_id, conversation_type) = match &outgoing.previous {
			Some((previous, kind)) => (self.previous_message_id(previous).await, *kind),
			None => (None, CONVERSATION_NEW),
		};
		let request = DraftCreateData {
			_format: 0,
			previousMessageId: previous_message_id,
			conversationType: conversation_type,
			ownerEncSessionKey: owner_enc_mail_key,
			ownerKeyVersion: group_key.version as i64,
			draftData: draft_data(
				sender,
				outgoing,
				DraftParts {
					confidential,
					method: 0,
					reply_tos: vec![],
					added,
					removed: vec![],
				},
			),
			_errors: Default::default(),
		};
		let created: DraftCreateReturn = self
			.sdk
			.get_service_executor()
			.post::<DraftService>(
				request,
				ExtraServiceParams {
					session_key: Some(mail_key.clone()),
					..Default::default()
				},
			)
			.await?;
		Ok(WrittenDraft { id: created.draft, mail_key })
	}

	/// As `updateDraft`: the draft keeps its session key, method and reply-tos, attachments it
	/// already has and keeps are not sent again, removed ones go in `removedAttachments`, and new
	/// ones are uploaded under the key version the draft is owned with.
	async fn update_draft(
		&self,
		sender: &TernAccount,
		outgoing: &OutgoingMail,
		existing: &ExistingDraft,
		confidential: bool,
	) -> Result<WrittenDraft, TernError> {
		let raw = self.sdk.entity_client.load(&Mail::type_ref(), &existing.mail).await?;
		let mail_key = self.session_key(&Mail::type_ref(), &raw, None).await?;
		let draft: Mail = self.decrypt_with(raw, mail_key.clone())?;
		if draft.state != MAIL_STATE_DRAFT {
			return Err(TernError::Other("this mail is no longer a draft".into()));
		}
		// "we assume that there is an _ownerEncSessionKey anyway, so we can default to 0"
		let version = draft._ownerKeyVersion.unwrap_or(0).max(0) as u64;
		let group_key = Versioned::new(
			self.parts()
				.key_loader_facade
				.load_sym_group_key(&sender.mail_group, version, None)
				.await
				.map_err(|e| TernError::Other(format!("cannot load the mailbox key: {e}")))?,
			version,
		);
		let reply_tos = match &draft.mailDetailsDraft {
			Some(details_id) => {
				let raw = self.sdk.entity_client.load(&MailDetailsDraft::type_ref(), details_id).await?;
				self.decrypt_with::<MailDetailsDraft>(raw, mail_key.clone())?.details.replyTos
			},
			None => return Err(TernError::Other("draft without mailDetailsDraft".into())),
		};
		let removed = removed_attachments(&draft.attachments, &existing.kept_attachments);
		let added = self.upload_attachments(&sender.mail_group, &group_key, &outgoing.attachments).await?;
		let id = draft._id.clone().unwrap_or_else(|| existing.mail.clone());
		let request = draft_update_data(
			id.clone(),
			draft_data(
				sender,
				outgoing,
				DraftParts {
					confidential,
					method: draft.method,
					reply_tos,
					added,
					removed,
				},
			),
		);
		let _: DraftUpdateReturn = self
			.sdk
			.get_service_executor()
			.put::<DraftService>(
				request,
				ExtraServiceParams {
					session_key: Some(mail_key.clone()),
					..Default::default()
				},
			)
			.await?;
		Ok(WrittenDraft { id, mail_key })
	}

	/// Uploads new attachments, each under its own key, as the DataFile branch of
	/// `_createAddedAttachments` does.
	async fn upload_attachments(
		&self,
		mail_group: &GeneratedId,
		group_key: &VersionedAesKey,
		attachments: &[OutgoingAttachment],
	) -> Result<Vec<DraftAttachment>, TernError> {
		if attachments.is_empty() {
			return Ok(Vec::new());
		}
		let file_keys: Vec<GenericAesKey> = attachments
			.iter()
			.map(|_| GenericAesKey::Aes256(Aes256Key::generate(&self.randomizer)))
			.collect();
		let file_data: Vec<FileData> = attachments
			.iter()
			.zip(&file_keys)
			.map(|(a, key)| FileData {
				session_key: key.clone(),
				data: a.data.as_slice(),
			})
			.collect();
		let tokens = self
			.sdk
			.blob_facade()
			.encrypt_and_upload_multiple(ArchiveDataType::Attachments, mail_group, file_data.iter())
			.await?;
		attachments
			.iter()
			.zip(&file_keys)
			.zip(tokens)
			.map(|((attachment, key), reference_tokens)| new_draft_attachment(&self.randomizer, group_key, key, attachment, reference_tokens))
			.collect()
	}

	/// The account's contacts, decrypted, as the TypeScript client loads them: the ContactList root
	/// of the user group, then every Contact in its list. A contact that does not decrypt is skipped.
	pub async fn contacts(&self) -> Result<Vec<TernContact>, TernError> {
		let root_id = IdTupleCustom {
			list_id: self.sdk.user_facade.get_user_group_id(),
			element_id: self
				.sdk
				.entity_client
				.resolve_client_type_ref(&ContactList::type_ref())?
				.root_id
				.clone(),
		};
		let root: RootInstance = match self.sdk.crypto_entity_client.load(&root_id).await {
			Ok(root) => root,
			// External mailboxes have no contact list.
			Err(ApiCallError::ServerResponseError {
				source: HttpError::NotFoundError,
			}) => return Ok(Vec::new()),
			Err(e) => return Err(e.into()),
		};
		let list: ContactList = self.sdk.crypto_entity_client.load(&root.reference).await?;
		let raws = self
			.sdk
			.entity_client
			.load_all(&Contact::type_ref(), &list.contacts, ListLoadDirection::ASC)
			.await?;
		let mut contacts = Vec::with_capacity(raws.len());
		for raw in raws {
			match self.decrypt::<Contact>(raw, None).await {
				Ok(contact) => contacts.push(tern_contact(&contact)),
				Err(e) => log::warn!("a contact does not decrypt: {e}"),
			}
		}
		Ok(contacts)
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
		let key_pair = self.key_pair(key_group, bucket_key.recipientKeyVersion as u64).await?;
		let decapsulated = AsymmetricCryptoFacade::decrypt_sym_key_with_key_pair(key_pair, &protocol, pub_enc_bucket_key)
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
		let url = elements_url(&parts.base_url, type_ref, &type_model.name, list, Vec::new(), ids);
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

	/// Loads mail bodies of one archive, which live on the blob storage servers, in one request.
	/// Like the TypeScript client's `loadMultipleBlobElements`: a read token for the archive, the
	/// ids comma-joined, and on 403 (an expired token) a fresh token and one more try.
	async fn load_mail_details_blobs(&self, archive: &GeneratedId, ids: &[GeneratedId]) -> Result<Vec<ParsedEntity>, TernError> {
		let type_ref = MailDetailsBlob::type_ref();
		let type_model = self.sdk.entity_client.resolve_client_type_ref(&type_ref)?;
		let mut retried = false;
		loop {
			let access = self.archive_read_token(archive).await?;
			let params = self.blob_query_params(&access, type_ref.clone())?;
			let url = |server: &str| elements_url(server, &type_ref, &type_model.name, archive, params.clone(), ids);
			match self.try_servers(&access, url).await {
				Err(ApiCallError::ServerResponseError { source: HttpError::NotAuthorizedError }) if !retried => {
					self.read_tokens.lock().unwrap().remove(archive);
					retried = true;
				},
				bytes => return self.parse_list_response(&type_ref, 200, &HashMap::new(), Some(bytes?)),
			}
		}
	}

	/// A read token for a whole archive, from the cache while it is still valid.
	async fn archive_read_token(&self, archive: &GeneratedId) -> Result<BlobServerAccessInfo, TernError> {
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.map_or(0, |d| d.as_millis() as u64);
		if let Some(token) = self.read_tokens.lock().unwrap().get(archive).filter(|t| t.expires.as_millis() > now) {
			return Ok(token.clone());
		}
		let token = self.read_token(None, archive, None).await?;
		if token.tokenKind == TOKEN_KIND_ARCHIVE {
			self.read_tokens.lock().unwrap().insert(archive.clone(), token.clone());
		}
		Ok(token)
	}

	/// The key pair of `group` at `version`, loaded once per session.
	async fn key_pair(&self, group: &GeneratedId, version: u64) -> Result<AsymmetricKeyPair, TernError> {
		let key = (group.clone(), version);
		if let Some(pair) = self.key_pairs.lock().unwrap().get(&key) {
			return Ok(pair.clone());
		}
		let pair = self
			.parts()
			.key_loader_facade
			.load_key_pair(group, version)
			.await
			.map_err(|e| TernError::Other(format!("cannot load the key pair: {e}")))?;
		self.key_pairs.lock().unwrap().insert(key, pair.clone());
		Ok(pair)
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
	async fn try_servers(&self, access: &BlobServerAccessInfo, url: impl Fn(&str) -> String) -> Result<Vec<u8>, ApiCallError> {
		let mut last_error = ApiCallError::InternalSdkError { error_message: "no blob server to load from".into() };
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
					last_error = ApiCallError::ServerResponseError { source: error };
				},
				Err(e) => last_error = ApiCallError::from(e),
			}
		}
		Err(last_error)
	}
}

/// Element ids grouped by list, lists and ids in the order first met.
fn by_list(ids: &[IdTupleGenerated]) -> Vec<(GeneratedId, Vec<GeneratedId>)> {
	let mut lists: Vec<(GeneratedId, Vec<GeneratedId>)> = Vec::new();
	for id in ids {
		match lists.iter_mut().find(|(list, _)| *list == id.list_id) {
			Some((_, elements)) if !elements.contains(&id.element_id) => elements.push(id.element_id.clone()),
			Some(_) => {},
			None => lists.push((id.list_id.clone(), vec![id.element_id.clone()])),
		}
	}
	lists
}

/// The GET for several elements of one list (or blob elements of one archive), `ids`
/// comma-joined after any credentials, as the TypeScript client's `loadMultiple` sends it.
fn elements_url(
	server: &str,
	type_ref: &TypeRef,
	type_name: &str,
	list: &GeneratedId,
	mut params: Vec<(String, String)>,
	ids: &[GeneratedId],
) -> String {
	params.push(("ids".to_string(), ids.iter().map(GeneratedId::as_str).collect::<Vec<_>>().join(",")));
	format!("{server}/rest/{}/{}/{list}{}", type_ref.app, type_name.to_lowercase(), encode_query_params(params))
}

/// The session key of a mail's details: from the bucket key if it covers them, else the mail's.
fn details_key(mail: &Decrypted, details: &GeneratedId) -> GenericAesKey {
	mail.instance_keys
		.as_ref()
		.and_then(|keys| keys.get(details).cloned())
		.unwrap_or_else(|| mail.mail_key.clone())
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

fn check_body_size(outgoing: &OutgoingMail) -> Result<(), TernError> {
	if outgoing.body_html.len() > UNCOMPRESSED_MAX_SIZE {
		return Err(TernError::Other("the message is too long".into()));
	}
	Ok(())
}

/// DraftData as `createDraft` and `updateDraft` build it. The SDK encrypts it with the mail's
/// session key, which also fills in the aggregate ids.
fn draft_data(sender: &TernAccount, outgoing: &OutgoingMail, parts: DraftParts) -> DraftData {
	// recipientToDraftRecipient: the address stays plain, the name is encrypted.
	let recipient = |r: &OutgoingRecipient| DraftRecipient {
		_id: None,
		name: r.name.clone(),
		mailAddress: r.address.clone(),
		_errors: Default::default(),
	};
	DraftData {
		_id: None,
		subject: outgoing.subject.clone(),
		bodyText: String::new(),
		senderMailAddress: sender.mail_address.clone(),
		senderName: sender.name.clone(),
		confidential: parts.confidential,
		method: parts.method,
		compressedBodyText: Some(outgoing.body_html.clone()),
		toRecipients: outgoing.to.iter().map(recipient).collect(),
		ccRecipients: outgoing.cc.iter().map(recipient).collect(),
		bccRecipients: outgoing.bcc.iter().map(recipient).collect(),
		addedAttachments: parts.added,
		removedAttachments: parts.removed,
		replyTos: parts.reply_tos,
		_errors: Default::default(),
	}
}

fn draft_update_data(draft: IdTupleGenerated, draft_data: DraftData) -> DraftUpdateData {
	DraftUpdateData {
		_format: 0,
		draftData: draft_data,
		draft,
		_errors: Default::default(),
	}
}

/// `_getRemovedAttachments`: the draft's files that are not kept.
fn removed_attachments(current: &[IdTupleGenerated], kept: &[IdTupleGenerated]) -> Vec<IdTupleGenerated> {
	current.iter().filter(|file| !kept.contains(file)).cloned().collect()
}

/// `createAndEncryptDraftAttachment`: name and type under the file's own key, the file key under
/// the mailbox key.
fn new_draft_attachment(
	random: &RandomizerFacade,
	group_key: &VersionedAesKey,
	file_key: &GenericAesKey,
	attachment: &OutgoingAttachment,
	reference_tokens: Vec<crate::entities::generated::sys::BlobReferenceTokenWrapper>,
) -> Result<DraftAttachment, TernError> {
	let iv = || InitializationVector::generate(random);
	let encrypt_string = |value: &str| {
		file_key
			.encrypt_data(value.as_bytes(), iv())
			.map_err(|e| TernError::Other(format!("cannot encrypt: {e}")))
	};
	Ok(DraftAttachment {
		_id: None,
		ownerEncFileSessionKey: group_key.object.encrypt_key(file_key, iv()),
		ownerKeyVersion: group_key.version as i64,
		existingFile: None,
		newFile: Some(NewDraftAttachment {
			_id: None,
			encFileName: encrypt_string(&attachment.name)?,
			encMimeType: encrypt_string(&attachment.mime_type)?,
			encCid: None,
			referenceTokens: reference_tokens,
		}),
	})
}

/// The name as "first last", and the contact's addresses that are not blank.
fn tern_contact(contact: &Contact) -> TernContact {
	TernContact {
		name: format!("{} {}", contact.firstName.trim(), contact.lastName.trim()).trim().to_string(),
		addresses: contact
			.mailAddresses
			.iter()
			.map(|a| a.address.trim().to_string())
			.filter(|a| !a.is_empty())
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
		// As Tuta's own apps send by default: the server queues the mail and sends it on its own
		// after a few seconds.
		allowUndo: true,
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
		let group_key = Versioned::new(GenericAesKey::Aes256(Aes256Key::generate(&random)), 0);
		let attachment = new_draft_attachment(&random, &group_key, &file_key, &test_attachment("report.pdf"), vec![]).unwrap();
		let enc_name = attachment.newFile.as_ref().unwrap().encFileName.clone();
		let draft = DraftCreateData {
			_format: 0,
			previousMessageId: None,
			conversationType: CONVERSATION_NEW,
			ownerEncSessionKey: vec![1; 48],
			ownerKeyVersion: 0,
			draftData: draft_data(
				&test_sender(),
				&test_outgoing(None),
				DraftParts {
					// As for a recipient outside Tuta.
					confidential: false,
					method: 0,
					reply_tos: vec![],
					added: vec![attachment],
					removed: vec![],
				},
			),
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
		// The server decrypts these itself for recipients outside Tuta; empty strings there got a 500.
		assert_eq!(decrypt(&mail_key, field(&data, DraftData::type_ref(), "bodyText")).unwrap(), b"");
		assert_eq!(decrypt(&mail_key, field(&data, DraftData::type_ref(), "method")).unwrap(), b"0");
		assert_eq!(decrypt(&mail_key, field(&data, DraftData::type_ref(), "confidential")).unwrap(), b"0");
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

	fn test_sender() -> TernAccount {
		TernAccount {
			mail_address: "me@tuta.com".into(),
			name: "Me".into(),
			mail_group: GeneratedId("mailgroup".into()),
		}
	}

	fn test_attachment(name: &str) -> OutgoingAttachment {
		OutgoingAttachment {
			name: name.into(),
			mime_type: "application/pdf".into(),
			data: b"%PDF".to_vec(),
		}
	}

	fn test_outgoing(existing: Option<ExistingDraft>) -> OutgoingMail {
		OutgoingMail {
			to: vec![OutgoingRecipient {
				name: "Ann".into(),
				address: "ann@tuta.com".into(),
			}],
			cc: vec![],
			bcc: vec![],
			subject: "Hello".into(),
			body_html: "<div>Hi</div>".into(),
			attachments: vec![test_attachment("report.pdf")],
			previous: None,
			existing,
		}
	}

	/// Updating a draft as `updateDraft` does: files the draft keeps are not sent again, the dropped
	/// one goes in removedAttachments, only the new upload is added, and the request is encrypted
	/// with the draft's own session key like a new draft.
	#[test]
	fn draft_update_request_removes_dropped_files_and_adds_only_new_ones() {
		use crate::instance_mapper::InstanceMapper;
		use crate::json_serializer::JsonSerializer;
		use crate::util::test_utils::mock_type_model_provider;

		let provider = Arc::new(mock_type_model_provider());
		let mapper = InstanceMapper::new(provider.clone());
		let random = RandomizerFacade::from_core(rand_core::OsRng);
		let facade = EntityFacadeImpl::new(provider.clone(), random.clone());
		let mail_key = GenericAesKey::Aes256(Aes256Key::generate(&random));
		let file_key = GenericAesKey::Aes256(Aes256Key::generate(&random));
		let group_key = Versioned::new(GenericAesKey::Aes256(Aes256Key::generate(&random)), 3);
		let id = |list: &str, element: &str| IdTupleGenerated {
			list_id: GeneratedId(list.into()),
			element_id: GeneratedId(element.into()),
		};
		let (kept, dropped) = (id("files", "kept"), id("files", "dropped"));
		let draft_id = id("mails", "draft");
		let outgoing = test_outgoing(Some(ExistingDraft {
			mail: draft_id.clone(),
			kept_attachments: vec![kept.clone()],
		}));

		let removed = removed_attachments(&[kept.clone(), dropped.clone()], &outgoing.existing.as_ref().unwrap().kept_attachments);
		assert_eq!(removed, vec![dropped.clone()]);
		let token = crate::entities::generated::sys::BlobReferenceTokenWrapper {
			_id: None,
			blobReferenceToken: "token".into(),
		};
		let added = vec![new_draft_attachment(&random, &group_key, &file_key, &outgoing.attachments[0], vec![token]).unwrap()];
		let request = draft_update_data(
			draft_id.clone(),
			draft_data(
				&test_sender(),
				&outgoing,
				DraftParts {
					confidential: true,
					method: 0,
					// As loaded from the draft's MailDetailsDraft.
					reply_tos: vec![EncryptedMailAddress {
						_id: Some(CustomId("abcd".into())),
						name: "Reply Here".into(),
						address: "reply@tuta.com".into(),
						_errors: Default::default(),
					}],
					added,
					removed,
				},
			),
		);

		let parsed = mapper.serialize_entity(request).unwrap();
		let model = provider.resolve_client_type_ref(&DraftUpdateData::type_ref()).unwrap();
		let encrypted = facade.encrypt_and_map(model, &parsed, &mail_key).unwrap();
		JsonSerializer::new(provider.clone())
			.serialize(&DraftUpdateData::type_ref(), encrypted.clone())
			.expect("the encrypted draft update serializes for the wire");

		let field = |entity: &ParsedEntity, type_ref: TypeRef, name: &str| -> ElementValue {
			let model = provider.resolve_client_type_ref(&type_ref).unwrap();
			entity[&model.get_attribute_id_by_attribute_name(name).unwrap()].clone()
		};
		let decrypt = |key: &GenericAesKey, value: ElementValue| key.decrypt_data(value.assert_bytes());

		assert_eq!(field(&encrypted, DraftUpdateData::type_ref(), "draft").assert_array()[0].assert_tuple_id_generated(), &draft_id);
		let data = field(&encrypted, DraftUpdateData::type_ref(), "draftData").assert_array()[0].assert_dict();
		assert_eq!(decrypt(&mail_key, field(&data, DraftData::type_ref(), "subject")).unwrap(), b"Hello");
		assert_eq!(decrypt(&mail_key, field(&data, DraftData::type_ref(), "confidential")).unwrap(), b"1");

		let removed: Vec<IdTupleGenerated> = field(&data, DraftData::type_ref(), "removedAttachments")
			.assert_array()
			.iter()
			.map(|v| v.assert_tuple_id_generated().clone())
			.collect();
		assert_eq!(removed, vec![dropped], "only the dropped file is removed; the kept one stays untouched");

		let added = field(&data, DraftData::type_ref(), "addedAttachments").assert_array();
		assert_eq!(added.len(), 1, "the kept file is not added again");
		let attachment = added[0].assert_dict();
		assert!(field(&attachment, DraftAttachment::type_ref(), "existingFile").assert_array().is_empty());
		assert_eq!(field(&attachment, DraftAttachment::type_ref(), "ownerKeyVersion").assert_number(), 3);
		let owner_enc = field(&attachment, DraftAttachment::type_ref(), "ownerEncFileSessionKey");
		assert_eq!(group_key.object.decrypt_aes_key(owner_enc.assert_bytes()).unwrap(), file_key);
		let new_file = field(&attachment, DraftAttachment::type_ref(), "newFile").assert_array()[0].assert_dict();
		assert_eq!(decrypt(&file_key, field(&new_file, NewDraftAttachment::type_ref(), "encFileName")).unwrap(), b"report.pdf");
		assert_eq!(decrypt(&file_key, field(&new_file, NewDraftAttachment::type_ref(), "encMimeType")).unwrap(), b"application/pdf");

		let reply_to = field(&data, DraftData::type_ref(), "replyTos").assert_array()[0].assert_dict();
		assert_eq!(decrypt(&mail_key, field(&reply_to, EncryptedMailAddress::type_ref(), "name")).unwrap(), b"Reply Here");
		// Unlike a recipient's, a reply-to's address is encrypted too.
		assert_eq!(decrypt(&mail_key, field(&reply_to, EncryptedMailAddress::type_ref(), "address")).unwrap(), b"reply@tuta.com");
		let to = field(&data, DraftData::type_ref(), "toRecipients").assert_array()[0].assert_dict();
		assert_eq!(field(&to, DraftRecipient::type_ref(), "mailAddress").assert_string(), "ann@tuta.com");
	}

	#[test]
	fn contacts_read_as_first_and_last_name_with_their_addresses() {
		use crate::entities::generated::tutanota::ContactMailAddress;
		let address = |a: &str| ContactMailAddress {
			_id: None,
			r#type: 0,
			address: a.into(),
			customTypeName: String::new(),
			_errors: Default::default(),
		};
		let mut contact: Contact = crate::util::test_utils::create_test_entity();
		contact.firstName = " Ann ".into();
		contact.lastName = "Lee".into();
		contact.mailAddresses = vec![address("ann@tuta.com"), address("  "), address("ann@work.org")];
		assert_eq!(
			tern_contact(&contact),
			TernContact {
				name: "Ann Lee".into(),
				addresses: vec!["ann@tuta.com".into(), "ann@work.org".into()],
			}
		);
		contact.firstName = String::new();
		assert_eq!(tern_contact(&contact).name, "Lee");
	}

	/// The batched open: mails, bodies and files grouped by list (archive for bodies), each group
	/// in one GET with the ids comma-joined, credentials first on blob servers.
	#[test]
	fn batched_requests_group_by_list_and_join_ids() {
		let id = |list: &str, element: &str| IdTupleGenerated { list_id: GeneratedId(list.into()), element_id: GeneratedId(element.into()) };
		let ids = [id("bagA", "m1"), id("bagB", "m2"), id("bagA", "m3"), id("bagA", "m1")];
		let groups = by_list(&ids);
		let as_str = |group: &(GeneratedId, Vec<GeneratedId>)| (group.0.to_string(), group.1.iter().map(|e| e.to_string()).collect::<Vec<_>>());
		assert_eq!(groups.iter().map(as_str).collect::<Vec<_>>(), [("bagA".into(), vec!["m1".to_string(), "m3".into()]), ("bagB".into(), vec!["m2".into()])]);

		let (list, elements) = &groups[0];
		assert_eq!(
			elements_url("https://app.tuta.com", &Mail::type_ref(), "Mail", list, vec![], elements),
			"https://app.tuta.com/rest/tutanota/mail/bagA?ids=m1%2Cm3"
		);
		let credentials = vec![("accessToken".to_string(), "t".to_string()), ("v".to_string(), "115".to_string()), ("blobAccessToken".to_string(), "b".to_string())];
		assert_eq!(
			elements_url("https://w1.api.tuta.com", &MailDetailsBlob::type_ref(), "MailDetailsBlob", &GeneratedId("archive".into()), credentials, elements),
			"https://w1.api.tuta.com/rest/tutanota/maildetailsblob/archive?accessToken=t&v=115&blobAccessToken=b&ids=m1%2Cm3"
		);
	}

	#[test]
	fn rate_limits_are_not_failures() {
		let limited = |source| TernError::from(ApiCallError::ServerResponseError { source });
		assert!(matches!(limited(HttpError::TooManyRequestsError { suspension_time_sec: Some(30) }), TernError::RateLimited(Some(30))));
		assert!(matches!(limited(HttpError::TooManyRequestsError { suspension_time_sec: None }), TernError::RateLimited(None)));
		assert!(matches!(limited(HttpError::ServiceUnavailableError { suspension_time_sec: Some(5) }), TernError::RateLimited(Some(5))));
		assert!(matches!(limited(HttpError::ServiceUnavailableError { suspension_time_sec: None }), TernError::Offline));
	}

	#[test]
	fn knows_tuta_addresses() {
		assert!(is_tuta_address("Ann@Tuta.com"));
		assert!(is_tuta_address("x@keemail.me"));
		assert!(!is_tuta_address("x@gmail.com"));
		assert!(!is_tuta_address("x@nottuta.com"));
	}
}
