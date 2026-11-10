#!/usr/bin/env bash
# Writes THIRD-PARTY-LICENSES, the licenses of the crates compiled into jotty,
# which every package installs. Run it after changing dependencies, in the dev
# shell: nix develop -c scripts/third-party-licenses.sh
# With --check, CI only fails if the file is out of date.
set -euo pipefail

cd "$(dirname "$0")/.."
output=$(mktemp)
trap 'rm -f "$output"' EXIT

cargo about generate --locked -c scripts/about.toml scripts/about.hbs -o "$output"
# Crates without a license file get the SPDX template, with an unfilled notice
sed -i 's/Copyright (c) <year> <copyright holders>/Copyright (c) the authors of the crates listed above/' "$output"

if [ "${1:-}" = --check ]; then
    if ! diff -u THIRD-PARTY-LICENSES "$output" >&2; then
        echo "THIRD-PARTY-LICENSES is out of date: run scripts/third-party-licenses.sh" >&2
        exit 1
    fi
else
    cp "$output" THIRD-PARTY-LICENSES
    echo "wrote $PWD/THIRD-PARTY-LICENSES"
fi
