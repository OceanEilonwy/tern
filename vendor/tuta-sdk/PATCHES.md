# Changes to the vendored Tuta SDK

Copied from tutao/tutanota@73a3e0e9fd34213e0350572f0507be1e8f68f991 (`tuta-sdk/rust/{sdk,crypto-primitives,util}`), GPL-3.0.

- `sdk/src/tern.rs` (new): sign-in that returns credentials and refuses bcrypt/two-factor accounts cleanly,
  folder pages, mail bodies from the blob store, attachment download, drafts with uploaded attachments,
  sending (TutaCrypt/RSA for Tuta recipients, plain for others), session delete. Ported from the
  TypeScript client's MailFacade, BlobFacade, BlobAccessTokenFacade, CryptoFacade and LoginFacade.
- `sdk/src/entities/entity_facade.rs`: `encrypt_and_map_with_aggregate_keys`, which encrypts each aggregate
  carrying its own `_ownerEncSessionKey` under that key (the TypeScript "WithSessionKey" transfer encryption).
- `sdk/src/lib.rs`: `LoggedInSdk` keeps the facades `tern.rs` needs (`TernParts`).
- `sdk/src/folder_system.rs`: `FolderSystem::into_folders`.
- `sdk/src/crypto/asymmetric_crypto_facade.rs`: `PubEncSymKey` fields readable in the crate.
- `sdk/src/login/login_facade.rs`: `parse_session_id` readable in the crate.
- `crypto-primitives/src/compatibility_test_utils.rs`: test data path points at `../testdata/`.
