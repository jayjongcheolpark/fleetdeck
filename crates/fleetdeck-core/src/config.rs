//! The fleetdeck config file: which homes to show and how often to refresh.
//!
//! ```toml
//! refresh_secs = 30
//! timeout_secs = 15
//!
//! [[home]]
//! name = "main"
//! path = "/Users/me/firstmate"
//!
//! [[home]]
//! name = "devbox"
//! host = "devbox"        # an ssh alias; omit for a home on this machine
//! path = "~/firstmate"
//! discover = true        # also show second mates from data/secondmates.md
//! ```

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Where a home lives.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Location {
    Local(PathBuf),
    /// `path` is as given; a leading `~` expands on the remote host.
    Ssh {
        host: String,
        path: String,
    },
}

impl Location {
    pub fn path_str(&self) -> String {
        match self {
            Location::Local(p) => p.display().to_string(),
            Location::Ssh { path, .. } => path.clone(),
        }
    }

    pub fn host(&self) -> Option<&str> {
        match self {
            Location::Local(_) => None,
            Location::Ssh { host, .. } => Some(host),
        }
    }

    /// A location for another path on the same machine.
    pub fn sibling(&self, path: &str) -> Location {
        match self {
            Location::Local(_) => Location::Local(PathBuf::from(path)),
            Location::Ssh { host, .. } => Location::Ssh {
                host: host.clone(),
                path: path.to_string(),
            },
        }
    }

    pub fn label(&self) -> String {
        match self {
            Location::Local(p) => p.display().to_string(),
            Location::Ssh { host, path } => format!("{host}:{path}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeSpec {
    pub name: String,
    pub location: Location,
    pub discover: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub homes: Vec<HomeSpec>,
    pub refresh_secs: u64,
    pub timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    refresh_secs: Option<u64>,
    timeout_secs: Option<u64>,
    #[serde(default)]
    home: Vec<RawHome>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHome {
    name: Option<String>,
    path: String,
    host: Option<String>,
    discover: Option<bool>,
}

pub const DEFAULT_REFRESH_SECS: u64 = 30;
pub const DEFAULT_TIMEOUT_SECS: u64 = 15;

impl Config {
    pub fn parse(text: &str) -> Result<Config, String> {
        let raw: RawConfig = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut homes = Vec::new();
        for h in raw.home {
            if h.path.trim().is_empty() {
                return Err("a [[home]] entry has an empty path".into());
            }
            let location = match h.host {
                Some(host) if !host.trim().is_empty() => Location::Ssh {
                    host,
                    path: h.path.clone(),
                },
                _ => Location::Local(expand_tilde(&h.path)),
            };
            let name = h.name.unwrap_or_else(|| default_name(&location));
            homes.push(HomeSpec {
                name,
                location,
                discover: h.discover.unwrap_or(true),
            });
        }
        Ok(Config {
            homes,
            refresh_secs: raw.refresh_secs.unwrap_or(DEFAULT_REFRESH_SECS).max(1),
            timeout_secs: raw.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS).max(1),
        })
    }

    pub fn load(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Config::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// A config with one local home and default settings.
    pub fn single_local(path: PathBuf) -> Config {
        let location = Location::Local(path);
        Config {
            homes: vec![HomeSpec {
                name: default_name(&location),
                location,
                discover: true,
            }],
            refresh_secs: DEFAULT_REFRESH_SECS,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }
}

/// `$XDG_CONFIG_HOME/fleetdeck/config.toml`, else `~/.config/fleetdeck/config.toml`.
pub fn default_config_path() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(x).join("fleetdeck/config.toml"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/fleetdeck/config.toml"))
}

fn expand_tilde(p: &str) -> PathBuf {
    if (p == "~" || p.starts_with("~/"))
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(p.trim_start_matches('~').trim_start_matches('/'));
    }
    PathBuf::from(p)
}

fn default_name(location: &Location) -> String {
    let path = location.path_str();
    let base = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("home")
        .to_string();
    match location.host() {
        Some(h) => format!("{h}:{base}"),
        None => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_and_remote_homes() {
        let cfg = Config::parse(
            r#"
refresh_secs = 10
[[home]]
name = "main"
path = "/Users/me/firstmate"
[[home]]
host = "devbox"
path = "~/firstmate"
discover = false
"#,
        )
        .unwrap();
        assert_eq!(cfg.refresh_secs, 10);
        assert_eq!(cfg.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert_eq!(cfg.homes.len(), 2);
        assert_eq!(cfg.homes[0].name, "main");
        assert_eq!(
            cfg.homes[0].location,
            Location::Local(PathBuf::from("/Users/me/firstmate"))
        );
        assert!(cfg.homes[0].discover);
        assert_eq!(cfg.homes[1].name, "devbox:firstmate");
        assert_eq!(
            cfg.homes[1].location,
            Location::Ssh {
                host: "devbox".into(),
                path: "~/firstmate".into()
            }
        );
        assert!(!cfg.homes[1].discover);
    }

    #[test]
    fn rejects_unknown_keys_and_empty_paths() {
        assert!(Config::parse("[[home]]\npath = \"/x\"\nwrite = true\n").is_err());
        assert!(Config::parse("[[home]]\npath = \"\"\n").is_err());
    }

    #[test]
    fn empty_config_has_defaults() {
        let cfg = Config::parse("").unwrap();
        assert!(cfg.homes.is_empty());
        assert_eq!(cfg.refresh_secs, DEFAULT_REFRESH_SECS);
    }
}
