# Changes to the vendored Tuta SDK

Copied from the tutanota-desktop-release-362.261023.2 tag (tutao/tutanota@d589cf8c), the release
whose type models (sys 156, tutanota 115) the production servers accept. `master` runs ahead of
production: its sys 157 models get `400 Bad Request` from app.tuta.com. Update to a newer *release*
tag, never to master. GPL-3.0.

- `sdk/src/tern.rs` (new): sign-in that returns credentials and refuses bcrypt/two-factor accounts
  cleanly, folder pages, mail bodies from the blob store, attachment download, drafts with uploaded
  attachments, saving drafts and updating saved ones (DraftService PUT, keeping, removing and adding
  attachments), sending new or saved drafts (TutaCrypt/RSA for Tuta recipients, plain for others),
  loading the contact list, session delete. Ported from the TypeScript MailFacade, BlobFacade,
  BlobAccessTokenFacade, CryptoFacade, LoginFacade and ContactModel at the same tag.
- `sdk/src/lib.rs`: `LoggedInSdk` keeps the facades `tern.rs` needs (`TernParts`).
- `sdk/src/folder_system.rs`: `FolderSystem::into_folders`.
- `sdk/src/crypto/asymmetric_crypto_facade.rs`: `PubEncSymKey` fields readable in the crate.
- `sdk/src/login/login_facade.rs`: `parse_session_id` readable in the crate.
- `crypto-primitives/src/compatibility_test_utils.rs`: test data path points at `../testdata/`.
- `sdk/src/entities/entity_facade.rs`: encrypted fields holding their default value (empty string, 0, false)
  are encrypted like any other value instead of being sent as an empty string, as the TypeScript
  client at this tag does. Tuta's server decrypts drafts itself for recipients outside Tuta and
  answered 500 to the empty strings.
- `sdk/src/tern.rs`: batched open (`open_many`): mails, MailDetailsBlobs (one archive read token, ids
  comma-joined, as `EntityRestClient.loadMultipleBlobElements` at the same tag), MailDetailsDrafts and
  files loaded several per request. Key pairs are kept per group and version instead of loading the
  Group for every bucket key; archive read tokens are kept until they expire and evicted on 403, as
  `BlobAccessTokenFacade` does. 429, and 503 with a suspension time, become `TernError::RateLimited`.
- `sdk/src/mail_facade.rs`: `move_mails` (MoveMailService, to any mail set by id) and `apply_label`
  (ApplyLabelService), in batches of `MAX_MAIL_UPDATE_LIMIT` like the simple move beside them;
  `tern.rs` exposes both.
- `sdk/src/folder_system.rs`: `MailSetKind::Label` (8), so labels are told apart from unknown sets.
- `sdk/src/tern.rs`: `create_label` (ManageLabelService POST), with a new session key encrypted with
  the mailbox key, as `MailModel.createLabel` at the same tag.
- `sdk/src/tern.rs`: `TernAccount.aliases`, the mailbox's enabled alias addresses; a draft is sent
  from one by setting `mail_address` to it. A personal mailbox's addresses are read from the user's
  GroupInfo, a shared one's from the mail group's, as `get_group_id_for_mail_address` does.
- Build warnings, without changing behaviour:
  - `sdk/src/lib.rs`: the crate-level `#![macro_use]`, which did nothing, is gone.
  - `sdk/src/instance_mapper.rs`: `Unexpected<'_>` spelled out.
  - `sdk/src/blobs/binary_blob_wrapper_serializer.rs`: `BinaryBlobWrapperSerializationError` is
    `#[cfg(test)]`, like the deserializers that return it.
  - `crypto-primitives/Cargo.toml`: takes the workspace's lints, like `sdk` and `util`. The clippy
    style lints upstream code trips are allowed in the root `Cargo.toml`.
