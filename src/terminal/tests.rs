use super::*;

fn term(cols: usize, rows: usize) -> Terminal {
    Terminal::new(cols, rows, 100)
}

fn lines(t: &Terminal) -> Vec<String> {
    t.screen().grid().rows.iter().map(Row::text).collect()
}

fn cell(t: &Terminal, row: usize, col: usize) -> Cell {
    t.screen().grid().rows[row].cells[col]
}

#[test]
fn prints_text_and_moves_cursor() {
    let mut t = term(10, 3);
    t.feed(b"hello");
    assert_eq!(lines(&t), ["hello", "", ""]);
    assert_eq!(t.screen().cursor(), (0, 5));
}

#[test]
fn crlf_and_scrolling_into_history() {
    let mut t = term(10, 2);
    t.feed(b"one\r\ntwo\r\nthree");
    assert_eq!(lines(&t), ["two", "three"]);
    let history: Vec<_> = t.screen().grid().history.iter().map(Row::text).collect();
    assert_eq!(history, ["one"]);
}

#[test]
fn autowrap_marks_row_wrapped() {
    let mut t = term(4, 3);
    t.feed(b"abcdef");
    assert_eq!(lines(&t), ["abcd", "ef", ""]);
    assert!(t.screen().grid().rows[0].wrapped);
    assert_eq!(t.screen().cursor(), (1, 2));
}

#[test]
fn pending_wrap_holds_cursor_on_last_column() {
    let mut t = term(4, 2);
    t.feed(b"abcd");
    assert_eq!(t.screen().cursor(), (0, 3));
    t.feed(b"\r");
    assert_eq!(t.screen().cursor(), (0, 0));
    assert_eq!(lines(&t), ["abcd", ""]);
}

#[test]
fn ascii_runs_wrap_across_rows_and_scroll() {
    let mut t = term(4, 2);
    t.feed(b"abcdefghij");
    assert_eq!(lines(&t), ["efgh", "ij"]);
    assert!(t.screen().grid().rows[0].wrapped);
    assert_eq!(t.screen().cursor(), (1, 2));
    // A run that exactly fills the row leaves a pending wrap.
    t.feed(b"\r\nwxyz");
    assert_eq!(lines(&t), ["ij", "wxyz"]);
    assert_eq!(t.screen().cursor(), (1, 3));
    t.feed(b"!");
    assert_eq!(lines(&t), ["wxyz", "!"]);
}

#[test]
fn queued_ascii_is_written_before_the_next_sequence() {
    let mut t = term(10, 2);
    // Across feeds, before a report, and before an attribute change.
    t.feed(b"ab");
    t.feed(b"c\x1b[6nd\x1b[1me");
    assert_eq!(t.screen_mut().take_responses(), b"\x1b[1;4R");
    assert_eq!(lines(&t), ["abcde", ""]);
    assert!(!cell(&t, 0, 3).flags.contains(Flags::BOLD));
    assert!(cell(&t, 0, 4).flags.contains(Flags::BOLD));
    // REP repeats the last character of a run.
    t.feed(b"xy\x1b[2b");
    assert_eq!(lines(&t), ["abcdexyyy", ""]);
}

#[test]
fn ascii_runs_over_wide_characters_split_them() {
    let mut t = term(6, 1);
    // The run starts on the spacer of one and ends on the first half of
    // the other; both lose their other half.
    t.feed("日本\x1b[2Gab".as_bytes());
    assert_eq!(lines(&t), [" ab"]);
    for col in 0..4 {
        let flags = cell(&t, 0, col).flags;
        assert!(
            !flags.intersects(Flags::WIDE | Flags::WIDE_SPACER),
            "column {col}"
        );
    }
}

#[test]
fn ascii_runs_in_one_column() {
    let mut t = term(1, 2);
    t.feed(b"abc");
    assert_eq!(lines(&t), ["b", "c"]);
    t.feed(b"\r\x1b[?7lxyz");
    assert_eq!(lines(&t), ["b", "z"]);
    assert_eq!(t.screen().cursor(), (1, 0));
}

/// Output arrives in pieces of any size; the grid mustn't depend on where
/// they split, whether in the middle of a sequence, a character or a run
/// of ASCII.
#[test]
fn output_split_anywhere_gives_the_same_screen() {
    let pieces: &[&str] = &[
        "abc",
        "0123456789",
        " ",
        "\r",
        "\n",
        "\x08",
        "\t",
        "\x1b[1m",
        "\x1b[0m",
        "\x1b[2;3H",
        "\x1b[?7l",
        "\x1b[?7h",
        "\x1b[4h",
        "\x1b[4l",
        "\x1b(0",
        "\x1b(B",
        "\x0e",
        "\x0f",
        "\x1b[3b",
        "\x1b[2;3r",
        "\x1b[3@",
        "\x1b[2P",
        "\x1b[S",
        "\x1b[6n",
        "\x1b]0;t\x07",
        "日本",
        "é",
        "e\u{301}",
        "\u{200d}",
        "❄\u{fe0f}",
        "\u{600}",
        "\u{1f1fb}\u{1f1f3}",
    ];
    // A fixed xorshift sequence, so that failures reproduce.
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed as usize
    };
    for _ in 0..2000 {
        let (cols, rows) = (1 + next() % 8, 1 + next() % 4);
        let input: String = (0..next() % 24)
            .map(|_| pieces[next() % pieces.len()])
            .collect();
        let mut whole = Terminal::new(cols, rows, 3);
        whole.feed(input.as_bytes());
        let mut bytewise = Terminal::new(cols, rows, 3);
        for byte in input.bytes() {
            bytewise.feed(&[byte]);
        }
        let (a, b) = (whole.screen(), bytewise.screen());
        assert_eq!(a.grid().rows, b.grid().rows, "{cols}x{rows} {input:?}");
        assert_eq!(
            a.grid().history,
            b.grid().history,
            "{cols}x{rows} {input:?}"
        );
        assert_eq!(a.cursor(), b.cursor(), "{cols}x{rows} {input:?}");
    }
}

#[test]
fn autowrap_disabled_overwrites_last_column() {
    let mut t = term(4, 2);
    t.feed(b"\x1b[?7labcdef");
    assert_eq!(lines(&t), ["abcf", ""]);
}

#[test]
fn wide_characters_take_two_cells() {
    let mut t = term(6, 2);
    t.feed("a中b".as_bytes());
    assert_eq!(lines(&t), ["a中b", ""]);
    assert!(cell(&t, 0, 1).flags.contains(Flags::WIDE));
    assert!(cell(&t, 0, 2).flags.contains(Flags::WIDE_SPACER));
    assert_eq!(t.screen().cursor(), (0, 4));
}

#[test]
fn wide_character_wraps_at_line_end() {
    let mut t = term(3, 2);
    t.feed("ab中".as_bytes());
    assert_eq!(lines(&t), ["ab", "中"]);
}

#[test]
fn overwriting_half_of_wide_char_clears_other_half() {
    let mut t = term(6, 1);
    t.feed("中\x1b[2Gx".as_bytes());
    assert_eq!(cell(&t, 0, 0).c, ' ');
    assert_eq!(cell(&t, 0, 1).c, 'x');
    assert!(!cell(&t, 0, 0).flags.contains(Flags::WIDE));
}

#[test]
fn ascii_over_the_left_half_of_wide_char_clears_the_right_half() {
    let mut t = term(6, 1);
    t.feed("中\r\x1b[1mx\x7fy".as_bytes());
    assert_eq!(lines(&t), ["xy"]);
    assert!(cell(&t, 0, 0).flags.contains(Flags::BOLD));
    assert!(!cell(&t, 0, 0).flags.contains(Flags::WIDE));
    assert!(!cell(&t, 0, 1).flags.contains(Flags::WIDE_SPACER));
    assert_eq!(t.screen().cursor(), (0, 2));
}

