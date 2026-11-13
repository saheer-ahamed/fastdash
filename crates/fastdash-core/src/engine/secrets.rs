//! Thin wrapper over the OS keychain (`keyring`).
//!
//! Every secret is stored under service `fastdash` with the account key
//! `{connector}/{label}`, e.g. `github/work` or `github/personal`. The backing
//! store is the Credential Manager on Windows and Keychain Services on macOS
//! (see the per-target `keyring` features in `Cargo.toml`). Secrets NEVER touch
//! `config.toml`.

#[cfg(any(windows, target_os = "macos"))]
use keyring::Entry;

#[cfg(any(windows, target_os = "macos"))]
type StoreError = keyring::Error;

/// Linux has no `keyring` backend in this crate (see `Cargo.toml`). Calls fail
/// loudly instead of compiling a store that would drop secrets on the floor.
#[cfg(not(any(windows, target_os = "macos")))]
#[derive(Debug)]
pub struct Unsupported;

#[cfg(not(any(windows, target_os = "macos")))]
impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the OS keychain is not available on this platform")
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
impl std::error::Error for Unsupported {}

#[cfg(not(any(windows, target_os = "macos")))]
type StoreError = Unsupported;

#[cfg(not(any(windows, target_os = "macos")))]
struct Entry;

#[cfg(not(any(windows, target_os = "macos")))]
impl Entry {
    fn new(_service: &str, _account: &str) -> Result<Self, StoreError> {
        Err(Unsupported)
    }

    fn get_password(&self) -> Result<String, StoreError> {
        Err(Unsupported)
    }

    fn set_password(&self, _value: &str) -> Result<(), StoreError> {
        Err(Unsupported)
    }

    fn delete_credential(&self) -> Result<(), StoreError> {
        Err(Unsupported)
    }
}

const SERVICE: &str = "fastdash";

fn entry(connector: &str, label: &str) -> Result<Entry, StoreError> {
    Entry::new(SERVICE, &format!("{connector}/{label}"))
}

/// Read a secret. `Ok(None)` means "no such credential", which callers treat as
/// "not configured yet" rather than an error.
pub fn get(connector: &str, label: &str) -> Result<Option<String>, StoreError> {
    match entry(connector, label)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        #[cfg(any(windows, target_os = "macos"))]
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(err),
    }
}

/// Create or overwrite the secret for `{connector}/{label}`.
pub fn set(connector: &str, label: &str, value: &str) -> Result<(), StoreError> {
    entry(connector, label)?.set_password(value)
}

/// Delete the secret. Deleting a credential that does not exist is a no-op.
pub fn delete(connector: &str, label: &str) -> Result<(), StoreError> {
    match entry(connector, label)?.delete_credential() {
        Ok(()) => Ok(()),
        #[cfg(any(windows, target_os = "macos"))]
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err),
    }
}
