//! Hints: short labels on the links and other useful text on the screen
//! (paths, hashes, IP addresses), so that the keyboard can pick one to
//! copy, open or insert.

use std::net::Ipv6Addr;
use std::ops::Range;

use regex::Regex;

use crate::links::{self, Detector, LINK_REACH, Link};
use crate::selection::{Point, Selection, Unit};
use crate::terminal::Screen;
use crate::text::LineText;

/// The characters labels are made of by default, the easiest to type
/// first.
pub const DEFAULT_ALPHABET: &str = "asdfghjklqwertyuiopzxcvbnm";

/// At most this many files named on the screen are hinted.
const MAX_FILES: usize = 1000;

/// Text worth a hint besides links, built in or configured.
pub struct Patterns {
    /// `hints.regex` from the config.
    extra: Vec<Regex>,
    /// Runs that may be IPv6 addresses; [`ipv6`] checks them.
    ipv6: Regex,
    ipv4: Regex,
    /// Hashes in the SRI format Nix prints (`sha256-…=`), each algorithm
    /// with its length in base 64.
    sri: Regex,
    /// Paths that start with `/`, `~/`, `./` or `../`.
    path: Regex,
    /// SHA-256 hashes in Nix's base-32.
    nix32: Regex,
    uuid: Regex,
    /// Hexadecimal hashes, such as git commits; [`hex_hash`] checks them.
    hex: Regex,
}

impl Patterns {
    /// The built-in patterns and `extra`, from `hints.regex`.
    pub fn try_new(extra: &[String]) -> Result<Patterns, regex::Error> {
        let octet = r"(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)";
        let path_char = r"[\w.@+~-]";
        Ok(Patterns {
            extra: extra
                .iter()
                .map(|p| Regex::new(p))
                .collect::<Result<_, _>>()?,
            ipv6: Regex::new(r"[0-9A-Fa-f:]+")?,
            ipv4: Regex::new(&format!(
                r"\b(?:{octet}\.){{3}}{octet}(?:/\d{{1,2}}|:\d{{1,5}})?\b"
            ))?,
            sri: Regex::new(
                r"\b(?:md5-[A-Za-z0-9+/]{22}==|sha1-[A-Za-z0-9+/]{27}=|sha256-[A-Za-z0-9+/]{43}=|sha512-[A-Za-z0-9+/]{86}==)",
            )?,
            // With a line number, and maybe a column, after it.
            path: Regex::new(&format!(
                r"(?:~|\.{{1,2}})?(?:/{path_char}+)+/?(?::\d+(?::\d+)?\b)?"
            ))?,
            nix32: Regex::new(r"\b[0-9a-df-np-sv-z]{52}\b")?,
            uuid: Regex::new(r"\b[0-9A-Fa-f]{8}(?:-[0-9A-Fa-f]{4}){3}-[0-9A-Fa-f]{12}\b")?,
            hex: Regex::new(r"\b(?:[0-9a-f]{7,128}|[0-9A-F]{7,128})\b")?,
        })
    }

    /// The byte ranges of `text` worth a hint, by precedence: the
    /// configured patterns first. They may overlap, but none is empty
    /// (a configured pattern may match the empty string).
    fn matches(&self, text: &str) -> Vec<Range<usize>> {
        let mut found = Vec::new();
        for pattern in &self.extra {
            found.extend(pattern.find_iter(text).map(|m| m.range()));
        }
        found.extend(
            self.ipv6
                .find_iter(text)
                .filter_map(|m| ipv6(text, m.range())),
        );
        found.extend(self.ipv4.find_iter(text).map(|m| m.range()));
        // Not followed by more base 64: a hash too long isn't one.
        let base64 = |c: char| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=');
        found.extend(
            self.sri
                .find_iter(text)
                .filter(|m| !text[m.end()..].starts_with(base64))
                .map(|m| m.range()),
        );
        // A path starts a word: the `/` in `and/or`, in base 64 (`a+/b`)
        // or in `</div>` doesn't start one. Without a letter, it's a
        // number (`/2`).
        let in_word = |c: char| c.is_alphanumeric() || "_.@+~-/<".contains(c);
        found.extend(
            self.path
                .find_iter(text)
                .filter(|m| !text[..m.start()].chars().next_back().is_some_and(in_word))
                .filter(|m| m.as_str().chars().any(char::is_alphabetic))
                .map(|m| without_final_dot(m.as_str(), m.range())),
        );
        for pattern in [&self.nix32, &self.uuid] {
            found.extend(pattern.find_iter(text).map(|m| m.range()));
        }
        found.extend(
            self.hex
                .find_iter(text)
                .filter(|m| hex_hash(m.as_str()))
                .map(|m| m.range()),
        );
        found.retain(|span| !span.is_empty());
        found
    }
}

