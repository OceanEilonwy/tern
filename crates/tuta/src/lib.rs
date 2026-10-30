//! Tern's [`Backend`] for a Tuta account.

use std::sync::Arc;

use tern_core::{Backend, Error, Result};

/// Signs in with an email address and password. Returns the account and a saved session
/// to keep in the keyring, which [`resume`] turns back into the account without the password.
pub async fn login(_email: &str, _password: &str) -> Result<(Arc<dyn Backend>, String)> {
	Err(Error::Other("Tuta sign-in is not built yet".into()))
}

/// Reopens a session saved by [`login`].
pub async fn resume(_saved: &str) -> Result<Arc<dyn Backend>> {
	Err(Error::SessionExpired)
}
