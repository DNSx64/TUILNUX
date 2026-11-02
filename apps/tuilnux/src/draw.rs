//! Zeichnen der Sitzung: Desktop-Terminal, schwebende Fenster, Statusleiste, Overlays.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Row, Table, Widget};
use tuilnux_ipc::TaskInfo;
use tuilnux_status::Bar;
use tuilnux_term::vt100::Screen;
use tuilnux_term::{Wallpaper, render_screen};
use tuilnux_ui::theme;
use tuilnux_window_manager::{Button, Focus, Window, Wm};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    Tasks { selected: usize },
    Help,
    Logout,
}

pub struct View<'a> {
    pub wm: &'a Wm,
    pub desktop: &'a Screen,
    pub windows: Vec<(usize, &'a Screen)>,
    pub wallpaper: Option<&'a Wallpaper>,
    pub bar: Bar<'a>,
    pub overlay: Option<Overlay>,
    pub tasks: &'a [TaskInfo],
    pub prefix: &'a str,
    pub ascii: bool,
}

pub fn desktop_area(size: Rect) -> Rect {
    Rect {
        height: size.height.saturating_sub(1),
        ..size
    }
}

pub fn bar_area(size: Rect) -> Rect {
    Rect::new(
        size.x,
        size.bottom().saturating_sub(1),
        size.width,
        size.height.min(1),
    )
}