/// `span`, the path `path`, without a period that ends a sentence
/// (`see /etc/hosts.`); `..` and `/.` stay.
fn without_final_dot(path: &str, span: Range<usize>) -> Range<usize> {
    if path.ends_with('.') && !path.ends_with("..") && !path.ends_with("/.") {
        span.start..span.end - 1
    } else {
        span
    }
}

/// The IPv6 address in `span` of `text`, a run of hex digits and colons,
/// without a colon that ends a sentence. The run must stand on its own,
/// so that `std::fs` (whose `d::f` would parse) is not an address.
fn ipv6(text: &str, mut span: Range<usize>) -> Option<Range<usize>> {
    let word = |c: char| c.is_alphanumeric() || c == '_' || c == '.';
    // A period after it may end a sentence.
    let mut after = text[span.end..].chars();
    let followed = match after.next() {
        Some('.') => after.next().is_some_and(word),
        next => next.is_some_and(word),
    };
    let alone = !text[..span.start].chars().next_back().is_some_and(word) && !followed;
    if !alone {
        return None;
    }
    if text[span.clone()].ends_with(':') && !text[span.clone()].ends_with("::") {
        span.end -= 1;
    }
    let candidate = &text[span.clone()];
    let valid = candidate.matches(':').count() >= 2
        && candidate.chars().any(|c| c.is_ascii_hexdigit())
        && candidate.parse::<Ipv6Addr>().is_ok();
    valid.then_some(span)
}

/// Whether `text`, all hexadecimal, looks like a hash rather than a word
/// (`deadbeef`) or a number: it has both letters and digits.
fn hex_hash(text: &str) -> bool {
    text.bytes().any(|b| b.is_ascii_digit()) && text.bytes().any(|b| b.is_ascii_alphabetic())
}

/// Something on the screen a hint can pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// First and last cell, inclusive.
    pub start: Point,
    pub end: Point,
    /// What copying or inserting it gives: the link's target, or the text.
    pub text: String,
    /// The link, for targets that can be opened.
    pub link: Option<Link>,
    /// The text on the screen, to notice when output changes it.
    shown: String,
}

