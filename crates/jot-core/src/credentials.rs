// Copyright 2026 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! The Gemini API key, held in Windows Credential Manager.
//!
//! The key is the one secret Jot stores. It never lands in `settings.json`, is
//! never logged, and is only ever sent to the configured Gemini endpoint.

use anyhow::{Context, Result};

const SERVICE: &str = "com.ammaar.jot";
const ACCOUNT: &str = "gemini-api-key";

fn entry() -> Result<keyring::Entry> {
    keyring::Entry::new(&service_for(crate::test_profile::root()), ACCOUNT)
        .context("opening the credential store")
}

/// Returns `None` when no key has been stored yet — a first launch, or after
/// the user cleared it. A credential-store error is logged and also read as
/// "no key": failing the dictation with a storage error would be less useful
/// than the onboarding prompt the caller shows for a missing key.
pub fn api_key() -> Option<String> {
    match entry().and_then(|entry| entry.get_password().map_err(Into::into)) {
        Ok(key) if !key.trim().is_empty() => Some(key),
        Ok(_) => None,
        Err(error) => {
            // The message never contains the secret — only why the store failed.
            tracing::debug!(%error, "no Gemini API key available");
            None
        }
    }
}

pub fn set_api_key(key: &str) -> Result<()> {
    entry()?
        .set_password(key)
        .context("writing the API key to the credential store")
}

pub fn clear_api_key() -> Result<()> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error).context("clearing the API key"),
    }
}

// Distinct Credential Manager target per portable folder; never copy a real key.
fn service_for(root: Option<&std::path::Path>) -> String {
    use std::hash::{Hash, Hasher};
    match root {
        None => SERVICE.to_string(),
        Some(root) => {
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            root.hash(&mut hash);
            format!("{SERVICE}.test.{:016x}", hash.finish())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_credentials_are_separate_and_stable() {
        let a = std::path::Path::new("build-a/profile");
        let b = std::path::Path::new("build-b/profile");
        assert_eq!(service_for(None), SERVICE);
        assert_ne!(service_for(Some(a)), service_for(None));
        assert_ne!(service_for(Some(a)), service_for(Some(b)));
        assert_eq!(service_for(Some(a)), service_for(Some(a)));
    }
}
