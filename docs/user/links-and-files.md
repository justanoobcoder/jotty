# Links and files

## Opening links and files

Hold Ctrl over a link to underline it, and Ctrl+click to open it. Links
are:

- URLs in the text,
- links that programs print (`ls --hyperlink`, `gcc`, `delta`),
- names of image, video, audio and document files in the output, such as
  `cat.png` in `ls` output.

## Links

```toml
[url]
launcher = ["xdg-open"]
regex = ['\bJOT-\d+\b']
```

| Key | Default | Description |
| --- | --- | --- |
| `launcher` | `["xdg-open"]` | Command that opens links. The link is added at the end. |
| `regex` | `[]` | More text to treat as links, as [regular expressions](https://docs.rs/regex). |

The launcher gets the text a `regex` matched as it is. To open issue keys
such as `JOT-123` in a tracker, give it a small script:

```toml
[url]
launcher = ["sh", "-c", 'case "$0" in JOT-*) xdg-open "https://tracker.example/$0";; *) xdg-open "$0";; esac']
regex = ['\bJOT-\d+\b']
```

## Files

Files come in kinds, by extension. Each kind is a table
`[files.<name>]`:

```toml
[files.video]
launcher = ["mpv"]

[files.document]
launcher = ["zathura"]
```

| Key | Default | Description |
| --- | --- | --- |
| `extensions` | depends on the kind | File extensions of this kind. `[]` turns the kind off. |
| `launcher` | `[]` | Command that opens these files. Empty means `url.launcher`. |

Four kinds are built in:

| Kind | Extensions |
| --- | --- |
| `image` | png, jpg, jpeg, gif, webp, avif, jxl, heic, heif, bmp, tif, tiff, svg, ico |
| `video` | mp4, mkv, webm, mov, avi, m4v, wmv, flv, mpg, mpeg, ogv, 3gp |
| `audio` | mp3, flac, ogg, oga, opus, wav, m4a, aac, wma, aiff, aif, ape, alac, mka |
| `document` | pdf, epub, djvu, doc, docx, odt, rtf, xls, xlsx, ods, ppt, pptx, odp |

Setting a built-in kind changes only the keys you set, so the example
above keeps their extensions. Add kinds of your own with new names:

```toml
[files.sheet]
extensions = ["csv", "tsv"]
launcher = ["libreoffice", "--calc"]
```

A kind you add wins over the built-in ones for an extension they share.
Only files that exist are links; relative names are found in your shell's
current directory.

## Dragging files out

Ctrl+drag the name of a file or directory in the output, as `ls` prints
it, into another application: a file manager, a browser upload or a chat.

To drag several at once:

1. Ctrl+right click each name to mark it, or Ctrl+right drag over output
   to mark every file named in it.
2. Ctrl+drag one of the marked names.

Escape clears the marks. `colors.mark_background` sets their color.

## Dropping files in

Drop files onto the window to type their paths at the cursor, quoted for
the shell. Nothing runs until you press Enter.
