//! ttop – Systemmonitor und Taskmanager von TUILNUX.

use std::collections::VecDeque;
use std::io;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Clear, Paragraph, Row, Sparkline, Table, TableState};
use ratatui::{DefaultTerminal, Frame};
use sysinfo::{Disks, Pid, ProcessesToUpdate, System, Users};
use tuilnux_ui::{
    App, Flow, InputResult, LineInput, centered_rect, frame_block, human_size, is_ctrl, keybar,
    status_line, theme,
};

const HISTORY: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Cpu,
    Memory,
    Pid,
    Name,
}

impl SortKey {
    fn label(self) -> &'static str {
        match self {
            SortKey::Cpu => "CPU",
            SortKey::Memory => "RAM",
            SortKey::Pid => "PID",
            SortKey::Name => "Name",
        }
    }
}

struct ProcRow {
    pid: Pid,
    name: String,
    user: String,
    cpu: f32,
    memory: u64,
    status: String,
    run_time: u64,
}

struct Monitor {
    sys: System,
    disks: Option<Disks>,
    users: Option<Users>,
    cpu_history: VecDeque<u64>,
    mem_history: VecDeque<u64>,
    rows: Vec<ProcRow>,
    table: TableState,
    sort: SortKey,
    descending: bool,
    filter: String,
    filter_input: Option<LineInput>,
    confirm_kill: Option<(Pid, String)>,
    paused: bool,
    message: Option<(String, bool)>,
    table_area: Rect,
    host: String,
}

fn bar(ratio: f64, width: usize) -> (String, String) {
    let filled = ((ratio.clamp(0.0, 1.0) * width as f64).round() as usize).min(width);
    ("█".repeat(filled), "░".repeat(width - filled))
}

fn load_color(ratio: f64) -> Color {
    if ratio >= 0.85 {
        theme::error()
    } else if ratio >= 0.6 {
        theme::warn()
    } else {
        theme::accent()
    }
}

fn gauge_line(label: &str, ratio: f64, text: String, width: u16) -> Line<'static> {
    let bar_width =
        (width as usize).saturating_sub(label.chars().count() + text.chars().count() + 3);
    let (full, empty) = bar(ratio, bar_width);
    Line::from(vec![
        Span::styled(format!("{label} "), Style::new().fg(theme::fg())),
        Span::styled(full, Style::new().fg(load_color(ratio))),
        Span::styled(empty, Style::new().fg(theme::dim())),
        Span::styled(format!(" {text}"), Style::new().fg(theme::fg())),
    ])
}

fn format_duration(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    if d > 0 {
        format!("{d}d {h:02}:{m:02}")
    } else {
        format!("{h:02}:{m:02}:{s:02}")
    }
}

fn push_history(history: &mut VecDeque<u64>, value: u64) {
    if history.len() == HISTORY {
        history.pop_front();
    }
    history.push_back(value);
}

impl Monitor {
    fn new() -> anyhow::Result<Self> {
        let mut sys =
            System::new_all().map_err(|e| anyhow::anyhow!("Systemdaten nicht lesbar: {e:?}"))?;
        // CPU-Last braucht zwei Messungen im Abstand von mindestens MINIMUM_CPU_UPDATE_INTERVAL.
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        sys.refresh_cpu_usage();
        sys.refresh_processes(ProcessesToUpdate::All, true);
        let mut monitor = Self {
            sys,
            disks: Disks::new_with_refreshed_list().ok(),
            users: Users::new_with_refreshed_list().ok(),
            cpu_history: VecDeque::with_capacity(HISTORY),
            mem_history: VecDeque::with_capacity(HISTORY),
            rows: Vec::new(),
            table: TableState::default().with_selected(0),
            sort: SortKey::Cpu,
            descending: true,
            filter: String::new(),
            filter_input: None,
            confirm_kill: None,
            paused: false,
            message: None,
            table_area: Rect::default(),
            host: System::host_name().unwrap_or_else(|_| "tuilnux".into()),
        };
        monitor.record();
        monitor.rebuild_rows();
        Ok(monitor)
    }

    fn refresh(&mut self) {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.sys.refresh_processes(ProcessesToUpdate::All, true);
        if let Some(disks) = &mut self.disks {
            disks.refresh(true);
        }
        self.record();
        self.rebuild_rows();
    }