#[test]
fn zero_width_characters_are_dropped() {
    let mut t = term(6, 1);
    t.feed("e\u{301}x".as_bytes());
    assert_eq!(lines(&t), ["ex"]);
}

#[test]
fn cursor_movement() {
    let mut t = term(10, 5);
    t.feed(b"\x1b[3;4H");
    assert_eq!(t.screen().cursor(), (2, 3));
    t.feed(b"\x1b[2A");
    assert_eq!(t.screen().cursor(), (0, 3));
    t.feed(b"\x1b[10B");
    assert_eq!(t.screen().cursor(), (4, 3));
    t.feed(b"\x1b[2C\x1b[D");
    assert_eq!(t.screen().cursor(), (4, 4));
    t.feed(b"\x1b[7G");
    assert_eq!(t.screen().cursor(), (4, 6));
    t.feed(b"\x1b[2d");
    assert_eq!(t.screen().cursor(), (1, 6));
    t.feed(b"\x1b[H");
    assert_eq!(t.screen().cursor(), (0, 0));
}

#[test]
fn backspace_and_tab() {
    let mut t = term(20, 1);
    t.feed(b"ab\x08c\tX");
    assert_eq!(lines(&t), ["ac      X"]);
}

#[test]
fn erase_in_line_and_display() {
    let mut t = term(5, 3);
    t.feed(b"aaaaa\r\nbbbbb\r\nccccc");
    t.feed(b"\x1b[2;3H\x1b[K");
    assert_eq!(lines(&t), ["aaaaa", "bb", "ccccc"]);
    t.feed(b"\x1b[1K");
    assert_eq!(lines(&t), ["aaaaa", "", "ccccc"]);
    t.feed(b"\x1b[1;3H\x1b[J");
    assert_eq!(lines(&t), ["aa", "", ""]);
    t.feed(b"\x1b[2J");
    assert_eq!(lines(&t), ["", "", ""]);
}

#[test]
fn erase_uses_background_color() {
    let mut t = term(3, 1);
    t.feed(b"\x1b[41m\x1b[2K");
    assert_eq!(cell(&t, 0, 2).bg, Color::Indexed(1));
}

#[test]
fn clear_scrollback() {
    let mut t = term(3, 1);
    t.feed(b"a\r\nb\r\nc");
    assert_eq!(t.screen().grid().history.len(), 2);
    t.feed(b"\x1b[3J");
    assert!(t.screen().grid().history.is_empty());
}

#[test]
fn insert_and_delete_characters() {
    let mut t = term(6, 1);
    t.feed(b"abcdef\x1b[1;3H\x1b[2@");
    assert_eq!(lines(&t), ["ab  cd"]);
    t.feed(b"\x1b[3P");
    assert_eq!(lines(&t), ["abd"]);
    t.feed(b"\x1b[1;1H\x1b[2X");
    assert_eq!(lines(&t), ["  d"]);
}

#[test]
fn insert_mode() {
    let mut t = term(6, 1);
    t.feed(b"abc\x1b[1;2H\x1b[4hX\x1b[4lY");
    assert_eq!(lines(&t), ["aXYc"]);
}

#[test]
fn insert_and_delete_lines() {
    let mut t = term(3, 4);
    t.feed(b"a\r\nb\r\nc\r\nd\x1b[2;1H\x1b[L");
    assert_eq!(lines(&t), ["a", "", "b", "c"]);
    t.feed(b"\x1b[2M");
    assert_eq!(lines(&t), ["a", "c", "", ""]);
    // Deleting lines never feeds the history.
    assert!(t.screen().grid().history.is_empty());
}

#[test]
fn scroll_region() {
    let mut t = term(3, 4);
    t.feed(b"a\r\nb\r\nc\r\nd");
    t.feed(b"\x1b[2;3r");
    assert_eq!(t.screen().cursor(), (0, 0));
    t.feed(b"\x1b[3;1H\nX");
    assert_eq!(lines(&t), ["a", "c", "X", "d"]);
    assert!(t.screen().grid().history.is_empty());
    t.feed(b"\x1b[2;1H\x1bMY");
    assert_eq!(lines(&t), ["a", "Y", "c", "d"]);
}

#[test]
fn scroll_up_down_commands() {
    let mut t = term(3, 3);
    t.feed(b"a\r\nb\r\nc\x1b[S");
    assert_eq!(lines(&t), ["b", "c", ""]);
    t.feed(b"\x1b[2T");
    assert_eq!(lines(&t), ["", "", "b"]);
}

#[test]
fn origin_mode_is_relative_to_region() {
    let mut t = term(5, 5);
    t.feed(b"\x1b[2;4r\x1b[?6h\x1b[1;1HX\x1b[10;1HY");
    assert_eq!(lines(&t), ["", "X", "", "Y", ""]);
}

#[test]
fn sgr_attributes() {
    let mut t = term(10, 1);
    t.feed(b"\x1b[1;3;4;31;42ma\x1b[22;39mb\x1b[0mc");
    let a = cell(&t, 0, 0);
    assert!(a.flags.contains(Flags::BOLD | Flags::ITALIC));
    assert_eq!(a.fg, Color::Indexed(1));
    assert_eq!(a.bg, Color::Indexed(2));
    let b = cell(&t, 0, 1);
    assert!(!b.flags.contains(Flags::BOLD));
    assert!(b.flags.contains(Flags::ITALIC));
    assert_eq!(b.fg, Color::Default);
    assert_eq!(
        cell(&t, 0, 2),
        Cell {
            c: 'c',
            ..Cell::default()
        }
    );
}

#[test]
fn sgr_extended_colors() {
    let mut t = term(10, 1);
    t.feed(b"\x1b[38;5;200;48;2;1;2;3ma");
    t.feed(b"\x1b[38:2::10:20:30;48:5:7mb");
    t.feed(b"\x1b[38:2:40:50:60;91mc");
    assert_eq!(cell(&t, 0, 0).fg, Color::Indexed(200));
    assert_eq!(cell(&t, 0, 0).bg, Color::Rgb(1, 2, 3));
    assert_eq!(cell(&t, 0, 1).fg, Color::Rgb(10, 20, 30));
    assert_eq!(cell(&t, 0, 1).bg, Color::Indexed(7));
    assert_eq!(cell(&t, 0, 2).fg, Color::Indexed(9));
}

#[test]
fn sgr_truncated_extended_color_is_ignored() {
    let mut t = term(10, 1);
    t.feed(b"\x1b[38;2;1ma");
    assert_eq!(cell(&t, 0, 0).fg, Color::Default);
}

#[test]
fn alternate_screen_preserves_primary() {
    let mut t = term(5, 2);
    t.feed(b"main\x1b[?1049h");
    assert!(t.screen().is_alt_screen());
    assert_eq!(lines(&t), ["", ""]);
    t.feed(b"\x1b[Halt");
    assert_eq!(lines(&t), ["alt", ""]);
    t.feed(b"\x1b[?1049l");
    assert!(!t.screen().is_alt_screen());
    assert_eq!(lines(&t), ["main", ""]);
    assert_eq!(t.screen().cursor(), (0, 4));
}

#[test]
fn alternate_screen_has_no_history() {
    let mut t = term(3, 1);
    t.feed(b"\x1b[?1049ha\r\nb\r\nc");
    assert!(t.screen().grid().history.is_empty());
}

#[test]
fn save_and_restore_cursor() {
    let mut t = term(10, 3);
    t.feed(b"\x1b[2;3H\x1b[31m\x1b7\x1b[H\x1b[0m\x1b8x");
    assert_eq!(cell(&t, 1, 2).fg, Color::Indexed(1));
}

#[test]
fn device_status_reports() {
    let mut t = term(10, 5);
    t.feed(b"\x1b[3;7H\x1b[6n\x1b[5n");
    assert_eq!(t.screen_mut().take_responses(), b"\x1b[3;7R\x1b[0n");
    assert!(t.screen_mut().take_responses().is_empty());
    t.feed(b"\x1b[c");
    assert!(t.screen_mut().take_responses().starts_with(b"\x1b[?"));
}

