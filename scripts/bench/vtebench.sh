#!/usr/bin/env bash
# Runs vtebench (https://github.com/alacritty/vtebench) inside each terminal
# and prints a table of milliseconds per MiB for each of its scenarios:
# colored cells, cursor motion, scrolling in regions, unicode.
#
# Usage: scripts/bench/vtebench.sh [-n ROUNDS] [-s SECONDS] [TERMINAL...]
#   -n  rounds (default 2)
#   -s  seconds per scenario (default 5)
# Needs vtebench on PATH, for example in `nix shell nixpkgs#vtebench`; its
# scenarios are taken from the package (or from $VTEBENCH_BENCHMARKS).
set -euo pipefail
here=$(dirname "$(readlink -f "$0")")
# shellcheck source=scripts/bench/lib.sh
. "$here/lib.sh"

rounds=2 seconds=5
while getopts "n:s:" opt; do
    case $opt in
        n) rounds=$OPTARG ;;
        s) seconds=$OPTARG ;;
        *) exit 2 ;;
    esac
done
shift $((OPTIND - 1))
terminals=("$@")
[ ${#terminals[@]} -gt 0 ] || terminals=("${DEFAULT_TERMINALS[@]}")
check_labels "${terminals[@]}"

vtebench=$(command -v vtebench) || {
    echo "vtebench not found; try: nix shell nixpkgs#vtebench -c $0" >&2
    exit 1
}
benchmarks=${VTEBENCH_BENCHMARKS:-$(dirname "$(readlink -f "$vtebench")")/../share/vtebench/benchmarks}
[ -d "$benchmarks" ] || {
    echo "no vtebench scenarios in $benchmarks; set VTEBENCH_BENCHMARKS" >&2
    exit 1
}

dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
write_configs "$dir"

# vtebench writes the scenarios to its standard output, which must be the
# terminal measured: redirecting it measures the file instead (and fills the
# disk). The results go to the DAT file.
cat >"$dir/inner.sh" <<EOF
#!/bin/sh
sleep 0.7
"$vtebench" -s -b "$(readlink -f "$benchmarks")" --max-secs $seconds --dat "\$1"
EOF
chmod +x "$dir/inner.sh"

echo "vtebench, $seconds s per scenario, $rounds rounds; ms per MiB, lower is better"
for round in $(seq "$rounds"); do
    for spec in "${terminals[@]}"; do
        launch "$spec" "$dir" "$dir/inner.sh" "$dir/$(label_of "$spec")-$round.dat" || true
        sleep 0.4
    done
done
labels=()
for spec in "${terminals[@]}"; do labels+=("$(label_of "$spec")"); done
python3 -I "$here/vtebench_table.py" "$dir" "${labels[@]}"
