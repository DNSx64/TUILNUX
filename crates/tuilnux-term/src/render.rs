//! Zeichnet den vt100-Bildschirm (plus optionales Hintergrundbild) in einen ratatui-Buffer.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use tuilnux_media::{FitMode, Rgb, WallpaperConfig};

pub fn color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

type Grid = Vec<Vec<Option<(Rgb, Rgb)>>>;

/// Hintergrundbild, vorberechnet als Halbblock-Zellen in Terminalgröße.
#[derive(Default)]
pub struct Wallpaper {
    cells: Grid,
    size: (u16, u16),
    stamp: Option<(PathBuf, Option<SystemTime>, u32)>,
    checked: Option<Instant>,
}

impl Wallpaper {
    fn cell(&self, col: u16, row: u16) -> Option<(Rgb, Rgb)> {
        *self.cells.get(row as usize)?.get(col as usize)?
    }

    /// Lädt die Konfiguration neu, wenn sich Datei oder Größe geändert haben (alle 2 s geprüft).
    pub fn update(&mut self, cols: u16, rows: u16) -> bool {
        let resized = self.size != (cols, rows);
        if !resized
            && self
                .checked
                .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            return false;
        }
        self.checked = Some(Instant::now());
        let Ok(config) = WallpaperConfig::load() else {
            return self.clear();
        };
        let Some(path) = config.path.clone() else {
            return self.clear();
        };
        let modified = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let config_time = WallpaperConfig::file().ok().and_then(|p| modified(&p));
        let stamp = (
            path.clone(),
            modified(&path).max(config_time),
            config.dim.to_bits(),
        );
        if !resized && self.stamp.as_ref() == Some(&stamp) {
            return false;
        }
        self.size = (cols, rows);
        self.stamp = Some(stamp);
        self.cells = build(&config, &path, cols, rows).unwrap_or_default();
        true
    }

    fn clear(&mut self) -> bool {
        let had = !self.cells.is_empty();
        self.cells.clear();
        self.stamp = None;
        had
    }
}

fn build(
    config: &WallpaperConfig,
    path: &std::path::Path,
    cols: u16,
    rows: u16,
) -> anyhow::Result<Grid> {
    let mut img = tuilnux_media::load_image(path)?;
    let (cols, rows) = (cols.max(1), rows.max(1));
    let (w, h, x0, y0) = match config.mode {
        FitMode::Fill => {
            // Auf das Seitenverhältnis des Terminals zuschneiden (Zelle = 1×2 Pixel).
            let target = cols as f64 / (rows as f64 * 2.0);
            let (iw, ih) = (img.width() as f64, img.height() as f64);
            if iw / ih > target {
                let nw = (ih * target).round() as u32;
                img = img.crop_imm((iw as u32 - nw) / 2, 0, nw.max(1), ih as u32);
            } else {
                let nh = (iw / target).round() as u32;
                img = img.crop_imm(0, (ih as u32 - nh) / 2, iw as u32, nh.max(1));
            }
            (cols, rows, 0, 0)
        }
        FitMode::Fit | FitMode::Center => {
            let (w, h) = tuilnux_media::fit_cells(img.width(), img.height(), cols, rows);
            let (w, h) = if config.mode == FitMode::Fit {
                // Fit darf vergrößern, Center behält höchstens Originalgröße.
                let scale = (cols as f64 / w as f64).min(rows as f64 / h as f64);
                (
                    ((w as f64 * scale) as u16).clamp(1, cols),
                    ((h as f64 * scale) as u16).clamp(1, rows),
                )
            } else {
                (w, h)
            };
            (w, h, (cols - w) / 2, (rows - h) / 2)
        }
    };
    let keep = (1.0 - config.dim.clamp(0.0, 1.0)) as f64;
    let dim = |(r, g, b): Rgb| {
        let f = |c: u8| (c as f64 * keep).round() as u8;
        (f(r), f(g), f(b))
    };
    let cells = tuilnux_media::halfblock_cells(&img, w, h);
    let mut out = vec![vec![None; cols as usize]; rows as usize];
    for (y, line) in cells.into_iter().enumerate() {
        for (x, (top, bottom)) in line.into_iter().enumerate() {
            out[y0 as usize + y][x0 as usize + x] = Some((dim(top), dim(bottom)));
        }
    }
    Ok(out)
}