#[test]
fn private_modes() {
    let mut t = term(10, 1);
    t.feed(b"\x1b[?1h\x1b[?2004h\x1b[?25l\x1b[?1004h");
    let modes = t.screen().modes;
    assert!(modes.app_cursor);
    assert!(modes.bracketed_paste);
    assert!(!modes.show_cursor);
    assert!(modes.focus_events);
    t.feed(b"\x1b[?1;2004l\x1b[?25h");
    let modes = t.screen().modes;
    assert!(!modes.app_cursor && !modes.bracketed_paste && modes.show_cursor);
}

#[test]
fn cursor_shape() {
    let mut t = term(10, 1);
    t.feed(b"\x1b[6 q");
    assert_eq!(t.screen().cursor_shape, CursorShape::Beam);
    t.feed(b"\x1b[4 q");
    assert_eq!(t.screen().cursor_shape, CursorShape::Underline);
    t.feed(b"\x1b[0 q");
    assert_eq!(t.screen().cursor_shape, CursorShape::Block);
}

#[test]
fn title_is_reported_once() {
    let mut t = term(10, 1);
    t.feed(b"\x1b]2;hello;world\x07");
    assert_eq!(t.screen_mut().take_title().as_deref(), Some("hello;world"));
    assert_eq!(t.screen_mut().take_title(), None);
}

#[test]
fn line_drawing_charset() {
    let mut t = term(10, 1);
    t.feed(b"\x1b(0lqk\x1b(Bq");
    assert_eq!(lines(&t), ["┌─┐q"]);
    t.feed(b"\x1b)0\x0eq\x0fq");
    assert_eq!(lines(&t), ["┌─┐q─q"]);
}

#[test]
fn repeat_last_character() {
    let mut t = term(10, 1);
    t.feed(b"x\x1b[3b");
    assert_eq!(lines(&t), ["xxxx"]);
}

#[test]
fn repeat_is_limited_to_a_row() {
    let mut t = term(10, 3);
    t.feed(b"x\x1b[65535b");
    assert_eq!(lines(&t), ["xxxxxxxxxx", "x", ""]);
}

#[test]
fn bell() {
    let mut t = term(10, 1);
    t.feed(b"\x07");
    assert!(t.screen_mut().take_bell());
    assert!(!t.screen_mut().take_bell());
}

#[test]
fn full_reset() {
    let mut t = term(10, 2);
    t.feed(b"abc\x1b[?25l\x1b[31m\x1bc");
    assert_eq!(lines(&t), ["", ""]);
    assert!(t.screen().modes.show_cursor);
    assert_eq!(t.screen().cursor(), (0, 0));
}

#[test]
fn resize_keeps_cursor_line_visible() {
    let mut t = term(10, 4);
    t.feed(b"1\r\n2\r\n3\r\n4");
    t.screen_mut().resize(10, 2);
    assert_eq!(lines(&t), ["3", "4"]);
    assert_eq!(t.screen().cursor(), (1, 1));
    // Reflowing brings history back into the taller screen.
    t.screen_mut().resize(3, 3);
    assert_eq!(t.screen().cols(), 3);
    assert_eq!(lines(&t), ["2", "3", "4"]);
    assert_eq!(t.screen().cursor(), (2, 1));
    t.feed(b"abcd");
    assert_eq!(lines(&t), ["3", "4ab", "cd"]);
}

#[test]
fn scrollback_view() {
    let mut t = term(5, 2);
    t.feed(b"a\r\nb\r\nc\r\nd");
    let s = t.screen_mut();
    assert_eq!(s.visible_row(0).text(), "c");
    s.scroll_display(1);
    assert_eq!(s.visible_row(0).text(), "b");
    assert_eq!(s.visible_row(1).text(), "c");
    assert!(!s.cursor_visible());
    s.scroll_display(100);
    assert_eq!(s.display_offset(), 2);
    assert_eq!(s.visible_row(0).text(), "a");
    // New output keeps the view anchored.
    t.feed(b"\r\ne");
    assert_eq!(t.screen().display_offset(), 3);
    assert_eq!(t.screen().visible_row(0).text(), "a");
    t.screen_mut().reset_display();
    assert_eq!(t.screen().visible_row(1).text(), "e");
}

#[test]
fn prompt_marker_sets_input_start() {
    let mut t = term(20, 3);
    t.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls");
    assert_eq!(t.screen().input_start(), 2);
    t.feed(b"\r\n");
    assert_eq!(t.screen().input_start(), 0);
}

#[test]
fn prompt_marker_follows_scrolling() {
    let mut t = term(20, 2);
    t.feed(b"x\r\n$ \x1b]133;B\x07");
    assert_eq!(t.screen().input_start(), 2);
    // A long command wraps and scrolls the prompt row up.
    t.feed(&[b'y'; 20]);
    assert_eq!(t.screen().cursor().0, 1);
    assert_eq!(t.screen().input_start(), 0);
    t.feed(b"\x1b[A");
    assert_eq!(t.screen().input_start(), 2);
}

const PROMPT: &str = "\x1b]133;A\x07$ \x1b]133;B\x07";

/// Feeds a prompt, a command and its output as a shell with OSC 133 does.
fn run_command(t: &mut Terminal, command: &str, output: &str) {
    t.feed(format!("{PROMPT}{command}\r\n\x1b]133;C\x07{output}\x1b]133;D;0\x07").as_bytes());
}

#[test]
fn command_output_between_marks() {
    let mut t = term(20, 10);
    run_command(&mut t, "ls", "a\r\nb\r\n\r\n");
    run_command(&mut t, "true", "");
    t.feed(PROMPT.as_bytes());
    let s = t.screen();
    // Lines 1-2; the blank line 3 is left out.
    assert_eq!(s.command_output(1), Some((1, 2)));
    assert_eq!(s.command_output(2), Some((1, 2)));
    // The prompt and command line, and a command without output.
    assert_eq!(s.command_output(0), None);
    assert_eq!(s.command_output(3), None);
    assert_eq!(s.command_output(4), None);
    assert_eq!(s.command_output(5), None);
}

#[test]
fn command_output_while_running() {
    let mut t = term(20, 5);
    t.feed(format!("{PROMPT}cat\r\n\x1b]133;C\x07one\r\ntwo\r\n").as_bytes());
    assert_eq!(t.screen().command_output(2), Some((1, 2)));
    assert_eq!(t.screen().command_output(3), None);
}

#[test]
fn command_output_in_history() {
    let mut t = term(20, 3);
    run_command(&mut t, "seq 4", "1\r\n2\r\n3\r\n4\r\n");
    t.feed(PROMPT.as_bytes());
    // Lines -3..=-1 are history; the prompt is on line 2.
    assert_eq!(t.screen().command_output(-1), Some((-2, 1)));
    assert_eq!(t.screen().command_output(1), Some((-2, 1)));
}

#[test]
fn command_output_without_c_mark_follows_command_line() {
    let mut t = term(4, 10);
    // The command wraps onto a second row.
    t.feed(format!("{PROMPT}echo\r\nhi\r\n{PROMPT}").as_bytes());
    let s = t.screen();
    assert_eq!(s.command_output(1), None);
    assert_eq!(s.command_output(2), Some((2, 2)));
}

#[test]
fn command_output_after_c_mark_mid_row() {
    let mut t = term(20, 5);
    t.feed(format!("{PROMPT}ls\x1b]133;C\x07\r\nx\r\n{PROMPT}").as_bytes());
    assert_eq!(t.screen().command_output(0), None);
    assert_eq!(t.screen().command_output(1), Some((1, 1)));
}

#[test]
fn erasing_removes_marks() {
    let mut t = term(20, 5);
    run_command(&mut t, "ls", "a\r\n");
    t.feed(b"\x1b[2J");
    assert_eq!(t.screen().command_output(1), None);
}