/// The targets on the visible part of `screen`, in reading order: links
/// (as Ctrl+click finds them, with `resolve_link`), text that `patterns`
/// match, and the names of files that `is_file` says exist. Where targets
/// overlap, the one first in that list wins.
pub fn find(
    screen: &Screen,
    detector: &Detector,
    patterns: &Patterns,
    resolve_link: impl Fn(&str) -> Option<String>,
    is_file: impl Fn(&str) -> bool,
) -> Vec<Target> {
    let top = -(screen.display_offset() as isize);
    let bottom = top + screen.rows() as isize - 1;
    let mut targets: Vec<Target> = Vec::new();
    let mut add = |start: Point, end: Point, text: String, link: Option<Link>| {
        let overlaps = targets.iter().any(|t| t.start <= end && start <= t.end);
        if !overlaps && (top..=bottom).contains(&start.line) {
            let shown = shown(screen, start, end);
            targets.push(Target {
                start,
                end,
                text,
                link,
                shown,
            });
        }
    };
    let mut line = top;
    while line <= bottom {
        let Some(text) = LineText::around(screen, Point { line, col: 0 }, LINK_REACH) else {
            break;
        };
        for link in detector.links_in(screen, &text, &resolve_link) {
            add(link.start, link.end, link.target.clone(), Some(link));
        }
        for span in patterns.matches(&text.text) {
            let (start, end) = (text.point_at(span.start), text.point_before(span.end));
            add(start, end, text.text[span].to_string(), None);
        }
        line = text.last + 1;
    }
    let names = links::paths_in(
        screen,
        Point { line: top, col: 0 },
        Point {
            line: bottom,
            col: usize::MAX,
        },
        MAX_FILES,
        |name| {
            // `/`, `.` and `~/` exist, but are punctuation in text (`4 / 5`).
            if !name.chars().any(char::is_alphanumeric) {
                return None;
            }
            if is_file(name) {
                return Some(name.to_string());
            }
            // A file and a line number, without the colon after them.
            let (path, _) = split_line(name)?;
            is_file(path).then(|| name.trim_end_matches(':').to_string())
        },
    );
    for name in names {
        add(name.start, name.end, name.value, None);
    }
    targets.sort_by_key(|t| t.start);
    targets
}

/// The path and the line number of a path followed by one, and maybe a
/// column, as compilers and grep print them: `src/main.rs:12:5`, `x.c:12:`.
#[must_use]
pub fn split_line(text: &str) -> Option<(&str, u32)> {
    let number = |s: &str| {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse::<u32>().ok()
    };
    let text = text.strip_suffix(':').unwrap_or(text);
    let (rest, last) = text.rsplit_once(':')?;
    let last = number(last)?;
    // The last number was the column.
    if let Some((path, line)) = rest.rsplit_once(':')
        && let Some(line) = number(line)
        && !path.is_empty()
    {
        return Some((path, line));
    }
    (!rest.is_empty()).then_some((rest, last))
}

/// The text of `start..=end` on `screen`.
fn shown(screen: &Screen, start: Point, end: Point) -> String {
    Selection::span(start, end, Unit::Fixed).text(screen)
}

/// Why `alphabet` can't make labels. Labels are typed by the key's
/// unshifted letter in a Latin layout, which only `a` to `z` have in every
/// layout (digits need Shift in AZERTY, punctuation turns into letters in
/// Cyrillic ones), and only an alphabet of two characters or more makes
/// labels that grow.
pub fn check_alphabet(alphabet: &str) -> Result<(), String> {
    if let Some(c) = alphabet.chars().find(|c| !c.is_ascii_lowercase()) {
        return Err(format!("{c:?} can't be typed; use lowercase letters"));
    }
    if let Some((_, c)) = alphabet
        .char_indices()
        .find(|&(i, c)| alphabet[..i].contains(c))
    {
        return Err(format!("{c:?} is repeated"));
    }
    if alphabet.len() < 2 {
        return Err("needs at least two characters".into());
    }
    Ok(())
}

/// `n` labels made of `alphabet`, none the start of another, shortest
/// first. `alphabet` passed [`check_alphabet`].
fn labels(n: usize, alphabet: &str) -> Vec<String> {
    let alphabet: Vec<char> = alphabet.chars().collect();
    let mut labels: Vec<String> = alphabet.iter().map(char::to_string).collect();
    // Until there are enough, the last of the shortest labels (the hardest
    // to type) becomes the prefix of longer ones. The labels stay sorted
    // by length.
    while labels.len() < n {
        let shortest = labels[0].len();
        let last = labels
            .iter()
            .rposition(|l| l.len() == shortest)
            .expect("the labels start with one of the shortest");
        let prefix = labels.remove(last);
        labels.extend(alphabet.iter().map(|c| format!("{prefix}{c}")));
    }
    labels.truncate(n);
    labels
}

/// A target with its label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub label: String,
    pub target: Target,
}