pub fn draw(frame: &mut Frame, view: View) {
    let size = frame.area();
    let area = desktop_area(size);
    let buf = frame.buffer_mut();
    let mut cursor = render_screen(view.desktop, view.wallpaper, area, buf)
        .filter(|_| view.wm.focus() == Focus::Desktop);
    for win in view.wm.visible() {
        let focused = view.wm.focus() == Focus::Window(win.id);
        draw_frame(buf, win, focused, view.ascii);
        if let Some((_, screen)) = view.windows.iter().find(|(id, _)| *id == win.id) {
            let pos = render_screen(screen, None, win.inner(), buf);
            if focused {
                cursor = pos;
            }
        }
    }
    if size.height > 0 {
        view.bar.render(bar_area(size), buf);
    }
    if let Some(overlay) = view.overlay {
        draw_overlay(buf, area, overlay, view.tasks, view.prefix, view.ascii);
        cursor = None;
    }
    if let Some(pos) = cursor {
        frame.set_cursor_position(pos);
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else if max == 0 {
        String::new()
    } else {
        let mut t: String = s.chars().take(max - 1).collect();
        t.push('…');
        t
    }
}

fn put(buf: &mut Buffer, x: u16, y: u16, symbol: &str, style: Style) {
    if buf.area.contains(Position::new(x, y)) {
        buf[(x, y)].set_symbol(symbol).set_style(style);
    }
}

/// Fensterrahmen `┌ 2 tfiles ───── _ □ x ┐` samt leerem Innenbereich.
pub fn draw_frame(buf: &mut Buffer, win: &Window, focused: bool, ascii: bool) {
    let r = win.rect.intersection(buf.area);
    if r.width < 2 || r.height < 2 {
        return;
    }
    Clear.render(r, buf);
    let color = if focused {
        theme::accent()
    } else {
        theme::dim()
    };
    let border = Style::new().fg(color).bg(Color::Reset);
    let (tl, tr, bl, br, h, v) = if ascii {
        ("+", "+", "+", "+", "-", "|")
    } else {
        ("┌", "┐", "└", "┘", "─", "│")
    };
    let (right, bottom) = (r.right() - 1, r.bottom() - 1);
    for x in r.x..=right {
        put(buf, x, r.y, h, border);
        put(buf, x, bottom, h, border);
    }
    for y in r.y..=bottom {
        put(buf, r.x, y, v, border);
        put(buf, right, y, v, border);
    }
    put(buf, r.x, r.y, tl, border);
    put(buf, right, r.y, tr, border);
    put(buf, r.x, bottom, bl, border);
    put(buf, right, bottom, br, border);

    let room = r.width.saturating_sub(12) as usize;
    if room >= 3 {
        let title = format!(" {} {} ", win.id, truncate(&win.title, room - 2));
        let style = if focused {
            Style::new()
                .fg(Color::Black)
                .bg(theme::accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme::fg())
        };
        buf.set_string(r.x + 2, r.y, title, style);
    }
    if r.width >= 10 {
        for button in Button::ALL {
            let (symbol, style) = match button {
                Button::Minimize => ("_", border),
                Button::Maximize if win.maximized => (if ascii { "o" } else { "▫" }, border),
                Button::Maximize => (if ascii { "O" } else { "□" }, border),
                Button::Close => (
                    "x",
                    if focused {
                        Style::new().fg(theme::error())
                    } else {
                        border
                    },
                ),
            };
            let x = button.x(win.rect);
            put(buf, x - 1, r.y, " ", border);
            put(buf, x, r.y, symbol, style.add_modifier(Modifier::BOLD));
        }
        put(buf, Button::Close.x(win.rect) + 1, r.y, " ", border);
    }
}

pub fn overlay_rect(area: Rect, overlay: Overlay, tasks: usize) -> Rect {
    let (w, h) = match overlay {
        Overlay::Tasks { .. } => (86, tasks as u16 + 5),
        Overlay::Help => (64, HELP.len() as u16 + 4),
        Overlay::Logout => (50, 5),
    };
    let (w, h) = (w.min(area.width), h.min(area.height));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// Welche Zeile der Taskliste liegt bei `y`? (Rahmen und Kopfzeile abgezogen.)
pub fn task_row_at(rect: Rect, y: u16, tasks: usize) -> Option<usize> {
    let row = y.checked_sub(rect.y + 2)? as usize;
    (row < tasks && y < rect.bottom().saturating_sub(2)).then_some(row)
}

pub fn format_memory(bytes: u64) -> String {
    let mib = bytes as f64 / (1024.0 * 1024.0);
    if mib >= 1024.0 {
        format!("{:.1}G", mib / 1024.0)
    } else {
        format!("{mib:.0}M")
    }
}

pub const HELP: &[(&str, &str)] = &[
    ("Tab", "nächstes Fenster (Desktop → 1 → 2 …)"),
    ("1 … 9", "Fenster mit dieser Nummer"),
    ("0 / d", "Desktop (Vollbild-Terminal)"),
    ("Enter / n", "neues Terminal-Fenster"),
    ("a / Super+F", "Programme suchen und starten"),
    ("t", "Taskmanager"),
    ("s", "Einstellungen (tsettings)"),
    ("x", "Fenster schließen"),
    ("m", "minimieren"),
    ("f", "maximieren / wiederherstellen"),
    ("← ↑ → ↓", "Fenster verschieben"),
    ("Shift+Pfeile", "Größe ändern"),
    ("q", "abmelden"),
    ("Präfix 2×", "Präfix an das Programm senden"),
    ("Maus", "Titel ziehen, Rand/Ecken = Größe, _ □ x"),
    ("Strg+Alt+1…9", "ohne Präfix: Fenster holen (0 = Desktop)"),
    ("tab NAME", "minimierten Task aus der Shell holen"),
    ("tuilnux open", "Programm aus der Shell als Fenster öffnen"),
];

fn overlay_block(title: &str, ascii: bool) -> Block<'_> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(if ascii {
            BorderType::Plain
        } else {
            BorderType::Rounded
        })
        .border_style(Style::new().fg(theme::accent2()))
        .title(Span::styled(
            format!(" {title} "),
            Style::new()
                .fg(theme::accent2())
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::new().bg(theme::bar_bg()).fg(theme::fg()))
}

