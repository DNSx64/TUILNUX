//! Untere Statusleiste im i3bar-Stil: Workspace und Tasks links, Systemwerte rechts.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use chrono::Local;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use sysinfo::{
    CpuRefreshKind, MemoryRefreshKind, Networks, Pid, ProcessesToUpdate, RefreshKind, System,
};
use tuilnux_ui::theme;

pub use tuilnux_config::BarSettings;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    /// (Kürzel E/W, Adresse) der ersten aktiven Schnittstelle.
    pub net: Option<(char, String)>,
    pub battery: Option<u8>,
}

pub struct Monitor {
    sys: Option<System>,
    networks: Option<Networks>,
    last: Option<Instant>,
    pub stats: Stats,
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new()
    }
}

impl Monitor {
    pub fn new() -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::nothing().with_usage())
                .with_memory(MemoryRefreshKind::nothing().with_ram()),
        );
        Self {
            sys: sys.ok(),
            networks: Networks::new_with_refreshed_list().ok(),
            last: None,
            stats: Stats::default(),
        }
    }

    /// Aktualisiert höchstens einmal pro Sekunde; gibt `true` zurück, wenn neu gemessen wurde.
    pub fn refresh(&mut self) -> bool {
        if self
            .last
            .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
        {
            return false;
        }
        self.last = Some(Instant::now());
        if let Some(sys) = &mut self.sys {
            sys.refresh_cpu_usage();
            sys.refresh_memory();
            self.stats.cpu = sys.global_cpu_usage();
            self.stats.mem_used = sys.used_memory();
            self.stats.mem_total = sys.total_memory();
        }
        if let Some(networks) = &mut self.networks {
            networks.refresh(true);
        }
        self.stats.net = self.network();
        self.stats.battery = battery();
        true
    }

    /// CPU-Last (%) und Speicher (Bytes) der angegebenen Prozesse inklusive ihrer Kinder.
    pub fn processes(&mut self, pids: &[u32]) -> HashMap<u32, (f32, u64)> {
        let Some(sys) = &mut self.sys else {
            return HashMap::new();
        };
        sys.refresh_processes(ProcessesToUpdate::All, true);
        let mut children: HashMap<Pid, Vec<Pid>> = HashMap::new();
        for (pid, p) in sys.processes() {
            if let Some(parent) = p.parent() {
                children.entry(parent).or_default().push(*pid);
            }
        }
        pids.iter()
            .map(|&root| {
                let (mut cpu, mut mem) = (0.0, 0);
                let mut stack = vec![Pid::from_u32(root)];
                let mut seen = 0;
                while let Some(pid) = stack.pop() {
                    seen += 1;
                    if seen > 4096 {
                        break;
                    }
                    if let Some(p) = sys.process(pid) {
                        cpu += p.cpu_usage();
                        mem += p.memory();
                    }
                    if let Some(kids) = children.get(&pid) {
                        stack.extend(kids);
                    }
                }
                (root, (cpu, mem))
            })
            .collect()
    }

    fn network(&self) -> Option<(char, String)> {
        let mut list: Vec<_> = self.networks.as_ref()?.iter().collect();
        list.sort_by_key(|(name, _)| name.to_string_lossy().into_owned());
        list.into_iter().find_map(|(name, data)| {
            let lower = name.to_string_lossy().to_lowercase();
            if lower == "lo" || lower.contains("loopback") {
                return None;
            }
            let ip = data
                .ip_networks()
                .iter()
                .find(|n| n.addr.is_ipv4() && !n.addr.is_loopback())?;
            let kind =
                if lower.starts_with("wl") || lower.contains("wi-fi") || lower.contains("wlan") {
                    'W'
                } else {
                    'E'
                };
            Some((kind, ip.addr.to_string()))
        })
    }
}

fn battery() -> Option<u8> {
    let dir = std::fs::read_dir("/sys/class/power_supply").ok()?;
    dir.flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("BAT"))
        .find_map(|e| {
            std::fs::read_to_string(e.path().join("capacity"))
                .ok()?
                .trim()
                .parse()
                .ok()
        })
}

fn gib(bytes: u64) -> String {
    format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

/// Ein Eintrag in der Taskliste der Statusleiste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: usize,
    pub title: String,
    pub focused: bool,
    pub minimized: bool,
    /// Grafisches Programm (Wayland-Fenster unter labwc).
    pub gui: bool,
}

/// Klickbarer Bereich der Statusleiste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarHit {
    /// Workspace-Abzeichen links: öffnet den Taskmanager.
    Menu,
    /// Der Desktop (die Vollbild-Shell).
    Desktop,
    Task(usize),
}

pub struct Bar<'a> {
    /// Titel des Desktops (Shell oder per OSC gesetzter Titel).
    pub title: &'a str,
    pub desktop_focused: bool,
    pub tasks: &'a [Task],
    pub stats: &'a Stats,
    pub mouse: bool,
    pub scrolled: usize,
    pub message: Option<&'a str>,
    pub ascii: bool,
    pub options: &'a BarSettings,
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

