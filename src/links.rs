//! Links in the terminal: hyperlinks set by programs (OSC 8), URLs and
//! paths of files such as images, videos, audio files and documents
//! recognized in the text. Ctrl+click opens them.

use std::cell::Cell;
use std::ops::Range;

use regex::Regex;

use crate::selection::Point;
use crate::terminal::{Flags, Screen};
use crate::text::LineText;

/// URLs with a scheme, `mailto:` addresses and magnet links. They end at
/// whitespace and at characters that usually delimit URLs in text.
const URL_PATTERN: &str = r#"(?:(?:https?|ftp|file|sftp|ssh|git|gemini|gopher|irc|ircs|news|nntp|telnet|wss?)://|mailto:|magnet:\?)[^\s<>"'`{}|\\^]+"#;

/// A link found in the text: a URL, an OSC 8 hyperlink or a file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// First and last cell, inclusive.
    pub start: Point,
    pub end: Point,
    /// What to open.
    pub target: String,
    /// For a file, the index of its kind in the list given to the detector.
    pub kind: Option<usize>,
}

/// Finds links in the text: URLs, the patterns in `url.regex`, and
/// names of files of the configured kinds.
pub struct Detector {
    url: Regex,
    /// Paths ending in one of the configured extensions, bare or in single
    /// quotes (as `ls` quotes names with spaces); `None` with no extensions.
    file: Option<Regex>,
    /// The extensions of each kind of file, lowercase.
    kinds: Vec<Vec<String>>,
    /// The configured patterns.
    extra: Vec<Regex>,
}

impl Detector {
    /// A detector for URLs, paths of files with the extensions of each of
    /// `kinds` (such as `png`), and the `extra` patterns from the config.
    pub fn try_new(kinds: &[Vec<String>], extra: &[String]) -> Result<Detector, regex::Error> {
        let kinds: Vec<Vec<String>> = kinds
            .iter()
            .map(|k| {
                k.iter()
                    .map(|e| e.trim_start_matches('.').to_lowercase())
                    .collect()
            })
            .collect();
        let extensions: Vec<&String> = kinds.iter().flatten().collect();
        let file = (!extensions.is_empty())
            .then(|| {
                let ext = extensions
                    .iter()
                    .map(|e| regex::escape(e))
                    .collect::<Vec<_>>()
                    .join("|");
                // Delimiters of paths in text, besides whitespace.
                let c = r#"[^\s'"`<>|;,()\[\]{}]"#;
                // A bare path must end there: not `x.png.bak`, but `x.png:`
                // and a sentence's final `x.png.` are fine.
                let end = r"(?:$|[^\w.]|\.(?:$|\s))";
                Regex::new(&format!(
                    r"(?i)'([^'\n]+\.(?:{ext}))'|({c}*\.(?:{ext})){end}"
                ))
            })
            .transpose()?;
        let extra = extra
            .iter()
            .map(|p| Regex::new(p))
            .collect::<Result<_, _>>()?;
        Ok(Detector {
            url: Regex::new(URL_PATTERN)?,
            file,
            kinds,
            extra,
        })
    }

    /// The link at `at`: a hyperlink, or else a URL, a file or a
    /// configured pattern in the text, which may continue on soft-wrapped
    /// rows. `resolve` turns a path from the text into the file to open,
    /// or `None` if there is no such file.
    pub fn link_at(
        &self,
        screen: &Screen,
        at: Point,
        resolve: impl Fn(&str) -> Option<String>,
    ) -> Option<Link> {
        let line = LineText::around(screen, at, LINK_REACH)?;
        let index = line.index_of(at)?;
        let offset = line.cells[index].offset;
        let link = |start: usize, end: usize, target: String, kind: Option<usize>| Link {
            start: line.point_at(start),
            end: line.point_before(end),
            target,
            kind,
        };
        if let Some(link) = hyperlink(&line, screen, index) {
            return Some(link);
        }
        for m in self.url.find_iter(&line.text) {
            let end = m.start() + trim_url(m.as_str());
            if m.start() <= offset && offset < end {
                let target = line.text[m.start()..end].to_string();
                return Some(link(m.start(), end, target, None));
            }
        }
        if let Some(file) = &self.file {
            for caps in file.captures_iter(&line.text) {
                // Quoted: the link covers the quotes. Bare: just the path.
                let (span, path) = match caps.get(1) {
                    Some(path) => (caps.get(0).expect("group 0 is the whole match"), path),
                    None => {
                        let path = caps.get(2).expect("the bare alternative matched");
                        (path, path)
                    }
                };
                if span.start() <= offset
                    && offset < span.end()
                    && let Some(target) = resolve(path.as_str())
                {
                    let ext = path
                        .as_str()
                        .rsplit('.')
                        .next()
                        .unwrap_or("")
                        .to_lowercase();
                    let kind = self.kinds.iter().position(|k| k.contains(&ext));
                    return Some(link(span.start(), span.end(), target, kind));
                }
            }
        }
        for pattern in &self.extra {
            for m in pattern.find_iter(&line.text) {
                if m.start() <= offset && offset < m.end() {
                    return Some(link(m.start(), m.end(), m.as_str().to_string(), None));
                }
            }
        }
        None
    }
}

