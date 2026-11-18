"""Prints a Markdown table of vtebench results.

Usage: vtebench_table.py DIR LABEL...

Reads DIR/LABEL-ROUND.dat, vtebench's DAT files: a header of scenario names,
then one row per sample with a column per scenario, in milliseconds, `_` where
a scenario has no more samples. Each cell is the mean over all samples of all
rounds; the fastest of a row is in bold.
"""

import glob
import os
import sys


def samples(path):
    """The samples of each scenario in one DAT file."""
    with open(path) as f:
        lines = f.read().splitlines()
    if not lines:
        return {}
    names = lines[0].split()
    found = {name: [] for name in names}
    for line in lines[1:]:
        for name, value in zip(names, line.split()):
            if value != "_":
                found[name].append(float(value))
    return found


def main():
    directory, labels = sys.argv[1], sys.argv[2:]
    means = {}
    for label in labels:
        merged = {}
        for path in sorted(glob.glob(os.path.join(directory, f"{label}-*.dat"))):
            for name, values in samples(path).items():
                merged.setdefault(name, []).extend(values)
        means[label] = {name: sum(v) / len(v) for name, v in merged.items() if v}
    scenarios = sorted({name for found in means.values() for name in found})
    print("| scenario | " + " | ".join(labels) + " |")
    print("|---|" + "---|" * len(labels))
    for scenario in scenarios:
        values = [means[label].get(scenario) for label in labels]
        known = [v for v in values if v is not None]
        best = min(known) if known else None
        cells = []
        for value in values:
            if value is None:
                cells.append("-")
            elif value == best:
                cells.append(f"**{value:.1f}**")
            else:
                cells.append(f"{value:.1f}")
        print(f"| {scenario} | " + " | ".join(cells) + " |")


if __name__ == "__main__":
    main()
