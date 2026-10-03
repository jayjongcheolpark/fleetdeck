//! `state/<id>.meta`: one task's `key=value` metadata.
//!
//! `bin/fm-spawn.sh` writes the base fields; later writers append keys such
//! as `pr=` (`bin/fm-pr-check.sh`). When a key repeats, the last line wins.

use serde::Serialize;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Meta {
    /// Every `key=value` pair in file order, repeats included.
    pub pairs: Vec<(String, String)>,
}

impl Meta {
    pub fn parse(text: &str) -> Meta {
        let pairs = text
            .lines()
            .filter_map(|l| {
                let l = l.trim_end_matches('\r');
                let (k, v) = l.split_once('=')?;
                let k = k.trim();
                if k.is_empty() || k.starts_with('#') || k.contains(char::is_whitespace) {
                    return None;
                }
                Some((k.to_string(), v.to_string()))
            })
            .collect();
        Meta { pairs }
    }

    /// The last value of `key`, if any and not empty.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.pairs
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    }

    /// `ship`, `scout` or `secondmate`; a missing key means `ship`.
    pub fn kind(&self) -> &str {
        self.get("kind").unwrap_or("ship")
    }

    /// The runtime backend; a missing key means `tmux`.
    pub fn backend(&self) -> &str {
        self.get("backend").unwrap_or("tmux")
    }

    pub fn is_remote(&self) -> bool {
        self.get("remote_host").is_some()
    }

    pub fn pr(&self) -> Option<&str> {
        self.get("pr")
    }

    /// The backend endpoint: `terminal=` for Orca, else `window=`.
    pub fn endpoint(&self) -> Option<&str> {
        if self.backend() == "orca" {
            self.get("terminal").or_else(|| self.get("window"))
        } else {
            self.get("window")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_value_wins_and_defaults_apply() {
        let m = Meta::parse("window=w1\nkind=scout\npr=https://a/pull/1\npr=https://a/pull/2\n");
        assert_eq!(m.get("window"), Some("w1"));
        assert_eq!(m.kind(), "scout");
        assert_eq!(m.backend(), "tmux");
        assert_eq!(m.pr(), Some("https://a/pull/2"));
        assert!(!m.is_remote());
        let m = Meta::parse("window=x\n");
        assert_eq!(m.kind(), "ship");
    }

    #[test]
    fn values_keep_equals_signs() {
        let m = Meta::parse("traceparent=00-a=b\n");
        assert_eq!(m.get("traceparent"), Some("00-a=b"));
    }
}