#[test]
fn synchronized_output_mode() {
    let mut t = term(10, 2);
    t.feed(b"\x1b[?2026h");
    assert!(t.screen().modes.synchronized);
    t.feed(b"\x1b[?2026l");
    assert!(!t.screen().modes.synchronized);
}

#[test]
fn mode_reports() {
    let mut t = term(10, 2);
    t.feed(b"\x1b[?2026h\x1b[?2026$p\x1b[?25l\x1b[?25$p\x1b[?9999$p\x1b[4$p");
    assert_eq!(
        t.screen_mut().take_responses(),
        b"\x1b[?2026;1$y\x1b[?25;2$y\x1b[?9999;0$y\x1b[4;2$y"
    );
}

#[test]
fn narrowing_rewraps_lines() {
    let mut t = term(10, 4);
    t.feed(b"abcdefgh\r\nxy");
    t.screen_mut().resize(4, 4);
    assert_eq!(lines(&t), ["abcd", "efgh", "xy", ""]);
    assert!(t.screen().grid().rows[0].wrapped);
    assert!(!t.screen().grid().rows[1].wrapped);
    assert_eq!(t.screen().cursor(), (2, 2));
}

#[test]
fn widening_joins_wrapped_rows() {
    let mut t = term(4, 4);
    t.feed(b"abcdefgh\r\nxy");
    assert_eq!(lines(&t), ["abcd", "efgh", "xy", ""]);
    t.screen_mut().resize(10, 4);
    assert_eq!(lines(&t), ["abcdefgh", "xy", "", ""]);
    assert_eq!(t.screen().cursor(), (1, 2));
}

#[test]
fn reflow_moves_rows_through_history() {
    let mut t = term(6, 2);
    t.feed(b"aaaaaa\r\nbb\r\ncc");
    // "aaaaaa" is in history; narrowing wraps it to two rows.
    t.screen_mut().resize(3, 2);
    let history: Vec<_> = t.screen().grid().history.iter().map(Row::text).collect();
    assert_eq!(history, ["aaa", "aaa"]);
    assert_eq!(lines(&t), ["bb", "cc"]);
    t.screen_mut().resize(6, 2);
    let history: Vec<_> = t.screen().grid().history.iter().map(Row::text).collect();
    assert_eq!(history, ["aaaaaa"]);
}

#[test]
fn reflow_keeps_wide_characters_whole() {
    let mut t = term(6, 3);
    t.feed("ab中文".as_bytes());
    t.screen_mut().resize(3, 3);
    assert_eq!(lines(&t), ["ab", "中", "文"]);
    t.screen_mut().resize(6, 3);
    assert_eq!(lines(&t), ["ab中文", "", ""]);
}

#[test]
fn reflow_keeps_the_cursor_after_the_prompt() {
    let mut t = term(10, 3);
    t.feed(b"$ \x1b]133;B\x07");
    t.screen_mut().resize(5, 3);
    assert_eq!(t.screen().cursor(), (0, 2));
    assert_eq!(t.screen().input_start(), 2);
}

#[test]
fn reflow_carries_marks() {
    let mut t = term(4, 6);
    t.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07abcdef\r\n\x1b]133;A\x07");
    t.screen_mut().resize(8, 6);
    assert_eq!(lines(&t), ["$ ls", "abcdef", "", "", "", ""]);
    assert_eq!(t.screen().command_output(1), Some((1, 1)));
}

#[test]
fn hyperlinks_mark_cells() {
    let mut t = term(20, 1);
    t.feed(b"a\x1b]8;;https://x.org/a;b\x1b\\link\x1b[0m!\x1b]8;;\x1b\\b");
    let s = t.screen();
    let id = cell(&t, 0, 1).link;
    assert_ne!(id, 0);
    assert_eq!(s.link(id), Some("https://x.org/a;b"));
    // SGR reset keeps the link; OSC 8 with an empty URI ends it.
    assert_eq!(cell(&t, 0, 5).link, id);
    assert_eq!(cell(&t, 0, 0).link, 0);
    assert_eq!(cell(&t, 0, 6).link, 0);
}

#[test]
fn identical_hyperlinks_share_an_id() {
    let mut t = term(20, 1);
    t.feed(b"\x1b]8;id=1;file:///a\x07x\x1b]8;;\x07 \x1b]8;;file:///a\x07y");
    assert_eq!(cell(&t, 0, 0).link, cell(&t, 0, 2).link);
}

fn text_at(t: &Terminal, row: usize, col: usize) -> String {
    let mut text = String::new();
    t.screen().cell_text(&cell(t, row, col), &mut text);
    text
}

#[test]
fn combining_marks_join_the_previous_cell() {
    let mut t = term(10, 1);
    t.feed("e\u{301}x".as_bytes());
    assert_eq!(text_at(&t, 0, 0), "e\u{301}");
    assert_eq!(cell(&t, 0, 1).c, 'x');
    assert_eq!(t.screen().cursor(), (0, 2));
}

#[test]
fn ascii_after_a_cluster_starts_a_new_cell() {
    let mut t = term(10, 1);
    // The ASCII fast path must not skip a base cell that already has a tail.
    t.feed("e\u{301}\u{302}ab".as_bytes());
    assert_eq!(text_at(&t, 0, 0), "e\u{301}\u{302}");
    assert_eq!(text_at(&t, 0, 1), "a");
    assert_eq!(text_at(&t, 0, 2), "b");
    assert_eq!(t.screen().cursor(), (0, 3));
}

#[test]
fn a_cluster_at_the_last_column_without_autowrap_is_overwritten() {
    // The cursor stays on the last column, so a character that would join
    // the cluster there (U+0600 is a prepended mark: it joins whatever
    // follows) has nowhere to go. It overwrites the cell instead, as any
    // character does, rather than turning the cluster's own cell into a
    // continuation.
    let mut t = term(10, 1);
    t.feed("\x1b[?7lxxxxxxxxx\u{600}\u{600} ".as_bytes());
    assert_eq!(lines(&t), ["xxxxxxxxx"]);
    t.feed("\u{600}a".as_bytes());
    assert_eq!(text_at(&t, 0, 9), "a");
    assert!(!cell(&t, 0, 9).flags.contains(Flags::CONTINUATION));
}

#[test]
fn emoji_sequences_join_and_keep_their_width() {
    let mut t = term(20, 1);
    // A ZWJ family (2+0+2 columns) and a flag (1+1), as wcwidth counts.
    t.feed("👨\u{200d}👩🇻🇳!".as_bytes());
    assert_eq!(text_at(&t, 0, 0), "👨\u{200d}👩");
    assert!(cell(&t, 0, 2).flags.contains(Flags::CONTINUATION));
    assert_eq!(text_at(&t, 0, 2), "");
    assert_eq!(text_at(&t, 0, 4), "🇻🇳");
    assert!(cell(&t, 0, 5).flags.contains(Flags::CONTINUATION));
    assert_eq!(cell(&t, 0, 6).c, '!');
}

#[test]
fn clusters_end_at_controls_and_new_clusters() {
    let mut t = term(10, 2);
    // A second flag starts a new cluster; so does anything after a CR.
    t.feed("🇻🇳🇫🇷\r\n\u{301}a".as_bytes());
    assert_eq!(text_at(&t, 0, 0), "🇻🇳");
    assert_eq!(text_at(&t, 0, 2), "🇫🇷");
    assert_eq!(text_at(&t, 1, 0), "a");
}

#[test]
fn variation_selector_is_kept_in_the_cluster() {
    let mut t = term(10, 1);
    t.feed("❤\u{fe0f}x".as_bytes());
    assert_eq!(text_at(&t, 0, 0), "❤\u{fe0f}");
    assert!(cell(&t, 0, 0).flags.contains(Flags::WIDE));
    assert_eq!(cell(&t, 0, 2).c, 'x');
}

