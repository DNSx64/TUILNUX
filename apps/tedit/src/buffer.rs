//! Textpuffer von tedit: Zeilen als Strings, Positionen in Zeichen (nicht Bytes).

use unicode_width::UnicodeWidthChar;

pub const TAB_WIDTH: usize = 4;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub row: usize,
    pub col: usize,
}

impl Pos {
    pub fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Buffer {
    pub lines: Vec<String>,
}

fn byte_index(line: &str, col: usize) -> usize {
    line.char_indices().nth(col).map_or(line.len(), |(i, _)| i)
}

pub fn char_width(c: char, x: usize) -> usize {
    if c == '\t' {
        TAB_WIDTH - x % TAB_WIDTH
    } else {
        c.width().unwrap_or(0)
    }
}

/// Bildschirmspalte vor dem Zeichen `col`.
pub fn display_col(line: &str, col: usize) -> usize {
    line.chars().take(col).fold(0, |x, c| x + char_width(c, x))
}

/// Zeichenindex, der der Bildschirmspalte `target` am nächsten liegt.
pub fn col_from_display(line: &str, target: usize) -> usize {
    let mut x = 0;
    for (i, c) in line.chars().enumerate() {
        let w = char_width(c, x);
        if x + w > target {
            return if target - x > w / 2 { i + 1 } else { i };
        }
        x += w;
    }
    line.chars().count()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Buffer {
    /// Liest Text ein; gibt zurück, ob Zeilenenden CRLF waren.
    pub fn from_text(text: &str) -> (Self, bool) {
        let crlf = text.contains("\r\n");
        let lines = text
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
            .collect();
        (Self { lines }, crlf)
    }

    pub fn to_text(&self, crlf: bool) -> String {
        self.lines.join(if crlf { "\r\n" } else { "\n" })
    }

    pub fn line_len(&self, row: usize) -> usize {
        self.lines.get(row).map_or(0, |l| l.chars().count())
    }

    pub fn end(&self) -> Pos {
        let row = self.lines.len() - 1;
        Pos::new(row, self.line_len(row))
    }

    pub fn clamp(&self, pos: Pos) -> Pos {
        let row = pos.row.min(self.lines.len() - 1);
        Pos::new(row, pos.col.min(self.line_len(row)))
    }

    /// Fügt Text (auch mehrzeilig) ein und gibt die Position danach zurück.
    pub fn insert(&mut self, pos: Pos, text: &str) -> Pos {
        let pos = self.clamp(pos);
        let line = &mut self.lines[pos.row];
        let split = byte_index(line, pos.col);
        let tail = line.split_off(split);
        let mut parts = text.split('\n').map(|p| p.strip_suffix('\r').unwrap_or(p));
        line.push_str(parts.next().unwrap_or(""));
        let mut row = pos.row;
        for part in parts {
            row += 1;
            self.lines.insert(row, part.to_string());
        }
        let col = self.line_len(row);
        self.lines[row].push_str(&tail);
        Pos::new(row, col)
    }

    pub fn text(&self, a: Pos, b: Pos) -> String {
        let (a, b) = (self.clamp(a.min(b)), self.clamp(a.max(b)));
        if a.row == b.row {
            let l = &self.lines[a.row];
            return l[byte_index(l, a.col)..byte_index(l, b.col)].to_string();
        }
        let first = &self.lines[a.row];
        let mut out = first[byte_index(first, a.col)..].to_string();
        for row in a.row + 1..b.row {
            out.push('\n');
            out.push_str(&self.lines[row]);
        }
        let last = &self.lines[b.row];
        out.push('\n');
        out.push_str(&last[..byte_index(last, b.col)]);
        out
    }

    /// Löscht den Bereich [a, b) und gibt den entfernten Text zurück.
    pub fn delete(&mut self, a: Pos, b: Pos) -> String {
        let (a, b) = (self.clamp(a.min(b)), self.clamp(a.max(b)));
        let removed = self.text(a, b);
        let last = &self.lines[b.row];
        let tail = last[byte_index(last, b.col)..].to_string();
        let first = &mut self.lines[a.row];
        first.truncate(byte_index(first, a.col));
        first.push_str(&tail);
        self.lines.drain(a.row + 1..=b.row);
        removed
    }

    pub fn prev(&self, pos: Pos) -> Pos {
        if pos.col > 0 {
            Pos::new(pos.row, pos.col - 1)
        } else if pos.row > 0 {
            Pos::new(pos.row - 1, self.line_len(pos.row - 1))
        } else {
            pos
        }
    }

    pub fn next(&self, pos: Pos) -> Pos {
        if pos.col < self.line_len(pos.row) {
            Pos::new(pos.row, pos.col + 1)
        } else if pos.row + 1 < self.lines.len() {
            Pos::new(pos.row + 1, 0)
        } else {
            pos
        }
    }

    pub fn word_left(&self, pos: Pos) -> Pos {
        if pos.col == 0 {
            return self.prev(pos);
        }
        let chars: Vec<char> = self.lines[pos.row].chars().collect();
        let mut i = pos.col;
        while i > 0 && !is_word(chars[i - 1]) {
            i -= 1;
        }
        while i > 0 && is_word(chars[i - 1]) {
            i -= 1;
        }
        Pos::new(pos.row, i)
    }

    pub fn word_right(&self, pos: Pos) -> Pos {
        let chars: Vec<char> = self.lines[pos.row].chars().collect();
        if pos.col >= chars.len() {
            return self.next(pos);
        }
        let mut i = pos.col;
        while i < chars.len() && is_word(chars[i]) {
            i += 1;
        }
        while i < chars.len() && !is_word(chars[i]) {
            i += 1;
        }
        Pos::new(pos.row, i)
    }

    /// Sucht `needle` (Groß-/Kleinschreibung egal) ab `from`, mit Umlauf am Dateiende.
    pub fn find(&self, needle: &str, from: Pos, forward: bool) -> Option<Pos> {
        let needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
        if needle.is_empty() {
            return None;
        }
        let matches_at = |line: &[char], i: usize| {
            i + needle.len() <= line.len()
                && line[i..i + needle.len()]
                    .iter()
                    .flat_map(|c| c.to_lowercase())
                    .eq(needle.iter().copied())
        };
        let n = self.lines.len();
        for step in 0..=n {
            let row = if forward {
                (from.row + step) % n
            } else {
                (from.row + n * 2 - step) % n
            };
            let line: Vec<char> = self.lines[row].chars().collect();
            let candidates: Box<dyn Iterator<Item = usize>> = if forward {
                let start = if step == 0 { from.col } else { 0 };
                Box::new(start..line.len())
            } else {
                let end = if step == 0 { from.col } else { line.len() + 1 };
                Box::new((0..end.min(line.len() + 1)).rev())
            };
            for i in candidates {
                if matches_at(&line, i) {
                    return Some(Pos::new(row, i));
                }
            }
        }
        None
    }

    /// Ersetzt alle Vorkommen (Groß-/Kleinschreibung beachten). Gibt die Anzahl zurück.
    pub fn replace_all(&mut self, find: &str, with: &str) -> usize {
        if find.is_empty() || find.contains('\n') {
            return 0;
        }
        let mut count = 0;
        for line in &mut self.lines {
            let n = line.matches(find).count();
            if n > 0 {
                *line = line.replace(find, with);
                count += n;
            }
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str) -> Buffer {
        Buffer::from_text(text).0
    }

    #[test]
    fn roundtrip_keeps_line_endings() {
        let (b, crlf) = Buffer::from_text("a\r\nb\r\n");
        assert!(crlf);
        assert_eq!(b.lines, ["a", "b", ""]);
        assert_eq!(b.to_text(true), "a\r\nb\r\n");
        assert_eq!(buf("x\ny").to_text(false), "x\ny");
    }

    #[test]
    fn insert_and_delete_multiline() {
        let mut b = buf("Hallo Welt");
        let end = b.insert(Pos::new(0, 5), ",\nschöne");
        assert_eq!(b.lines, ["Hallo,", "schöne Welt"]);
        assert_eq!(end, Pos::new(1, 6));
        assert_eq!(b.text(Pos::new(0, 3), Pos::new(1, 3)), "lo,\nsch");
        assert_eq!(b.delete(Pos::new(1, 3), Pos::new(0, 3)), "lo,\nsch");
        assert_eq!(b.lines, ["Halöne Welt"]);
    }

    #[test]
    fn display_columns_handle_tabs_and_wide_chars() {
        assert_eq!(display_col("\tx", 1), 4);
        assert_eq!(display_col("ab\tx", 3), 4);
        assert_eq!(display_col("日本", 2), 4);
        assert_eq!(col_from_display("日本", 2), 1);
        assert_eq!(col_from_display("abc", 10), 3);
    }

    #[test]
    fn word_motion() {
        let b = buf("foo bar_baz  qux");
        assert_eq!(b.word_right(Pos::new(0, 0)), Pos::new(0, 4));
        assert_eq!(b.word_right(Pos::new(0, 4)), Pos::new(0, 13));
        assert_eq!(b.word_left(Pos::new(0, 13)), Pos::new(0, 4));
    }

    #[test]
    fn find_wraps_and_ignores_case() {
        let b = buf("eins\nZwei\nzwei drei");
        assert_eq!(b.find("zwei", Pos::new(0, 0), true), Some(Pos::new(1, 0)));
        assert_eq!(b.find("zwei", Pos::new(1, 1), true), Some(Pos::new(2, 0)));
        assert_eq!(b.find("eins", Pos::new(2, 3), true), Some(Pos::new(0, 0)));
        assert_eq!(b.find("zwei", Pos::new(2, 0), false), Some(Pos::new(1, 0)));
        assert_eq!(b.find("fehlt", Pos::new(0, 0), true), None);
    }

    #[test]
    fn replace_all_counts() {
        let mut b = buf("a-a\na");
        assert_eq!(b.replace_all("a", "bb"), 3);
        assert_eq!(b.lines, ["bb-bb", "bb"]);
    }
}
