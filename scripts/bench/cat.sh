#!/usr/bin/env bash
# Times `cat` of a large file inside each terminal: how fast it takes in
# output. The terminals run in turn, round after round, so that drift in the
# machine's speed hits them alike.
#
# Usage: scripts/bench/cat.sh [-n ROUNDS] [-x REPEAT] [-f FILE] [TERMINAL...]
#   -n  rounds (default 11)
#   -x  cat the file this many times in one go (default 1)
#   -f  the file (default: 23 MB of generated ASCII lines)
# Terminals are as in lib.sh; the default is all of them.
set -euo pipefail
here=$(dirname "$(readlink -f "$0")")
# shellcheck source=scripts/bench/lib.sh
. "$here/lib.sh"

rounds=11 repeat=1 file=
while getopts "n:x:f:" opt; do
    case $opt in
        n) rounds=$OPTARG ;;
        x) repeat=$OPTARG ;;
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
    # About 405,000 lines of 57 characters, like a long build log.
    file=$dir/lines.txt
    awk 'BEGIN { for (i = 0; i < 405000; i++) printf "%08x %s\n", i * 2654435761 % 4294967296, "the quick brown fox jumps over the lazy dog 0123" }' >"$file"
fi

cat >"$dir/inner.sh" <<'EOF'
#!/bin/sh
# In the terminal: times cat of $2, $3 times over, and adds "ms rows cols" to $1.
out=$1 file=$2 repeat=$3
sleep 0.7
size=$(stty size)
set --
i=0
while [ "$i" -lt "$repeat" ]; do set -- "$@" "$file"; i=$((i + 1)); done
s=$(date +%s%N)
cat "$@"
e=$(date +%s%N)
echo "$(( (e - s) / 1000000 )) $size" >>"$out"
EOF
chmod +x "$dir/inner.sh"

echo "cat of $(du -h "$file" | cut -f1) x$repeat, $rounds rounds; times in ms"
for _ in $(seq "$rounds"); do
    for spec in "${terminals[@]}"; do
        launch "$spec" "$dir" "$dir/inner.sh" "$dir/$(label_of "$spec").txt" "$file" "$repeat" || true
        sleep 0.4
    done
done
for spec in "${terminals[@]}"; do
    out=$dir/$(label_of "$spec").txt
    [ -e "$out" ] || : >"$out"
    printf '%s  grid %s\n' "$(summarize "$(label_of "$spec")" "$out")" \
        "$(awk '{print $2 "x" $3}' "$out" | sort -u | tr '\n' ' ')"
done
