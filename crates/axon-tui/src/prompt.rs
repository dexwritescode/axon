//! The text being typed, with a cursor, and how it wraps on screen.

use std::ops::{Range, RangeInclusive};

use unicode_width::UnicodeWidthChar;

/// Private-use chars: the ones from `TOKENS.start()` on stand for pastes in
/// `Prompt::text`; the rest are never typed in.
const TOKENS: RangeInclusive<char> = '\u{E000}'..='\u{F8FF}';

#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Prompt {
    /// The typed text, with one token char for each long paste.
    text: String,
    /// Byte index into `text`, always on a char boundary.
    cursor: usize,
    /// The text of long pastes; token char `TOKENS.start() + i` stands for
    /// `pastes[i]`.
    pastes: Vec<String>,
}

/// `text` with the cursor at its end.
impl From<&str> for Prompt {
    fn from(text: &str) -> Self {
        Self {
            text: text.into(),
            cursor: text.len(),
            pastes: Vec::new(),
        }
    }
}

/// A run of a prompt's row as drawn.
#[derive(Debug, PartialEq)]
pub(crate) enum Piece<'a> {
    Text(&'a str),
    /// A long paste, drawn as its label.
    Paste(String),
}

impl Prompt {
    /// The typed text, a long paste standing as one private-use char.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// The text with each long paste in full, as sent to the model.
    pub(crate) fn expanded(&self) -> String {
        self.map_pastes(|paste| paste.to_string())
    }

    /// The text with each long paste as its label.
    pub(crate) fn shown(&self) -> String {
        self.map_pastes(label)
    }

    fn map_pastes(&self, f: impl Fn(&str) -> String) -> String {
        let mut out = String::new();
        for c in self.text.chars() {
            match self.paste_of(c) {
                Some(paste) => out.push_str(&f(paste)),
                None => out.push(c),
            }
        }
        out
    }

    /// The text without surrounding whitespace, the cursor at its end.
    pub(crate) fn trimmed(&self) -> Self {
        let text = self.text.trim().to_string();
        Self {
            cursor: text.len(),
            text,
            pastes: self.pastes.clone(),
        }
    }

    /// Append `other` on a new line, the cursor at the end.
    pub(crate) fn push_line(&mut self, other: &Self) {
        self.text.push('\n');
        for c in other.text.chars() {
            match other.paste_of(c) {
                Some(paste) => self.push_paste(paste.to_string()),
                None => self.text.push(c),
            }
        }
        self.cursor = self.text.len();
    }

    /// The pieces of `range` as drawn: text, and long pastes as labels.
    pub(crate) fn pieces(&self, range: Range<usize>) -> Vec<Piece<'_>> {
        let mut pieces = Vec::new();
        let mut start = range.start;
        for (i, c) in self.text[range.clone()].char_indices() {
            if let Some(paste) = self.paste_of(c) {
                let i = range.start + i;
                if i > start {
                    pieces.push(Piece::Text(&self.text[start..i]));
                }
                pieces.push(Piece::Paste(label(paste)));
                start = i + c.len_utf8();
            }
        }
        if range.end > start {
            pieces.push(Piece::Text(&self.text[start..range.end]));
        }
        pieces
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Insert `c` at the cursor; control chars other than newline are dropped.
    pub(crate) fn insert(&mut self, c: char) {
        if c.is_control() && c != '\n' || TOKENS.contains(&c) {
            return;
        }
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Insert pasted `text` at the cursor, its line breaks as newlines. Over
    /// `token_chars` chars it goes in as one token; otherwise as typed, its
    /// tabs as four spaces.
    pub(crate) fn paste(&mut self, text: &str, token_chars: usize) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if text.chars().count() > token_chars && self.pastes.len() < TOKENS.count() {
            let at = self.cursor;
            let end = self.text.split_off(at);
            self.push_paste(text);
            self.cursor = self.text.len();
            self.text.push_str(&end);
            return;
        }
        for c in text.chars() {
            match c {
                '\t' => (0..4).for_each(|_| self.insert(' ')),
                c => self.insert(c),
            }
        }
    }

    pub(crate) fn backspace(&mut self) {
        if self.before().is_some() {
            self.left();
            self.text.remove(self.cursor);
        }
    }

    pub(crate) fn delete(&mut self) {
        if self.after().is_some() {
            self.text.remove(self.cursor);
        }
    }

    pub(crate) fn left(&mut self) {
        if let Some(c) = self.before() {
            self.cursor -= c.len_utf8();
        }
    }