/// Hints shown over the screen: what has been typed of a label, and the
/// target picked.
#[derive(Debug)]
pub struct Hints {
    hints: Vec<Hint>,
    typed: String,
    chosen: Option<String>,
}

impl Hints {
    /// Labels `targets` with characters of `alphabet`, the shortest labels
    /// going to the last ones (the newest output), or `None` without
    /// targets.
    #[must_use]
    pub fn new(targets: Vec<Target>, alphabet: &str) -> Option<Hints> {
        if targets.is_empty() {
            return None;
        }
        let mut labels = labels(targets.len(), alphabet);
        labels.reverse();
        let hints = targets
            .into_iter()
            .zip(labels)
            .map(|(target, label)| Hint { label, target })
            .collect();
        Some(Hints {
            hints,
            typed: String::new(),
            chosen: None,
        })
    }

    /// Adds `c` to the label being typed. Completing a label picks its
    /// target (`true`); starting a longer one drops the pick, so that
    /// nothing acts on a target whose label isn't shown. A character that
    /// no label continues with is ignored.
    pub fn type_char(&mut self, c: char) -> bool {
        let typed = format!("{}{c}", self.typed);
        if !self.hints.iter().any(|h| h.label.starts_with(&typed)) {
            return false;
        }
        if self.hints.iter().any(|h| h.label == typed) {
            self.chosen = Some(typed);
            self.typed.clear();
            return true;
        }
        self.typed = typed;
        self.chosen = None;
        false
    }

    /// Takes back the last character typed of a label, or else the pick.
    pub fn backspace(&mut self) {
        if self.typed.pop().is_none() {
            self.chosen = None;
        }
    }

    /// The picked target.
    #[must_use]
    pub fn chosen(&self) -> Option<&Target> {
        let chosen = self.chosen.as_ref()?;
        self.hints
            .iter()
            .find(|h| &h.label == chosen)
            .map(|h| &h.target)
    }

    /// The hints to draw, with how many characters of each were typed:
    /// those whose label starts with what was typed.
    pub fn visible(&self) -> impl Iterator<Item = (&Hint, usize)> {
        self.hints
            .iter()
            .filter(|h| h.label.starts_with(&self.typed))
            .map(|h| (h, self.typed.len()))
    }

