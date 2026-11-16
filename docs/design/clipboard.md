# Clipboard (`app.rs`)

Copying creates a `wl_data_source` for `text/plain;charset=utf-8`,
`UTF8_STRING` and `text/plain`. When another client asks for the data, a
short-lived thread writes it to the pipe, so a slow reader can't block the
event loop. Pasting reads the current selection offer through a calloop
source on the pipe. When jotty owns the selection itself, it pastes directly
without a round trip. The primary selection works the same way through
`primary-selection-unstable-v1`: finishing a selection offers it, and a
middle click pastes it. Text a program sends with OSC 52 (`OSC 52 ; c ; <base64> ST`; `p` or `s`
for the primary selection, `pc` for both) takes the same
path, after the read of child output that carried it; of several in one
read, the last for each selection wins. Before the window's first keyboard focus it waits for
the focus, whose serial the compositor accepts.

**Selection and output.** Before reading child output, the app takes a
fingerprint of each selected row on the live screen and the grid's count
of rows scrolled off the top. Afterwards it moves the selection up by the
number of rows that scrolled, and keeps it only if every fingerprinted row
still holds the same text. Pasted text is wrapped in bracketed-paste markers when
the application asked for them, and ESC characters are removed so the paste
can't end bracketed mode early.
