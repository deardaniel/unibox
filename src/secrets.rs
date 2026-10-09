//! Where app passwords live. On Windows that is the Credential Manager; the
//! Linux development build keeps them in a private file next to the database.

use anyhow::Result;
use std::path::Path;

#[cfg(windows)]
mod imp {
    use super::*;
    const SERVICE: &str = "Unibox";

    pub fn set(_dir: &Path, email: &str, password: &str) -> Result<()> {
        Ok(keyring::Entry::new(SERVICE, email)?.set_password(password)?)
    }
    pub fn get(_dir: &Path, email: &str) -> Result<String> {
        Ok(keyring::Entry::new(SERVICE, email)?.get_password()?)
    }
    pub fn delete(_dir: &Path, email: &str) -> Result<()> {
        Ok(keyring::Entry::new(SERVICE, email)?.delete_credential()?)
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    use anyhow::anyhow;
    use std::collections::BTreeMap;
    use std::os::unix::fs::OpenOptionsExt;

    fn load(dir: &Path) -> BTreeMap<String, String> {
        std::fs::read(dir.join("secrets.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    fn save(dir: &Path, map: &BTreeMap<String, String>) -> Result<()> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(dir.join("secrets.json"))?;
        f.write_all(&serde_json::to_vec(map)?)?;
        Ok(())
    }
    pub fn set(dir: &Path, email: &str, password: &str) -> Result<()> {
        let mut m = load(dir);
        m.insert(email.to_string(), password.to_string());
        save(dir, &m)
    }
    pub fn get(dir: &Path, email: &str) -> Result<String> {
        load(dir).remove(email).ok_or_else(|| anyhow!("no saved password for {email}"))
    }
    pub fn delete(dir: &Path, email: &str) -> Result<()> {
        let mut m = load(dir);
        m.remove(email);
        save(dir, &m)
    }
}

pub use imp::{delete, get, set};