impl Bar<'_> {
    /// Linke Seite mit den Klickbereichen (Startspalte relativ zur Leiste, Breite, Ziel).
    fn left(&self) -> (Vec<Span<'static>>, Vec<(u16, u16, BarHit)>) {
        let mut spans = Vec::new();
        let mut hits = Vec::new();
        let mut x = 0u16;
        let mut push =
            |spans: &mut Vec<Span<'static>>, span: Span<'static>, hit: Option<BarHit>| {
                let w = span.width() as u16;
                if let Some(hit) = hit {
                    hits.push((x, w, hit));
                }
                x += w;
                spans.push(span);
            };
        push(
            &mut spans,
            Span::styled(
                format!(" {} ", self.options.label),
                Style::new()
                    .bg(theme::accent2())
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            ),
            Some(BarHit::Menu),
        );
        push(&mut spans, Span::raw(" "), None);
        let focused = Style::new()
            .bg(theme::sel_bg())
            .fg(theme::accent())
            .add_modifier(Modifier::BOLD);
        let normal = Style::new().fg(theme::fg());
        let full = (self.options.task_width as usize).max(4);
        let max = if self.tasks.len() > 3 {
            (full / 2).max(4)
        } else {
            full
        };
        push(
            &mut spans,
            Span::styled(
                format!(" {} ", truncate(self.title, max)),
                if self.desktop_focused {
                    focused
                } else {
                    normal
                },
            ),
            Some(BarHit::Desktop),
        );
        for task in self.tasks {
            let style = if task.focused {
                focused
            } else if task.minimized {
                Style::new().fg(theme::dim())
            } else {
                normal
            };
            let marker = if task.minimized { "_" } else { "" };
            push(&mut spans, Span::raw(" "), None);
            push(
                &mut spans,
                Span::styled(
                    format!(" {}:{}{marker} ", task.id, truncate(&task.title, max)),
                    style,
                ),
                Some(BarHit::Task(task.id)),
            );
        }
        if self.scrolled > 0 {
            let arrow = if self.ascii { "^" } else { "↑" };
            push(
                &mut spans,
                Span::styled(
                    format!("  {arrow} {}", self.scrolled),
                    Style::new().fg(theme::warn()),
                ),
                None,
            );
        }
        if let Some(msg) = self.message {
            push(
                &mut spans,
                Span::styled(format!("  {msg}"), Style::new().fg(theme::warn())),
                None,
            );
        }
        (spans, hits)
    }

    /// Welcher Bereich liegt unter Spalte `col` (relativ zum Leistenanfang)?
    pub fn hit(&self, col: u16) -> Option<BarHit> {
        self.left()
            .1
            .into_iter()
            .find(|&(x, w, _)| col >= x && col < x + w)
            .map(|(_, _, hit)| hit)
    }

    fn right(&self) -> Vec<Span<'static>> {
        let label = Style::new().fg(theme::dim());
        let value = Style::new().fg(theme::fg());
        let s = self.stats;
        let o = self.options;
        let mut blocks: Vec<Vec<Span<'static>>> = Vec::new();
        if o.mouse {
            blocks.push(vec![
                Span::styled("MOUSE ", label),
                if self.mouse {
                    Span::styled("an", Style::new().fg(theme::accent()))
                } else {
                    Span::styled("aus", label)
                },
            ]);
        }
        if o.cpu {
            blocks.push(vec![
                Span::styled("CPU ", label),
                Span::styled(
                    format!("{:>2.0}%", s.cpu),
                    if s.cpu > 85.0 {
                        Style::new().fg(theme::warn())
                    } else {
                        value
                    },
                ),
            ]);
        }
        if o.ram {
            blocks.push(vec![
                Span::styled("RAM ", label),
                Span::styled(format!("{}/{}", gib(s.mem_used), gib(s.mem_total)), value),
            ]);
        }
        if o.network {
            blocks.push(match &s.net {
                Some((kind, ip)) => vec![
                    Span::styled(format!("{kind}: "), label),
                    Span::styled(ip.clone(), Style::new().fg(theme::accent())),
                ],
                None => vec![Span::styled("NET down", Style::new().fg(theme::error()))],
            });
        }
        if let Some(b) = s.battery.filter(|_| o.battery) {
            let color = match b {
                0..=15 => theme::error(),
                16..=30 => theme::warn(),
                _ => theme::accent(),
            };
            blocks.push(vec![
                Span::styled("BAT ", label),
                Span::styled(format!("{b}%"), Style::new().fg(color)),
            ]);
        }
        if o.clock {
            // Ungültige Formate würden in chrono panisch abbrechen.
            let format = if tuilnux_config::clock_format_ok(&o.clock_format) {
                o.clock_format.as_str()
            } else {
                "%Y-%m-%d %H:%M:%S"
            };
            blocks.push(vec![Span::styled(
                Local::now().format(format).to_string(),
                value,
            )]);
        }
        let sep = if self.ascii { " | " } else { " │ " };
        let mut out = Vec::new();
        for (i, block) in blocks.into_iter().enumerate() {
            if i > 0 {
                out.push(Span::styled(sep, Style::new().fg(theme::dim())));
            }
            out.extend(block);
        }
        out.push(Span::raw(" "));
        out
    }
}

