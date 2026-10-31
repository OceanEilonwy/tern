# Changes to the vendored Tuta SDK

Copied from the tutanota-desktop-release-362.261023.2 tag (tutao/tutanota@d589cf8c), the release
whose type models (sys 156, tutanota 115) the production servers accept. `master` runs ahead of
production: its sys 157 models get `400 Bad Request` from app.tuta.com. Update to a newer *release*
tag, never to master. GPL-3.0.

- `sdk/src/tern.rs` (new): sign-in that returns credentials and refuses bcrypt/two-factor accounts
  cleanly, folder pages, mail bodies from the blob store, attachment download, drafts with uploaded
  attachments, sending (TutaCrypt/RSA for Tuta recipients, plain for others), session delete. Ported
  from the TypeScript MailFacade, BlobFacade, BlobAccessTokenFacade, CryptoFacade and LoginFacade at
  the same tag.
- `sdk/src/lib.rs`: `LoggedInSdk` keeps the facades `tern.rs` needs (`TernParts`).
- `sdk/src/folder_system.rs`: `FolderSystem::into_folders`.
- `sdk/src/crypto/asymmetric_crypto_facade.rs`: `PubEncSymKey` fields readable in the crate.
- `sdk/src/login/login_facade.rs`: `parse_session_id` readable in the crate.
- `crypto-primitives/src/compatibility_test_utils.rs`: test data path points at `../testdata/`.
- `entities/entity_facade.rs`: encrypted fields holding their default value (empty string, 0, false)
  are encrypted like any other value instead of being sent as an empty string, as the TypeScript
  client at this tag does. Tuta's server decrypts drafts itself for recipients outside Tuta and
  answered 500 to the empty strings.