/// Length of `url` without trailing punctuation that belongs to the
/// surrounding text, such as a final period or an unmatched `)`.
fn trim_url(url: &str) -> usize {
    // Counted once, then kept up to date as brackets are trimmed off.
    let mut open = (0usize, 0usize);
    let mut close = (0usize, 0usize);
    for c in url.chars() {
        match c {
            '(' => open.0 += 1,
            ')' => close.0 += 1,
            '[' => open.1 += 1,
            ']' => close.1 += 1,
            _ => {}
        }
    }
    let mut end = url.len();
    while let Some(c) = url[..end].chars().next_back() {
        let trim = match c {
            '.' | ',' | ':' | ';' | '!' | '?' => true,
            ')' => {
                let unbalanced = open.0 < close.0;
                close.0 -= unbalanced as usize;
                unbalanced
            }
            ']' => {
                let unbalanced = open.1 < close.1;
                close.1 -= unbalanced as usize;
                unbalanced
            }
            _ => false,
        };
        if !trim {
            break;
        }
        end -= c.len_utf8();
    }
    end
}

/// The hyperlink covering cell `index` of `line`: the run of cells around
/// it with the same link id.
fn hyperlink(line: &LineText, screen: &Screen, index: usize) -> Option<Link> {
    let id = line.cells[index].link;
    let target = screen.link(id)?;
    let first = line.cells[..index]
        .iter()
        .rposition(|c| c.link != id)
        .map_or(0, |i| i + 1);
    let last = line.cells[index..]
        .iter()
        .position(|c| c.link != id)
        .map_or(line.cells.len(), |i| index + i)
        - 1;
    Some(Link {
        start: line.cells[first].point,
        end: line.cells[last].point,
        target: target.to_string(),
        kind: None,
    })
}

/// A file name found in the text, with the first and last cell it
/// covers (quotes included) and what `resolve` made of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found<T> {
    pub start: Point,
    pub end: Point,
    pub value: T,
}

/// The file or directory named at `at`, for dragging it out: the name in
/// quotes around it (as `ls` quotes names with spaces), or else the word
/// around it, delimited by whitespace. `resolve` turns a name from the
/// text into an existing file or directory (its path, and whatever else
/// the caller needs), or `None`. A trailing `ls -F` indicator is dropped
/// when the name with it doesn't exist.
pub fn path_at<T>(
    screen: &Screen,
    at: Point,
    resolve: impl Fn(&str) -> Option<T>,
) -> Option<Found<T>> {
    let line = LineText::around(screen, at, LINK_REACH)?;
    let offset = line.cells[line.index_of(at)?].offset;
    let name = names(&line.text).find(|n| n.span.contains(&offset))?;
    found(screen, &line, &name, &resolve).or_else(|| {
        // Quotes around more than a name, as in a sentence.
        let word = words(&line.text, name.quoted()?).find(|w| w.span.contains(&offset))?;
        found(screen, &line, &word, &resolve)
    })
}

