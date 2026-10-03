//! `data/backlog.md`: the tasks-axi markdown backlog.
//!
//! The grammar follows tasks-axi's markdown backend, which firstmate's
//! `docs/configuration.md` names as the owner:
//!
//! - `## In flight`, `## Queued` and `## Done…` headers open sections (case
//!   does not matter, and the order varies by home).
//! - `- [ ] <id> - <title>` and `- [x] <id> - <title>` are items; the legacy
//!   `- **<id>** - <title>` form is also accepted.
//! - Lines that are blank or indented by two spaces belong to the item body.
//! - Tags such as `(repo: x)`, `(kind: ship)`, `(since 2026-10-02)`,
//!   `(hold: why)`, `(hold-kind: captain)` and `(hold-until: 2026-10-09)`
//!   are stripped from the end of the title line, rightmost wins.

use std::sync::LazyLock;

use base64::Engine;
use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    InFlight,
    Queued,
    Done,
    Other,
}

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::InFlight => "In flight",
            Section::Queued => "Queued",
            Section::Done => "Done",
            Section::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub checked: bool,
    pub repo: Option<String>,
    pub kind: Option<String>,
    pub priority: Option<u8>,
    pub since: Option<String>,
    /// `(merged|reported|done|closed DATE)` as (verb, date).
    pub closed: Option<(String, String)>,
    pub hold: Option<String>,
    pub hold_kind: Option<String>,
    pub hold_until: Option<String>,
    /// From the first body line `Captain hold set: …`.
    pub hold_set: Option<String>,
    pub blocked_by: Vec<String>,
    pub pr_urls: Vec<String>,
    pub body: String,
    /// 0-based line of the item in the file.
    pub line_no: usize,
}

impl Item {
    /// An active hold: present, not done, and not past `hold-until`.
    pub fn held(&self, section: Section, today: &str) -> bool {
        self.hold.is_some()
            && section != Section::Done
            && self.hold_until.as_deref().is_none_or(|u| u > today)
    }

    /// An open captain call: a captain hold on a row that is not done.
    pub fn captain_hold(&self, section: Section) -> bool {
        section != Section::Done && self.hold_kind.as_deref() == Some("captain")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SectionItems {
    pub section: Section,
    pub heading: String,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Backlog {
    pub sections: Vec<SectionItems>,
}

impl Backlog {
    pub fn items(&self, section: Section) -> impl Iterator<Item = &Item> {
        self.sections
            .iter()
            .filter(move |s| s.section == section)
            .flat_map(|s| s.items.iter())
    }

    pub fn count(&self, section: Section) -> usize {
        self.items(section).count()
    }

    pub fn find(&self, id: &str) -> Option<(Section, &Item)> {
        self.sections
            .iter()
            .flat_map(|s| s.items.iter().map(move |i| (s.section, i)))
            .find(|(_, i)| i.id == id)
    }
}

const ID: &str = r"[A-Za-z0-9][A-Za-z0-9._-]*";
const DATE: &str = r"\d{4}-\d{2}-\d{2}";

static ITEM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^- \[([ x])\] ({ID}) - (.*)$")).unwrap());
static LEGACY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^- \*\*({ID})\*\* - (.*)$")).unwrap());
static PR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https://[A-Za-z0-9.-]+/[A-Za-z0-9._-]+/[A-Za-z0-9._-]+/pulls?/[1-9][0-9]*")
        .unwrap()
});
static HOLD_SET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^Captain hold set:\s*({DATE}(?:T\d{{2}}:\d{{2}}:\d{{2}}Z)?)$"
    ))
    .unwrap()
});
static BARE_DEP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"blocked-by:\s+({ID})")).unwrap());

enum Tag {
    Dep(String),
    Repo(String),
    Kind(String),
    Priority(u8),
    Since(String),
    Closed(String, String),
    Hold(String),
    HoldKind(String),
    HoldUntil(String),
}

/// A tail-tag pattern and how to turn its captures into a [`Tag`].
type TagRule = (Regex, fn(&regex::Captures) -> Tag);

static TAIL_RES: LazyLock<Vec<TagRule>> = LazyLock::new(|| {
    let t = |p: String| Regex::new(&p).unwrap();
    vec![
        (
            t(format!(
                r"\s*(?:blocked-by|parent|discovered-from):\s*({ID})(?:\s+-\s+.*)?\s*$"
            )),
            (|c: &regex::Captures| Tag::Dep(c[1].to_string())) as fn(&regex::Captures) -> Tag,
        ),
        (
            t(r"\s*\((?:[^()]*\+\s*)?repo:\s*([^)]+)\)\s*$".into()),
            |c| Tag::Repo(c[1].trim().to_string()),
        ),
        (t(r"\s*\(kind:\s*([^)]+)\)\s*$".into()), |c| {
            Tag::Kind(c[1].trim().to_string())
        }),
        (t(r"\s*\(priority:\s*([0-4])\)\s*$".into()), |c| {
            Tag::Priority(c[1].parse().unwrap_or(0))
        }),
        (t(format!(r"\s*\(since\s+({DATE})\)\s*$")), |c| {
            Tag::Since(c[1].to_string())
        }),
        (
            t(format!(
                r"\s*\((merged|reported|done|closed)\s+({DATE})\)\s*$"
            )),
            |c| Tag::Closed(c[1].to_string(), c[2].to_string()),
        ),
        (t(r"\s*\(hold:\s*([^()]+)\)\s*$".into()), |c| {
            Tag::Hold(c[1].trim().to_string())
        }),
        (
            t(r"\s*\(hold-kind:\s*(captain|external|load|parked|future)\)\s*$".into()),
            |c| Tag::HoldKind(c[1].to_string()),
        ),
        (t(format!(r"\s*\(hold-until:\s*({DATE})\)\s*$")), |c| {
            Tag::HoldUntil(c[1].to_string())
        }),
    ]
});

