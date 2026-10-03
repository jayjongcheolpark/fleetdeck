# fleetdeck gather script: READ-ONLY.
#
# Usage: sh -s -- <boundary> gather <home>...
#        sh -s -- <boundary> file <home> <relative-path> <max-bytes>
#
# This is the complete set of commands fleetdeck runs on a machine that holds
# a firstmate home, locally or over ssh. It only reads: head, tail, ls, stat,
# wc, awk, tr, ps, date, hostname and uname, and its only redirections go to
# /dev/null. It never writes, moves or deletes a file, never sends a signal,
# and never runs a firstmate script.
# The record protocol is documented in src/bundle.rs.

B=$1
MODE=$2
shift 2

if stat -c %Y / >/dev/null 2>&1; then
  st() { stat -c '%s %Y' -- "$1" 2>/dev/null; }
else
  st() { stat -f '%z %m' -- "$1" 2>/dev/null; }
fi

# emit <full|tail> <limit> <path>: a file's head bytes or tail lines.
emit() {
  [ -f "$3" ] && [ -r "$3" ] || return 0
  s=$(st "$3") || return 0
  printf '%s FILE %s %s %s\n' "$B" "$1" "$s" "$3"
  if [ "$1" = tail ]; then
    tail -n "$2" -- "$3" 2>/dev/null
  else
    head -c "$2" -- "$3" 2>/dev/null
  fi
  printf '\n%s END\n' "$B"
}

# big <limit> <path>: the whole file when it fits, else its last bytes
# (a "tail" record whose first line can be cut).
big() {
  [ -f "$2" ] && [ -r "$2" ] || return 0
  s=$(st "$2") || return 0
  printf '%s FILE tail %s %s\n' "$B" "$s" "$2"
  tail -c "$1" -- "$2" 2>/dev/null
  printf '\n%s END\n' "$B"
}

stat_only() {
  [ -e "$1" ] || return 0
  s=$(st "$1") || return 0
  printf '%s STAT %s %s\n' "$B" "$s" "$1"
}

list() {
  [ -d "$1" ] || return 0
  printf '%s LIST %s\n' "$B" "$1"
  ls -Ap -- "$1" 2>/dev/null
  printf '%s END\n' "$B"
}

# count <glob>: how many paths match, without reading them.
count() {
  n=0
  for f in $1; do
    [ -e "$f" ] && n=$((n + 1))
  done
  printf '%s COUNT %s %s\n' "$B" "$n" "$1"
}

# proc <pid>: "<command name><TAB><argv[0]>" of a live process, empty when it
# is gone. Only argv[0] is kept, so no arguments are read.
proc() {
  case $1 in '' | *[!0-9]*) return 0 ;; esac
  c=$(ps -p "$1" -o comm= 2>/dev/null | head -n 1)
  a=$(ps -p "$1" -o args= 2>/dev/null | head -n 1 | awk '{ print $1 }')
  if [ -n "$c" ]; then
    printf '%s PS %s %s\t%s\n' "$B" "$1" "$c" "$a"
  else
    printf '%s PS %s \n' "$B" "$1"
  fi
}

first_line() {
  [ -f "$1" ] && [ -r "$1" ] && head -n 1 -- "$1" 2>/dev/null
}

# enter <home>: cd into a home; a literal leading ~ means $HOME here.
enter() {
  H=$1
  # shellcheck disable=SC2088 # matching a literal ~ on purpose
  case $H in
    '~') H=$HOME ;;
    '~/'*) H=$HOME/${H#'~/'} ;;
  esac
  cd -- "$H" 2>/dev/null || return 1
}

gather_home() {
  printf '%s HOME %s\n' "$B" "$1"
  if ! enter "$1"; then
    printf '%s NOHOME\n' "$B"
    printf '%s HOMEEND\n' "$B"
    return 0
  fi
  printf '%s META pwd %s\n' "$B" "$(pwd -P)"

  emit full 4096 .fm-secondmate-home
  emit full 4096 .fm-secondmate-parent
  emit full 4096 .tasks.toml

  for f in backlog secondmates projects captain captain-shared learnings charter; do
    big 2097152 "data/$f.md"
  done
  for f in data/*.md; do stat_only "$f"; done
  list data
  list projects

  list config
  for f in config/* config/.[!.]*; do
    case ${f##*/} in
      *password* | *secret* | *token* | *credential* | *.env | *key* | *.bak*)
        stat_only "$f"
        continue
        ;;
    esac
    emit full 65536 "$f"
  done

  emit full 2097152 state/home-summary.json
  emit tail 400 state/fleet-ledger.jsonl

  emit full 64 state/.lock
  emit full 128 state/.lock-session
  emit full 64 state/.session-start-complete
  proc "$(first_line state/.lock)"
  emit full 256 state/.afk
  stat_only state/.afk-contract
  emit full 256 state/.watcher-down
  stat_only state/.last-watcher-beat
  stat_only state/.last-heartbeat
  if [ -d state/.watch.lock ]; then
    emit full 64 state/.watch.lock/pid
    emit full 512 state/.watch.lock/pid-identity
    emit full 1024 state/.watch.lock/fm-home
    proc "$(first_line state/.watch.lock/pid)"
  fi
  if [ -d state/.supervise-daemon.lock ]; then
    emit full 64 state/.supervise-daemon.lock/pid
    proc "$(first_line state/.supervise-daemon.lock/pid)"
  fi

  if [ -f state/.wake-queue ]; then
    printf '%s COUNT %s state/.wake-queue\n' "$B" "$(awk -F '\t' 'NF >= 5' state/.wake-queue 2>/dev/null | wc -l | tr -d ' ')"
  fi
  count 'state/operational-inbox/*.msg'
  count 'state/inbox/*.note'
  count 'state/terminal-outcomes/*.pending'
  count 'state/when/*.spec'
  if [ -d state/pending-replies ]; then
    printf '%s COUNT %s state/pending-replies/open\n' "$B" "$(
      awk -F= '$1 == "phase" { p[FILENAME] = $2 } END { n = 0; for (f in p) if (p[f] != "resolved") n++; print n }' state/pending-replies/* 2>/dev/null
    )"
  fi

  list state
  for m in state/*.meta; do
    [ -f "$m" ] || continue
    id=${m#state/}
    id=${id%.meta}
    emit full 65536 "$m"
    big 1048576 "state/$id.status"
    emit full 512 "state/$id.busy-state"
    emit full 256 "state/$id.busy-gen"
    stat_only "state/$id.turn-ended"
    count "state/$id.inbox/*.msg"
    count "state/$id.inbox/handled/*.msg"
  done
  for s in state/*.status; do
    [ -f "$s" ] || continue
    [ -f "${s%.status}.meta" ] && continue
    big 262144 "$s"
  done
  printf '%s HOMEEND\n' "$B"
}

printf '%s META now %s\n' "$B" "$(date +%s)"
printf '%s META host %s\n' "$B" "$(hostname 2>/dev/null || uname -n)"

case $MODE in
  gather)
    for h in "$@"; do
      (gather_home "$h")
    done
    ;;
  file)
    printf '%s HOME %s\n' "$B" "$1"
    if enter "$1"; then
      case $2 in
        /* | ../* | */../* | *'/..') ;;
        *) emit full "$3" "$2" ;;
      esac
    else
      printf '%s NOHOME\n' "$B"
    fi
    printf '%s HOMEEND\n' "$B"
    ;;
esac