/// Every file or directory named in `start..=end`, for marking them: the
/// names (as [`path_at`] finds them) that overlap the range, in order, and
/// at most `limit` of them. Resolving names may touch the file system, so
/// at most [`MAX_NAMES_TRIED`] are tried.
pub fn paths_in<T>(
    screen: &Screen,
    start: Point,
    end: Point,
    limit: usize,
    resolve: impl Fn(&str) -> Option<T>,
) -> Vec<Found<T>> {
    let tried = Cell::new(0usize);
    let resolve = |name: &str| {
        tried.set(tried.get() + 1);
        resolve(name)
    };
    let mut paths = Vec::new();
    let mut line = start.line;
    while line <= end.line && paths.len() < limit && tried.get() < MAX_NAMES_TRIED {
        // The rest of a logical line wrapped over the range is read at
        // most to its end.
        let reach = end.line - line + 1;
        let Some(text) = LineText::around(screen, Point { line, col: 0 }, reach) else {
            line += 1;
            continue;
        };
        let in_range = |name: &Name| {
            text.point_before(name.span.end) >= start && text.point_at(name.span.start) <= end
        };
        for name in names(&text.text).filter(in_range) {
            if paths.len() == limit || tried.get() >= MAX_NAMES_TRIED {
                break;
            }
            if let Some(found) = found(screen, &text, &name, &resolve) {
                paths.push(found);
                continue;
            }
            // Quotes around more than a name, as in a sentence.
            let Some(quoted) = name.quoted() else {
                continue;
            };
            for word in words(&text.text, quoted).filter(in_range) {
                if paths.len() == limit || tried.get() >= MAX_NAMES_TRIED {
                    break;
                }
                paths.extend(found(screen, &text, &word, &resolve));
            }
        }
        line = text.last + 1;
    }
    paths
}

/// How many names [`paths_in`] tries at most.
const MAX_NAMES_TRIED: usize = 10_000;

/// A name in the text: its byte range, quotes included, and the name's
/// own range within it.
struct Name {
    span: Range<usize>,
    name: Range<usize>,
}

impl Name {
    /// The span, if the name is in quotes.
    fn quoted(&self) -> Option<Range<usize>> {
        (self.span != self.name).then(|| self.span.clone())
    }
}

/// The runs without whitespace in `span` of `text`, without quotes at
/// their ends.
fn words(text: &str, span: Range<usize>) -> impl Iterator<Item = Name> + '_ {
    let mut at = span.start;
    std::iter::from_fn(move || {
        loop {
            let start = at + text[at..span.end].find(|c: char| !c.is_whitespace())?;
            let end = text[start..span.end]
                .find(char::is_whitespace)
                .map_or(span.end, |i| start + i);
            at = end;
            // Without the quotes that ended up in the first and last words.
            let word = &text[start..end];
            let trimmed = word.trim_start_matches(['\'', '"']);
            let start = start + (word.len() - trimmed.len());
            let end = start + trimmed.trim_end_matches(['\'', '"']).len();
            if start < end {
                return Some(Name {
                    span: start..end,
                    name: start..end,
                });
            }
        }
    })
}

/// The names in `text`, left to right: runs in single or double quotes
/// (as `ls` prints names with spaces), or else runs without whitespace.
fn names(text: &str) -> impl Iterator<Item = Name> + '_ {
    let mut at = 0;
    std::iter::from_fn(move || {
        let start = at + text[at..].find(|c: char| !c.is_whitespace())?;
        let rest = &text[start..];
        let quote = rest.chars().next().filter(|&c| c == '\'' || c == '"');
        // A quote that isn't closed on the line is part of a word.
        let closed = quote.and_then(|q| {
            rest[1..]
                .find(['\n', q])
                .filter(|&i| rest[1 + i..].starts_with(q))
        });
        let found = match closed {
            Some(i) if i > 0 => Name {
                span: start..start + i + 2,
                name: start + 1..start + 1 + i,
            },
            _ => {
                let end = rest
                    .find(char::is_whitespace)
                    .map_or(text.len(), |i| start + i);
                Name {
                    span: start..end,
                    name: start..end,
                }
            }
        };
        at = found.span.end;
        Some(found)
    })
}

/// `name` from `line` resolved, with the cells it covers. A trailing
/// `ls -F` indicator is dropped when the name with it doesn't resolve.
fn found<T>(
    screen: &Screen,
    line: &LineText,
    name: &Name,
    resolve: &impl Fn(&str) -> Option<T>,
) -> Option<Found<T>> {
    let text = &line.text[name.name.clone()];
    let value = resolve(text).or_else(|| {
        let bare = text.strip_suffix(['/', '*', '@', '=', '|'])?;
        (!bare.is_empty()).then(|| resolve(bare)).flatten()
    })?;
    let mut end = line.point_before(name.span.end);
    // A wide character ends in its second cell.
    if screen
        .row_at(end.line)
        .and_then(|row| row.cells().get(end.col))
        .is_some_and(|cell| cell.flags.contains(Flags::WIDE))
    {
        end.col += 1;
    }
    Some(Found {
        start: line.point_at(name.span.start),
        end,
        value,
    })
}

/// How many rows of a wrapped line around the pointer are searched for links.
/// This runs on every pointer motion while Ctrl is held, and a line can wrap
/// over the whole history; no link is that long.
const LINK_REACH: isize = 64;

