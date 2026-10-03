//! Command-line arguments.

use std::path::PathBuf;

use fleetdeck_core::config::{self, Config, HomeSpec, Location};

pub const USAGE: &str = "\
fleetdeck - read-only terminal dashboard for firstmate fleets

USAGE:
    fleetdeck [OPTIONS]

OPTIONS:
    -c, --config <FILE>      Config file (default: ~/.config/fleetdeck/config.toml)
        --home <PATH>        Show this local home (repeatable; replaces config homes)
        --ssh <HOST:PATH>    Show this remote home over ssh (repeatable)
        --no-discover        Do not add second mates from data/secondmates.md
        --json               Print the collected snapshots as JSON and exit
        --frame <WxH>        Render one frame as text and exit (for screenshots)
        --tab <NAME>         Tab for --frame: work, prs, context, activity
        --select <N>         Home row for --frame (0-based)
    -h, --help               Show this help
    -V, --version            Show the version

With no config file and no --home, fleetdeck shows $FM_HOME, else the
current directory when it looks like a firstmate home.
";

#[derive(Debug, Default)]
pub struct Args {
    pub config: Option<PathBuf>,
    pub homes: Vec<HomeSpec>,
    pub no_discover: bool,
    pub json: bool,
    pub frame: Option<(u16, u16)>,
    pub tab: Option<String>,
    pub select: usize,
}

pub enum Parsed {
    Run(Args),
    Help,
    Version,
}

pub fn parse(mut it: impl Iterator<Item = String>) -> Result<Parsed, String> {
    let mut a = Args::default();
    let value = |it: &mut dyn Iterator<Item = String>, flag: &str| {
        it.next().ok_or_else(|| format!("{flag} needs a value"))
    };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "-V" | "--version" => return Ok(Parsed::Version),
            "-c" | "--config" => a.config = Some(PathBuf::from(value(&mut it, &arg)?)),
            "--home" => {
                let p = value(&mut it, &arg)?;
                a.homes.push(local_spec(&p));
            }
            "--ssh" => {
                let v = value(&mut it, &arg)?;
                let (host, path) = v
                    .split_once(':')
                    .filter(|(h, p)| !h.is_empty() && !p.is_empty())
                    .ok_or_else(|| format!("--ssh expects HOST:PATH, got {v}"))?;
                a.homes.push(HomeSpec {
                    name: format!("{host}:{}", basename(path)),
                    location: Location::Ssh {
                        host: host.into(),
                        path: path.into(),
                    },
                    discover: true,
                });
            }
            "--no-discover" => a.no_discover = true,
            "--json" => a.json = true,
            "--frame" => {
                let v = value(&mut it, &arg)?;
                let (w, h) = v
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                    .ok_or_else(|| format!("--frame expects WxH, got {v}"))?;
                a.frame = Some((w, h));
            }
            "--tab" => a.tab = Some(value(&mut it, &arg)?),
            "--select" => {
                let v = value(&mut it, &arg)?;
                a.select = v
                    .parse()
                    .map_err(|_| format!("--select expects a number, got {v}"))?;
            }
            other => return Err(format!("unknown argument: {other}\n\n{USAGE}")),
        }
    }
    Ok(Parsed::Run(a))
}

fn basename(p: &str) -> &str {
    p.trim_end_matches('/').rsplit('/').next().unwrap_or(p)
}

fn local_spec(p: &str) -> HomeSpec {
    let path = std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p));
    let mut cfg = Config::single_local(path);
    cfg.homes.remove(0)
}

fn looks_like_home(p: &std::path::Path) -> bool {
    p.join("state").is_dir() && p.join("data").is_dir()
}

/// Builds the effective config from the arguments, the config file and
/// the environment.
pub fn effective_config(args: &Args) -> Result<Config, String> {
    let path = args.config.clone().or_else(config::default_config_path);
    let mut cfg = match path {
        Some(p) if p.exists() => Config::load(&p)?,
        Some(p) if args.config.is_some() => {
            return Err(format!("config file not found: {}", p.display()));
        }
        _ => Config::parse("").expect("empty config parses"),
    };
    if !args.homes.is_empty() {
        cfg.homes.clone_from(&args.homes);
    }
    if cfg.homes.is_empty() {
        let candidate = std::env::var_os("FM_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok().filter(|d| looks_like_home(d)));
        match candidate {
            Some(p) => cfg.homes.push(local_spec(&p.display().to_string())),
            None => {
                return Err(
                    "no homes to show: add [[home]] entries to the config file, pass --home, or set FM_HOME"
                        .into(),
                );
            }
        }
    }
    if args.no_discover {
        for h in &mut cfg.homes {
            h.discover = false;
        }
    }
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Result<Parsed, String> {
        parse(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_homes_and_frame() {
        let Ok(Parsed::Run(a)) = args(&["--ssh", "box:~/firstmate", "--frame", "120x40", "--json"])
        else {
            panic!("expected run");
        };
        assert_eq!(a.homes.len(), 1);
        assert_eq!(a.homes[0].name, "box:firstmate");
        assert_eq!(a.frame, Some((120, 40)));
        assert!(a.json);
    }

    #[test]
    fn rejects_bad_values() {
        assert!(args(&["--ssh", "nohost"]).is_err());
        assert!(args(&["--frame", "12"]).is_err());
        assert!(args(&["--bogus"]).is_err());
        assert!(matches!(args(&["-h"]), Ok(Parsed::Help)));
    }
}
