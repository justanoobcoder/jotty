//! Links in the terminal: hyperlinks set by programs (OSC 8) and URLs
//! recognized in the text. Ctrl+click opens them.

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
    /// The built-in URL pattern first, then the configured ones.
    patterns: Vec<Regex>,
}

impl Detector {
    /// A detector for URLs and for the `extra` patterns from the config.
    pub fn new(extra: &[String]) -> Result<Detector, regex::Error> {
        let mut patterns = vec![Regex::new(URL_PATTERN)?];
        for pattern in extra {
            patterns.push(Regex::new(pattern)?);
        }
        Ok(Detector { patterns })
    }

    /// The link at `at`: a hyperlink, or else a URL or configured pattern
    /// in the text, which may continue on soft-wrapped rows.
    pub fn link_at(&self, screen: &Screen, at: Point) -> Option<Link> {
        let line = LineText::new(screen, at)?;
        let index = line.index_of(at)?;
        hyperlink(&line, screen, index).or_else(|| self.match_at(&line, index))
    }

    fn match_at(&self, line: &LineText, index: usize) -> Option<Link> {
        let offset = line.cells[index].offset;
        for (i, pattern) in self.patterns.iter().enumerate() {
            for m in pattern.find_iter(&line.text) {
                let end = if i == 0 {
                    m.start() + trim_url(m.as_str())
                } else {
                    m.end()
                };
                if m.start() <= offset && offset < end {
                    return Some(Link {
                        start: line.point_at(m.start()),
                        end: line.point_before(end),
                        target: line.text[m.start()..end].to_string(),
                    });
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
        detector.link_at(t.screen(), at).map(|l| l.target)
    }

    #[test]
    fn finds_urls_under_the_pointer() {
        let mut t = Terminal::new(60, 1, 0);
        t.feed(b"see https://example.org/a_(b)?q=1, or mailto:me@x.org.");
        let d = Detector::new(&[]).unwrap();
        let link = d.link_at(t.screen(), p(0, 10)).unwrap();
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
        let d = Detector::new(&[]).unwrap();
        let link = d.link_at(t.screen(), p(1, 3)).unwrap();
        assert_eq!(link.target, "https://example.org");
        assert_eq!((link.start, link.end), (p(0, 2), p(2, 0)));
    }

    #[test]
    fn hyperlinks_take_precedence() {
        let mut t = Terminal::new(40, 1, 0);
        t.feed(b"\x1b]8;;https://a.org\x1b\\click here\x1b]8;;\x1b\\ https://b.org");
        let d = Detector::new(&[]).unwrap();
        let link = d.link_at(t.screen(), p(0, 3)).unwrap();
        assert_eq!(link.target, "https://a.org");
        assert_eq!((link.start, link.end), (p(0, 0), p(0, 9)));
        assert_eq!(target(&d, &t, p(0, 15)).as_deref(), Some("https://b.org"));
    }

    #[test]
    fn configured_patterns() {
        let mut t = Terminal::new(40, 1, 0);
        t.feed(b"fixes JOT-123 and #45");
        let d = Detector::new(&[r"JOT-\d+".into()]).unwrap();
        assert_eq!(target(&d, &t, p(0, 8)).as_deref(), Some("JOT-123"));
        assert_eq!(target(&d, &t, p(0, 19)), None);
        assert!(Detector::new(&["(".into()]).is_err());
    }

    #[test]
    fn wide_characters_map_to_their_cells() {
        let mut t = Terminal::new(40, 1, 0);
        t.feed("中文 https://例え.jp/パス end".as_bytes());
        let d = Detector::new(&[]).unwrap();
        let link = d.link_at(t.screen(), p(0, 12)).unwrap();
        assert_eq!(link.target, "https://例え.jp/パス");
        assert_eq!((link.start, link.end), (p(0, 5), p(0, 23)));
    }
}