fn avg((r1, g1, b1): Rgb, (r2, g2, b2): Rgb) -> Color {
    let m = |a: u8, b: u8| ((a as u16 + b as u16) / 2) as u8;
    Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
}

/// Überträgt den sichtbaren Bildschirm in `area`. Gibt die Cursorposition zurück, falls sichtbar.
pub fn render_screen(
    screen: &vt100::Screen,
    wallpaper: Option<&Wallpaper>,
    area: Rect,
    buf: &mut Buffer,
) -> Option<Position> {
    let (rows, cols) = screen.size();
    let (rows, cols) = (rows.get().min(area.height), cols.get().min(area.width));
    for row in 0..rows {
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            let pos = (area.x + col, area.y + row);
            if cell.is_wide_continuation() {
                buf[pos].set_symbol("");
                continue;
            }
            let mut fg = color(cell.fgcolor());
            let mut bg = color(cell.bgcolor());
            let mut mods = Modifier::empty();
            if cell.bold() {
                mods |= Modifier::BOLD;
            }
            if cell.dim() {
                mods |= Modifier::DIM;
            }
            if cell.italic() {
                mods |= Modifier::ITALIC;
            }
            if cell.underline() {
                mods |= Modifier::UNDERLINED;
            }
            if cell.inverse() {
                mods |= Modifier::REVERSED;
            }
            let mut symbol = cell.contents();
            if symbol.is_empty() {
                symbol = " ";
            }
            let paper = wallpaper.and_then(|w| w.cell(col, row));
            if let (vt100::Color::Default, false, Some((top, bottom))) =
                (cell.bgcolor(), cell.inverse(), paper)
            {
                if symbol == " " {
                    symbol = "▀";
                    fg = Color::Rgb(top.0, top.1, top.2);
                    bg = Color::Rgb(bottom.0, bottom.1, bottom.2);
                } else {
                    bg = avg(top, bottom);
                }
            }
            let c = &mut buf[pos];
            c.set_symbol(symbol);
            c.set_style(Style::new().fg(fg).bg(bg).add_modifier(mods));
        }
    }
    let (cr, cc) = screen.cursor_position();
    (screen.scrollback() == 0 && !screen.hide_cursor() && cr < rows && cc < cols)
        .then(|| Position::new(area.x + cc, area.y + cr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU16;

    fn parser(rows: u16, cols: u16) -> vt100::Parser {
        vt100::Parser::new(
            NonZeroU16::new(rows).unwrap(),
            NonZeroU16::new(cols).unwrap(),
            100,
        )
    }

    #[test]
    fn renders_text_colors_and_wide_chars() {
        let mut p = parser(3, 10);
        p.process(b"\x1b[1;31mrot\x1b[0m \xe5\xad\x97x\r\n\x1b[48;2;1;2;3mbg");
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        let cursor = render_screen(p.screen(), None, area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "r");
        assert_eq!(buf[(0, 0)].fg, Color::Indexed(1));
        assert!(buf[(0, 0)].modifier.contains(Modifier::BOLD));
        assert_eq!(buf[(4, 0)].symbol(), "字");
        assert_eq!(buf[(6, 0)].symbol(), "x");
        assert_eq!(buf[(0, 1)].bg, Color::Rgb(1, 2, 3));
        assert_eq!(cursor, Some(Position::new(2, 1)));
        p.process(b"\x1b[?25l");
        assert_eq!(render_screen(p.screen(), None, area, &mut buf), None);
    }

    #[test]
    fn wallpaper_shows_behind_default_background() {
        let mut p = parser(1, 3);
        p.process(b"a");
        let wall = Wallpaper {
            cells: vec![vec![Some(((10, 10, 10), (30, 30, 30))); 3]],
            ..Default::default()
        };
        let area = Rect::new(0, 0, 3, 1);
        let mut buf = Buffer::empty(area);
        render_screen(p.screen(), Some(&wall), area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "a");
        assert_eq!(buf[(0, 0)].bg, Color::Rgb(20, 20, 20));
        assert_eq!(buf[(1, 0)].symbol(), "▀");
        assert_eq!(buf[(1, 0)].fg, Color::Rgb(10, 10, 10));
    }
}