    fn record(&mut self) {
        push_history(
            &mut self.cpu_history,
            self.sys.global_cpu_usage().round() as u64,
        );
        let total = self.sys.total_memory().max(1);
        push_history(&mut self.mem_history, self.sys.used_memory() * 100 / total);
    }

    fn selected_pid(&self) -> Option<Pid> {
        self.table
            .selected()
            .and_then(|i| self.rows.get(i))
            .map(|r| r.pid)
    }

    fn rebuild_rows(&mut self) {
        let keep = self.selected_pid();
        let needle = self.filter.to_lowercase();
        let users = self.users.as_ref();
        let mut rows: Vec<ProcRow> = self
            .sys
            .processes()
            .values()
            .filter(|p| p.thread_kind().is_none())
            .map(|p| ProcRow {
                pid: p.pid(),
                name: p.name().to_string_lossy().into_owned(),
                user: p
                    .user_id()
                    .and_then(|uid| users.and_then(|u| u.get_user_by_id(uid)))
                    .map_or_else(|| "-".into(), |u| u.name().to_string()),
                cpu: p.cpu_usage(),
                memory: p.memory(),
                status: p.status().to_string(),
                run_time: p.run_time(),
            })
            .filter(|r| {
                needle.is_empty()
                    || r.name.to_lowercase().contains(&needle)
                    || r.pid.to_string() == needle
                    || r.user.to_lowercase().contains(&needle)
            })
            .collect();
        rows.sort_by(|a, b| {
            let ord = match self.sort {
                SortKey::Cpu => a.cpu.total_cmp(&b.cpu),
                SortKey::Memory => a.memory.cmp(&b.memory),
                SortKey::Pid => a.pid.cmp(&b.pid),
                SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            };
            if self.descending { ord.reverse() } else { ord }
        });
        self.rows = rows;
        let index = keep
            .and_then(|pid| self.rows.iter().position(|r| r.pid == pid))
            .or_else(|| {
                self.table
                    .selected()
                    .map(|i| i.min(self.rows.len().saturating_sub(1)))
            });
        self.table.select(if self.rows.is_empty() {
            None
        } else {
            index.or(Some(0))
        });
    }

    fn set_sort(&mut self, key: SortKey) {
        if self.sort == key {
            self.descending = !self.descending;
        } else {
            self.sort = key;
            // Zahlen standardmäßig absteigend, Text aufsteigend.
            self.descending = matches!(key, SortKey::Cpu | SortKey::Memory);
        }
        self.rebuild_rows();
    }

    fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let current = self.table.selected().unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, self.rows.len() as isize - 1);
        self.table.select(Some(next as usize));
    }

    fn kill(&mut self, pid: Pid, name: &str) {
        match self.sys.process(pid) {
            Some(p) if p.kill() => {
                self.message = Some((format!("Prozess {name} ({pid}) beendet"), false));
                self.refresh();
            }
            Some(_) => {
                self.message = Some((
                    format!("Keine Berechtigung, {name} ({pid}) zu beenden"),
                    true,
                ));
            }
            None => self.message = Some((format!("Prozess {pid} existiert nicht mehr"), true)),
        }
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let uptime = System::uptime()
            .map(format_duration)
            .unwrap_or_else(|_| "?".into());
        let load = System::load_average()
            .map(|l| format!("  Last {:.2} {:.2} {:.2}", l.one, l.five, l.fifteen))
            .unwrap_or_default();
        let os = System::long_os_version().unwrap_or_default();
        let line = Line::from(vec![
            Span::styled(" ttop ", Style::new().fg(Color::Black).bg(theme::accent())),
            Span::styled(
                format!(" {} ", self.host),
                Style::new().fg(theme::fg()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("· {os} · up {uptime}{load}"),
                Style::new().fg(theme::dim()),
            ),
            Span::styled(
                if self.paused { "  [PAUSIERT]" } else { "" },
                Style::new().fg(theme::warn()).add_modifier(Modifier::BOLD),
            ),
        ]);
        frame.render_widget(line, area);
    }

    fn draw_cpu(&self, frame: &mut Frame, area: Rect) {
        let block = frame_block("CPU", false);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let cpus = self.sys.cpus();
        let total = self.sys.global_cpu_usage() as f64 / 100.0;
        let brand = cpus
            .first()
            .map(|c| c.brand().trim().to_string())
            .unwrap_or_default();

        let [left, right] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .areas(inner);
        let columns = if cpus.len() > (left.height as usize).saturating_sub(1) {
            2
        } else {
            1
        };
        let col_width = left.width / columns as u16;
        let mut lines = vec![gauge_line(
            "Σ  ",
            total,
            format!("{:5.1}%", total * 100.0),
            left.width,
        )];
        let per_col = cpus.len().div_ceil(columns);
        for row in 0..per_col {
            let mut spans = Vec::new();
            for col in 0..columns {
                if let Some(cpu) = cpus.get(col * per_col + row) {
                    let ratio = cpu.usage() as f64 / 100.0;
                    let label = format!("{:<3}", col * per_col + row);
                    spans.extend(
                        gauge_line(&label, ratio, format!("{:5.1}%", cpu.usage()), col_width).spans,
                    );
                }
            }
            lines.push(Line::from(spans));
        }
        frame.render_widget(Paragraph::new(lines), left);

        let [title, spark] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(right);
        frame.render_widget(
            Line::from(Span::styled(
                format!(" {brand} · {} Kerne", cpus.len()),
                Style::new().fg(theme::dim()),
            )),
            title,
        );
        let data: Vec<u64> = self
            .cpu_history
            .iter()
            .rev()
            .take(spark.width as usize)
            .rev()
            .copied()
            .collect();
        frame.render_widget(
            Sparkline::default()
                .data(&data)
                .max(100)
                .style(Style::new().fg(theme::accent())),
            spark,
        );
    }

    fn draw_memory(&self, frame: &mut Frame, area: Rect) {
        let block = frame_block("Speicher & Datenträger", false);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let (used, total) = (self.sys.used_memory(), self.sys.total_memory());
        let (swap_used, swap_total) = (self.sys.used_swap(), self.sys.total_swap());
        let ratio = |a: u64, b: u64| if b == 0 { 0.0 } else { a as f64 / b as f64 };
        let mut lines = vec![
            gauge_line(
                "RAM ",
                ratio(used, total),
                format!("{} / {}", human_size(used), human_size(total)),
                inner.width,
            ),
            gauge_line(
                "Swap",
                ratio(swap_used, swap_total),
                format!("{} / {}", human_size(swap_used), human_size(swap_total)),
                inner.width,
            ),
        ];
        if let Some(disks) = &self.disks {
            for disk in disks.iter().filter(|d| d.total_space() > 0) {
                let total = disk.total_space();
                let used = total - disk.available_space().min(total);
                let mount = disk.mount_point().display().to_string();
                let label: String = format!("{mount:<4}").chars().take(12).collect();
                lines.push(gauge_line(
                    &label,
                    ratio(used, total),
                    format!("{} / {}", human_size(used), human_size(total)),
                    inner.width,
                ));
            }
        }
        frame.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_processes(&mut self, frame: &mut Frame, area: Rect) {
        let title = format!(
            "Prozesse ({}) · sortiert nach {} {}{}",
            self.rows.len(),
            self.sort.label(),
            if self.descending { "▼" } else { "▲" },
            if self.filter.is_empty() {
                String::new()
            } else {
                format!(" · Filter „{}“", self.filter)
            },
        );
        let block = frame_block(&title, true);
        self.table_area = block.inner(area);
        let header_style = Style::new()
            .fg(theme::accent2())
            .add_modifier(Modifier::BOLD);
        let header = Row::new(
            [
                "PID", "Name", "Benutzer", "CPU %", "RAM", "Status", "Laufzeit",
            ]
            .map(|h| Cell::from(h).style(header_style)),
        );
        let total_mem = self.sys.total_memory().max(1) as f64;
        let rows = self.rows.iter().map(|r| {
            let cpu_style = Style::new().fg(load_color(r.cpu as f64 / 100.0));
            let mem_style = Style::new().fg(load_color(r.memory as f64 / total_mem * 4.0));
            Row::new([
                Cell::from(r.pid.to_string()).style(Style::new().fg(theme::dim())),
                Cell::from(r.name.clone()).style(Style::new().fg(theme::fg())),
                Cell::from(r.user.clone()).style(Style::new().fg(theme::dim())),
                Cell::from(format!("{:>6.1}", r.cpu)).style(cpu_style),
                Cell::from(format!("{:>9}", human_size(r.memory))).style(mem_style),
                Cell::from(r.status.clone()).style(Style::new().fg(theme::dim())),
                Cell::from(format_duration(r.run_time)).style(Style::new().fg(theme::dim())),
            ])
        });
        let table = Table::new(
            rows,
            [
                Constraint::Length(8),
                Constraint::Fill(1),
                Constraint::Length(12),
                Constraint::Length(7),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(11),
            ],
        )
        .header(header)
        .block(block)
        .row_highlight_style(
            Style::new()
                .bg(theme::sel_bg())
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
        frame.render_stateful_widget(table, area, &mut self.table);
    }

    fn draw_confirm(&self, frame: &mut Frame, pid: Pid, name: &str) {
        let text = format!("Prozess „{name}“ (PID {pid}) beenden?");
        let width = (text.chars().count() as u16 + 6).max(36);
        let area = centered_rect(width, 5, frame.area());
        frame.render_widget(Clear, area);
        let block = frame_block("Beenden", true).border_style(Style::new().fg(theme::error()));
        let lines = vec![
            Line::from(Span::styled(text, Style::new().fg(theme::fg()))),
            Line::from(""),
            Line::from(vec![
                Span::styled(" j ", Style::new().fg(Color::Black).bg(theme::error())),
                Span::raw(" Ja   "),
                Span::styled(" n ", Style::new().fg(Color::Black).bg(theme::dim())),
                Span::raw(" Nein"),
            ]),
        ];
        frame.render_widget(Paragraph::new(lines).block(block).centered(), area);
    }
}

impl App for Monitor {
    fn draw(&mut self, frame: &mut Frame) {
        let disk_count = self
            .disks
            .as_ref()
            .map_or(0, |d| d.iter().filter(|d| d.total_space() > 0).count());
        let cpu_rows = (self.sys.cpus().len().div_ceil(2) as u16 + 3).clamp(6, 12);
        let [header, top, procs, status, keys] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(cpu_rows.max(disk_count as u16 + 4).min(14)),
            Constraint::Min(5),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let [cpu, mem] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(top);

        self.draw_header(frame, header);
        self.draw_cpu(frame, cpu);
        self.draw_memory(frame, mem);
        self.draw_processes(frame, procs);

        if let Some(input) = &self.filter_input {
            input.render(frame, status, " Filter (Name, PID, Benutzer): ");
        } else {
            let left = match &self.message {
                Some((msg, _)) => format!(" {msg}"),
                None => " Enter = Details · Maus: Klick wählt, Rad scrollt".to_string(),
            };
            let mut line = status_line(
                left,
                format!("{} Prozesse ", self.sys.processes().len()),
                status.width,
            );
            if self.message.as_ref().is_some_and(|(_, err)| *err) {
                line.spans[0].style = Style::new().fg(theme::error());
            }
            frame.render_widget(line, status);
        }

        frame.render_widget(
            keybar(&[
                ("c", "CPU"),
                ("m", "RAM"),
                ("p", "PID"),
                ("n", "Name"),
                ("/", "Filter"),
                ("k", "Beenden"),
                ("␣", "Pause"),
                ("q", "Quit"),
            ]),
            keys,
        );

        if let Some((pid, name)) = &self.confirm_kill {
            self.draw_confirm(frame, *pid, name);
        }
    }

    fn on_key(&mut self, key: KeyEvent, _terminal: &mut DefaultTerminal) -> io::Result<Flow> {
        if let Some(mut input) = self.filter_input.take() {
            let result = input.handle_key(key);
            match result {
                InputResult::Submit => self.filter = input.text().trim().to_string(),
                InputResult::Cancel => {}
                InputResult::Continue => {
                    self.filter = input.text().trim().to_string();
                    self.filter_input = Some(input);
                }
            }
            self.rebuild_rows();
            return Ok(Flow::Continue);
        }

        if let Some((pid, name)) = self.confirm_kill.take() {
            if matches!(
                key.code,
                KeyCode::Char('j' | 'y' | 'J' | 'Y') | KeyCode::Enter
            ) {
                self.kill(pid, &name);
            }
            return Ok(Flow::Continue);
        }

        self.message = None;
        let page = self.table_area.height.saturating_sub(2).max(1) as isize;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::F(10) => return Ok(Flow::Quit),
            _ if is_ctrl(&key, 'c') => return Ok(Flow::Quit),
            KeyCode::Char('c') | KeyCode::Char('P') => self.set_sort(SortKey::Cpu),
            KeyCode::Char('m') | KeyCode::Char('M') => self.set_sort(SortKey::Memory),
            KeyCode::Char('p') => self.set_sort(SortKey::Pid),
            KeyCode::Char('n') | KeyCode::Char('N') => self.set_sort(SortKey::Name),
            KeyCode::Char('/') | KeyCode::F(4) => {
                self.filter_input = Some(LineInput::new(self.filter.clone()));
            }
            KeyCode::Char(' ') => self.paused = !self.paused,
            KeyCode::Char('r') | KeyCode::F(5) => self.refresh(),
            KeyCode::Char('k') | KeyCode::Delete | KeyCode::F(9) => {
                if let Some(row) = self.table.selected().and_then(|i| self.rows.get(i)) {
                    self.confirm_kill = Some((row.pid, row.name.clone()));
                }
            }
            KeyCode::Enter => {
                if let Some(p) = self.selected_pid().and_then(|pid| self.sys.process(pid)) {
                    let cmd: Vec<String> = p
                        .cmd()
                        .iter()
                        .map(|s| s.to_string_lossy().into_owned())
                        .collect();
                    let detail = if cmd.is_empty() {
                        p.exe()
                            .map(|e| e.display().to_string())
                            .unwrap_or_else(|| "(keine Details)".into())
                    } else {
                        cmd.join(" ")
                    };
                    self.message = Some((format!("{}: {detail}", p.pid()), false));
                }
            }
            KeyCode::Up | KeyCode::Char('K') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-page),
            KeyCode::PageDown => self.move_selection(page),
            KeyCode::Home | KeyCode::Char('g') => self.table.select(Some(0)),
            KeyCode::End | KeyCode::Char('G') => {
                self.table.select(self.rows.len().checked_sub(1));
            }
            _ => {}
        }
        Ok(Flow::Continue)
    }

    fn on_mouse(&mut self, mouse: MouseEvent) -> Flow {
        if self.confirm_kill.is_some() || self.filter_input.is_some() {
            return Flow::Continue;
        }
        let area = self.table_area;
        match mouse.kind {
            MouseEventKind::ScrollUp => self.move_selection(-3),
            MouseEventKind::ScrollDown => self.move_selection(3),
            MouseEventKind::Down(MouseButton::Left)
                if mouse.row > area.y && mouse.row < area.bottom() && mouse.column >= area.x =>
            {
                // Zeile 0 der Tabelle ist die Kopfzeile.
                let index = self.table.offset() + (mouse.row - area.y - 1) as usize;
                if index < self.rows.len() {
                    self.table.select(Some(index));
                }
            }
            _ => {}
        }
        Flow::Continue
    }

    fn tick(&mut self) {
        if !self.paused {
            self.refresh();
        }
    }

    fn tick_rate(&self) -> Duration {
        Duration::from_millis(1000)
    }
}

fn main() -> anyhow::Result<()> {
    if std::env::args()
        .nth(1)
        .is_some_and(|a| a == "-h" || a == "--help")
    {
        println!("ttop – Systemmonitor und Taskmanager von TUILNUX");
        println!(
            "Tasten: c/m/p/n sortieren · / filtern · k beenden · Leertaste pausieren · q beenden"
        );
        return Ok(());
    }
    let mut monitor = Monitor::new()?;
    tuilnux_ui::run(&mut monitor)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn renders_at_various_sizes() {
        let mut monitor = Monitor::new().unwrap();
        assert!(!monitor.rows.is_empty());
        for (w, h) in [(120, 40), (80, 24), (40, 12), (10, 4)] {
            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
            t.draw(|f| monitor.draw(f)).unwrap();
        }
        monitor.set_sort(SortKey::Name);
        assert!(
            monitor
                .rows
                .windows(2)
                .all(|w| w[0].name.to_lowercase() <= w[1].name.to_lowercase())
        );
        monitor.filter = "zzz-gibt-es-nicht".into();
        monitor.rebuild_rows();
        assert!(monitor.rows.is_empty());
        let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
        t.draw(|f| monitor.draw(f)).unwrap();
    }
}