/// Decodes an `fm-hold-v1:<base64>` hold reason; other text is returned as is.
pub fn decode_hold_reason(reason: &str) -> String {
    if let Some(b64) = reason.strip_prefix("fm-hold-v1:") {
        let engine = base64::engine::general_purpose::STANDARD;
        if let Ok(bytes) = engine.decode(b64.trim())
            && engine.encode(&bytes) == b64.trim()
            && let Ok(s) = String::from_utf8(bytes)
        {
            return s;
        }
    }
    reason.to_string()
}

fn apply_tags(item: &mut Item, line: &str) {
    let mut rest = line.trim_end().to_string();
    // Tags are taken from the end; the first match seen is the rightmost one.
    'outer: loop {
        for (re, make) in TAIL_RES.iter() {
            if let Some(c) = re.captures(&rest) {
                let start = c.get(0).unwrap().start();
                match make(&c) {
                    Tag::Dep(d) => item.blocked_by.insert(0, d),
                    Tag::Repo(v) => {
                        item.repo.get_or_insert(v);
                    }
                    Tag::Kind(v) => {
                        item.kind.get_or_insert(v);
                    }
                    Tag::Priority(v) => {
                        item.priority.get_or_insert(v);
                    }
                    Tag::Since(v) => {
                        item.since.get_or_insert(v);
                    }
                    Tag::Closed(a, b) => {
                        item.closed.get_or_insert((a, b));
                    }
                    Tag::Hold(v) => {
                        item.hold.get_or_insert(decode_hold_reason(&v));
                    }
                    Tag::HoldKind(v) => {
                        item.hold_kind.get_or_insert(v);
                    }
                    Tag::HoldUntil(v) => {
                        item.hold_until.get_or_insert(v);
                    }
                }
                rest.truncate(start);
                continue 'outer;
            }
        }
        break;
    }
    // Bare deps can sit between the title and the tags.
    for c in BARE_DEP_RE.captures_iter(&rest) {
        if !item.blocked_by.contains(&c[1].to_string()) {
            item.blocked_by.push(c[1].to_string());
        }
    }
    item.title = rest.trim().to_string();
    if item.kind.is_none() {
        let t = item.title.as_str();
        item.kind = [
            ("PERSISTENT SECONDMATE", "secondmate"),
            ("SHIP", "ship"),
            ("SCOUT", "scout"),
            ("DOCS-ONLY", "docs"),
        ]
        .iter()
        .find(|(p, _)| t.starts_with(p))
        .map(|(_, k)| k.to_string());
    }
}

fn section_of(heading: &str) -> Section {
    let h = heading.trim().to_lowercase();
    if h == "in flight" {
        Section::InFlight
    } else if h == "queued" {
        Section::Queued
    } else if h.starts_with("done") {
        Section::Done
    } else {
        Section::Other
    }
}