/// Whether a target can be given to a launcher as its argument. Programs can
/// set a hyperlink's target to anything, and an option isn't a place to open.
#[must_use]
pub fn is_openable(target: &str) -> bool {
    !target.is_empty() && !target.starts_with('-')
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn long_lines_are_searched_only_around_the_pointer() {
        let mut t = Terminal::new(10, 5, 1000);
        t.feed("x".repeat(5000).as_bytes());
        let at = Point { line: -100, col: 0 };
        let line = LineText::around(t.screen(), at, LINK_REACH).unwrap();
        assert!(line.first >= at.line - LINK_REACH && line.last <= at.line + LINK_REACH);
        assert!(line.cells.len() <= 10 * (2 * LINK_REACH as usize + 1));
    }

    #[test]
    fn targets_that_look_like_options_are_not_opened() {
        assert!(is_openable("https://example.com"));
        assert!(is_openable("/tmp/-file"));
        assert!(!is_openable("--config=/tmp/evil"));
        assert!(!is_openable(""));
    }
    use crate::terminal::Terminal;

    fn p(line: isize, col: usize) -> Point {
        Point { line, col }
    }

    fn target(detector: &Detector, t: &Terminal, at: Point) -> Option<String> {
        detector.link_at(t.screen(), at, |_| None).map(|l| l.target)
    }

    #[test]
    fn finds_urls_under_the_pointer() {
        let mut t = Terminal::new(60, 1, 0);
        t.feed(b"see https://example.org/a_(b)?q=1, or mailto:me@x.org.");
        let d = Detector::try_new(&[], &[]).unwrap();
        let link = d.link_at(t.screen(), p(0, 10), |_| None).unwrap();
        assert_eq!(link.target, "https://example.org/a_(b)?q=1");
        assert_eq!((link.start, link.end), (p(0, 4), p(0, 32)));
        assert_eq!(target(&d, &t, p(0, 40)).as_deref(), Some("mailto:me@x.org"));
        assert_eq!(target(&d, &t, p(0, 1)), None);
        // The trailing comma is not part of the URL.
        assert_eq!(target(&d, &t, p(0, 33)), None);
    }

    #[test]
    fn trims_unbalanced_brackets_and_punctuation() {
        assert_eq!(trim_url("https://x.org/a)."), "https://x.org/a".len());
        assert_eq!(trim_url("https://x.org/(a)"), "https://x.org/(a)".len());
        assert_eq!(trim_url("https://x.org/a]"), "https://x.org/a".len());
        assert_eq!(trim_url("https://x.org/(a))]."), "https://x.org/(a)".len());
        let many = format!("https://x.org/a{}", ")".repeat(10_000));
        assert_eq!(trim_url(&many), "https://x.org/a".len());
    }

    #[test]
    fn urls_continue_on_wrapped_rows() {
        let mut t = Terminal::new(10, 3, 0);
        t.feed(b"x https://example.org y");
        let d = Detector::try_new(&[], &[]).unwrap();
        let link = d.link_at(t.screen(), p(1, 3), |_| None).unwrap();
        assert_eq!(link.target, "https://example.org");
        assert_eq!((link.start, link.end), (p(0, 2), p(2, 0)));
    }

    #[test]
    fn hyperlinks_take_precedence() {
        let mut t = Terminal::new(40, 1, 0);
        t.feed(b"\x1b]8;;https://a.org\x1b\\click here\x1b]8;;\x1b\\ https://b.org");
        let d = Detector::try_new(&[], &[]).unwrap();
        let link = d.link_at(t.screen(), p(0, 3), |_| None).unwrap();
        assert_eq!(link.target, "https://a.org");
        assert_eq!((link.start, link.end), (p(0, 0), p(0, 9)));
        assert_eq!(target(&d, &t, p(0, 15)).as_deref(), Some("https://b.org"));
    }

    #[test]
    fn configured_patterns() {
        let mut t = Terminal::new(40, 1, 0);
        t.feed(b"fixes JOT-123 and #45");
        let d = Detector::try_new(&[], &[r"JOT-\d+".into()]).unwrap();
        assert_eq!(target(&d, &t, p(0, 8)).as_deref(), Some("JOT-123"));
        assert_eq!(target(&d, &t, p(0, 19)), None);
        assert!(Detector::try_new(&[], &["(".into()]).is_err());
    }

    fn media() -> Detector {
        let kinds = [vec!["png".into(), "JPG".into()], vec![".mp4".into()]];
        Detector::try_new(&kinds, &[]).unwrap()
    }

    /// Resolves paths as if every file existed under `/home/u`.
    fn exists(path: &str) -> Option<String> {
        Some(format!("/home/u/{path}"))
    }

    #[test]
    fn finds_media_files() {
        let mut t = Terminal::new(80, 1, 0);
        t.feed(b"a.txt shot.PNG pics/cat.jpg 'my clip.mp4' x.png.bak b.png: c.png.");
        let d = media();
        let target = |col| d.link_at(t.screen(), p(0, col), exists).map(|l| l.target);
        assert_eq!(target(0), None);
        assert_eq!(target(8).as_deref(), Some("/home/u/shot.PNG"));
        let kind = |col| {
            d.link_at(t.screen(), p(0, col), exists)
                .and_then(|l| l.kind)
        };
        assert_eq!((kind(8), kind(17), kind(32)), (Some(0), Some(0), Some(1)));
        assert_eq!(target(17).as_deref(), Some("/home/u/pics/cat.jpg"));
        // Quoted names with spaces; the link covers the quotes.
        let link = d.link_at(t.screen(), p(0, 32), exists).unwrap();
        assert_eq!(link.target, "/home/u/my clip.mp4");
        assert_eq!((link.start, link.end), (p(0, 28), p(0, 40)));
        assert_eq!(target(45), None);
        let link = d.link_at(t.screen(), p(0, 52), exists).unwrap();
        assert_eq!(
            (link.target.as_str(), link.end),
            ("/home/u/b.png", p(0, 56))
        );
        assert_eq!(target(59).as_deref(), Some("/home/u/c.png"));
    }

    #[test]
    fn missing_files_are_not_links() {
        let mut t = Terminal::new(20, 1, 0);
        t.feed(b"gone.png");
        assert_eq!(media().link_at(t.screen(), p(0, 1), |_| None), None);
    }

    #[test]
    fn urls_win_over_files() {
        let mut t = Terminal::new(40, 1, 0);
        t.feed(b"https://x.org/a.png");
        let link = media().link_at(t.screen(), p(0, 16), exists).unwrap();
        assert_eq!(link.target, "https://x.org/a.png");
    }

    #[test]
    fn wide_characters_map_to_their_cells() {
        let mut t = Terminal::new(40, 1, 0);
        t.feed("中文 https://例え.jp/パス end".as_bytes());
        let d = Detector::try_new(&[], &[]).unwrap();
        let link = d.link_at(t.screen(), p(0, 12), |_| None).unwrap();
        assert_eq!(link.target, "https://例え.jp/パス");
        assert_eq!((link.start, link.end), (p(0, 5), p(0, 23)));
    }

    /// Resolves names as if only `names` existed under `/home/u`.
    fn only<'n>(names: &'n [&str]) -> impl Fn(&str) -> Option<PathBuf> + 'n {
        move |name| {
            names
                .contains(&name)
                .then(|| PathBuf::from(format!("/home/u/{name}")))
        }
    }

    #[test]
    fn finds_paths_of_files_and_directories() {
        let mut t = Terminal::new(60, 1, 0);
        t.feed(b"main.rs  src/  'a b.txt'  run*  gone");
        let names = ["main.rs", "src/", "a b.txt", "run"];
        let path = |col| path_at(t.screen(), p(0, col), only(&names)).map(|f| f.value);
        assert_eq!(path(2), Some(PathBuf::from("/home/u/main.rs")));
        assert_eq!(path(10), Some(PathBuf::from("/home/u/src/")));
        // Anywhere in a quoted name, quotes included.
        assert_eq!(path(15), Some(PathBuf::from("/home/u/a b.txt")));
        assert_eq!(path(19), Some(PathBuf::from("/home/u/a b.txt")));
        // An `ls -F` indicator is dropped.
        assert_eq!(path(27), Some(PathBuf::from("/home/u/run")));
        assert_eq!(path(33), None);
        // Between names.
        assert_eq!(path(7), None);
    }

    #[test]
    fn stray_apostrophes_fall_back_to_the_word() {
        let mut t = Terminal::new(60, 1, 0);
        t.feed(b"it's notes.md 'x'");
        let names = ["notes.md"];
        let path = |col| path_at(t.screen(), p(0, col), only(&names)).map(|f| f.value);
        assert_eq!(path(6), Some(PathBuf::from("/home/u/notes.md")));
    }

    #[test]
    fn wide_characters_in_paths() {
        let mut t = Terminal::new(40, 1, 0);
        t.feed("né 中文.png".as_bytes());
        let names = ["中文.png"];
        let path = |col| path_at(t.screen(), p(0, col), only(&names)).map(|f| f.value);
        assert_eq!(path(7), Some(PathBuf::from("/home/u/中文.png")));
    }

    #[test]
    fn paths_cover_their_quotes() {
        let mut t = Terminal::new(60, 1, 0);
        t.feed(b"x 'a b.txt' \"it's\"");
        let names = ["a b.txt", "it's"];
        let found = path_at(t.screen(), p(0, 4), only(&names)).unwrap();
        assert_eq!((found.start, found.end), (p(0, 2), p(0, 10)));
        let found = path_at(t.screen(), p(0, 13), only(&names)).unwrap();
        assert_eq!(found.value, PathBuf::from("/home/u/it's"));
        assert_eq!((found.start, found.end), (p(0, 12), p(0, 17)));
    }

    #[test]
    fn finds_every_path_in_a_range() {
        let mut t = Terminal::new(30, 3, 0);
        t.feed(b"a.rs  'b c'  d/\r\ne.rs  gone  f.rs\r\ng.rs");
        let names = ["a.rs", "b c", "d/", "e.rs", "f.rs", "g.rs"];
        let paths = |start, end, limit| -> Vec<String> {
            paths_in(t.screen(), start, end, limit, only(&names))
                .into_iter()
                .map(|f| f.value.to_string_lossy().into_owned())
                .collect()
        };
        // Names partly in the range count, from the first row's `'b c'`
        // to the second row's `f.rs`.
        assert_eq!(
            paths(p(0, 8), p(1, 12), 100),
            ["/home/u/b c", "/home/u/d/", "/home/u/e.rs", "/home/u/f.rs"]
        );
        assert_eq!(paths(p(0, 0), p(2, 0), 2), ["/home/u/a.rs", "/home/u/b c"]);
        assert!(paths(p(0, 4), p(0, 5), 100).is_empty());
    }

    #[test]
    fn ranges_follow_wrapped_rows() {
        let mut t = Terminal::new(10, 3, 0);
        t.feed(b"aaaa.rs bbbbbbb.rs");
        let names = ["aaaa.rs", "bbbbbbb.rs"];
        let found = paths_in(t.screen(), p(1, 0), p(1, 0), 100, only(&names));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].value, PathBuf::from("/home/u/bbbbbbb.rs"));
        assert_eq!((found[0].start, found[0].end), (p(0, 8), p(1, 7)));
    }

    #[test]
    fn names_end_in_the_second_cell_of_a_wide_character() {
        let mut t = Terminal::new(20, 1, 0);
        t.feed("x 中文 y".as_bytes());
        let names = ["中文"];
        let found = path_at(t.screen(), p(0, 5), only(&names)).unwrap();
        assert_eq!((found.start, found.end), (p(0, 2), p(0, 5)));
    }

    #[test]
    fn words_in_quotes_that_name_nothing_are_tried_alone() {
        let mut t = Terminal::new(40, 1, 0);
        t.feed(b"\"see a.txt and b.txt\"");
        let names = ["a.txt", "b.txt"];
        let found = path_at(t.screen(), p(0, 7), only(&names)).unwrap();
        assert_eq!(found.value, PathBuf::from("/home/u/a.txt"));
        assert_eq!((found.start, found.end), (p(0, 5), p(0, 9)));
        let all: Vec<PathBuf> = paths_in(t.screen(), p(0, 0), p(0, 39), 100, only(&names))
            .into_iter()
            .map(|f| f.value)
            .collect();
        assert_eq!(
            all,
            [
                PathBuf::from("/home/u/a.txt"),
                PathBuf::from("/home/u/b.txt")
            ]
        );
    }

    #[test]
    fn sweeps_try_a_bounded_number_of_names() {
        let mut t = Terminal::new(10, 2000, 0);
        for _ in 0..1999 {
            t.feed(b"a b c d e\r\n");
        }
        let tried = Cell::new(0);
        let found = paths_in(t.screen(), p(0, 0), p(1999, 9), 100, |_| {
            tried.set(tried.get() + 1);
            None::<()>
        });
        assert!(found.is_empty());
        // Each name is tried bare and without an `ls -F` indicator at most.
        assert!(tried.get() <= 2 * MAX_NAMES_TRIED, "tried {}", tried.get());
    }
}