    pub(crate) fn right(&mut self) {
        if let Some(c) = self.after() {
            self.cursor += c.len_utf8();
        }
    }

    /// Back over whitespace, then to the start of the word.
    pub(crate) fn word_left(&mut self) {
        while self.before().is_some_and(char::is_whitespace) {
            self.left();
        }
        while self.before().is_some_and(|c| !c.is_whitespace()) {
            self.left();
        }
    }

    /// On over whitespace, then to the end of the word.
    pub(crate) fn word_right(&mut self) {
        while self.after().is_some_and(char::is_whitespace) {
            self.right();
        }
        while self.after().is_some_and(|c| !c.is_whitespace()) {
            self.right();
        }
    }

    /// To the start of the line.
    pub(crate) fn home(&mut self) {
        self.cursor = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
    }

    /// To the end of the line.
    pub(crate) fn end(&mut self) {
        self.cursor = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i);
    }

    /// Up one screen row at `width` columns, as near the same column as it
    /// gets; false on the first row.
    pub(crate) fn up(&mut self, width: usize) -> bool {
        self.move_row(width, -1)
    }

    /// Down one screen row at `width` columns, as near the same column as it
    /// gets; false on the last row.
    pub(crate) fn down(&mut self, width: usize) -> bool {
        self.move_row(width, 1)
    }

    fn move_row(&mut self, width: usize, step: isize) -> bool {
        let rows = self.rows(width);
        let (col, row) = self.place(&rows, width);
        let Some(target) = row.checked_add_signed(step).filter(|&r| r < rows.len()) else {
            return false;
        };
        let range = rows[target].clone();
        let mut at = range.start;
        let mut used = 0;
        for (i, c) in self.text[range.clone()].char_indices() {
            used += self.char_width(c);
            if used > col {
                break;
            }
            at = range.start + i + c.len_utf8();
        }
        // The end of a wrapped row is the start of the next one; stay on this one.
        if row_of(&rows, at) != target {
            at -= self.text[..at]
                .chars()
                .next_back()
                .map_or(0, char::len_utf8);
        }
        self.cursor = at;
        true
    }

    /// Byte ranges of the screen rows at `width` columns, newlines left out.
    /// Each line wraps after the last space that fits, or mid-word when a
    /// word is longer than a row. Spaces at a break stay at the row's end,
    /// even past `width`.
    pub(crate) fn rows(&self, width: usize) -> Vec<Range<usize>> {
        let mut rows = Vec::new();
        let mut start = 0;
        for line in self.text.split('\n') {
            let end = start + line.len();
            let mut row = start;
            let mut col = 0;
            let mut after_space = None;
            for (i, c) in line.char_indices() {
                let i = start + i;
                let w = self.char_width(c);
                if c == ' ' {
                    col += w;
                    after_space = Some(i + 1);
                    continue;
                }
                if col + w > width && i > row {
                    let at = after_space.filter(|&s| s > row).unwrap_or(i);
                    rows.push(row..at);
                    col = self.str_width(&self.text[at..i]);
                    row = at;
                }
                col += w;
            }
            rows.push(row..end);
            start = end + 1;
        }
        rows
    }

    /// The cursor's column and row at `width` columns. Where a row wraps, the
    /// cursor is at the start of the next one; it never goes past `width`.
    pub(crate) fn cursor_at(&self, width: usize) -> (usize, usize) {
        self.place(&self.rows(width), width)
    }

    fn place(&self, rows: &[Range<usize>], width: usize) -> (usize, usize) {
        let row = row_of(rows, self.cursor);
        let col = self.str_width(&self.text[rows[row].start..self.cursor]);
        (col.min(width), row)
    }

    /// Add `paste` as a token at the end of the text.
    fn push_paste(&mut self, paste: String) {
        let token = (*TOKENS.start() as u32 + self.pastes.len() as u32)
            .try_into()
            .expect("in the private-use range");
        self.pastes.push(paste);
        self.text.push(token);
    }

    /// The long paste `c` stands for, if it is a token.
    fn paste_of(&self, c: char) -> Option<&str> {
        let i = (c as u32).checked_sub(*TOKENS.start() as u32)?;
        self.pastes.get(i as usize).map(String::as_str)
    }

    /// Columns `c` takes: a token takes its label's.
    fn char_width(&self, c: char) -> usize {
        match self.paste_of(c) {
            Some(paste) => label(paste).chars().count(),
            None => c.width().unwrap_or(0),
        }
    }

    fn str_width(&self, text: &str) -> usize {
        text.chars().map(|c| self.char_width(c)).sum()
    }

    fn before(&self) -> Option<char> {
        self.text[..self.cursor].chars().next_back()
    }

    fn after(&self) -> Option<char> {
        self.text[self.cursor..].chars().next()
    }
}

