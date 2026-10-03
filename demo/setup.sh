#!/bin/sh
# Builds the fleetdeck demo fleet from demo/homes into <root>.
#
# Usage: demo/setup.sh <root>
#
# It renders the time and path placeholders in demo/homes relative to now,
# starts stand-in "claude" processes so the session locks read as held, and
# writes <root>/.config/fleetdeck/config.toml. Put demo/bin first on PATH
# and export FLEETDECK_DEMO_ROOT=<root> before you run fleetdeck, so that ssh,
# gh and hostname are the offline demo stand-ins.
set -eu

root=${1:?usage: demo/setup.sh <root>}
here=$(cd "$(dirname "$0")" && pwd)

mkdir -p "$root"
root=$(cd "$root" && pwd)
rm -rf "$root/firstmate" "$root/secondmates" "$root/devbox"
cp -R "$here/homes/firstmate" "$here/homes/secondmates" "$here/homes/devbox" "$root/"
for p in shop docs-site billing-api; do mkdir -p "$root/firstmate/projects/$p"; done
mkdir -p "$root/devbox/firstmate/projects/billing-api"

# Stand-in harness and watcher processes: a copy of sleep named claude.
mkdir -p "$root/.bin"
cp "$(command -v sleep)" "$root/.bin/claude"
cp "$(command -v sleep)" "$root/.bin/fm-watch"
start() { "$root/.bin/$1" 3600 >/dev/null 2>&1 & echo $!; }
pid_main=$(start claude)
pid_mate=$(start claude)
pid_remote=$(start claude)
pid_watch=$(start fm-watch)

export ROOT="$root" PID_MAIN="$pid_main" PID_MATE="$pid_mate" PID_REMOTE="$pid_remote" PID_WATCH="$pid_watch"
# shellcheck disable=SC2016 # perl code, not shell
find "$root/firstmate" "$root/secondmates" "$root/devbox" -type f -print0 |
  xargs -0 perl -pi -e '
    use POSIX qw(strftime);
    BEGIN { $now = time }
    s/\@T-(\d+)\@/$now - $1/ge;
    s/\@D-(\d+)\@/strftime("%Y-%m-%d", gmtime($now - $1 * 86400))/ge;
    s/\@ISO-(\d+)\@/strftime("%Y-%m-%dT%H:%M:%SZ", gmtime($now - $1))/ge;
    s/\@ROOT\@/$ENV{ROOT}/g;
    s/\@(PID_\w+)\@/$ENV{$1}/g;
  '

# Session ages and watcher beats.
perl -e '
  my ($root) = @ARGV;
  my %age = (
    "firstmate/state/.lock" => 5 * 3600 + 12 * 60,
    "firstmate/state/.last-watcher-beat" => 9,
    "secondmates/web-mate/state/.lock" => 2 * 3600 + 40 * 60,
    "secondmates/web-mate/state/.last-watcher-beat" => 14,
    "devbox/firstmate/state/.lock" => 26 * 3600,
    "devbox/firstmate/state/.last-watcher-beat" => 6,
  );
  for my $f (keys %age) { my $t = time - $age{$f}; utime $t, $t, "$root/$f" or die "$f: $!" }
' "$root"

mkdir -p "$root/.config/fleetdeck"
cat >"$root/.config/fleetdeck/config.toml" <<TOML
refresh_secs = 30

[[home]]
name = "main"
path = "$root/firstmate"

[[home]]
name = "devbox"
host = "devbox"
path = "~/firstmate"
TOML