#[test]
fn split_input_across_feeds() {
    let mut t = term(10, 1);
    let bytes = "\x1b[31m中".as_bytes();
    for b in bytes {
        t.feed(std::slice::from_ref(b));
    }
    assert_eq!(cell(&t, 0, 0).c, '中');
    assert_eq!(cell(&t, 0, 0).fg, Color::Indexed(1));
}

#[test]
fn garbage_does_not_panic() {
    let mut t = term(7, 3);
    let mut seed = 0x1234_5678u32;
    let mut bytes = Vec::new();
    for _ in 0..20_000 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        // Bias towards escape-sequence building blocks.
        let pool = b"\x1b[;?0123456789HJKLMPX@ABCDrmhl\r\n\x08\t\x1bM(0";
        bytes.push(if seed.is_multiple_of(3) {
            (seed >> 8) as u8
        } else {
            pool[(seed >> 8) as usize % pool.len()]
        });
    }
    t.feed(&bytes);
    t.screen_mut().resize(3, 9);
    t.feed(&bytes);
    let (row, col) = t.screen().cursor();
    assert!(row < 9 && col < 3);
}

#[test]
fn emoji_presentation_selector_widens_character() {
    let mut t = term(10, 1);
    t.feed("\u{2744}\u{FE0F} x".as_bytes());
    assert!(cell(&t, 0, 0).flags.contains(Flags::WIDE));
    assert!(cell(&t, 0, 1).flags.contains(Flags::WIDE_SPACER));
    assert_eq!(cell(&t, 0, 3).c, 'x');
    assert_eq!(t.screen().cursor(), (0, 4));
}

#[test]
fn emoji_presentation_selector_is_ignored_where_it_does_not_apply() {
    // Not an emoji: the selector is dropped.
    let mut t = term(10, 1);
    t.feed("a\u{FE0F}b".as_bytes());
    assert_eq!(lines(&t), ["ab"]);
    assert!(!cell(&t, 0, 0).flags.contains(Flags::WIDE));

    // Already wide.
    let mut t = term(10, 1);
    t.feed("\u{1F600}\u{FE0F}x".as_bytes());
    assert_eq!(cell(&t, 0, 2).c, 'x');

    // No room in the last column.
    let mut t = term(3, 1);
    t.feed("ab\u{2744}\u{FE0F}".as_bytes());
    assert!(!cell(&t, 0, 2).flags.contains(Flags::WIDE));

    // Separated by an escape sequence.
    let mut t = term(10, 1);
    t.feed("\u{2744}\x1b[31m\u{FE0F}x".as_bytes());
    assert!(!cell(&t, 0, 0).flags.contains(Flags::WIDE));
    assert_eq!(cell(&t, 0, 1).c, 'x');
}

#[test]
fn emoji_presentation_at_second_to_last_column_sets_pending_wrap() {
    let mut t = term(4, 2);
    t.feed("ab\u{2744}\u{FE0F}c".as_bytes());
    assert!(cell(&t, 0, 2).flags.contains(Flags::WIDE));
    assert_eq!(lines(&t), ["ab\u{2744}", "c"]);
}

#[test]
fn osc_12_sets_and_resets_cursor_color() {
    let mut t = term(10, 1);
    assert_eq!(t.screen().cursor_color(), None);
    t.feed(b"\x1b]12;#ff8000\x07");
    assert_eq!(t.screen().cursor_color(), Some((255, 128, 0)));
    t.feed(b"\x1b]12;rgb:00/80/ff\x1b\\");
    assert_eq!(t.screen().cursor_color(), Some((0, 128, 255)));
    t.feed(b"\x1b]112\x07");
    assert_eq!(t.screen().cursor_color(), None);
    // Invalid specifications are ignored.
    t.feed(b"\x1b]12;not-a-color\x07");
    assert_eq!(t.screen().cursor_color(), None);
}

#[test]
fn osc_12_query_reports_current_color() {
    let mut t = term(10, 1);
    t.screen_mut().set_default_colors(DefaultColors {
        cursor: (0x12, 0x34, 0x56),
        ..DefaultColors::default()
    });
    t.feed(b"\x1b]12;?\x07");
    assert_eq!(
        t.screen_mut().take_responses(),
        b"\x1b]12;rgb:1212/3434/5656\x07"
    );
    t.feed(b"\x1b]12;#ffffff\x07\x1b]12;?\x1b\\");
    assert_eq!(
        t.screen_mut().take_responses(),
        b"\x1b]12;rgb:ffff/ffff/ffff\x1b\\"
    );
}

#[test]
fn full_reset_restores_cursor_color() {
    let mut t = term(10, 1);
    t.screen_mut().set_default_colors(DefaultColors {
        cursor: (1, 2, 3),
        ..DefaultColors::default()
    });
    t.feed(b"\x1b]12;#ffffff\x07\x1bc\x1b]12;?\x07");
    assert_eq!(t.screen().cursor_color(), None);
    assert_eq!(
        t.screen_mut().take_responses(),
        b"\x1b]12;rgb:0101/0202/0303\x07"
    );
}

fn colors() -> DefaultColors {
    let mut palette = [(0, 0, 0); 256];
    palette[1] = (0xaa, 0, 0);
    DefaultColors {
        foreground: (0xdc, 0xdc, 0xdc),
        background: (0x18, 0x18, 0x18),
        cursor: (0xff, 0xff, 0xff),
        palette,
    }
}

#[test]
fn osc_10_11_query_and_set() {
    let mut t = term(10, 1);
    t.screen_mut().set_default_colors(colors());
    t.feed(b"\x1b]11;?\x1b\\\x1b]10;?\x07");
    assert_eq!(
        t.screen_mut().take_responses(),
        b"\x1b]11;rgb:1818/1818/1818\x1b\\\x1b]10;rgb:dcdc/dcdc/dcdc\x07"
    );
    // OSC 10 with two colors also sets the background.
    t.feed(b"\x1b]10;#ffffff;#000000\x07");
    let o = t.screen().color_overrides();
    assert_eq!(o.foreground, Some((255, 255, 255)));
    assert_eq!(o.background, Some((0, 0, 0)));
    t.feed(b"\x1b]11;?\x07");
    assert_eq!(
        t.screen_mut().take_responses(),
        b"\x1b]11;rgb:0000/0000/0000\x07"
    );
    t.feed(b"\x1b]110\x07\x1b]111\x07");
    assert!(t.screen().color_overrides().is_empty());
}

#[test]
fn osc_4_palette_query_set_and_reset() {
    let mut t = term(10, 1);
    t.screen_mut().set_default_colors(colors());
    t.feed(b"\x1b]4;1;?\x07");
    assert_eq!(
        t.screen_mut().take_responses(),
        b"\x1b]4;1;rgb:aaaa/0000/0000\x07"
    );
    t.feed(b"\x1b]4;1;#00ff00;200;#0000ff\x07");
    let o = t.screen().color_overrides();
    assert_eq!(o.palette[1], Some((0, 255, 0)));
    assert_eq!(o.palette[200], Some((0, 0, 255)));
    t.feed(b"\x1b]104;1\x07");
    assert_eq!(t.screen().color_overrides().palette[1], None);
    assert_eq!(t.screen().color_overrides().palette[200], Some((0, 0, 255)));
    t.feed(b"\x1b]104\x07");
    assert!(t.screen().color_overrides().is_empty());
}

#[test]
fn color_specifications() {
    assert_eq!(parse_color(b"#fff"), Some((255, 255, 255)));
    assert_eq!(parse_color(b"#102030"), Some((0x10, 0x20, 0x30)));
    assert_eq!(parse_color(b"rgb:f/0/8"), Some((255, 0, 136)));
    assert_eq!(parse_color(b"rgb:ffff/0000/8080"), Some((255, 0, 128)));
    assert_eq!(parse_color(b"rgb:1/2"), None);
    assert_eq!(parse_color(b"rgb:1/2/3/4"), None);
    assert_eq!(parse_color(b"#12345"), None);
    assert_eq!(parse_color(b"red"), None);
}

