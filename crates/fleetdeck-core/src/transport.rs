//! Runs the read-only gather script on the machine that holds a home.
//!
//! A local home runs the script with `sh -s`; a remote home runs the same
//! script with `ssh <alias> sh -s`. The script only reads (`head`, `tail`,
//! `ls`, `stat`, `ps`), so both paths share one protocol and one parser.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::Location;

#[derive(Debug)]
pub enum TransportError {
    Spawn(String),
    Timeout(Duration),
    Failed { code: Option<i32>, stderr: String },
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Spawn(e) => write!(f, "cannot start: {e}"),
            TransportError::Timeout(d) => write!(f, "timed out after {}s", d.as_secs()),
            TransportError::Failed { code, stderr } => {
                let code = code.map_or_else(|| "signal".to_string(), |c| c.to_string());
                let first = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                write!(f, "exit {code}: {first}")
            }
        }
    }
}

impl std::error::Error for TransportError {}

/// Builds the command that runs `sh -s <args>` for a location.
fn shell_command(location: &Location, args: &[&str], connect_timeout: Duration) -> Command {
    match location {
        Location::Local(_) => {
            let mut cmd = Command::new("sh");
            cmd.arg("-s").arg("--").args(args);
            cmd
        }
        Location::Ssh { host, .. } => {
            let mut cmd = Command::new("ssh");
            cmd.arg("-o")
                .arg("BatchMode=yes")
                .arg("-o")
                .arg(format!(
                    "ConnectTimeout={}",
                    connect_timeout.as_secs().max(1)
                ))
                .arg("-o")
                .arg("ServerAliveInterval=5")
                .arg("-o")
                .arg("ServerAliveCountMax=2")
                .arg("-T")
                .arg(host)
                .arg("--");
            // ssh joins its remote arguments with spaces and the remote login
            // shell parses them, so every argument is single-quoted.
            let mut remote = String::from("sh -s --");
            for a in args {
                remote.push(' ');
                remote.push_str(&shell_quote(a));
            }
            cmd.arg(remote);
            cmd
        }
    }
}

/// Quotes a string for a POSIX shell.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Runs `script` with `args` at `location` and returns its stdout.
///
/// The child is killed when `timeout` passes, so an unreachable host never
/// holds a caller longer than that.
pub fn run_script(
    location: &Location,
    script: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<Vec<u8>, TransportError> {
    let cmd = shell_command(location, args, timeout.min(Duration::from_secs(10)));
    run_command(cmd, Some(script), timeout)
}

/// Runs a command with optional stdin text, killing it after `timeout`.
pub fn run_command(
    mut cmd: Command,
    stdin: Option<&str>,
    timeout: Duration,
) -> Result<Vec<u8>, TransportError> {
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| TransportError::Spawn(e.to_string()))?;
    // Writing on its own thread keeps a full stdout pipe from deadlocking us.
    let writer = match (child.stdin.take(), stdin) {
        (Some(mut pipe), Some(text)) => {
            let text = text.to_owned();
            Some(thread::spawn(move || {
                let _ = pipe.write_all(text.as_bytes());
            }))
        }
        _ => None,
    };
    let out = wait_with_timeout(child, timeout);
    if let Some(w) = writer {
        let _ = w.join();
    }
    out
}

fn wait_with_timeout(mut child: Child, timeout: Duration) -> Result<Vec<u8>, TransportError> {
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let (tx, rx) = mpsc::channel();
    let tx_err = tx.clone();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send((true, buf));
    });
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        let _ = tx_err.send((false, buf));
    });
    let deadline = Instant::now() + timeout;
    let mut out = None;
    let mut err = None;
    while out.is_none() || err.is_none() {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok((true, b)) => out = Some(b),
            Ok((false, b)) => err = Some(b),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(TransportError::Timeout(timeout));
            }
        }
    }
    let status = loop {
        if let Some(s) = child
            .try_wait()
            .map_err(|e| TransportError::Spawn(e.to_string()))?
        {
            break s;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(TransportError::Timeout(timeout));
        }
        thread::sleep(Duration::from_millis(5));
    };
    if status.success() {
        Ok(out.unwrap_or_default())
    } else {
        Err(TransportError::Failed {
            code: status.code(),
            stderr: String::from_utf8_lossy(&err.unwrap_or_default()).into_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn quotes_single_quotes() {
        assert_eq!(shell_quote("a'b"), r"'a'\''b'");
        assert_eq!(shell_quote("/x y"), "'/x y'");
    }

    #[test]
    fn local_script_receives_args() {
        let loc = Location::Local(PathBuf::from("/"));
        let out = run_script(
            &loc,
            "printf '%s|' \"$@\"",
            &["a b", "c"],
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "a b|c|");
    }

    #[test]
    fn local_script_times_out() {
        let loc = Location::Local(PathBuf::from("/"));
        let err = run_script(&loc, "sleep 5", &[], Duration::from_millis(200)).unwrap_err();
        assert!(matches!(err, TransportError::Timeout(_)));
    }

    #[test]
    fn failing_script_reports_stderr() {
        let loc = Location::Local(PathBuf::from("/"));
        let err =
            run_script(&loc, "echo nope >&2; exit 3", &[], Duration::from_secs(5)).unwrap_err();
        assert_eq!(err.to_string(), "exit 3: nope");
    }
}
