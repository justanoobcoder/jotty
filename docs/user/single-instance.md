# Single instance

By default each jotty you start is a process of its own. With single
instance, new windows open in the jotty already running instead:

- windows open faster, since fonts are already loaded,
- all windows share one process's memory.

Turn it on with `--single-instance`, or in the config:

```toml
[window]
single_instance = true
```

| Key | Default | Description |
| --- | --- | --- |
| `single_instance` | `false` | Open new windows in the jotty already running. Applies when jotty next starts. |

A window opened this way still starts in the directory and environment it
was launched from, so a jotty started from a `nix develop` shell runs in
that environment.

Each config file has its own instance: `jotty -c other.toml
--single-instance` doesn't join the default one.

All windows of an instance close together if it crashes.

Ctrl+Shift+N opens a new window in the same process whether single
instance is on or not.