fn draw_overlay(
    buf: &mut Buffer,
    area: Rect,
    overlay: Overlay,
    tasks: &[TaskInfo],
    prefix: &str,
    ascii: bool,
) {
    let rect = overlay_rect(area, overlay, tasks.len());
    Clear.render(rect, buf);
    let dim = Style::new().fg(theme::dim());
    match overlay {
        Overlay::Tasks { selected } => {
            let block = overlay_block("Taskmanager", ascii).title_bottom(Line::from(vec![
                Span::styled(" Enter", Style::new().fg(theme::accent())),
                Span::styled(" fokus  ", dim),
                Span::styled("m", Style::new().fg(theme::accent())),
                Span::styled(" min  ", dim),
                Span::styled("x", Style::new().fg(theme::accent())),
                Span::styled(" schließen  ", dim),
                Span::styled("n", Style::new().fg(theme::accent())),
                Span::styled(" neu  ", dim),
                Span::styled("Esc", Style::new().fg(theme::accent())),
                Span::styled(" zurück ", dim),
            ]));
            let rows = tasks.iter().enumerate().map(|(i, t)| {
                let status = match (t.id, t.focused, t.minimized) {
                    (_, true, _) => "aktiv",
                    _ if t.background => "Hintergrund",
                    (_, _, true) => "minimiert",
                    (0, ..) => "Desktop",
                    _ => "offen",
                };
                let style = if i == selected {
                    Style::new()
                        .bg(theme::sel_bg())
                        .fg(theme::accent())
                        .add_modifier(Modifier::BOLD)
                } else if t.minimized {
                    dim
                } else {
                    Style::new()
                };
                let (command, cpu, memory) = if t.gui {
                    (format!("{} (GUI)", t.command), "-".into(), "-".into())
                } else {
                    (
                        t.command.clone(),
                        format!("{:.1}%", t.cpu),
                        format_memory(t.memory),
                    )
                };
                Row::new(vec![
                    t.id.to_string(),
                    t.title.clone(),
                    command,
                    t.pid.map_or("-".into(), |p| p.to_string()),
                    cpu,
                    memory,
                    status.to_string(),
                ])
                .style(style)
            });
            Table::new(
                rows,
                [
                    Constraint::Length(3),
                    Constraint::Fill(2),
                    Constraint::Fill(2),
                    Constraint::Length(7),
                    Constraint::Length(6),
                    Constraint::Length(6),
                    Constraint::Length(9),
                ],
            )
            .header(
                Row::new(["Nr", "Titel", "Befehl", "PID", "CPU", "RAM", "Status"])
                    .style(dim.add_modifier(Modifier::BOLD)),
            )
            .block(block)
            .render(rect, buf);
        }
        Overlay::Help => {
            let mut lines = vec![Line::from(vec![
                Span::styled("Manager-Modus: ", dim),
                Span::styled(prefix.to_string(), Style::new().fg(theme::accent())),
                Span::styled(", danach:", dim),
            ])];
            lines.extend(HELP.iter().map(|(k, d)| {
                Line::from(vec![
                    Span::styled(format!("  {k:<14}"), Style::new().fg(theme::accent())),
                    Span::raw(*d),
                ])
            }));
            Paragraph::new(lines)
                .block(overlay_block("Hilfe", ascii))
                .render(rect, buf);
        }
        Overlay::Logout => {
            Paragraph::new(vec![
                Line::from("Abmelden? Alle Fenster werden geschlossen."),
                Line::from(""),
                Line::from(vec![
                    Span::styled("  j", Style::new().fg(theme::accent())),
                    Span::styled(" ja    ", dim),
                    Span::styled("n / Esc", Style::new().fg(theme::accent())),
                    Span::styled(" abbrechen", dim),
                ]),
            ])
            .block(overlay_block("Abmelden", ascii))
            .render(rect, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tuilnux_status::{Stats, Task};
    use tuilnux_term::vt100::Parser;

    fn dump(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect()
            })
            .collect()
    }

    fn screen(text: &str, cols: u16, rows: u16) -> Parser {
        let mut p = Parser::new(tuilnux_term::nz(rows), tuilnux_term::nz(cols), 0);
        p.process(text.as_bytes());
        p
    }

    static BAR: std::sync::LazyLock<tuilnux_status::BarSettings> =
        std::sync::LazyLock::new(Default::default);

    fn render(wm: &Wm, overlay: Option<Overlay>, tasks: &[TaskInfo]) -> (Vec<String>, Position) {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let desktop = screen("user@arch ~ $ ", 80, 23);
        let win = wm.windows().first().map(|w| {
            let i = w.inner();
            screen("Dateien\r\n  Bilder/", i.width, i.height)
        });
        let stats = Stats::default();
        let bar_tasks: Vec<Task> = wm
            .by_id()
            .iter()
            .map(|w| Task {
                id: w.id,
                title: w.title.clone(),
                focused: wm.focused() == Some(w.id),
                minimized: w.minimized,
                gui: false,
            })
            .collect();
        term.draw(|f| {
            draw(
                f,
                View {
                    wm,
                    desktop: desktop.screen(),
                    windows: win
                        .as_ref()
                        .map(|p| (wm.windows()[0].id, p.screen()))
                        .into_iter()
                        .collect(),
                    wallpaper: None,
                    bar: Bar {
                        title: "bash",
                        desktop_focused: wm.focused().is_none(),
                        tasks: &bar_tasks,
                        stats: &stats,
                        mouse: !wm.is_empty(),
                        scrolled: 0,
                        message: None,
                        ascii: false,
                        options: &BAR,
                    },
                    overlay,
                    tasks,
                    prefix: "Strg+Leertaste",
                    ascii: false,
                },
            )
        })
        .unwrap();
        let cursor = term.get_cursor_position().unwrap();
        (dump(term.backend().buffer()), cursor)
    }

    #[test]
    fn desktop_only() {
        let wm = Wm::new(Rect::new(0, 0, 80, 23));
        let (lines, cursor) = render(&wm, None, &[]);
        assert!(lines[0].starts_with("user@arch ~ $"));
        assert!(lines[23].starts_with(" 1   bash "), "{}", lines[23]);
        assert_eq!(cursor, Position::new(14, 0));
    }

    #[test]
    fn window_frame_buttons_and_content() {
        let mut wm = Wm::new(Rect::new(0, 0, 80, 23));
        let id = wm.open("tfiles", Some((40, 8)));
        let (lines, cursor) = render(&wm, None, &[]);
        let r = wm.get(id).unwrap().rect;
        let top = &lines[r.y as usize];
        assert!(top.contains("┌─ 1 tfiles ─"), "{top}");
        assert!(top.contains("_ □ x ┐"), "{top}");
        assert!(lines[r.y as usize + 1].contains("│Dateien"));
        assert!(lines[r.bottom() as usize - 1].contains("└────"));
        assert!(lines[23].contains(" 1:tfiles "), "{}", lines[23]);
        // Der Cursor steht im fokussierten Fenster.
        assert_eq!(cursor, Position::new(r.x + 1 + 9, r.y + 2));
        // Der Desktop bleibt außerhalb des Fensters sichtbar.
        assert!(lines[0].starts_with("user@arch ~ $"));
    }

    #[test]
    fn task_manager_overlay() {
        let mut wm = Wm::new(Rect::new(0, 0, 80, 23));
        wm.open("ttop", None);
        let tasks = vec![
            TaskInfo {
                id: 0,
                title: "bash".into(),
                command: "bash".into(),
                pid: Some(10),
                focused: false,
                minimized: false,
                background: false,
                cpu: 0.0,
                memory: 4 << 20,
                gui: false,
            },
            TaskInfo {
                id: 1,
                title: "ttop".into(),
                command: "ttop".into(),
                pid: Some(11),
                focused: true,
                minimized: false,
                background: false,
                cpu: 3.5,
                memory: 20 << 20,
                gui: false,
            },
        ];
        let (lines, _) = render(&wm, Some(Overlay::Tasks { selected: 1 }), &tasks);
        let all = lines.join("\n");
        assert!(all.contains("Taskmanager"), "{all}");
        assert!(all.contains("PID"));
        assert!(all.contains("3.5%") && all.contains("20M") && all.contains("aktiv"));
        let rect = overlay_rect(
            desktop_area(Rect::new(0, 0, 80, 24)),
            Overlay::Tasks { selected: 0 },
            2,
        );
        assert_eq!(task_row_at(rect, rect.y + 2, 2), Some(0));
        assert_eq!(task_row_at(rect, rect.y + 3, 2), Some(1));
        assert_eq!(task_row_at(rect, rect.y + 1, 2), None);
        assert_eq!(task_row_at(rect, rect.y + 4, 2), None);

        let (lines, _) = render(&wm, Some(Overlay::Help), &tasks);
        assert!(lines.join("\n").contains("Strg+Leertaste"));
        let (lines, _) = render(&wm, Some(Overlay::Logout), &tasks);
        assert!(lines.join("\n").contains("Abmelden?"));
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        let mut term = Terminal::new(TestBackend::new(5, 2)).unwrap();
        let mut wm = Wm::new(Rect::new(0, 0, 5, 1));
        wm.open("x", None);
        let desktop = screen("", 5, 1);
        let stats = Stats::default();
        term.draw(|f| {
            draw(
                f,
                View {
                    wm: &wm,
                    desktop: desktop.screen(),
                    windows: Vec::new(),
                    wallpaper: None,
                    bar: Bar {
                        title: "bash",
                        desktop_focused: false,
                        tasks: &[],
                        stats: &stats,
                        mouse: true,
                        scrolled: 0,
                        message: None,
                        ascii: true,
                        options: &BAR,
                    },
                    overlay: Some(Overlay::Help),
                    tasks: &[],
                    prefix: "x",
                    ascii: true,
                },
            )
        })
        .unwrap();
    }
}