#[test]
fn underline_styles() {
    let mut t = term(10, 1);
    t.feed(b"\x1b[4ma\x1b[4:3mb\x1b[4:2mc\x1b[21md\x1b[4:4me\x1b[4:5mf\x1b[4:0mg\x1b[4m\x1b[24mh");
    let styles: Vec<_> = (0..8).map(|c| cell(&t, 0, c).underline).collect();
    use Underline::*;
    assert_eq!(
        styles,
        [Single, Curly, Double, Double, Dotted, Dashed, None, None]
    );
}

#[test]
fn underline_color() {
    let mut t = term(10, 1);
    // Neovim's colon form, the semicolon form, then reset.
    t.feed(b"\x1b[4:3;58:2::255:0:0ma\x1b[58;5;4mb\x1b[59mc\x1b[0md");
    assert_eq!(cell(&t, 0, 0).underline_color, Color::Rgb(255, 0, 0));
    assert_eq!(cell(&t, 0, 1).underline_color, Color::Indexed(4));
    assert_eq!(cell(&t, 0, 2).underline_color, Color::Default);
    assert_eq!(cell(&t, 0, 3).underline, Underline::None);
    // The color doesn't leak into the text color.
    assert_eq!(cell(&t, 0, 0).fg, Color::Default);
}

#[test]
fn decrqss_reports_sgr() {
    let mut t = term(10, 1);
    t.feed(b"\x1bP$qm\x1b\\");
    assert_eq!(t.screen_mut().take_responses(), b"\x1bP1$r0m\x1b\\");

    // Neovim's undercurl probe: set 4:3, then ask.
    t.feed(b"\x1b[0m\x1b[4:3m\x1bP$qm\x1b\\");
    assert_eq!(t.screen_mut().take_responses(), b"\x1bP1$r0;4:3m\x1b\\");

    t.feed(b"\x1b[0;1;3;31;104;38:2::1:2:3;58:5:9m\x1bP$qm\x1b\\");
    assert_eq!(
        t.screen_mut().take_responses(),
        b"\x1bP1$r0;1;3;38:2::1:2:3;104;58:5:9m\x1b\\"
    );
}

#[test]
fn decrqss_reports_other_settings() {
    let mut t = term(10, 5);
    t.feed(b"\x1b[2;4r\x1bP$qr\x1b\\");
    assert_eq!(t.screen_mut().take_responses(), b"\x1bP1$r2;4r\x1b\\");
    t.feed(b"\x1b[5 q\x1bP$q q\x1b\\");
    assert_eq!(t.screen_mut().take_responses(), b"\x1bP1$r6 q\x1b\\");
    // Unknown requests are reported as invalid.
    t.feed(b"\x1bP$qx\x1b\\");
    assert_eq!(t.screen_mut().take_responses(), b"\x1bP0$r\x1b\\");
    // Other DCS strings are ignored.
    t.feed(b"\x1bPq#0;2;0;0;0\x1b\\");
    assert!(t.screen_mut().take_responses().is_empty());
}

#[test]
fn mouse_modes() {
    let mut t = term(10, 1);
    assert_eq!(t.screen().modes.mouse, MouseMode::Off);
    t.feed(b"\x1b[?1000h\x1b[?1006h");
    assert_eq!(t.screen().modes.mouse, MouseMode::Normal);
    assert!(t.screen().modes.mouse_sgr);
    // The last mode set wins; resetting another mode leaves it alone.
    t.feed(b"\x1b[?1003h\x1b[?1002l");
    assert_eq!(t.screen().modes.mouse, MouseMode::AnyMotion);
    t.feed(b"\x1b[?1003l\x1b[?1006l");
    assert_eq!(t.screen().modes.mouse, MouseMode::Off);
    assert!(!t.screen().modes.mouse_sgr);
    t.feed(b"\x1b[?9h");
    assert_eq!(t.screen().modes.mouse, MouseMode::X10);
    // A full reset turns reporting off.
    t.feed(b"\x1b[?1002h\x1bc");
    assert_eq!(t.screen().modes.mouse, MouseMode::Off);
}

/// A 1×1 RGB image with id 1, as a kitty graphics transmission.
const IMAGE: &[u8] = b"\x1b_Ga=t,i=1,f=24,s=1,v=1;AAAA\x1b\\";

#[test]
fn apc_strings_are_hidden_from_the_parser() {
    let mut t = term(10, 2);
    t.feed(b"a\x1b_Xunknown\x1b\\b");
    assert_eq!(lines(&t), ["ab", ""]);
    // Split anywhere, even between ESC and `_` or `\`.
    let mut t = term(10, 2);
    let input = b"a\x1b_Gi=7,a=q,s=1,v=1,f=24;AAAA\x1b\\b\x1b[1mc";
    for byte in input {
        t.feed(&[*byte]);
    }
    assert_eq!(lines(&t), ["abc", ""]);
    assert!(cell(&t, 0, 2).flags.contains(Flags::BOLD));
    assert_eq!(t.screen_mut().take_responses(), b"\x1b_Gi=7;OK\x1b\\");
}

#[test]
fn escape_sequence_cuts_apc_short() {
    let mut t = term(10, 2);
    t.feed(b"\x1b_Gjunk\x1b[1mx");
    assert_eq!(lines(&t), ["x", ""]);
    assert!(cell(&t, 0, 0).flags.contains(Flags::BOLD));
}

#[test]
fn placing_an_image_moves_the_cursor_past_it() {
    let mut t = term(10, 4);
    t.screen_mut().set_cell_size(10, 20);
    t.feed(IMAGE);
    t.feed(b"\x1b_Ga=p,i=1,c=3,r=2\x1b\\");
    assert_eq!(t.screen().cursor(), (1, 3));
    // C=1 leaves it.
    t.feed(b"\x1b_Ga=p,i=1,c=3,r=2,C=1\x1b\\");
    assert_eq!(t.screen().cursor(), (1, 3));
    // At the bottom, the screen scrolls to make room.
    t.feed(b"\x1b[4;1H\x1b_Ga=p,i=1,c=2,r=3\x1b\\");
    assert_eq!(t.screen().cursor(), (3, 2));
    let placements = t.screen().graphics().placements();
    assert_eq!(placements[2].line - t.screen().grid().scrolled as i64, 1);
}

#[test]
fn images_scroll_with_the_text() {
    let mut t = term(10, 3);
    t.feed(IMAGE);
    t.feed(b"\x1b[2;1H\x1b_Ga=p,i=1,C=1\x1b\\");
    let line = |t: &Terminal| {
        let screen = t.screen();
        screen.graphics().placements()[0].line - screen.grid().scrolled as i64
    };
    assert_eq!(line(&t), 1);
    t.feed(b"\x1b[3;1H\n");
    assert_eq!(line(&t), 0);
    // Deleting lines above moves it up within the region; off the top of
    // it, it is gone.
    t.feed(b"\x1b[1;1H\x1b[L");
    assert_eq!(line(&t), 1);
    t.feed(b"\x1b[2;3r\x1b[2;1H\x1b[M");
    assert!(t.screen().graphics().placements().is_empty());
}

#[test]
fn clearing_the_screen_removes_images() {
    let mut t = term(10, 3);
    t.feed(IMAGE);
    t.feed(b"\x1b_Ga=p,i=1\x1b\\");
    t.feed(b"\x1b[2J");
    assert!(t.screen().graphics().placements().is_empty());
}

#[test]
fn alternate_screen_has_its_own_images() {
    let mut t = term(10, 3);
    t.feed(IMAGE);
    t.feed(b"\x1b_Ga=p,i=1\x1b\\");
    t.feed(b"\x1b[?1049h");
    assert!(t.screen().graphics().placements().is_empty());
    t.feed(IMAGE);
    t.feed(b"\x1b_Ga=p,i=1\x1b\\\x1b[?1049l");
    assert_eq!(t.screen().graphics().placements().len(), 1);
}

