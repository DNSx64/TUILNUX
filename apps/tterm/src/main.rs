//! tterm – ein einzelnes Terminal mit TUILNUX-Statusleiste.
//!
//! Die volle Sitzung mit Fenstern und Taskmanager ist `tuilnux`; tterm ist das
//! schlanke Gegenstück für eine einzelne Shell (z. B. als Fallback-Sitzung).

use std::io::{Write, stdout};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind,
};
use crossterm::execute;
use ratatui::layout::Rect;
use tuilnux_status::{Bar, Monitor};
use tuilnux_term::{Pane, PaneOptions, Wallpaper, default_shell, forward_clipboard, render_screen};

const HELP: &str = "\
tterm – Terminal mit TUILNUX-Statusleiste

Aufruf:
  tterm                 Login-Shell ($SHELL) starten
  tterm [--] BEFEHL …   Befehl statt der Shell starten

Tasten:
  Shift+PgUp/PgDn       im Verlauf blättern
  Mausrad               blättern (Maus wird nur bei Bedarf eingefangen)
";

enum Msg {
    Output(Option<Vec<u8>>),
    Input(Event),
}

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(
        args.first().map(String::as_str),
        Some("-h" | "--help" | "help")
    ) {
        print!("{HELP}");
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "--") {
        args.remove(0);
    }
    let command = if args.is_empty() {
        default_shell()
    } else {
        args
    };
    let code = run(command)?;
    std::process::exit(code as i32);
}

fn pane_area(size: Rect) -> Rect {
    Rect {
        height: size.height.saturating_sub(1),
        ..size
    }
}

fn run(command: Vec<String>) -> Result<u32> {
    let mut terminal = ratatui::init();
    execute!(stdout(), EnableBracketedPaste)?;
    let result = session(&mut terminal, command);
    let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
}

fn session(terminal: &mut ratatui::DefaultTerminal, command: Vec<String>) -> Result<u32> {
    let size = terminal.size()?;
    let area = pane_area(Rect::new(0, 0, size.width, size.height));
    let settings = tuilnux_ui::tuilnux_config::Settings::load_or_default();
    let mut opts = PaneOptions::new(command, area.width, area.height);
    opts.scrollback = settings.terminal.scrollback;
    let (tx, rx) = mpsc::channel();
    let out = tx.clone();
    let mut pane = Pane::spawn(opts, move |data| out.send(Msg::Output(data)).is_ok())?;
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if tx.send(Msg::Input(ev)).is_err() {
                break;
            }
        }
    });

    let ascii = settings.windows.ascii || tuilnux_ui::ascii_console();
    let mut monitor = Monitor::new();
    let mut wallpaper = Wallpaper::default();
    let mut mouse = false;
    let mut ended = false;
    loop {
        let first = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(msg) => Some(msg),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        for msg in first.into_iter().chain(rx.try_iter()) {
            match msg {
                Msg::Output(Some(bytes)) => forward_clipboard(&pane.process(&bytes)),
                Msg::Output(None) => ended = true,
                Msg::Input(Event::Key(key)) if key.kind != KeyEventKind::Release => pane.key(key),
                Msg::Input(Event::Paste(text)) => pane.paste(&text),
                Msg::Input(Event::Mouse(m)) => {
                    let area = pane_area(terminal.size()?.into());
                    if m.row < area.height {
                        pane.mouse(m, m.column, m.row);
                    }
                }
                Msg::Input(Event::Resize(w, h)) => {
                    let area = pane_area(Rect::new(0, 0, w, h));
                    pane.resize(area.width, area.height);
                }
                Msg::Input(_) => {}
            }
        }
        if ended || pane.try_wait().is_some() {
            break;
        }
        let want = pane.wants_mouse() || pane.screen().scrollback() > 0;
        if want != mouse {
            mouse = want;
            if mouse {
                execute!(stdout(), EnableMouseCapture)?;
            } else {
                execute!(stdout(), DisableMouseCapture)?;
            }
        }
        if std::mem::take(&mut pane.parser.callbacks_mut().bell) {
            let _ = stdout().write_all(b"\x07");
        }
        monitor.refresh();
        let size = terminal.size()?;
        let area = pane_area(Rect::new(0, 0, size.width, size.height));
        wallpaper.update(area.width, area.height);
        terminal.draw(|f| {
            let buf = f.buffer_mut();
            if let Some(pos) = render_screen(pane.screen(), Some(&wallpaper), area, buf) {
                f.set_cursor_position(pos);
            }
            let bar = Rect::new(0, area.bottom(), size.width, 1);
            if size.height > 0 {
                f.render_widget(
                    Bar {
                        title: pane.title(),
                        desktop_focused: true,
                        tasks: &[],
                        stats: &monitor.stats,
                        mouse,
                        scrolled: pane.screen().scrollback(),
                        message: None,
                        ascii,
                        options: &settings.bar,
                    },
                    bar,
                );
            }
        })?;
    }
    Ok(pane.wait().unwrap_or(0))
}