pub fn parse(text: &str) -> Backlog {
    let mut sections: Vec<SectionItems> = Vec::new();
    let mut current: Option<Item> = None;
    let finish = |sections: &mut Vec<SectionItems>, item: Option<Item>| {
        if let (Some(mut it), Some(sec)) = (item, sections.last_mut()) {
            let trimmed = it.body.trim_end_matches('\n').to_string();
            it.body = trimmed;
            if let Some(first) = it.body.lines().next()
                && let Some(c) = HOLD_SET_RE.captures(first.trim())
            {
                it.hold_set = Some(c[1].to_string());
            }
            for m in PR_RE.find_iter(&it.body) {
                if !it.pr_urls.iter().any(|u| u == m.as_str()) {
                    it.pr_urls.push(m.as_str().to_string());
                }
            }
            sec.items.push(it);
        }
    };
    for (no, raw) in text.lines().enumerate() {
        let line = raw.trim_end_matches('\r');
        if let Some(h) = line.strip_prefix("##")
            && h.starts_with(char::is_whitespace)
        {
            finish(&mut sections, current.take());
            sections.push(SectionItems {
                section: section_of(h),
                heading: h.trim().to_string(),
                items: Vec::new(),
            });
            continue;
        }
        if sections.is_empty() {
            continue; // preamble such as `# Backlog`
        }
        let parsed = if let Some(c) = ITEM_RE.captures(line) {
            Some((c[1].eq("x"), c[2].to_string(), c[3].to_string()))
        } else {
            LEGACY_RE
                .captures(line)
                .map(|c| (false, c[1].to_string(), c[2].to_string()))
        };
        if let Some((checked, id, rest)) = parsed {
            finish(&mut sections, current.take());
            let mut item = Item {
                id,
                checked,
                line_no: no,
                ..Item::default()
            };
            for m in PR_RE.find_iter(&rest) {
                item.pr_urls.push(m.as_str().to_string());
            }
            apply_tags(&mut item, &rest);
            current = Some(item);
            continue;
        }
        if let Some(item) = current.as_mut()
            && (line.is_empty() || line.starts_with("  "))
        {
            item.body.push_str(line.strip_prefix("  ").unwrap_or(line));
            item.body.push('\n');
            continue;
        }
        // Any other column-0 line ends the item.
        finish(&mut sections, current.take());
    }
    finish(&mut sections, current.take());
    Backlog { sections }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Backlog

## In flight
- [ ] fleetdeck-v1 - fleetdeck: first version (repo: fleetdeck) (kind: ship) (since 2026-10-02)
  Mode direct-PR, yolo on.
- [ ] slot-guard - Pool hands slots out (repo: firstmate). Seen twice (kind: ship) (since 2026-08-12) (hold: captain call) (hold-kind: captain)
  Captain hold set: 2026-09-10T02:52:57Z
## Queued
- [ ] nm-scan - fm_nm_field scan is order-dependent (repo: firstmate) (kind: ship) (since 2026-08-08) (hold: deferred) (hold-kind: parked) (hold-until: 2026-08-01)
  Found by review.

  ## Not a section, an indented heading
- [ ] dep-row - writer paths blocked-by: nm-scan (repo: shop) (kind: ship) (priority: 2) (since 2026-08-21)
- **legacy-row** - legacy bullet (kind: scout)
## Done
- [x] lamp - follow main https://github.com/a/fleet-tools/pull/9 (repo: fleet-tools) (kind: ship) (merged 2026-10-02)
";

    #[test]
    fn parses_sections_and_items() {
        let b = parse(SAMPLE);
        assert_eq!(b.sections.len(), 3);
        assert_eq!(b.count(Section::InFlight), 2);
        assert_eq!(b.count(Section::Queued), 3);
        assert_eq!(b.count(Section::Done), 1);
        let (_, f) = b.find("fleetdeck-v1").unwrap();
        assert_eq!(f.title, "fleetdeck: first version");
        assert_eq!(f.repo.as_deref(), Some("fleetdeck"));
        assert_eq!(f.kind.as_deref(), Some("ship"));
        assert_eq!(f.since.as_deref(), Some("2026-10-02"));
        assert_eq!(f.body, "Mode direct-PR, yolo on.");
    }

    #[test]
    fn mid_title_parentheses_stay_prose_and_holds_parse() {
        let b = parse(SAMPLE);
        let (sec, s) = b.find("slot-guard").unwrap();
        assert_eq!(
            s.title,
            "Pool hands slots out (repo: firstmate). Seen twice"
        );
        assert_eq!(s.hold.as_deref(), Some("captain call"));
        assert_eq!(s.hold_kind.as_deref(), Some("captain"));
        assert_eq!(s.hold_set.as_deref(), Some("2026-09-10T02:52:57Z"));
        assert!(s.captain_hold(sec));
        assert!(s.held(sec, "2026-10-02"));
    }

    #[test]
    fn expired_hold_until_is_not_held() {
        let b = parse(SAMPLE);
        let (sec, n) = b.find("nm-scan").unwrap();
        assert_eq!(n.hold_until.as_deref(), Some("2026-08-01"));
        assert!(!n.held(sec, "2026-10-02"));
        assert!(n.body.contains("## Not a section"));
    }

    #[test]
    fn bare_deps_priority_and_legacy_rows() {
        let b = parse(SAMPLE);
        let (_, d) = b.find("dep-row").unwrap();
        assert_eq!(d.blocked_by, ["nm-scan"]);
        assert_eq!(d.priority, Some(2));
        let (_, l) = b.find("legacy-row").unwrap();
        assert_eq!(l.kind.as_deref(), Some("scout"));
    }

    #[test]
    fn done_rows_carry_closure_and_pr() {
        let b = parse(SAMPLE);
        let (sec, d) = b.find("lamp").unwrap();
        assert_eq!(sec, Section::Done);
        assert!(d.checked);
        assert_eq!(
            d.closed,
            Some(("merged".to_string(), "2026-10-02".to_string()))
        );
        assert_eq!(d.pr_urls, ["https://github.com/a/fleet-tools/pull/9"]);
    }

    #[test]
    fn hold_reason_decodes_only_valid_base64() {
        assert_eq!(decode_hold_reason("fm-hold-v1:aGVsbG8="), "hello");
        assert_eq!(decode_hold_reason("fm-hold-v1:###"), "fm-hold-v1:###");
        assert_eq!(decode_hold_reason("plain"), "plain");
    }
}
