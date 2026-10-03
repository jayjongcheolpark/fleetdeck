//! Parses the output of `gather.sh` into raw files, listings and counts.
//!
//! Every record starts on a line that begins with a per-run boundary token,
//! so file contents can hold any text:
//!
//! ```text
//! <B> META now 1790996745
//! <B> NOHOME
//! <B> FILE <full|tail> <size> <mtime> <path>
//! ...content...
//! <B> END
//! <B> LIST <dir>
//! name
//! subdir/
//! <B> END
//! <B> COUNT <n> <glob>
//! <B> STAT <size> <mtime> <path>
//! <B> PS <pid> <command name, empty when the process is gone>
//! ```
//!
//! One run can read several homes: `<B> HOME <path>` starts the records of
//! a home and `<B> HOMEEND` closes them. `META` records before the first
//! `HOME` (the host clock and name) apply to every home.
//!
//! The script writes one newline before each `END` after file content, and
//! the parser removes it again.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadMode {
    /// The first bytes of the file, up to a cap.
    Full,
    /// The last lines of the file.
    Tail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFile {
    pub mode: ReadMode,
    /// Size in bytes on disk when the script read it.
    pub size: u64,
    /// Modification time, Unix seconds.
    pub mtime: i64,
    pub content: String,
}

impl RawFile {
    /// True when `content` holds less than the whole file.
    pub fn partial(&self) -> bool {
        (self.content.len() as u64) < self.size
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bundle {
    pub meta: BTreeMap<String, String>,
    pub no_home: bool,
    pub files: BTreeMap<String, RawFile>,
    /// Directory listings from `ls -Ap`: directories end in `/`.
    pub lists: BTreeMap<String, Vec<String>>,
    pub counts: BTreeMap<String, u64>,
    /// `stat` results for files read for their size and mtime only.
    pub stats: BTreeMap<String, (u64, i64)>,
    /// Process command names by pid; empty when the process is gone.
    pub procs: BTreeMap<u32, String>,
}

impl Bundle {
    pub fn file(&self, path: &str) -> Option<&RawFile> {
        self.files.get(path)
    }

    pub fn text(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(|f| f.content.as_str())
    }

    pub fn list(&self, dir: &str) -> &[String] {
        self.lists.get(dir).map_or(&[], |v| v.as_slice())
    }

    pub fn count(&self, glob: &str) -> Option<u64> {
        self.counts.get(glob).copied()
    }

    pub fn now(&self) -> Option<i64> {
        self.meta.get("now").and_then(|v| v.parse().ok())
    }

    pub fn stat(&self, path: &str) -> Option<(u64, i64)> {
        self.stats
            .get(path)
            .copied()
            .or_else(|| self.files.get(path).map(|f| (f.size, f.mtime)))
    }

    /// Parses a run that read one home and returns its bundle.
    pub fn parse(boundary: &str, raw: &[u8]) -> Result<Bundle, String> {
        let mut homes = Bundle::parse_many(boundary, raw)?;
        match homes.len() {
            1 => Ok(homes.remove(0).1),
            n => Err(format!("expected 1 home, got {n}")),
        }
    }

    /// Parses a run and returns one bundle per `HOME` record, in order.
    pub fn parse_many(boundary: &str, raw: &[u8]) -> Result<Vec<(String, Bundle)>, String> {
        let text = String::from_utf8_lossy(raw);
        let mut global = BTreeMap::new();
        let mut homes: Vec<(String, Bundle)> = Vec::new();
        let mut open = false;
        let mut scratch = Bundle::default();
        let prefix = format!("{boundary} ");
        let end = format!("{boundary} END");
        let mut lines = text.split_inclusive('\n');
        while let Some(line) = lines.next() {
            let line_trim = line.strip_suffix('\n').unwrap_or(line);
            let Some(rest) = line_trim.strip_prefix(&prefix) else {
                continue;
            };
            let (kind, args) = rest.split_once(' ').unwrap_or((rest, ""));
            if kind == "HOME" {
                if open {
                    return Err(format!("HOME {args} inside another home"));
                }
                open = true;
                let mut b = Bundle::default();
                b.meta.clone_from(&global);
                homes.push((args.to_string(), b));
                continue;
            }
            if kind == "HOMEEND" {
                if !open {
                    return Err("HOMEEND without HOME".into());
                }
                open = false;
                continue;
            }
            let b = match homes.last_mut() {
                Some((_, b)) if open => b,
                _ => &mut scratch,
            };
            match kind {
                "META" => {
                    let (k, v) = args.split_once(' ').unwrap_or((args, ""));
                    if !open {
                        global.insert(k.to_string(), v.to_string());
                    }
                    b.meta.insert(k.to_string(), v.to_string());
                }
                "NOHOME" => b.no_home = true,
                "FILE" => {
                    let mut parts = args.splitn(4, ' ');
                    let mode = match parts.next() {
                        Some("full") => ReadMode::Full,
                        Some("tail") => ReadMode::Tail,
                        other => return Err(format!("bad FILE mode {other:?}")),
                    };
                    let size = parse_num(parts.next(), "size")? as u64;
                    let mtime = parse_num(parts.next(), "mtime")?;
                    let path = parts.next().ok_or("FILE without path")?.to_string();
                    let mut content = String::new();
                    let mut closed = false;
                    for l in lines.by_ref() {
                        if l.strip_suffix('\n').unwrap_or(l) == end {
                            closed = true;
                            break;
                        }
                        content.push_str(l);
                    }
                    if !closed {
                        return Err(format!("FILE {path} has no END"));
                    }
                    if content.ends_with('\n') {
                        content.pop();
                    }
                    b.files.insert(
                        path,
                        RawFile {
                            mode,
                            size,
                            mtime,
                            content,
                        },
                    );
                }
                "LIST" => {
                    let mut names = Vec::new();
                    let mut closed = false;
                    for l in lines.by_ref() {
                        let l = l.strip_suffix('\n').unwrap_or(l);
                        if l == end {
                            closed = true;
                            break;
                        }
                        if !l.is_empty() {
                            names.push(l.to_string());
                        }
                    }
                    if !closed {
                        return Err(format!("LIST {args} has no END"));
                    }
                    b.lists.insert(args.to_string(), names);
                }
                "COUNT" => {
                    let (n, glob) = args.split_once(' ').ok_or("COUNT without glob")?;
                    let n = parse_num(Some(n), "count")? as u64;
                    b.counts.insert(glob.to_string(), n);
                }
                "STAT" => {
                    let mut parts = args.splitn(3, ' ');
                    let size = parse_num(parts.next(), "size")? as u64;
                    let mtime = parse_num(parts.next(), "mtime")?;
                    let path = parts.next().ok_or("STAT without path")?;
                    b.stats.insert(path.to_string(), (size, mtime));
                }
                "PS" => {
                    let (pid, name) = args.split_once(' ').unwrap_or((args, ""));
                    let pid = parse_num(Some(pid), "pid")? as u32;
                    b.procs.insert(pid, name.trim().to_string());
                }
                _ => {}
            }
        }
        if open {
            return Err("gather output ended early".into());
        }
        Ok(homes)
    }
}

fn parse_num(s: Option<&str>, what: &str) -> Result<i64, String> {
    s.and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| format!("bad {what}: {s:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: &str = "\x1dFDtest";

    fn sample() -> String {
        [
            format!("{B} META now 100"),
            format!("{B} HOME /h"),
            format!("{B} FILE full 12 50 data/a.md"),
            "line one".into(),
            format!("{B} not a record inside content"),
            String::new(),
            format!("{B} END"),
            format!("{B} FILE tail 999 60 state/x.status"),
            "last".into(),
            format!("{B} END"),
            format!("{B} LIST state"),
            "x.meta".into(),
            "x.inbox/".into(),
            format!("{B} END"),
            format!("{B} COUNT 3 state/operational-inbox/*.msg"),
            format!("{B} STAT 0 70 state/.last-watcher-beat"),
            format!("{B} PS 42 claude"),
            format!("{B} PS 43 "),
            format!("{B} HOMEEND"),
        ]
        .join("\n")
            + "\n"
    }

    #[test]
    fn parses_all_record_kinds() {
        let b = Bundle::parse(B, sample().as_bytes()).unwrap();
        assert_eq!(b.now(), Some(100));
        let a = b.file("data/a.md").unwrap();
        assert_eq!(a.mode, ReadMode::Full);
        assert_eq!(a.mtime, 50);
        assert_eq!(
            a.content,
            format!("line one\n{B} not a record inside content\n")
        );
        let x = b.file("state/x.status").unwrap();
        assert_eq!(x.content, "last");
        assert!(x.partial());
        assert_eq!(b.list("state"), ["x.meta", "x.inbox/"]);
        assert_eq!(b.count("state/operational-inbox/*.msg"), Some(3));
        assert_eq!(b.stat("state/.last-watcher-beat"), Some((0, 70)));
        assert_eq!(b.stat("data/a.md"), Some((12, 50)));
        assert_eq!(b.procs.get(&42).map(String::as_str), Some("claude"));
        assert_eq!(b.procs.get(&43).map(String::as_str), Some(""));
    }

    #[test]
    fn content_without_trailing_newline_survives() {
        let raw = format!("{B} HOME /h\n{B} FILE full 3 1 f\nabc\n{B} END\n{B} HOMEEND\n");
        let b = Bundle::parse(B, raw.as_bytes()).unwrap();
        assert_eq!(b.text("f"), Some("abc"));
        assert!(!b.file("f").unwrap().partial());
    }

    #[test]
    fn truncated_output_is_an_error() {
        let raw = format!("{B} HOME /h\n{B} FILE full 3 1 f\nabc\n");
        assert!(Bundle::parse(B, raw.as_bytes()).is_err());
        let raw = format!("{B} HOME /h\n{B} META x 1\n");
        assert!(Bundle::parse(B, raw.as_bytes()).is_err());
    }

    #[test]
    fn splits_several_homes_and_shares_global_meta() {
        let raw = format!(
            "{B} META now 5\n{B} HOME /a\n{B} NOHOME\n{B} HOMEEND\n{B} HOME ~/b\n{B} COUNT 1 x\n{B} HOMEEND\n"
        );
        let homes = Bundle::parse_many(B, raw.as_bytes()).unwrap();
        assert_eq!(homes.len(), 2);
        assert_eq!(homes[0].0, "/a");
        assert!(homes[0].1.no_home);
        assert_eq!(homes[1].0, "~/b");
        assert_eq!(homes[1].1.now(), Some(5));
        assert_eq!(homes[1].1.count("x"), Some(1));
    }
}
