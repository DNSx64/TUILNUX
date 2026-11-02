//! TUILNUX-Schriftzug aus Rahmenlinien, im Stil der Vorlage.

const T: [&str; 5] = ["╺━┳━╸", "  ┃  ", "  ┃  ", "  ┃  ", "  ╹  "];
const U: [&str; 5] = ["╻   ╻", "┃   ┃", "┃   ┃", "┃   ┃", "┗━━━┛"];
const I: [&str; 5] = ["╻", "┃", "┃", "┃", "╹"];
const L: [&str; 5] = ["╻    ", "┃    ", "┃    ", "┃    ", "┗━━━╸"];
const N: [&str; 5] = ["┏┓  ╻", "┃┗┓ ┃", "┃ ┗┓┃", "┃  ┗┫", "╹   ╹"];
const X: [&str; 5] = ["╻   ╻", "┗┓ ┏┛", " ┗┳┛ ", " ┏┻┓ ", "┏┛ ┗┓"];

/// Arch-Dreieck als Symbol neben dem Schriftzug.
pub const ICON: [&str; 5] = [
    "    ╱╲    ",
    "   ╱  ╲   ",
    "  ╱    ╲  ",
    " ╱  ╱╲  ╲ ",
    "╱  ╱  ╲  ╲",
];

const SMALL: [&str; 3] = [
    "╺┳╸╻ ╻╻╻  ┏┓╻╻ ╻╻ ╻",
    " ┃ ┃ ┃┃┃  ┃┗┫┃ ┃┏╋┛",
    " ╹ ┗━┛╹┗━╸╹ ╹┗━┛╹ ╹",
];

/// Großer Schriftzug (5 Zeilen), Buchstaben mit einer Spalte Abstand.
pub fn big() -> Vec<String> {
    let letters = [T, U, I, L, N, U, X];
    (0..5)
        .map(|row| letters.iter().map(|l| l[row]).collect::<Vec<_>>().join(" "))
        .collect()
}

pub fn small() -> Vec<String> {
    SMALL.iter().map(|s| s.to_string()).collect()
}

/// Dünne Linien wie in der Vorlage; Diagonalen bleiben.
pub fn thin(text: &str) -> String {
    text.chars().map(thin_char).collect()
}

fn thin_char(c: char) -> char {
    match c {
        '━' | '╺' | '╸' => '─',
        '┃' | '╻' | '╹' => '│',
        '┏' => '┌',
        '┓' => '┐',
        '┗' => '└',
        '┛' => '┘',
        '┳' => '┬',
        '┻' => '┴',
        '┣' => '├',
        '┫' => '┤',
        '╋' => '┼',
        c => c,
    }
}

/// Dünne Linien für Konsolenschriften wie Terminus (max. 512 Glyphen, keine fetten Linien).
pub fn light(text: &str) -> String {
    text.chars()
        .map(|c| match thin_char(c) {
            '╱' => '/',
            '╲' => '\\',
            c => c,
        })
        .collect()
}

/// Ersatzzeichen für die Linux-Konsole, deren Schrift nicht alle Linien kennt.
pub fn ascii(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '━' | '╺' | '╸' => '-',
            '┃' | '╻' | '╹' => '|',
            '┏' | '┓' | '┗' | '┛' | '┳' | '┻' | '┣' | '┫' | '╋' => '+',
            '╱' => '/',
            '╲' => '\\',
            '•' => '*',
            c => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_have_equal_width() {
        let w = |s: &str| s.chars().count();
        let big = big();
        assert!(big.iter().all(|r| w(r) == w(&big[0])));
        assert!(ICON.iter().all(|r| w(r) == w(ICON[0])));
        assert!(SMALL.iter().all(|r| w(r) == w(SMALL[0])));
        assert!(ascii(&big[0]).is_ascii());
        assert!(ascii(ICON[4]).is_ascii());
        let thin = light(&(big.join("") + &SMALL.join("")));
        assert!(!thin.contains(['━', '┃', '╺', '╻', '┏', '┳', '╋']));
        assert_eq!(w(&light(&big[0])), w(&big[0]));
        assert_eq!(super::thin(ICON[0]), ICON[0]);
        assert!(!super::thin(&big.join("")).contains(['━', '┃']));
    }
}
