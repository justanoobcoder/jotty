# Hints

Hints let you pick text on the screen from the keyboard.

1. Press Ctrl+Shift+E. A short label appears over every link, path, git or
   Nix hash, IP address and name of an existing file on the screen.
2. Type a label to pick its text.
3. Press a key to use it:

| Key | Does |
| --- | --- |
| Ctrl+C | Copy it |
| Ctrl+O | Open it, as Ctrl+click does |
| Ctrl+I | Type it at the cursor |
| Enter | Open it, or copy it if it can't be opened |
| Escape | Leave |

The newest output gets the shortest labels. Labels are typed by the
key's letter in a Latin layout, whatever layout is active.

## Labels

```toml
[hints]
alphabet = "jfkdlsa"   # home row only
```

| Key | Default | Description |
| --- | --- | --- |
| `alphabet` | `"asdfghjklqwertyuiopzxcvbnm"` | The characters labels are made of, the easiest to type first. At least two lowercase letters, all different. |

## Labeling more text

```toml
[hints]
regex = ['\b[A-Z]+-\d+\b']   # issue keys, such as JOT-123
```

| Key | Default | Description |
| --- | --- | --- |
| `regex` | `[]` | More text to label, as [regular expressions](https://docs.rs/regex). |

Text matched this way can be copied and typed, and opened when it names
a file. To open it with the link launcher, put the pattern in
[`url.regex`](links-and-files.md#links) instead.
