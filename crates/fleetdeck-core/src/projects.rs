//! `data/projects.md`: the fleet's project registry.
//!
//! Grammar from `bin/fm-project-mode.sh`:
//!
//! ```text
//! - <name> - <desc> (added <date>)
//! - <name> [<mode> +yolo branch=<prefix> forge=gerrit] - <desc> (added <date>)
//! ```
//!
//! A missing mode means `no-mistakes`; `no-mistakes-prod-only` is shown as
//! written.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Project {
    pub name: String,
    pub mode: String,
    pub yolo: bool,
    pub branch_prefix: Option<String>,
    pub forge: Option<String>,
    pub description: String,
}

pub fn parse(text: &str) -> Vec<Project> {
    text.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<Project> {
    let rest = line.trim_end_matches('\r').strip_prefix("- ")?;
    let bracket = rest.find(" [");
    let dash = rest.find(" - ")?;
    let (name, tokens, desc) = match bracket {
        Some(b) if b < dash => {
            let close = rest[b..].find(']')? + b;
            let desc = rest[close + 1..].trim_start().strip_prefix("- ")?;
            (&rest[..b], &rest[b + 2..close], desc)
        }
        _ => (&rest[..dash], "", &rest[dash + 3..]),
    };
    if name.trim().is_empty() {
        return None;
    }
    let mut p = Project {
        name: name.trim().to_string(),
        mode: String::new(),
        yolo: false,
        branch_prefix: None,
        forge: None,
        description: desc.trim().to_string(),
    };
    for t in tokens.split_whitespace() {
        if t == "+yolo" {
            p.yolo = true;
        } else if let Some(v) = t.strip_prefix("branch=") {
            p.branch_prefix = Some(v.to_string());
        } else if let Some(v) = t.strip_prefix("forge=") {
            p.forge = Some(v.to_string());
        } else if p.mode.is_empty() {
            p.mode = t.to_string();
        }
    }
    if p.mode.is_empty() {
        p.mode = "no-mistakes".into();
    }
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_bracketed_projects() {
        let ps = parse(
            "# Fleet projects\n- firstmate - the fleet (added 2026-08-01)\n- fleetdeck [direct-PR +yolo branch=fm/] - TUI (added 2026-10-02; switched by captain)\n- my app [local-only] - spaces in name (added 2026-01-01)\n",
        );
        assert_eq!(ps.len(), 3);
        assert_eq!(ps[0].name, "firstmate");
        assert_eq!(ps[0].mode, "no-mistakes");
        assert!(!ps[0].yolo);
        assert_eq!(ps[1].mode, "direct-PR");
        assert!(ps[1].yolo);
        assert_eq!(ps[1].branch_prefix.as_deref(), Some("fm/"));
        assert_eq!(ps[2].name, "my app");
        assert_eq!(ps[2].mode, "local-only");
    }
}
