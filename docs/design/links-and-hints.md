# Links, files and hints (`links.rs`, `hints.rs`)

**URLs** are recognized by their scheme (`https://`, `file://`, `ssh://`,
`git://`, `mailto:`, ...) and may wrap across rows. Trailing punctuation
and unmatched closing brackets are left out, so `(see https://x.org/a).`
opens `https://x.org/a`. `url.regex` patterns are matched against the line
under the pointer; an invalid one is logged and only the built-in URL
pattern is used. A link opens when the button is released, unless the
pointer moved.

**Files.** A path is absolute, starts with `~/`, or is relative to the
current directory of the foreground program; names `ls` quotes for spaces
work too, and only files that exist are links. Extensions match
case-insensitively. When an extension is in several kinds, kinds added
under new names win over the built-in ones, and otherwise the first kind
in alphabetical order wins; a built-in kind keeps its place even when
changed. Ctrl+drag takes the whitespace-delimited word under the pointer,
or the single-quoted name around it, dropping a trailing `ls -F` indicator
(`/`, `*`, `@`) if needed. Marks keep the path they had when made, even
after a `cd`; if a marked file is gone when dragged, nothing is dragged.
Dropped links are typed like paths, and dropped text is pasted as it is;
files dragged out of a window aren't taken back by it.

**Hints** label, in this order of precedence where targets overlap: links
(as Ctrl+click finds them), `hints.regex` matches, IPv4 addresses (with a
`/24` prefix length or a `:8080` port) and IPv6 addresses, Nix hashes (SRI
`sha256-…=` of the right length for md5, sha1, sha256 or sha512, or 52
characters of base 32), paths starting with `/`, `~/`, `./` or `../`, hex
hashes of 7 to 128 characters with both letters and digits (`1af659b`, not
`deadbeef`), and names of existing files. The newest output gets the
shortest labels. Whether a target names a file is checked when it is
opened. Copying a link gives its target (the URL of a hyperlink, the
absolute path of a file). Output keeps arriving while the labels are up:
targets move with their text, those whose text changes are dropped, and a
resize takes the labels down.
