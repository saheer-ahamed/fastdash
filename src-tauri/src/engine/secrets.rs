//! Thin wrapper over the OS keychain (`keyring`).
//!
//! Every secret is stored under service `fastdash` with the account key
//! `{connector}/{label}`, e.g. `github/work` or `slack/default`. On Windows this
//! is the Credential Manager (the `windows-native` feature is enabled in
//! `Cargo.toml`). Secrets NEVER touch `config.toml`.

use keyring::Entry;

const SERVICE: &str = "fastdash";

fn entry(connector: &str, label: &str) -> Result<Entry, keyring::Error> {
    Entry::new(SERVICE, &format!("{connector}/{label}"))
}

/// Read a secret. `Ok(None)` means "no such credential", which callers treat as
/// "not configured yet" rather than an error.
pub fn get(connector: &str, label: &str) -> Result<Option<String>, keyring::Error> {
    match entry(connector, label)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(err),
    }
}

/// Create or overwrite the secret for `{connector}/{label}`.
pub fn set(connector: &str, label: &str, value: &str) -> Result<(), keyring::Error> {
    entry(connector, label)?.set_password(value)
}

/// Delete the secret. Deleting a credential that does not exist is a no-op.
pub fn delete(connector: &str, label: &str) -> Result<(), keyring::Error> {
    match entry(connector, label)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err),
    }
}