/// How a long paste shows in the input and the history.
fn label(paste: &str) -> String {
    format!("[pasted {} chars]", paste.chars().count())
}

/// The row holding byte `at`: the last one starting at or before it.
fn row_of(rows: &[Range<usize>], at: usize) -> usize {
    rows.iter().rposition(|r| r.start <= at).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text` with the cursor at `|`.
    fn at(text: &str) -> Prompt {
        let cursor = text.find('|').unwrap();
        Prompt {
            text: text.replace('|', ""),
            cursor,
            pastes: Vec::new(),
        }
    }

    /// The text with `|` at the cursor.
    fn shown(prompt: &Prompt) -> String {
        let mut text = prompt.text.clone();
        text.insert(prompt.cursor, '|');
        text
    }

    fn rows(text: &str, width: usize) -> Vec<&str> {
        let prompt = Prompt::from(text);
        prompt.rows(width).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn paste_keeps_line_breaks_as_newlines_and_expands_tabs() {
        let mut prompt = at("<|>");
        prompt.paste("a\r\nb\rc\n\td\u{7}", 1000);
        assert_eq!(shown(&prompt), "<a\nb\nc\n    d|>");
    }

    #[test]
    fn a_long_paste_goes_in_as_one_token() {
        let mut prompt = at("a|b");
        prompt.paste(&"x\r\n".repeat(6), 10);
        assert_eq!(prompt.shown(), "a[pasted 12 chars]b");
        assert_eq!(prompt.expanded(), format!("a{}b", "x\n".repeat(6)));
        assert_eq!(prompt.cursor_at(40), (18, 0));

        prompt.left();
        assert_eq!(prompt.cursor_at(40), (1, 0));
        prompt.delete();
        assert_eq!(prompt.expanded(), "ab");
    }

    #[test]
    fn backspace_deletes_a_paste_token_whole() {
        let mut prompt = at("|");
        prompt.paste("0123456789!", 10);
        prompt.backspace();
        assert_eq!(prompt.expanded(), "");
    }

    #[test]
    fn a_paste_token_wraps_by_its_label() {
        let mut prompt = Prompt::from("ab ");
        prompt.paste(&"x".repeat(20), 10);
        let rows: Vec<_> = prompt
            .rows(10)
            .into_iter()
            .map(|r| prompt.pieces(r))
            .collect();
        assert_eq!(
            rows,
            [
                vec![Piece::Text("ab ")],
                vec![Piece::Paste("[pasted 20 chars]".into())]
            ]
        );
    }

    #[test]
    fn typed_private_use_chars_are_dropped() {
        let mut prompt = Prompt::default();
        prompt.insert('\u{E000}');
        prompt.paste("\u{E001}", 10);
        assert_eq!(prompt.text(), "");
    }

    #[test]
    fn push_line_keeps_the_pastes_of_both() {
        let mut first = Prompt::default();
        first.paste("one long paste", 5);
        let mut second = Prompt::from("and ");
        second.paste("two long paste", 5);

        first.push_line(&second);
        assert_eq!(first.expanded(), "one long paste\nand two long paste");
        assert_eq!(first.shown(), "[pasted 14 chars]\nand [pasted 14 chars]");
    }

    #[test]
    fn insert_and_delete_at_the_cursor() {
        let mut prompt = at("hé|llo");
        prompt.insert('x');
        assert_eq!(shown(&prompt), "héx|llo");
        prompt.backspace();
        prompt.backspace();
        assert_eq!(shown(&prompt), "h|llo");
        prompt.delete();
        assert_eq!(shown(&prompt), "h|lo");
    }

    #[test]
    fn control_chars_are_dropped() {
        let mut prompt = Prompt::default();
        for c in "a\tb\r\nc".chars() {
            prompt.insert(c);
        }
        assert_eq!(prompt.text(), "ab\nc");
    }

    #[test]
    fn words_cross_newlines() {
        let mut prompt = at("one\n|two");
        prompt.word_left();
        assert_eq!(shown(&prompt), "|one\ntwo");
        prompt.word_right();
        prompt.word_right();
        assert_eq!(shown(&prompt), "one\ntwo|");
    }

    #[test]
    fn a_row_one_column_wide_still_holds_a_wide_char() {
        assert_eq!(rows("a日b", 1), ["a", "日", "b"]);
        assert_eq!(at("a日|b").cursor_at(1), (0, 2));
    }

    #[test]
    fn up_lands_before_a_wide_char_it_would_split() {
        let mut prompt = at("a日b\nab|c");
        prompt.up(10);
        assert_eq!(shown(&prompt), "a|日b\nabc");
    }

    #[test]
    fn edits_at_the_ends_do_nothing() {
        let mut prompt = at("|ab");
        prompt.backspace();
        prompt.left();
        assert_eq!(shown(&prompt), "|ab");
        prompt.end();
        prompt.delete();
        prompt.right();
        assert_eq!(shown(&prompt), "ab|");
    }

    #[test]
    fn left_and_right_step_over_whole_chars() {
        let mut prompt = at("a|é😀");
        prompt.right();
        prompt.right();
        assert_eq!(shown(&prompt), "aé😀|");
        prompt.left();
        assert_eq!(shown(&prompt), "aé|😀");
    }

    #[test]
    fn words_skip_whitespace_then_the_word() {
        let mut prompt = at("one  two|  three");
        prompt.word_left();
        assert_eq!(shown(&prompt), "one  |two  three");
        prompt.word_left();
        assert_eq!(shown(&prompt), "|one  two  three");
        prompt.word_right();
        prompt.word_right();
        assert_eq!(shown(&prompt), "one  two|  three");
        prompt.word_right();
        assert_eq!(shown(&prompt), "one  two  three|");
    }

    #[test]
    fn home_and_end_stay_on_the_line() {
        let mut prompt = at("one\ntw|o\nthree");
        prompt.home();
        assert_eq!(shown(&prompt), "one\n|two\nthree");
        prompt.end();
        assert_eq!(shown(&prompt), "one\ntwo|\nthree");
    }

    #[test]
    fn lines_wrap_after_the_last_space_that_fits() {
        assert_eq!(rows("aaa bbb ccc", 6), ["aaa ", "bbb ", "ccc"]);
        assert_eq!(rows("aaa bbb ccc", 7), ["aaa bbb ", "ccc"]);
        assert_eq!(rows("one\n\ntwo", 7), ["one", "", "two"]);
        assert_eq!(rows("", 7), [""]);
    }

    #[test]
    fn long_words_break_mid_word() {
        assert_eq!(rows("abcdefgh ij", 3), ["abc", "def", "gh ", "ij"]);
    }

    #[test]
    fn spaces_at_a_break_stay_on_the_row() {
        assert_eq!(rows("aaa   bbb", 4), ["aaa   ", "bbb"]);
    }

    #[test]
    fn wide_chars_count_two_columns() {
        assert_eq!(rows("日本語", 4), ["日本", "語"]);
        assert_eq!(at("日本|語").cursor_at(10), (4, 0));
    }

    #[test]
    fn cursor_at_a_wrap_is_on_the_next_row() {
        assert_eq!(at("aaa |bbb").cursor_at(5), (0, 1));
        assert_eq!(at("aaa| bbb").cursor_at(5), (3, 0));
        assert_eq!(at("aaaaa|").cursor_at(5), (5, 0));
        assert_eq!(at("aaa      |").cursor_at(5), (5, 0));
        assert_eq!(at("a\n|").cursor_at(5), (0, 1));
    }

    #[test]
    fn up_and_down_keep_the_column() {
        let mut prompt = at("abcdef\nab\nabc|def");
        prompt.up(10);
        assert_eq!(shown(&prompt), "abcdef\nab|\nabcdef");
        assert!(prompt.up(10));
        assert_eq!(shown(&prompt), "ab|cdef\nab\nabcdef");
        assert!(!prompt.up(10));
        assert_eq!(shown(&prompt), "ab|cdef\nab\nabcdef");
        prompt.down(10);
        assert!(prompt.down(10));
        assert!(!prompt.down(10));
        assert_eq!(shown(&prompt), "abcdef\nab\nab|cdef");
    }

    #[test]
    fn up_stays_on_a_wrapped_row() {
        let mut prompt = at("aaa bbbbbb|");
        prompt.up(7);
        assert_eq!(shown(&prompt), "aaa| bbbbbb");
    }
}
