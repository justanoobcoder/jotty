#!/usr/bin/env bash
# Measures, for each terminal: how long it takes from being started until a
# command runs in it, its memory (RSS) once idle, and its memory once its
# scrollback is full.
#
# Usage: scripts/bench/startup.sh [-n ROUNDS] [-f FILE] [TERMINAL...]
#   -n  rounds (default 5)
#   -f  the file printed to fill the scrollback (default: 100,000 lines)
# Terminals are as in lib.sh; the default is all of them.
set -euo pipefail
here=$(dirname "$(readlink -f "$0")")
# shellcheck source=scripts/bench/lib.sh
. "$here/lib.sh"

rounds=5 file=
while getopts "n:f:" opt; do
    case $opt in
        n) rounds=$OPTARG ;;
        f) file=$(readlink -f "$OPTARG") ;;
        *) exit 2 ;;
    esac
done
shift $((OPTIND - 1))
terminals=("$@")
[ ${#terminals[@]} -gt 0 ] || terminals=("${DEFAULT_TERMINALS[@]}")
check_labels "${terminals[@]}"

dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
write_configs "$dir"
if [ -z "$file" ]; then
    file=$dir/lines.txt
    awk 'BEGIN { for (i = 0; i < 100000; i++) printf "%08d %s\n", i, "the quick brown fox jumps over the lazy dog 0123" }' >"$file"
fi

cat >"$dir/inner.sh" <<'EOF'
#!/bin/sh
# In the terminal: appends "start_ms idle_kb full_kb" to $1. $3 is when the
# terminal was started, in nanoseconds.
out=$1 file=$2 started=$3
start_ms=$(( ($(date +%s%N) - started) / 1000000 ))
# The terminal: the first ancestor that isn't a shell or a wrapper.
p=$PPID
while :; do
    case $(cat "/proc/$p/comm") in
        sh | bash | zsh | fish | dash | env | timeout | login) p=$(awk '{print $4}' "/proc/$p/stat") ;;
        *) break ;;
    esac
done
sleep 1.5
idle=$(awk '/^VmRSS/ {print $2}' "/proc/$p/status")
cat "$file"
sleep 0.5
full=$(awk '/^VmRSS/ {print $2}' "/proc/$p/status")
echo "$start_ms $idle $full" >>"$out"
EOF
chmod +x "$dir/inner.sh"

echo "$rounds rounds; start in ms, memory in KiB"
for _ in $(seq "$rounds"); do
    for spec in "${terminals[@]}"; do
        launch "$spec" "$dir" "$dir/inner.sh" "$dir/$(label_of "$spec").txt" "$file" "$(date +%s%N)" || true
        sleep 0.4
    done
done
for spec in "${terminals[@]}"; do
    label=$(label_of "$spec")
    out=$dir/$label.txt
    [ -e "$out" ] || : >"$out"
    echo "$label"
    summarize "  start" "$out" 1
    summarize "  idle RSS" "$out" 2
    summarize "  RSS, full scrollback" "$out" 3
done
