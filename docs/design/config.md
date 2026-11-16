# Config and overrides (`config.rs`, `settings/`)

The config file and `~/.local/state/jotty/overrides.toml` (under
`$XDG_STATE_HOME`) are merged key by key, the overrides winning; lists are
replaced as a whole. jotty creates the directories of both at startup so
that `watch.rs` can watch them; a directory removed or replaced later
isn't watched again. Reload triggers are a change to either file
(inotify on their directories), the `reload_config` binding and SIGUSR1.
An invalid default config file is logged and the defaults are used; an
invalid `--config` file stops jotty from starting. A reload that fails
keeps the current settings. A new `font.size` replaces any zoom. Values
are clamped where a range makes sense: `font.size` to 4–200,
`font.size_step` to 0.1–10.

If the overrides file is broken, or valid alone but not over the config
file, jotty logs it and ignores the overrides. `jotty config` refuses to
open a broken file, so it can't overwrite it; `jotty config reset` removes
it. In the TUI, `Space` only toggles or cycles, `i` edits any value as
text, `R` removes a section's overrides, and lists are typed
comma-separated or as a TOML array. The palette is listed as 16 named
colors; changing one saves the whole list, which the TUI drops again once
it matches the config file. Saving an empty value removes the override of
an optional key such as `colors.cursor`. `export --merged` has only the
keys set in either file, without the config file's comments.
