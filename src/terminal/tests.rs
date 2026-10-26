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
    assert!(
        a.flags
            .contains(Flags::BOLD | Flags::ITALIC | Flags::UNDERLINE)
    );
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
    t.screen_mut().resize(3, 3);
    assert_eq!(t.screen().cols(), 3);
    assert_eq!(lines(&t), ["3", "4", ""]);
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
