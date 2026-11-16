//! What a drop into a window types: the files of a URI list as shell
//! words, for a command line.

use crate::notify::unescape;

/// The text a dropped `text/uri-list` pastes: each file's path (other URIs
/// as they are), quoted for a shell where needed, each followed by a space.
/// Control characters become `?`: a newline in a name must not type Enter
/// for a program that doesn't take bracketed pastes.
#[must_use]
pub fn uri_list_text(list: &[u8]) -> String {
    String::from_utf8_lossy(list)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|uri| {
            let word: String = file_path(uri)
                .unwrap_or_else(|| uri.to_string())
                .chars()
                .map(|c| if c.is_control() { '?' } else { c })
                .collect();
            shell_quote(&word) + " "
        })
        .collect()
}

/// The path a `file://` URI names. Its host, if it has one, is ignored:
/// the drop came from an application on this machine.
fn file_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    Some(String::from_utf8_lossy(&unescape(path).ok()?).into_owned())
}

/// `word` as one shell word: as it is when no shell gives any of its
/// characters a meaning, otherwise in single quotes, with `'` and `\\`
/// escaped outside them (fish takes `\\` as an escape inside).
fn shell_quote(word: &str) -> String {
    let plain = |c: char| c.is_alphanumeric() || "/._-+:@,".contains(c);
    if !word.is_empty() && word.chars().all(plain) {
        return word.to_string();
    }
    let mut quoted = String::from("'");
    for c in word.chars() {
        match c {
            '\'' => quoted.push_str(r"'\''"),
            '\\' => quoted.push_str(r"'\\'"),
            c => quoted.push(c),
        }
    }
    quoted.push('\'');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_become_quoted_paths() {
        let list = b"file:///home/u/Videos/Phim%20hay%20(2024).mkv\r\nfile:///tmp/a.txt\r\n";
        assert_eq!(
            uri_list_text(list),
            "'/home/u/Videos/Phim hay (2024).mkv' /tmp/a.txt "
        );
    }

    #[test]
    fn non_ascii_names_stay_plain() {
        assert_eq!(
            uri_list_text("file:///tmp/%C4%91%E1%BB%93.txt".as_bytes()),
            "/tmp/đồ.txt "
        );
    }

    #[test]
    fn single_quotes_are_closed_around() {
        assert_eq!(uri_list_text(b"file:///tmp/it's"), r"'/tmp/it'\''s' ");
    }

    #[test]
    fn backslashes_are_escaped_outside_the_quotes() {
        assert_eq!(
            uri_list_text(b"file:///tmp/a%5C%5Cb%5C"),
            r"'/tmp/a'\\''\\'b'\\'' "
        );
    }

    #[test]
    fn control_characters_are_replaced() {
        assert_eq!(
            uri_list_text(b"file:///tmp/x%0Arm%20-rf%20~%0A"),
            "'/tmp/x?rm -rf ~?' "
        );
    }

    #[test]
    fn a_host_is_ignored() {
        assert_eq!(uri_list_text(b"file://localhost/etc/hosts"), "/etc/hosts ");
        assert_eq!(uri_list_text(b"file://pc/etc/hosts"), "/etc/hosts ");
    }

    #[test]
    fn other_uris_are_kept() {
        assert_eq!(
            uri_list_text(b"https://example.com/?a=1&b=2\n"),
            "'https://example.com/?a=1&b=2' "
        );
        // Not an escape, so not a path: the URI is kept as it was.
        assert_eq!(uri_list_text(b"file:///tmp/100%"), "'file:///tmp/100%' ");
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        assert_eq!(
            uri_list_text(b"# from a file manager\r\n\r\nfile:///a\r\n"),
            "/a "
        );
        assert_eq!(uri_list_text(b""), "");
    }

    #[test]
    fn shell_characters_are_quoted() {
        for word in ["~/x", "=ls", "a b", "$HOME", "*", "a;b", "%self", ""] {
            assert!(shell_quote(word).starts_with('\''), "{word}");
        }
        assert_eq!(
            shell_quote("/usr/lib/x86_64-linux-gnu/a+b@c,d:e"),
            "/usr/lib/x86_64-linux-gnu/a+b@c,d:e"
        );
    }
}
