//! `config.toml` in the app data dir, plus the per-device ID file.
//!
//! ```toml
//! user_id = "alice"
//! device_name = "alice-laptop"
//! listen_ip = "100.101.102.103"   # this device's Tailscale IPv4
//! listen_port = 7420              # optional, default 7420
//! peers = ["bob-phone", "alice-phone:7420"]  # MagicDNS names, optional :port
//! ```

use serde::{Deserialize, Serialize};
use std::{fs, io, net::IpAddr, path::Path};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub user_id: String,
    pub device_name: String,
    pub listen_ip: IpAddr,
    #[serde(default = "default_port")]
    pub listen_port: u16,
    #[serde(default)]
    pub peers: Vec<String>,
}

fn default_port() -> u16 {
    crate::DEFAULT_PORT
}

pub const FILE: &str = "config.toml";

impl Config {
    /// `Ok(None)` if there is no config yet (first run).
    pub fn load(data_dir: &Path) -> io::Result<Option<Config>> {
        match fs::read_to_string(data_dir.join(FILE)) {
            Ok(s) => toml::from_str(&s)
                .map(Some)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{FILE}: {e}"))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, data_dir: &Path) -> io::Result<()> {
        fs::create_dir_all(data_dir)?;
        fs::write(data_dir.join(FILE), toml::to_string(self).map_err(io::Error::other)?)
    }
}

/// UUID generated on first run and stored in `<data_dir>/device_id`.
pub fn device_id(data_dir: &Path) -> io::Result<String> {
    let path = data_dir.join("device_id");
    match fs::read_to_string(&path) {
        Ok(s) => Ok(s.trim().to_string()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(data_dir)?;
            let id = uuid::Uuid::new_v4().to_string();
            fs::write(&path, &id)?;
            Ok(id)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let c: Config = toml::from_str("user_id='a'\ndevice_name='d'\nlisten_ip='100.64.0.1'").unwrap();
        assert_eq!((c.listen_port, c.peers.len()), (7420, 0));
        let dir = std::env::temp_dir().join(format!("household-cfg-{}", uuid::Uuid::new_v4()));
        assert_eq!(Config::load(&dir).unwrap(), None);
        c.save(&dir).unwrap();
        assert_eq!(Config::load(&dir).unwrap(), Some(c));
        let id = device_id(&dir).unwrap();
        assert_eq!(device_id(&dir).unwrap(), id);
        fs::remove_dir_all(&dir).unwrap();
    }
}