#[test]
fn reports_pixel_sizes() {
    let mut t = term(80, 24);
    t.screen_mut().set_cell_size(9, 18);
    t.feed(b"\x1b[16t\x1b[14t\x1b[18t");
    assert_eq!(
        t.screen_mut().take_responses(),
        b"\x1b[6;18;9t\x1b[4;432;720t\x1b[8;24;80t"
    );
}

#[test]
fn keyboard_flags_push_pop_and_set() {
    let mut t = term(10, 2);
    t.feed(b"\x1b[?u");
    assert_eq!(t.screen_mut().take_responses(), b"\x1b[?0u");
    t.feed(b"\x1b[>1u\x1b[>29u");
    assert_eq!(t.screen().keyboard_flags(), 29);
    t.feed(b"\x1b[=2;3u");
    assert_eq!(t.screen().keyboard_flags(), 29);
    t.feed(b"\x1b[=2;2u");
    assert_eq!(t.screen().keyboard_flags(), 31);
    t.feed(b"\x1b[<u");
    assert_eq!(t.screen().keyboard_flags(), 1);
    t.feed(b"\x1b[<5u");
    assert_eq!(t.screen().keyboard_flags(), 0);
    // `CSI u` without a marker still restores the cursor.
    t.feed(b"ab\x1b[s\x1b[1;1H\x1b[u");
    assert_eq!(t.screen().cursor(), (0, 2));
}

#[test]
fn screens_have_their_own_keyboard_flags() {
    let mut t = term(10, 2);
    t.feed(b"\x1b[>1u\x1b[?1049h");
    assert_eq!(t.screen().keyboard_flags(), 0);
    t.feed(b"\x1b[>15u\x1b[?1049l");
    assert_eq!(t.screen().keyboard_flags(), 1);
}

#[test]
fn osc_9_notification() {
    let mut t = term(10, 1);
    t.feed(b"\x1b]9;Build done; 3 warnings\x07");
    assert_eq!(
        t.screen_mut().take_notifications(),
        [Notification {
            title: None,
            body: "Build done; 3 warnings".into()
        }]
    );
    assert!(t.screen_mut().take_notifications().is_empty());
}

#[test]
fn osc_9_conemu_subcommands_are_not_notifications() {
    let mut t = term(10, 1);
    t.feed(b"\x1b]9;4;1;50\x07\x1b]9;1;100\x1b\\\x1b]9;\x07");
    assert!(t.screen_mut().take_notifications().is_empty());
}

#[test]
fn osc_777_notification() {
    let mut t = term(10, 1);
    t.feed(b"\x1b]777;notify;Claude Code;Done; all tests pass\x1b\\");
    t.feed(b"\x1b]777;notify;;No title\x07\x1b]777;notify;Only title\x07");
    assert_eq!(
        t.screen_mut().take_notifications(),
        [
            Notification {
                title: Some("Claude Code".into()),
                body: "Done; all tests pass".into()
            },
            Notification {
                title: None,
                body: "No title".into()
            },
            Notification {
                title: Some("Only title".into()),
                body: String::new()
            },
        ]
    );
}

#[test]
fn notifications_are_bounded() {
    let mut t = term(10, 1);
    let long = "é".repeat(5000);
    for _ in 0..10 {
        t.feed(format!("\x1b]9;{long}\x07").as_bytes());
    }
    let n = t.screen_mut().take_notifications();
    assert_eq!(n.len(), 4);
    assert_eq!(n[0].body.chars().count(), 1024);
}

#[test]
fn long_titles_and_osc_strings_are_cut() {
    let mut t = term(10, 1);
    let long = "x".repeat(100_000);
    t.feed(format!("\x1b]0;{long}\x07").as_bytes());
    let title = t.screen_mut().take_title().unwrap();
    assert_eq!(title.len(), MAX_TITLE_CHARS);
    t.feed(format!("\x1b]8;;{long}\x1b\\a").as_bytes());
    let link = t.screen().link(cell(&t, 0, 0).link).unwrap();
    assert!(link.len() < MAX_OSC_BYTES);
}

#[test]
fn combining_mark_floods_take_linear_time() {
    let mut t = term(10, 2);
    let mut input = b"a".to_vec();
    for _ in 0..200_000 {
        input.extend_from_slice("\u{301}".as_bytes());
    }
    let start = std::time::Instant::now();
    t.feed(&input);
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    let tail = t.screen().cluster(cell(&t, 0, 0).cluster);
    assert_eq!(tail.chars().count(), MAX_CLUSTER_CHARS);
    assert_eq!(t.screen().cursor(), (0, 1));
}

#[test]
fn unused_clusters_are_dropped_when_the_table_fills() {
    // Without history, every cluster scrolls away and its id is freed.
    let mut t = Terminal::new(10, 2, 0);
    for i in 0..70_000u32 {
        let a = char::from_u32(0x300 + i % 0x70).unwrap();
        let b = char::from_u32(0x300 + (i / 0x70) % 0x70).unwrap();
        t.feed(format!("e{a}{b}\r\n").as_bytes());
    }
    t.feed("e\u{323}\u{302}".as_bytes());
    let c = cell(&t, 1, 0);
    assert_eq!(t.screen().cluster(c.cluster), "\u{323}\u{302}");
}

#[test]
fn unused_hyperlinks_are_dropped_when_the_table_fills() {
    let mut t = Terminal::new(10, 2, 0);
    for i in 0..70_000 {
        t.feed(format!("\x1b]8;;https://x/{i}\x1b\\a\x1b]8;;\x1b\\\r\n").as_bytes());
    }
    t.feed(b"\x1b]8;;https://last\x1b\\b");
    let c = cell(&t, 1, 0);
    assert_eq!(t.screen().link(c.link), Some("https://last"));
}

#[test]
fn erased_rows_are_not_joined_by_reflow() {
    let mut t = term(10, 4);
    t.feed(b"aaaaaaaaaaBBBB[1;1H[2Kx");
    assert!(!t.screen().grid().rows[0].wrapped);
    t.screen_mut().resize(20, 4);
    assert_eq!(lines(&t)[..2], ["x".to_string(), "BBBB".to_string()]);
}

#[test]
fn cursor_report_stays_sane_when_restored_above_the_region() {
    let mut t = term(10, 10);
    // Saved in origin mode, then the region moves below the saved row.
    t.feed(b"\x1b[?6h\x1b7\x1b[?6l\x1b[5;10r\x1b8\x1b[6n");
    assert_eq!(t.screen_mut().take_responses(), b"\x1b[1;1R");
    assert_eq!(t.screen().cursor().0, 4);
}

#[test]
fn inserting_at_a_wide_character_moves_it_whole() {
    let mut t = term(10, 1);
    t.feed("a中b\x1b[1;2H\x1b[2@".as_bytes());
    assert_eq!(cell(&t, 0, 3).c, '中');
    assert!(cell(&t, 0, 3).flags.contains(Flags::WIDE));
    assert!(cell(&t, 0, 4).flags.contains(Flags::WIDE_SPACER));
    assert_eq!(cell(&t, 0, 5).c, 'b');
}

#[test]
fn inserting_clears_a_pending_wrap() {
    let mut t = term(3, 2);
    t.feed(b"abc\x1b[@d");
    assert_eq!(lines(&t), ["abd", ""]);
    assert_eq!(t.screen().cursor().0, 0);
}

#[test]
fn a_saved_pending_wrap_does_not_survive_a_resize() {
    let mut t = term(3, 2);
    t.feed(b"abc\x1b7");
    t.screen_mut().resize(6, 2);
    t.feed(b"\x1b8x");
    assert_eq!(t.screen().cursor().0, 0);
}

