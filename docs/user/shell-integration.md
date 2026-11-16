# Shell integration

When your shell tells jotty where its prompt and each command's output are,
jotty can:

- scroll from prompt to prompt with Ctrl+Shift+PageUp and PageDown,
- select a command's whole output with Ctrl+double click,
- copy or open the last command's output,
- [notify you](notifications.md#long-commands) when a long command
  finishes,
- give input methods just your command line, without the prompt.

## Setting it up

**fish** 4.0 and later does it by itself.

**bash**: add to `~/.bashrc`, after `PS1` is set:

```bash
PS1='\[\e]133;D;$?\a\e]133;A\a\]'"$PS1"'\[\e]133;B\a\]'
PS0='\e]133;C\a'
```

**zsh**: add to `~/.zshrc`, after the prompt is set up:

```zsh
autoload -Uz add-zsh-hook
_jotty_prompt_start() { print -n "\e]133;D;$?\a\e]133;A\a" }
add-zsh-hook precmd _jotty_prompt_start
_jotty_output_start() { print -n '\e]133;C\a' }
add-zsh-hook preexec _jotty_output_start
PS1="$PS1"$'%{\e]133;B\a%}'
```

**fish** before 4.0: add to `~/.config/fish/config.fish`:

```fish
functions -c fish_prompt __jotty_fish_prompt
function fish_prompt
    printf '\e]133;D;%d\a\e]133;A\a' $status
    __jotty_fish_prompt
    printf '\e]133;B\a'
end
function __jotty_output_start --on-event fish_preexec
    printf '\e]133;C\a'
end
```

Shell integration doesn't work inside zellij or tmux, which don't pass
these marks on to jotty.

## Command output

Ctrl+double click anywhere in a command's output to select all of it.
`terminal.output_click` changes what the click does:

```toml
[terminal]
output_click = "open"
```

| Value | Does |
| --- | --- |
| `"select"` (default) | Select the output |
| `"copy"` | Select it and copy it |
| `"open"` | Open it in your editor, in a new window |

`copy_last_output` and `open_last_output` do the same for the last command
from the keyboard. They have no keys by default; see
[Keybinds](keybinds.md#examples).

### The editor

Output opens in `terminal.editor`, or else `$VISUAL`, `$EDITOR`, then
`less`:

```toml
[terminal]
editor = ["nvim", "-R"]
```

- Pick an editor that runs in a terminal and stays open until you close
  it. Graphical editors such as `code` need their `--wait` flag.
- Set `editor` in the config if you start jotty from an app launcher,
  which doesn't see the variables your shell sets.
- The same editor opens files at a line from [hints](hints.md), with
  `+LINE` before the file.
