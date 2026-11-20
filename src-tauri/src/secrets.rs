//! API keys in the operating system's credential store: Windows Credential Manager, or the Secret Service on
//! Linux (GNOME Keyring, KWallet).

use transmogrifier_desktop_core::settings::SecretStore;

const SERVICE: &str = "Transmogrifier";

pub struct KeyringSecrets;

/// The entry for a connection's key.
fn entry(connection: &str) -> Result<keyring::Entry, String> {
    if let Err(error) = keyring::Entry::store_status() {
        return Err(explain(clone_error(error)));
    }
    keyring::Entry::new(SERVICE, &format!("{connection}-api-key")).map_err(explain)
}

// Report the store's initialization error, which Entry::new replaces with NoDefaultStore.
fn clone_error(error: &keyring::Error) -> keyring::Error {
    keyring::Error::Invalid("store".to_string(), error.to_string())
}

fn explain(error: keyring::Error) -> String {
    format!("This computer's credential store isn't available: {error}")
}

impl SecretStore for KeyringSecrets {
    fn get(&self, connection: &str) -> Result<Option<String>, String> {
        match entry(connection)?.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(explain(error)),
        }
    }

    fn set(&self, connection: &str, key: &str) -> Result<(), String> {
        entry(connection)?.set_password(key).map_err(explain)
    }

    fn delete(&self, connection: &str) -> Result<(), String> {
        match entry(connection)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(explain(error)),
        }
    }
}
