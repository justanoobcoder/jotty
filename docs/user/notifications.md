# Notifications

While jotty's window isn't focused, it shows the desktop notifications
programs ask for. Clicking one brings you back to the window.

```toml
[window]
notifications = true
command_notify_after = 30
bell_urgent = true
```

| Key | Default | Description |
| --- | --- | --- |
| `notifications` | `true` | Show notifications from programs and for long commands. |
| `command_notify_after` | `10` | Notify when a command that ran at least this many seconds finishes. `0` turns it off. |
| `bell_urgent` | `true` | Mark the window urgent when a program rings the bell. |

## From programs and scripts

Programs send notifications with an escape sequence. To be told when a
command finishes:

```sh
make; printf '\e]777;notify;make;finished with status %d\a' $?
```

## Long commands

With [shell integration](shell-integration.md), jotty tells you when a
command that ran for `command_notify_after` seconds or more finishes while
you're in another window. The notification shows the command, how long it
took and whether it failed.

This doesn't work inside zellij or tmux.
