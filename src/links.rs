//! Links in the terminal: hyperlinks set by programs (OSC 8), URLs and
//! paths of files such as images and videos recognized in the text.
//! Ctrl+click opens them.

use regex::Regex;

use crate::selection::Point;
use crate::terminal::Screen;
use crate::text::LineText;

/// URLs with a scheme, `mailto:` addresses and magnet links. They end at
/// whitespace and at characters that usually delimit URLs in text.
const URL_PATTERN: &str = r#"(?:(?:https?|ftp|file|sftp|ssh|git|gemini|gopher|irc|ircs|news|nntp|telnet|wss?)://|mailto:|magnet:\?)[^\s<>"'`{}|\\^]+"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// First and last cell, inclusive.
    pub start: Point,
    pub end: Point,
    /// What to open.
    pub target: String,
}

pub struct Detector {
    url: Regex,
    /// Paths ending in one of the configured extensions, bare or in single
    /// quotes (as `ls` quotes names with spaces); `None` with no extensions.
    file: Option<Regex>,
    /// The configured patterns.
    extra: Vec<Regex>,
}

impl Detector {
    /// A detector for URLs, paths of files with `extensions` (such as
    /// `png`), and the `extra` patterns from the config.
    pub fn new(extensions: &[String], extra: &[String]) -> Result<Detector, regex::Error> {
        let file = (!extensions.is_empty())
            .then(|| {
                let ext = extensions
                    .iter()
                    .map(|e| regex::escape(e.trim_start_matches('.')))
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
        let line = LineText::new(screen, at)?;
        let index = line.index_of(at)?;
        let offset = line.cells[index].offset;
        let link = |start: usize, end: usize, target: String| Link {
            start: line.point_at(start),
            end: line.point_before(end),
            target,
        };
        if let Some(link) = hyperlink(&line, screen, index) {
            return Some(link);
        }
        for m in self.url.find_iter(&line.text) {
            let end = m.start() + trim_url(m.as_str());
            if m.start() <= offset && offset < end {
                return Some(link(m.start(), end, line.text[m.start()..end].to_string()));
            }
        }
        if let Some(file) = &self.file {
            for caps in file.captures_iter(&line.text) {
                // Quoted: the link covers the quotes. Bare: just the path.
                let (span, path) = match caps.get(1) {
                    Some(path) => (caps.get(0).unwrap(), path),
                    None => (caps.get(2).unwrap(), caps.get(2).unwrap()),
                };
                if span.start() <= offset
                    && offset < span.end()
                    && let Some(target) = resolve(path.as_str())
                {
                    return Some(link(span.start(), span.end(), target));
                }
            }
        }
        for pattern in &self.extra {
            for m in pattern.find_iter(&line.text) {
                if m.start() <= offset && offset < m.end() {
                    return Some(link(m.start(), m.end(), m.as_str().to_string()));
                }
            }
        }
        None
    }
}

/// Length of `url` without trailing punctuation that belongs to the
/// surrounding text, such as a final period or an unmatched `)`.
fn trim_url(url: &str) -> usize {
    let mut end = url.len();
    while let Some(c) = url[..end].chars().next_back() {
        let s = &url[..end];
        let trim = match c {
            '.' | ',' | ':' | ';' | '!' | '?' => true,
            ')' => s.matches('(').count() < s.matches(')').count(),
            ']' => s.matches('[').count() < s.matches(']').count(),
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let d = Detector::new(&[], &[]).unwrap();
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
    }

    #[test]
    fn urls_continue_on_wrapped_rows() {
        let mut t = Terminal::new(10, 3, 0);
        t.feed(b"x https://example.org y");
        let d = Detector::new(&[], &[]).unwrap();
        let link = d.link_at(t.screen(), p(1, 3), |_| None).unwrap();
        assert_eq!(link.target, "https://example.org");
        assert_eq!((link.start, link.end), (p(0, 2), p(2, 0)));
    }

    #[test]
    fn hyperlinks_take_precedence() {
        let mut t = Terminal::new(40, 1, 0);
        t.feed(b"\x1b]8;;https://a.org\x1b\\click here\x1b]8;;\x1b\\ https://b.org");
        let d = Detector::new(&[], &[]).unwrap();
        let link = d.link_at(t.screen(), p(0, 3), |_| None).unwrap();
        assert_eq!(link.target, "https://a.org");
        assert_eq!((link.start, link.end), (p(0, 0), p(0, 9)));
        assert_eq!(target(&d, &t, p(0, 15)).as_deref(), Some("https://b.org"));
    }

    #[test]
    fn configured_patterns() {
        let mut t = Terminal::new(40, 1, 0);
        t.feed(b"fixes JOT-123 and #45");
        let d = Detector::new(&[], &[r"JOT-\d+".into()]).unwrap();
        assert_eq!(target(&d, &t, p(0, 8)).as_deref(), Some("JOT-123"));
        assert_eq!(target(&d, &t, p(0, 19)), None);
        assert!(Detector::new(&[], &["(".into()]).is_err());
    }

    fn media() -> Detector {
        Detector::new(&["png".into(), "jpg".into(), "mp4".into()], &[]).unwrap()
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
        let d = Detector::new(&[], &[]).unwrap();
        let link = d.link_at(t.screen(), p(0, 12), |_| None).unwrap();
        assert_eq!(link.target, "https://例え.jp/パス");
        assert_eq!((link.start, link.end), (p(0, 5), p(0, 23)));
    }
}
