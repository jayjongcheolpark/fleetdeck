//! `state/<id>.status`: the append-only event log of one task.
//!
//! The grammar follows firstmate's `bin/fm-classify-lib.sh`:
//!
//! ```text
//! <verb>[ corr=<16hex>]…[ [name=value]]…: <note>
//! ```
//!
//! Tags sit between the verb and the first colon. `[at=<epoch>]` stamps the
//! emission time; a line without a valid stamp has an unknown time. Open
//! decisions fold by `[key=<slug>]` (default key `default`).

use serde::Serialize;

/// The recognized verbs (`fm-classify-lib.sh` `status_prefix_recognized`).
pub const VERBS: &[&str] = &[
    "working",
    "needs-decision",
    "blocked",
    "done",
    "failed",
    "note",
    "paused",
    "resolved",
    "captain-held",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusEvent {
    /// 0-based line number in the text that was read.
    pub line_no: usize,
    /// The verb word, for example `working`; empty when the line has none.
    pub verb: String,
    /// True when `verb` is one of [`VERBS`].
    pub recognized: bool,
    pub at: Option<i64>,
    pub key: Option<String>,
    pub corr: Option<String>,
    pub note: String,
    pub raw: String,
}

impl StatusEvent {
    /// The decision key; keyless lines use `default`.
    pub fn decision_key(&self) -> &str {
        self.key.as_deref().unwrap_or("default")
    }
}

/// Removes every `[at=…]` run that appears before the first colon, so a
/// malformed `[at=10:30]` cannot move the head boundary.
fn unstamped(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    loop {
        let colon = rest.find(':');
        let at = rest.find("[at=");
        match (at, colon) {
            (Some(a), c) if c.is_none_or(|c| a < c) => {
                out.push_str(&rest[..a]);
                match rest[a..].find(']') {
                    Some(close) => rest = &rest[a + close + 1..],
                    None => {
                        rest = "";
                        break;
                    }
                }
            }
            _ => break,
        }
    }
    out.push_str(rest);
    out
}

fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn is_corr_word(w: &str) -> bool {
    w.strip_prefix("corr=")
        .is_some_and(|h| h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Parses the `[at=N]` stamp from the head (before the first colon).
fn head_at(line: &str) -> Option<i64> {
    // The head ends at the first colon that is not inside an [at=…] run.
    let mut found = None;
    let mut rest = line;
    loop {
        let colon = rest.find(':');
        let Some(a) = rest.find("[at=") else { break };
        if colon.is_some_and(|c| c < a) {
            break;
        }
        let close = rest[a..].find(']')?;
        let value = &rest[a + 4..a + close];
        if found.is_some() {
            return None; // more than one stamp
        }
        found = Some(value.to_string());
        rest = &rest[a + close + 1..];
    }
    let v = found?;
    let ok = v == "0"
        || (!v.is_empty()
            && v.len() <= 12
            && !v.starts_with('0')
            && v.bytes().all(|b| b.is_ascii_digit()));
    if ok { v.parse().ok() } else { None }
}

fn bracket_tag<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("[{name}=");
    let start = head.find(&open)? + open.len();
    let end = head[start..].find(']')? + start;
    Some(&head[start..end])
}

/// Parses one line. Returns `None` for prose, continuation and blank lines.
pub fn parse_line(line_no: usize, raw: &str) -> Option<StatusEvent> {
    let line = raw.trim_end_matches('\r');
    if line.trim().is_empty() {
        return None;
    }
    let un = unstamped(line);
    let colon = un.find(':');
    let head = match colon {
        Some(c) => &un[..c],
        None => un.as_str(),
    };
    // A colonless line only counts when it carries a [key=…] tag.
    if colon.is_none() && !un.contains("[key=") {
        return None;
    }
    let verb_part = head.split('[').next().unwrap_or("").trim();
    let mut words = verb_part.split_whitespace();
    let verb = words.next().unwrap_or("").to_string();
    let mut corr = None;
    let mut clean = true;
    for w in words {
        if is_corr_word(w) {
            corr = Some(w[5..].to_string());
        } else {
            clean = false;
        }
    }
    let verb_ok = !verb.is_empty()
        && verb
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b'-' || b == b'_');
    if !clean || !verb_ok {
        return None;
    }
    if corr.is_none() {
        corr = bracket_tag(head, "corr").map(str::to_string);
    }
    let mut key = None;
    let mut key_bad = false;
    if let Some(k) = bracket_tag(head, "key") {
        if valid_slug(k) {
            key = Some(k.to_string());
        } else {
            key_bad = true;
        }
    }
    let mut note = match colon {
        Some(c) => un[c + 1..].trim_start().to_string(),
        None => un[head.find(']').map_or(head.len(), |i| i + 1)..]
            .trim_start()
            .to_string(),
    };
    if colon.is_none() {
        // For a colonless keyed line the note is the text after the last tag.
        let after = un.rfind(']').map_or("", |i| &un[i + 1..]);
        note = after.trim_start().to_string();
    }
    if key.is_none()
        && !key_bad
        && let Some(rest) = note.strip_prefix("[key=")
        && let Some(end) = rest.find(']')
    {
        let k = &rest[..end];
        if valid_slug(k) {
            key = Some(k.to_string());
            note = rest[end + 1..].trim_start().to_string();
        } else {
            key_bad = true;
        }
    }
    if key_bad {
        return None;
    }
    Some(StatusEvent {
        line_no,
        recognized: VERBS.contains(&verb.as_str()),
        verb,
        at: head_at(line),
        key,
        corr,
        note,
        raw: line.to_string(),
    })
}

/// Parses every event line in a status file's text.
pub fn parse(text: &str) -> Vec<StatusEvent> {
    text.lines()
        .enumerate()
        .filter_map(|(i, l)| parse_line(i, l))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenDecision {
    pub key: String,
    /// `needs-decision` or `blocked`.
    pub verb: String,
    pub note: String,
    pub at: Option<i64>,
}

/// Folds events into the open decisions and blockers, oldest first.
///
/// `kind` is the task's meta `kind=`; a ship or scout `done`/`failed` clears
/// every open decision, while a secondmate's does not.
pub fn open_decisions(events: &[StatusEvent], kind: Option<&str>) -> Vec<OpenDecision> {
    let clears_on_terminal = matches!(kind.unwrap_or("ship"), "ship" | "scout");
    let mut open: Vec<OpenDecision> = Vec::new();
    for e in events {
        let key = e.decision_key();
        // The pending-reply namespace only moves on its own notes.
        if key.starts_with("pending-reply-") && !e.note.starts_with("pending-reply-") {
            continue;
        }
        match e.verb.as_str() {
            "done" | "failed" if clears_on_terminal && e.raw.contains(':') => open.clear(),
            "needs-decision" | "blocked" => {
                open.retain(|d| d.key != key);
                open.push(OpenDecision {
                    key: key.to_string(),
                    verb: e.verb.clone(),
                    note: e.note.clone(),
                    at: e.at,
                });
            }
            "resolved" | "captain-held" => open.retain(|d| d.key != key),
            _ => {}
        }
    }
    open
}

/// The task's coarse current state from its log, as `fm-crew-state.sh`
/// derives it when it falls back to the status log.
pub fn log_state(events: &[StatusEvent], open: &[OpenDecision]) -> &'static str {
    let verb = if let Some(d) = open.last() {
        d.verb.as_str()
    } else if let Some(w) = declared_wait(events) {
        w.verb.as_str()
    } else {
        events
            .iter()
            .rev()
            .find(|e| e.recognized && e.verb != "note")
            .map_or("", |e| e.verb.as_str())
    };
    match verb {
        "paused" | "captain-held" => "paused",
        "working" => "working",
        "needs-decision" => "parked",
        "blocked" => "blocked",
        "done" => "done",
        "failed" => "failed",
        _ => "unknown",
    }
}

/// The latest `paused`/`captain-held` line when only `resolved` lines with
/// other keys follow it.
fn declared_wait(events: &[StatusEvent]) -> Option<&StatusEvent> {
    for e in events.iter().rev() {
        match e.verb.as_str() {
            "paused" | "captain-held" => return Some(e),
            "resolved" | "note" => continue,
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stamped_keyed_line() {
        let e = parse_line(
            0,
            "blocked [at=1790996621] [key=pr1780-staging-redeploy-2] [corr=254aec7f782f2122]: after 20 more minutes",
        )
        .unwrap();
        assert_eq!(e.verb, "blocked");
        assert_eq!(e.at, Some(1790996621));
        assert_eq!(e.key.as_deref(), Some("pr1780-staging-redeploy-2"));
        assert_eq!(e.corr.as_deref(), Some("254aec7f782f2122"));
        assert_eq!(e.note, "after 20 more minutes");
    }

    #[test]
    fn parses_bare_corr_word_and_legacy_lines() {
        let e = parse_line(0, "done corr=765ad6131dfae635: PR https://x/pull/1").unwrap();
        assert_eq!(e.verb, "done");
        assert_eq!(e.corr.as_deref(), Some("765ad6131dfae635"));
        assert_eq!(e.at, None);
        let e = parse_line(0, "done: PR https://github.com/a/b/pull/2 - passed").unwrap();
        assert_eq!(e.note, "PR https://github.com/a/b/pull/2 - passed");
        assert_eq!(e.decision_key(), "default");
    }

    #[test]
    fn key_at_note_head_is_consumed() {
        let e = parse_line(0, "needs-decision: [key=x1] pick one").unwrap();
        assert_eq!(e.key.as_deref(), Some("x1"));
        assert_eq!(e.note, "pick one");
    }

    #[test]
    fn bad_key_slug_skips_line() {
        assert!(parse_line(0, "blocked [key=a b]: x").is_none());
    }

    #[test]
    fn malformed_stamp_means_unknown_time() {
        let e = parse_line(0, "working [at=10:30]: x").unwrap();
        assert_eq!(e.verb, "working");
        assert_eq!(e.at, None);
        assert_eq!(e.note, "x");
        assert_eq!(parse_line(0, "working [at=0123]: x").unwrap().at, None);
        assert_eq!(parse_line(0, "working [at=1] [at=2]: x").unwrap().at, None);
    }

    #[test]
    fn prose_and_continuations_are_not_events() {
        assert!(parse_line(0, "  continued text: still prose").is_none());
        assert!(parse_line(0, "").is_none());
        assert!(parse_line(0, "Some Prose: here").is_none());
        assert!(parse_line(0, "working on it without colon").is_none());
    }

    #[test]
    fn colonless_keyed_line_counts() {
        let e = parse_line(0, "working [key=k1] doing a thing").unwrap();
        assert_eq!(e.key.as_deref(), Some("k1"));
        assert_eq!(e.note, "doing a thing");
    }

    #[test]
    fn unknown_lowercase_verb_is_an_unrecognized_event() {
        let e = parse_line(0, "parked: waiting").unwrap();
        assert!(!e.recognized);
    }

    #[test]
    fn fold_opens_and_closes_by_key() {
        let ev = parse(
            "needs-decision [key=a]: pick\nblocked [key=b]: stuck\nresolved [key=a]: answered\n",
        );
        let open = open_decisions(&ev, Some("ship"));
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].key, "b");
        assert_eq!(log_state(&ev, &open), "blocked");
    }

    #[test]
    fn ship_done_clears_but_secondmate_done_does_not() {
        let ev = parse("blocked [key=a]: stuck\ndone: shipped\n");
        assert!(open_decisions(&ev, Some("ship")).is_empty());
        assert_eq!(open_decisions(&ev, Some("secondmate")).len(), 1);
        assert_eq!(log_state(&ev, &open_decisions(&ev, Some("ship"))), "done");
    }

    #[test]
    fn keyless_resolved_closes_default() {
        let ev = parse("needs-decision: which?\nresolved: answered\n");
        assert!(open_decisions(&ev, None).is_empty());
    }

    #[test]
    fn declared_pause_survives_other_resolutions() {
        let ev = parse("paused [key=w]: waiting on CI\nresolved [key=z]: old thing\n");
        assert_eq!(log_state(&ev, &[]), "paused");
    }
}