    /// Follows output that scrolled `scrolled` rows into the history:
    /// targets move with their text, and those whose text changed or left
    /// the history are dropped. Returns whether any are left.
    pub fn follow_output(&mut self, screen: &Screen, scrolled: isize) -> bool {
        self.hints.retain_mut(|h| {
            let t = &mut h.target;
            t.start.line -= scrolled;
            t.end.line -= scrolled;
            if let Some(link) = &mut t.link {
                link.start.line -= scrolled;
                link.end.line -= scrolled;
            }
            screen.row_at(t.start.line).is_some() && shown(screen, t.start, t.end) == t.shown
        });
        if self.chosen().is_none() {
            self.chosen = None;
        }
        // Typed towards labels that are all gone: no label would show.
        if !self.hints.iter().any(|h| h.label.starts_with(&self.typed)) {
            self.typed.clear();
        }
        !self.hints.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Terminal;

    fn p(line: isize, col: usize) -> Point {
        Point { line, col }
    }

    fn patterns() -> Patterns {
        Patterns::try_new(&[]).unwrap()
    }

    fn texts(t: &Terminal, is_file: impl Fn(&str) -> bool) -> Vec<String> {
        let d = Detector::try_new(&[], &[]).unwrap();
        find(t.screen(), &d, &patterns(), |_| None, is_file)
            .into_iter()
            .map(|t| t.text)
            .collect()
    }

    /// The matches in `text`, overlaps left to the first, as [`find`] does.
    fn matched(text: &str) -> Vec<&str> {
        let mut kept: Vec<Range<usize>> = Vec::new();
        for span in patterns().matches(text) {
            if !kept
                .iter()
                .any(|k| k.start < span.end && span.start < k.end)
            {
                kept.push(span);
            }
        }
        kept.sort_by_key(|r| r.start);
        kept.into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn labels_are_shortest_first_and_none_starts_another() {
        assert_eq!(labels(3, DEFAULT_ALPHABET), ["a", "s", "d"]);
        let many = labels(100, DEFAULT_ALPHABET);
        assert_eq!(many.len(), 100);
        assert!(many.windows(2).all(|w| w[0].len() <= w[1].len()));
        for a in &many {
            for b in &many {
                assert!(a == b || !b.starts_with(a.as_str()), "{a} starts {b}");
            }
        }
        // With more targets than letters, some letters become prefixes.
        assert!(many.iter().any(|l| l.len() == 1));
        assert!(many.iter().all(|l| l.len() <= 2));
    }

    #[test]
    fn labels_use_the_given_alphabet() {
        assert_eq!(labels(4, "jkl"), ["j", "k", "lj", "lk"]);
    }

    #[test]
    fn finds_addresses() {
        assert_eq!(
            matched("ip 10.0.0.1, net 192.168.1.0/24 and 127.0.0.1:8080 not 1.2.3.999"),
            ["10.0.0.1", "192.168.1.0/24", "127.0.0.1:8080"]
        );
        assert_eq!(
            matched("via fe80::1ff:fe23:4567:890a and ::1: done"),
            ["fe80::1ff:fe23:4567:890a", "::1"]
        );
        assert_eq!(matched("ping ::1."), ["::1"]);
        // Rust paths and times aren't addresses.
        assert_eq!(matched("std::fs at 12:30:45"), Vec::<&str>::new());
    }

    #[test]
    fn finds_hashes() {
        assert_eq!(
            matched("commit 1af659b and deadbeef 1234567 3f14d33e"),
            ["1af659b", "3f14d33e"]
        );
        // All uppercase too, but not mixed case, which is more likely a word.
        assert_eq!(
            matched("sum 3F14D33E9A and DEADBEEF 3f14D33e"),
            ["3F14D33E9A"]
        );
        let sri = "sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=";
        assert_eq!(matched(&format!("hash = \"{sri}\";")), [sri]);
        let sha512 = format!("sha512-{}==", "A".repeat(86));
        assert_eq!(matched(&sha512), [sha512.as_str()]);
        // A character too few or too many, or another algorithm, and it's
        // not a hash, nor a path for its `/`.
        for broken in [
            "sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuF=",
            "sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFUx=",
            "sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU==",
            "sha25-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=",
        ] {
            assert_eq!(matched(broken), Vec::<&str>::new(), "{broken}");
        }
        let nix32 = "0ijcq3g2pxdixh6k6vkfwvx7xjyjnbgvx3nmrh0gmfdmlz7y7rlp";
        assert_eq!(matched(&format!("got: {nix32}")), [nix32]);
        let uuid = "123e4567-E89B-12d3-a456-426614174000";
        assert_eq!(matched(&format!("id {uuid}")), [uuid]);
    }

    #[test]
    fn finds_paths_from_the_root_home_or_current_directory() {
        assert_eq!(
            matched("/etc/x:120:5 ~/.config/jotty ./run ../lib --prefix=/usr/local"),
            [
                "/etc/x:120:5",
                "~/.config/jotty",
                "./run",
                "../lib",
                "/usr/local"
            ]
        );
        // A line number, but not a final colon or a number in a word.
        assert_eq!(
            matched("/etc/x:120: oops ./a.rs:7. /b:12ab"),
            ["/etc/x:120", "./a.rs:7", "/b"]
        );
        assert_eq!(
            matched("see /etc/hosts. or ../lib/.."),
            ["/etc/hosts", "../lib/.."]
        );
    }

    #[test]
    fn configured_patterns_come_first() {
        let p = Patterns::try_new(&[r"JOT-\d+".into()]).unwrap();
        assert_eq!(p.matches("JOT-12 abc1234"), [0..6, 7..14]);
        // Empty matches aren't targets.
        let p = Patterns::try_new(&[r"\d*".into()]).unwrap();
        assert_eq!(p.matches("ab 12"), vec![3..5]);
        assert!(Patterns::try_new(&["(".into()]).is_err());
    }

    #[test]
    fn links_win_over_text_and_files() {
        let mut t = Terminal::new(80, 2, 0);
        t.feed(b"https://x.org/a/b 1af659b Cargo.toml\r\n./src/a.rs");
        let found = texts(&t, |name| name == "Cargo.toml");
        assert_eq!(
            found,
            ["https://x.org/a/b", "1af659b", "Cargo.toml", "./src/a.rs"]
        );
    }

    #[test]
    fn names_of_files_may_have_a_line_number() {
        let mut t = Terminal::new(60, 1, 0);
        t.feed(b"src/a.rs:12:5: warning src/b.rs:3: src/c.rs:4");
        let found = texts(&t, |name| ["src/a.rs", "src/b.rs"].contains(&name));
        assert_eq!(found, ["src/a.rs:12:5", "src/b.rs:3"]);
    }

    #[test]
    fn splits_off_line_numbers() {
        assert_eq!(split_line("src/a.rs:12:5"), Some(("src/a.rs", 12)));
        assert_eq!(split_line("src/a.rs:12:"), Some(("src/a.rs", 12)));
        assert_eq!(split_line("/etc/x:120"), Some(("/etc/x", 120)));
        assert_eq!(split_line("a:b:12"), Some(("a:b", 12)));
        for none in [
            "src/a.rs",
            "a.rs:",
            "a.rs:x",
            "a.rs:+1",
            ":12",
            "a.rs:99999999999",
        ] {
            assert_eq!(split_line(none), None, "{none}");
        }
    }

    #[test]
    fn names_without_letters_or_digits_are_not_files() {
        let mut t = Terminal::new(40, 1, 0);
        t.feed(b"4 / 5 . .. ~/ a");
        // As if every name existed, as `/` and `.` do.
        assert_eq!(texts(&t, |_| true), ["4", "5", "a"]);
    }

    #[test]
    fn only_paths_that_start_a_word_are_paths() {
        assert_eq!(
            matched("src/app.rs and/or 1/2 2024/11/16 /2 </div> x+/abc"),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn only_the_visible_screen_is_hinted() {
        let mut t = Terminal::new(20, 2, 100);
        t.feed(b"aaa1111\r\nbbb2222\r\nccc3333\r\nddd4444");
        assert_eq!(texts(&t, |_| false), ["ccc3333", "ddd4444"]);
        t.screen_mut().scroll_display(1);
        assert_eq!(texts(&t, |_| false), ["bbb2222", "ccc3333"]);
    }

    fn target(text: &str, line: isize) -> Target {
        Target {
            start: p(line, 0),
            end: p(line, text.len() - 1),
            text: text.into(),
            link: None,
            shown: text.into(),
        }
    }

    #[test]
    fn typing_a_label_picks_its_target() {
        let targets: Vec<Target> = (0..30).map(|i| target("x", i)).collect();
        let mut hints = Hints::new(targets, DEFAULT_ALPHABET).unwrap();
        // The newest target gets the first label.
        let last = hints.visible().find(|(h, _)| h.label == "a").unwrap().0;
        assert_eq!(last.target.start.line, 29);
        let two = hints
            .visible()
            .find(|(h, _)| h.label.len() == 2)
            .unwrap()
            .0
            .label
            .clone();
        let mut chars = two.chars();
        let (first, second) = (chars.next().unwrap(), chars.next().unwrap());
        assert!(!hints.type_char(first));
        assert!(
            hints
                .visible()
                .all(|(h, typed)| h.label.starts_with(first) && typed == 1)
        );
        // Nothing continues with a character outside the alphabet.
        assert!(!hints.type_char('1'));
        assert!(hints.type_char(second));
        assert_eq!(
            hints.chosen().unwrap(),
            &hints.hints.iter().find(|h| h.label == two).unwrap().target
        );
        // Typing starts over after a pick, so another label can be picked.
        assert_eq!(hints.visible().count(), 30);
        assert!(hints.type_char('a'));
        assert_eq!(hints.chosen().unwrap().start.line, 29);
    }

    #[test]
    fn backspace_takes_back_a_character() {
        let targets: Vec<Target> = (0..30).map(|i| target("x", i)).collect();
        let mut hints = Hints::new(targets, DEFAULT_ALPHABET).unwrap();
        let prefix = hints
            .visible()
            .find(|(h, _)| h.label.len() == 2)
            .unwrap()
            .0
            .label
            .chars()
            .next()
            .unwrap();
        assert!(hints.type_char('a'));
        // Starting another label drops the pick.
        hints.type_char(prefix);
        assert!(hints.chosen().is_none());
        hints.backspace();
        assert_eq!(hints.visible().count(), 30);
        assert!(hints.type_char('a'));
        // With nothing typed, Backspace takes back the pick.
        hints.backspace();
        assert!(hints.chosen().is_none());
    }

    #[test]
    fn no_targets_means_no_hints() {
        assert!(Hints::new(Vec::new(), DEFAULT_ALPHABET).is_none());
    }

    #[test]
    fn hints_follow_their_text_and_drop_what_changed() {
        let mut t = Terminal::new(20, 3, 100);
        t.feed(b"aaa1111\r\nbbb2222");
        let d = Detector::try_new(&[], &[]).unwrap();
        let targets = find(t.screen(), &d, &patterns(), |_| None, |_| false);
        let mut hints = Hints::new(targets, DEFAULT_ALPHABET).unwrap();
        // The newest target has the first label.
        assert!(hints.type_char('a'));
        assert_eq!(hints.chosen().unwrap().text, "bbb2222");
        // A row scrolls into the history; the text moves up with it.
        let before = t.screen().grid().scrolled;
        t.feed(b"\r\nx\r\ny");
        let scrolled = t.screen().grid().scrolled.wrapping_sub(before) as isize;
        assert_eq!(scrolled, 1);
        assert!(hints.follow_output(t.screen(), scrolled));
        assert_eq!(hints.chosen().unwrap().start, p(0, 0));
        // Rewriting a target's text drops it, and the pick with it.
        t.feed(b"\x1b[2A\x1b[2K");
        assert!(hints.follow_output(t.screen(), 0));
        assert_eq!(hints.visible().count(), 1);
        assert!(hints.chosen().is_none());
    }

    #[test]
    fn labels_show_again_when_those_being_typed_are_dropped() {
        let mut t = Terminal::new(5, 30, 0);
        t.feed(b"x");
        for _ in 1..30 {
            t.feed(b"\r\nx");
        }
        let targets: Vec<Target> = (0..30).map(|i| target("x", i)).collect();
        let mut hints = Hints::new(targets, DEFAULT_ALPHABET).unwrap();
        let long: Vec<(isize, char)> = hints
            .visible()
            .filter(|(h, _)| h.label.len() == 2)
            .map(|(h, _)| (h.target.start.line, h.label.chars().next().unwrap()))
            .collect();
        assert!(!hints.type_char(long[0].1));
        assert_eq!(hints.visible().count(), long.len());
        for (line, _) in &long {
            t.feed(format!("\x1b[{};1H\x1b[2K", line + 1).as_bytes());
        }
        assert!(hints.follow_output(t.screen(), 0));
        assert_eq!(hints.visible().count(), 30 - long.len());
    }

    #[test]
    fn hints_end_when_no_target_is_left() {
        let mut t = Terminal::new(20, 2, 0);
        t.feed(b"aaa1111");
        let d = Detector::try_new(&[], &[]).unwrap();
        let targets = find(t.screen(), &d, &patterns(), |_| None, |_| false);
        let mut hints = Hints::new(targets, DEFAULT_ALPHABET).unwrap();
        t.feed(b"\x1b[2J");
        assert!(!hints.follow_output(t.screen(), 0));
    }
}
