# Benchmarks

Scripts that compare jotty with other terminals, or one jotty build with
another. Each one opens the terminals on your desktop in turn, round after
round, so that changes in the machine's speed hit them alike, and prints
medians or means.

| Script | Measures |
| --- | --- |
| `cat.sh` | How long `cat` of a large file takes: how fast the terminal takes in output |
| `vtebench.sh` | [vtebench](https://github.com/alacritty/vtebench)'s scenarios: colored cells, cursor motion, scrolling in regions, unicode |
| `startup.sh` | Time from start until a command runs, idle memory, memory with a full scrollback |

```sh
scripts/bench/cat.sh                         # all terminals, 11 rounds
scripts/bench/cat.sh -x 4 -n 5 jotty-cpu foot
nix shell nixpkgs#vtebench -c scripts/bench/vtebench.sh
scripts/bench/startup.sh jotty-cpu jotty-gpu
```

Terminals are named `jotty-cpu`, `jotty-gpu`, `foot`, `kitty`, `ghostty`,
`alacritty`, `rio-cpu` and `rio-gpu`, and use the program of that name on
`PATH`. `KIND@BINARY` runs another binary, and `LABEL=` names it in the
results; to compare a build with the installed jotty:

```sh
nix build
scripts/bench/cat.sh new=jotty-cpu@./result/bin/jotty old=jotty-cpu
```

Every terminal gets a fresh config with the same font, size, opacity and
scrollback (`FONT`, `FONT_SIZE`, `RIO_FONT_SIZE`, `OPACITY`, `SCROLLBACK` in
the environment; 12 pt JetBrainsMono Nerd Font, 0.8, 5000 lines by default).
jotty runs without your overrides. Results are only comparable within one
run of a script on one machine: from one day to the next they drift by
about 10%.

Things that skew results:

- The compositor picks the window size; check the grid size that `cat.sh`
  prints. A terminal with more cells to draw is at a disadvantage.
- ghostty's line limit (`scrollback-limit-lines`) makes it much slower at
  scrolling than with its default byte limit.
- rio before 0.6 has no scrollback setting and keeps 10,000 lines.
- Run nothing else meanwhile, and keep the machine on power.
