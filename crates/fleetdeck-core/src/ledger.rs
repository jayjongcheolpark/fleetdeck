//! `state/fleet-ledger.jsonl`: the append-only fleet activity ledger.
//!
//! Contract from firstmate's `docs/fleet-ledger.md`: every record has `v`,
//! `ts`, `event` and `task`; readers ignore unknown members and events. The
//! writer appends without a reader lock, so a partial last line is skipped.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LedgerEvent {
    pub ts: i64,
    pub event: String,
    pub task: String,
    /// A one-line summary of the event-specific members.
    pub summary: String,
    pub raw: String,
}

pub fn parse(text: &str) -> Vec<LedgerEvent> {
    text.lines().filter_map(parse_line).collect()
}

pub fn parse_line(line: &str) -> Option<LedgerEvent> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let ts = v.get("ts")?.as_i64()?;
    let event = v.get("event")?.as_str()?.to_string();
    let task = v.get("task")?.as_str()?.to_string();
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("");
    let summary = match event.as_str() {
        "task.dispatched" => {
            let mut parts = vec![s("kind").to_string()];
            for k in ["project", "harness", "model"] {
                if !s(k).is_empty() {
                    parts.push(s(k).to_string());
                }
            }
            parts.retain(|p| !p.is_empty());
            parts.join(" · ")
        }
        "task.status" => {
            let state = s("state");
            let key = s("key");
            let text = s("text").trim();
            let head = if key.is_empty() {
                state.to_string()
            } else {
                format!("{state} [{key}]")
            };
            format!("{head}: {text}")
        }
        "task.pr_ready" => s("pr").to_string(),
        "task.merged" => {
            let via = s("via");
            match v.get("pr").and_then(Value::as_str) {
                Some(pr) => format!("via {via} {pr}"),
                None => format!("via {via}"),
            }
        }
        _ => String::new(),
    };
    Some(LedgerEvent {
        ts,
        event,
        task,
        summary,
        raw: line.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_events_and_skips_partial_lines() {
        let text = concat!(
            r#"{"v":1,"ts":10,"event":"task.dispatched","task":"a","kind":"ship","project":"p","harness":"claude","model":null}"#,
            "\n",
            r#"{"v":1,"ts":11,"event":"task.status","task":"a","state":"working","key":null,"text":" setup done"}"#,
            "\n",
            r#"{"v":1,"ts":12,"event":"task.merged","task":"a","via":"pr","pr":"https://x/pull/1"}"#,
            "\n",
            r#"{"v":1,"ts":13,"event":"task.future","task":"a","new":1}"#,
            "\n",
            r#"{"v":1,"ts":14,"event":"task.st"#,
        );
        let ev = parse(text);
        assert_eq!(ev.len(), 4);
        assert_eq!(ev[0].summary, "ship · p · claude");
        assert_eq!(ev[1].summary, "working: setup done");
        assert_eq!(ev[2].summary, "via pr https://x/pull/1");
        assert_eq!(ev[3].event, "task.future");
    }
}