fn width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.width()).sum()
}

impl Widget for Bar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        buf.set_style(area, Style::new().bg(theme::statusbar()).fg(theme::fg()));
        let (left, _) = self.left();
        let mut right = self.right();
        // Bei schmalen Bildschirmen von links her Blöcke weglassen, die Uhrzeit bleibt.
        while width(&left) + width(&right) + 1 > area.width as usize && right.len() > 3 {
            let cut = right
                .iter()
                .position(|s| s.content.contains('│') || s.content.contains('|'))
                .map_or(1, |p| p + 1);
            right.drain(..cut);
        }
        let right_w = width(&right) as u16;
        let left_w = area.width.saturating_sub(right_w + 1);
        Line::from(left).render(
            Rect {
                width: left_w,
                ..area
            },
            buf,
        );
        if right_w <= area.width {
            let x = area.right() - right_w;
            Line::from(right).render(Rect::new(x, area.y, right_w, 1), buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats() -> Stats {
        Stats {
            cpu: 12.0,
            mem_used: 2 << 30,
            mem_total: 8 << 30,
            net: Some(('E', "192.168.1.5".into())),
            battery: Some(80),
        }
    }

    static OPTIONS: std::sync::LazyLock<BarSettings> =
        std::sync::LazyLock::new(BarSettings::default);

    fn bar<'a>(stats: &'a Stats, tasks: &'a [Task]) -> Bar<'a> {
        Bar {
            title: "bash",
            desktop_focused: tasks.iter().all(|t| !t.focused),
            tasks,
            stats,
            mouse: !tasks.is_empty(),
            scrolled: 0,
            message: None,
            ascii: false,
            options: &OPTIONS,
        }
    }

    fn render(width: u16, bar: Bar) -> String {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, 1));
        bar.render(buf.area, &mut buf);
        (0..width)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect()
    }

    #[test]
    fn shows_all_blocks_and_degrades() {
        let stats = stats();
        let wide = render(140, bar(&stats, &[]));
        for part in [
            " 1   bash ",
            "MOUSE aus",
            "CPU 12%",
            "RAM 2.0G/8.0G",
            "E: 192.168.1.5",
            "BAT 80%",
        ] {
            assert!(wide.contains(part), "{part} fehlt in {wide}");
        }
        let narrow = render(40, bar(&stats, &[]));
        assert!(narrow.contains("bash"), "{narrow}");
        assert!(!narrow.contains("MOUSE"), "{narrow}");
        assert!(
            narrow.trim_end().ends_with(|c: char| c.is_ascii_digit()),
            "{narrow}"
        );
        render(3, bar(&stats, &[]));
    }

    #[test]
    fn tasks_are_listed_and_clickable() {
        let stats = stats();
        let tasks = [
            Task {
                id: 2,
                title: "tfiles".into(),
                focused: true,
                minimized: false,
                gui: false,
            },
            Task {
                id: 3,
                title: "ttop".into(),
                focused: false,
                minimized: true,
                gui: true,
            },
        ];
        let b = bar(&stats, &tasks);
        let line = render(140, bar(&stats, &tasks));
        assert!(line.contains(" 2:tfiles "), "{line}");
        assert!(line.contains(" 3:ttop_ "), "{line}");
        assert!(line.contains("MOUSE an"), "{line}");
        assert_eq!(b.hit(0), Some(BarHit::Menu));
        assert_eq!(b.hit(5), Some(BarHit::Desktop));
        let x = line.find(" 2:tfiles").unwrap() as u16 + 2;
        assert_eq!(b.hit(x), Some(BarHit::Task(2)));
        let x = line.find(" 3:ttop").unwrap() as u16 + 2;
        assert_eq!(b.hit(x), Some(BarHit::Task(3)));
        assert_eq!(b.hit(130), None);
    }

    #[test]
    fn modules_can_be_hidden() {
        let stats = stats();
        let options = BarSettings {
            label: "TUI".into(),
            mouse: false,
            ram: false,
            battery: false,
            clock_format: "%H:%M".into(),
            ..BarSettings::default()
        };
        let line = render(
            140,
            Bar {
                options: &options,
                ..bar(&stats, &[])
            },
        );
        assert!(line.starts_with(" TUI "), "{line}");
        assert!(!line.contains("MOUSE") && !line.contains("RAM") && !line.contains("BAT"));
        assert!(line.contains("CPU 12%"), "{line}");
        let clock = line.trim_end().rsplit(' ').next().unwrap();
        assert_eq!(clock.len(), 5, "{line}");
        let broken = BarSettings {
            clock_format: "%Q".into(),
            ..BarSettings::default()
        };
        render(
            140,
            Bar {
                options: &broken,
                ..bar(&stats, &[])
            },
        );
    }
}
