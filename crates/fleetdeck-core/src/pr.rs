//! Optional, read-only GitHub PR status through the `gh` CLI.
//!
//! Only two read calls are used: `gh pr view --json …` and a `gh api graphql`
//! query for review threads. Nothing posts, edits, approves or merges.

use std::process::Command;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::transport;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Checks {
    pub pass: u32,
    pub fail: u32,
    pub pending: u32,
}

impl Checks {
    pub fn total(&self) -> u32 {
        self.pass + self.fail + self.pending
    }

    pub fn label(&self) -> String {
        if self.total() == 0 {
            "none".into()
        } else if self.fail > 0 {
            format!("{} failing", self.fail)
        } else if self.pending > 0 {
            format!("{} pending", self.pending)
        } else {
            format!("{} passing", self.pass)
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PrStatus {
    pub url: String,
    pub number: Option<u64>,
    pub title: String,
    /// OPEN, CLOSED or MERGED.
    pub state: String,
    pub draft: bool,
    /// MERGEABLE, CONFLICTING or UNKNOWN.
    pub mergeable: String,
    pub merge_state: String,
    pub review_decision: String,
    pub checks: Checks,
    pub unresolved_threads: Option<u64>,
    pub fetched_at: i64,
    pub error: Option<String>,
}

/// `(owner, repo, number)` of a github.com pull request URL.
pub fn github_parts(url: &str) -> Option<(String, String, u64)> {
    let rest = url.strip_prefix("https://github.com/")?;
    let mut it = rest.split('/');
    let owner = it.next()?.to_string();
    let repo = it.next()?.to_string();
    if it.next()? != "pull" {
        return None;
    }
    let number = it.next()?.split(['#', '?']).next()?.parse().ok()?;
    Some((owner, repo, number))
}

/// Parses `gh pr view --json number,title,state,isDraft,mergeable,
/// mergeStateStatus,reviewDecision,statusCheckRollup` output.
pub fn parse_view(url: &str, json: &str) -> Result<PrStatus, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut checks = Checks::default();
    for c in v
        .get("statusCheckRollup")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let field = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("");
        // A CheckRun has status + conclusion; a StatusContext has state.
        let outcome = if !field("conclusion").is_empty() {
            field("conclusion")
        } else if !field("state").is_empty() {
            field("state")
        } else {
            field("status")
        };
        match outcome {
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => checks.pass += 1,
            "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "ERROR"
            | "STARTUP_FAILURE" => checks.fail += 1,
            _ => checks.pending += 1,
        }
    }
    Ok(PrStatus {
        url: url.to_string(),
        number: v.get("number").and_then(Value::as_u64),
        title: s("title"),
        state: s("state"),
        draft: v.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
        mergeable: s("mergeable"),
        merge_state: s("mergeStateStatus"),
        review_decision: s("reviewDecision"),
        checks,
        unresolved_threads: None,
        fetched_at: 0,
        error: None,
    })
}

/// Counts unresolved threads in the review-threads GraphQL response.
pub fn parse_threads(json: &str) -> Option<u64> {
    let v: Value = serde_json::from_str(json).ok()?;
    let nodes = v
        .pointer("/data/repository/pullRequest/reviewThreads/nodes")?
        .as_array()?;
    Some(
        nodes
            .iter()
            .filter(|n| n.get("isResolved").and_then(Value::as_bool) == Some(false))
            .count() as u64,
    )
}

const THREADS_QUERY: &str = "query($o:String!,$r:String!,$n:Int!){repository(owner:$o,name:$r){pullRequest(number:$n){reviewThreads(first:100){nodes{isResolved}}}}}";

fn gh(args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut cmd = Command::new("gh");
    cmd.args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1");
    transport::run_command(cmd, None, timeout)
        .map(|o| String::from_utf8_lossy(&o).into_owned())
        .map_err(|e| e.to_string())
}

/// Fetches one PR's status with read-only `gh` calls.
pub fn fetch(url: &str, timeout: Duration) -> PrStatus {
    let now = crate::collect::now_epoch();
    let fail = |e: String| PrStatus {
        url: url.to_string(),
        fetched_at: now,
        error: Some(e),
        ..PrStatus::default()
    };
    let Some((owner, repo, number)) = github_parts(url) else {
        return fail("only github.com pull requests are supported".into());
    };
    let view = gh(
        &[
            "pr",
            "view",
            url,
            "--json",
            "number,title,state,isDraft,mergeable,mergeStateStatus,reviewDecision,statusCheckRollup",
        ],
        timeout,
    );
    let mut status = match view.and_then(|j| parse_view(url, &j)) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let q = format!("query={THREADS_QUERY}");
    let o = format!("o={owner}");
    let r = format!("r={repo}");
    let n = format!("n={number}");
    status.unresolved_threads = gh(
        &["api", "graphql", "-f", &q, "-f", &o, "-f", &r, "-F", &n],
        timeout,
    )
    .ok()
    .and_then(|j| parse_threads(&j));
    status.fetched_at = now;
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_github_urls_only() {
        assert_eq!(
            github_parts("https://github.com/acme/shop/pull/438"),
            Some(("acme".into(), "shop".into(), 438))
        );
        assert_eq!(
            github_parts("https://gitlab.com/a/b/-/merge_requests/1"),
            None
        );
        assert_eq!(github_parts("https://github.com/a/b/issues/3"), None);
    }

    #[test]
    fn parses_view_and_summarizes_checks() {
        let json = r#"{"number":58,"title":"fix","state":"OPEN","isDraft":false,
            "mergeable":"MERGEABLE","mergeStateStatus":"BLOCKED","reviewDecision":"REVIEW_REQUIRED",
            "statusCheckRollup":[
              {"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"},
              {"__typename":"CheckRun","status":"COMPLETED","conclusion":"CANCELLED"},
              {"__typename":"CheckRun","status":"IN_PROGRESS","conclusion":""},
              {"__typename":"StatusContext","state":"SUCCESS"}]}"#;
        let s = parse_view("u", json).unwrap();
        assert_eq!(s.number, Some(58));
        assert_eq!(
            s.checks,
            Checks {
                pass: 2,
                fail: 1,
                pending: 1
            }
        );
        assert_eq!(s.checks.label(), "1 failing");
        assert_eq!(s.merge_state, "BLOCKED");
    }

    #[test]
    fn counts_unresolved_threads() {
        let json = r#"{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[
            {"isResolved":true},{"isResolved":false},{"isResolved":false}]}}}}}"#;
        assert_eq!(parse_threads(json), Some(2));
        assert_eq!(parse_threads("{}"), None);
    }
}