#[test]
fn decaln_homes_the_cursor_and_resets_the_margins() {
    let mut t = term(4, 4);
    t.feed(b"\x1b[2;3r\x1b[3;3H\x1b#8");
    assert_eq!(t.screen().cursor(), (0, 0));
    assert_eq!(lines(&t)[3], "EEEE");
    t.feed(b"\x1b[4;1H\n");
    assert_eq!(t.screen().cursor(), (3, 0));
    assert_eq!(lines(&t)[0], "EEEE");
}

#[test]
fn reset_keeps_replies_that_are_due() {
    let mut t = term(4, 2);
    t.feed(b"\x1b[5n\x1bc");
    assert_eq!(t.screen_mut().take_responses(), b"\x1b[0n");
}

#[test]
fn sequences_with_too_many_parameters_are_ignored() {
    let mut t = term(10, 1);
    let many = "0;".repeat(40);
    t.feed(format!("\x1b[1m\x1b[{many}1;31mx").as_bytes());
    assert!(cell(&t, 0, 0).flags.contains(Flags::BOLD));
    assert_eq!(cell(&t, 0, 0).fg, Color::Default);
}

#[test]
fn can_and_sub_cancel_an_apc_string() {
    let mut t = term(10, 1);
    t.feed(b"\x1b_Gjunk\x18ok");
    assert_eq!(lines(&t), ["ok"]);
    let mut t = term(10, 1);
    t.feed(b"\x1b_Gjunk\x1aok");
    assert_eq!(lines(&t), ["ok"]);
}

#[test]
fn an_apc_string_ends_the_sequence_it_interrupts() {
    let mut t = term(10, 1);
    t.feed(b"\x1b[1\x1b_Gx\x1b\\mA");
    assert_eq!(lines(&t), ["mA"]);
    assert!(!cell(&t, 0, 0).flags.contains(Flags::BOLD));
}

#[test]
fn a_second_apc_string_cuts_the_first_short() {
    let mut t = term(10, 1);
    t.feed(b"\x1b_Gfirst\x1b_Gi=7,a=q,s=1,v=1,f=24;AAAA\x1b\\ok");
    assert_eq!(lines(&t), ["ok"]);
    assert_eq!(t.screen_mut().take_responses(), b"\x1b_Gi=7;OK\x1b\\");
}

#[test]
fn a_wide_character_at_a_wrap_leaves_no_text_behind() {
    let mut t = term(5, 2);
    t.feed("abcd中e".as_bytes());
    assert!(cell(&t, 0, 4).flags.contains(Flags::WRAP_GAP));
    assert_eq!(lines(&t), ["abcd", "中e"]);
    // Reflow keeps treating it as a gap, in both directions.
    t.screen_mut().resize(10, 2);
    assert_eq!(lines(&t)[0], "abcd中e");
    t.screen_mut().resize(5, 2);
    assert!(cell(&t, 0, 4).flags.contains(Flags::WRAP_GAP));
    // A real space before the wide character is text, and stays.
    let mut t = term(5, 2);
    t.feed("abc 中".as_bytes());
    t.screen_mut().resize(10, 2);
    assert_eq!(lines(&t)[0], "abc 中");
}

#[test]
fn growing_brings_rows_back_from_the_history() {
    let mut t = term(4, 4);
    t.feed(b"1\r\n2\r\n3\r\n4");
    t.screen_mut().resize(4, 2);
    assert_eq!(lines(&t), ["3", "4"]);
    assert_eq!(t.screen().cursor().0, 1);
    t.screen_mut().resize(4, 4);
    assert_eq!(lines(&t), ["1", "2", "3", "4"]);
    assert_eq!(t.screen().cursor().0, 3);
    assert!(t.screen().grid().history.is_empty());
    assert_eq!(t.screen().grid().scrolled, 0);
    // Without history, blank rows go below.
    t.screen_mut().resize(4, 5);
    assert_eq!(lines(&t), ["1", "2", "3", "4", ""]);
}

#[test]
fn the_prompt_end_follows_rows_moved_by_line_edits() {
    let mut t = term(10, 4);
    t.feed(b"\x1b[2;1H$ \x1b]133;B\x07");
    assert_eq!(t.screen().prompt_end, Some((1, 2)));
    t.feed(b"\x1b[1;1H\x1b[L");
    assert_eq!(t.screen().prompt_end, Some((2, 2)));
    t.feed(b"\x1b[1;1H\x1b[M");
    assert_eq!(t.screen().prompt_end, Some((1, 2)));
    t.feed(b"\x1b[T");
    assert_eq!(t.screen().prompt_end, Some((2, 2)));
    t.feed(b"\x1b[5T");
    assert_eq!(t.screen().prompt_end, None);
}

/// A small deterministic generator, so that a failure can be repeated.
struct Random(u64);

impl Random {
    fn next(&mut self) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 as usize
    }
}

fn assert_grid_shape(t: &Terminal, seed: u64) {
    let screen = t.screen();
    let (cols, rows) = (screen.cols(), screen.rows());
    let grid = screen.grid();
    assert_eq!(grid.rows.len(), rows, "seed {seed}");
    for row in grid.rows.iter().chain(&grid.history) {
        assert_eq!(row.cells.len(), cols, "seed {seed}");
    }
    let (row, col) = screen.cursor();
    assert!(row < rows && col < cols, "seed {seed}");
}

#[test]
fn random_sequences_keep_the_grid_consistent() {
    const FRAGMENTS: &[&str] = &[
        "a",
        "中",
        "e\u{301}",
        "👍🏽",
        "\u{200d}",
        "\u{fe0f}",
        "\r\n",
        "\n",
        "\t",
        "\x08",
        "\x18",
        "\x1b",
        "\x1b[",
        "\x1b]",
        "\x1bP",
        "\x1b_G",
        "\x1b\\",
        "\x07",
        ";",
        "?",
        "1",
        "0",
        "65535",
        "\x1b[5;20r",
        "\x1b[r",
        "\x1b[?6h",
        "\x1b[?6l",
        "\x1b7",
        "\x1b8",
        "\x1b[3;3H",
        "\x1b[2J",
        "\x1b[K",
        "\x1b[2@",
        "\x1b[3P",
        "\x1b[2L",
        "\x1b[2M",
        "\x1b[2S",
        "\x1b[2T",
        "\x1b[9b",
        "\x1b[?1049h",
        "\x1b[?1049l",
        "\x1b[4h",
        "\x1b[4l",
        "\x1b[?7l",
        "\x1b[?7h",
        "\x1bc",
        "\x1b#8",
        "\x1b[6n",
        "\x1b[c",
        "\x1b]0;title\x07",
        "\x1b]8;;http://x\x1b\\",
        "\x1b]133;A\x07",
        "\x1b]133;B\x07",
        "\x1b]4;1;?\x07",
        "\x1b]9;hi\x07",
        "\x1bP$qm\x1b\\",
        "\x1b_Ga=T,f=24,s=1,v=1;AAAA\x1b\\",
        "\x1b_Ga=p,i=1\x1b\\",
        "\x1b_Ga=d\x1b\\",
        "\x1b(0",
        "\x1b(B",
        "q",
        "\x1b[38:5:1m",
        "\x1b[0m",
        "\x1b[4:3m",
        "\x1b[>1u",
        "\x1b[<u",
    ];
    for seed in 1..=60u64 {
        let mut random = Random(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let mut t = Terminal::new(1 + random.next() % 30, 1 + random.next() % 12, 20);
        for step in 0..1500 {
            let fragment = FRAGMENTS[random.next() % FRAGMENTS.len()];
            t.feed(fragment.as_bytes());
            if step % 100 == 0 {
                let (cols, rows) = (1 + random.next() % 30, 1 + random.next() % 12);
                t.screen_mut().resize(cols, rows);
            }
            assert_grid_shape(&t, seed);
        }
    }
}
