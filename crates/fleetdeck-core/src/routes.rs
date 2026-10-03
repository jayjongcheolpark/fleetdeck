//! `data/secondmates.md`: the registry of persistent second mates.
//!
//! One line per route (regexes from `bin/fm-secondmate-registry-lib.sh`,
//! local form tried first):
//!
//! ```text
//! - <id> - <summary> (home: <abs>; scope: <text>; projects: <a>, <b>; added YYYY-MM-DD)
//! - <id> - <summary> (host: <alias>; root: <abs>; home: <abs>; scope: <text>; projects: <a>; added YYYY-MM-DD)
//! ```

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Route {
    pub id: String,
    pub summary: String,
    /// The ssh alias for a remote route.
    pub host: Option<String>,
    /// The remote host's firstmate code root.
    pub root: Option<String>,
    pub home: String,
    pub scope: String,
    pub projects: Vec<String>,
    pub added: String,
}

static LOCAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^- ([A-Za-z0-9._-]+) - (.+) \(home:\s*([^;)]*);\s*scope:\s*(.*);\s*projects:\s*([^;)]*);\s*added\s+([0-9]{4}-[0-9]{2}-[0-9]{2})\)\s*$",
    )
    .unwrap()
});
static REMOTE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^- ([A-Za-z0-9._-]+) - (.+) \(host:\s*([^;)]*);\s*root:\s*([^;)]*);\s*home:\s*([^;)]*);\s*scope:\s*(.*);\s*projects:\s*([^;)]*);\s*added\s+([0-9]{4}-[0-9]{2}-[0-9]{2})\)\s*$",
    )
    .unwrap()
});

fn projects(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn parse(text: &str) -> Vec<Route> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim_end_matches('\r');
            if let Some(c) = LOCAL_RE.captures(l) {
                return Some(Route {
                    id: c[1].to_string(),
                    summary: c[2].trim().to_string(),
                    host: None,
                    root: None,
                    home: c[3].trim().to_string(),
                    scope: c[4].trim().to_string(),
                    projects: projects(&c[5]),
                    added: c[6].to_string(),
                });
            }
            let c = REMOTE_RE.captures(l)?;
            Some(Route {
                id: c[1].to_string(),
                summary: c[2].trim().to_string(),
                host: Some(c[3].trim().to_string()),
                root: Some(c[4].trim().to_string()),
                home: c[5].trim().to_string(),
                scope: c[6].trim().to_string(),
                projects: projects(&c[7]),
                added: c[8].to_string(),
            })
        })
        .filter(|r| r.home.starts_with('/'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_route_with_parentheses_in_summary() {
        let r = parse(
            "- shop-mate - Own the shop work (web; team 7). Escalate. (home: /h/1/firstmate; scope: app; billing (refunds); projects: shop; added 2026-08-05)\n",
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "shop-mate");
        assert_eq!(r[0].home, "/h/1/firstmate");
        assert_eq!(r[0].scope, "app; billing (refunds)");
        assert_eq!(r[0].projects, ["shop"]);
        assert_eq!(r[0].host, None);
    }

    #[test]
    fn parses_remote_route() {
        let r = parse(
            "- bot - Own the bot. (host: devbox; root: /home/u/firstmate; home: /home/u/secondmates/bot; scope: bot work; projects: a, b; added 2026-09-30)\n",
        );
        assert_eq!(r[0].host.as_deref(), Some("devbox"));
        assert_eq!(r[0].root.as_deref(), Some("/home/u/firstmate"));
        assert_eq!(r[0].home, "/home/u/secondmates/bot");
        assert_eq!(r[0].projects, ["a", "b"]);
    }

    #[test]
    fn ignores_prose_and_relative_homes() {
        assert!(parse("# Second mates\nsome prose\n").is_empty());
        assert!(
            parse("- x - y (home: rel/path; scope: s; projects: p; added 2026-01-01)\n").is_empty()
        );
    }
}
