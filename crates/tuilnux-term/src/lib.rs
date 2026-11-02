//! Terminal-Emulation für TUILNUX: ein [`Pane`] ist ein Programm in einem PTY samt
//! VT-Zustand (vt100). Daraus bestehen sowohl der Desktop (die Vollbild-Shell) als
//! auch jedes Fenster, das darüber geöffnet wird.

pub mod keys;
pub mod render;

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

pub use render::{Wallpaper, render_screen};
pub use vt100;

pub const SCROLLBACK: usize = 10_000;

/// Reagiert auf Escape-Sequenzen, die vt100 selbst nicht beantwortet.
#[derive(Default)]
pub struct Host {
    pub title: Option<String>,
    pub replies: Vec<u8>,
    pub clipboard: Vec<String>,
    pub bell: bool,
}

impl vt100::Callbacks for Host {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell = true;
    }

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        let title = String::from_utf8_lossy(title).trim().to_string();
        self.title = (!title.is_empty()).then_some(title);
    }

    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, ty: &[u8], data: &[u8]) {
        let (ty, data) = (String::from_utf8_lossy(ty), String::from_utf8_lossy(data));
        self.clipboard.push(format!("\x1b]52;{ty};{data}\x07"));
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, c, first) {
            // Gerätestatus und Cursorposition (fragen z. B. fish, zsh und crossterm ab).
            (None, 'n', 5) => self.replies.extend(b"\x1b[0n"),
            (None, 'n', 6) => {
                let (r, c) = screen.cursor_position();
                self.replies
                    .extend(format!("\x1b[{};{}R", r + 1, c + 1).as_bytes());
            }
            // Primary/Secondary Device Attributes.
            (None, 'c', 0) => self.replies.extend(b"\x1b[?62;22c"),
            (Some(b'>'), 'c', 0) => self.replies.extend(b"\x1b[>1;10;0c"),
            _ => {}
        }
    }
}

pub fn nz(v: u16) -> NonZeroU16 {
    NonZeroU16::new(v.max(1)).unwrap_or(NonZeroU16::MIN)
}

/// Login-Shell des Nutzers (`$SHELL`), sonst bash bzw. PowerShell unter Windows.
pub fn default_shell() -> Vec<String> {
    if let Ok(shell) = std::env::var("SHELL")
        && !shell.is_empty()
    {
        return vec![shell];
    }
    if cfg!(windows) {
        vec!["powershell.exe".into(), "-NoLogo".into()]
    } else {
        vec!["/bin/bash".into()]
    }
}

/// Kurzname eines Befehls für Titel und Taskleiste (`/usr/bin/tfiles` → `tfiles`).
pub fn program_name(command: &[String]) -> String {
    let first = command.first().map_or("?", String::as_str);
    Path::new(first)
        .file_stem()
        .map_or_else(|| first.to_string(), |s| s.to_string_lossy().into_owned())
}

/// PATH mit dem Ordner des laufenden Programms vorne, damit tfiles, tedit … auch
/// direkt aus `target/` heraus gefunden werden.
fn path_with_own_dir() -> Option<OsString> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let mut paths = vec![dir];
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path));
    }
    paths.dedup();
    std::env::join_paths(paths).ok()
}

#[derive(Debug, Clone)]
pub struct PaneOptions {
    pub command: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
    /// Zeilen im Verlauf (Scrollback).
    pub scrollback: usize,
}

impl PaneOptions {
    pub fn new(command: Vec<String>, cols: u16, rows: u16) -> Self {
        Self {
            command,
            cwd: None,
            env: Vec::new(),
            cols,
            rows,
            scrollback: SCROLLBACK,
        }
    }
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Ein Programm in einem Pseudo-Terminal.
pub struct Pane {
    pub parser: vt100::Parser<Host>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    pub name: String,
    pub command: Vec<String>,
}

impl Pane {
    /// Startet `opts.command`. Ausgaben liefert ein Hintergrund-Thread an `on_output`
    /// (`Some(bytes)` für Daten, `None` beim Ende); gibt der Callback `false` zurück,
    /// endet der Thread.
    pub fn spawn(
        opts: PaneOptions,
        on_output: impl Fn(Option<Vec<u8>>) -> bool + Send + 'static,
    ) -> Result<Self> {
        anyhow::ensure!(!opts.command.is_empty(), "kein Befehl angegeben");
        let pair = native_pty_system()
            .openpty(pty_size(opts.cols, opts.rows))
            .context("PTY konnte nicht geöffnet werden")?;
        let mut cmd = CommandBuilder::new(&opts.command[0]);
        cmd.args(&opts.command[1..]);
        if let Some(dir) = opts
            .cwd
            .clone()
            .filter(|d| d.is_dir())
            .or_else(|| std::env::current_dir().ok())
            .or_else(std::env::home_dir)
        {
            cmd.cwd(dir);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "tterm");
        cmd.env("TUILNUX", "1");
        if let Some(path) = path_with_own_dir() {
            cmd.env("PATH", path);
        }
        for (k, v) in &opts.env {
            cmd.env(k, v);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("„{}“ konnte nicht gestartet werden", opts.command[0]))?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        std::thread::spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => {
                        on_output(None);
                        return;
                    }
                    Ok(n) => {
                        if !on_output(Some(buf[..n].to_vec())) {
                            return;
                        }
                    }
                }
            }
        });
        let writer = pair.master.take_writer()?;
        Ok(Self {
            parser: vt100::Parser::new_with_callbacks(
                nz(opts.rows),
                nz(opts.cols),
                opts.scrollback,
                Host::default(),
            ),
            writer,
            master: pair.master,
            child,
            name: program_name(&opts.command),
            command: opts.command,
        })
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// Fenstertitel per OSC, sonst der Programmname.
    pub fn title(&self) -> &str {
        self.parser
            .callbacks()
            .title
            .as_deref()
            .unwrap_or(&self.name)
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    pub fn send(&mut self, bytes: &[u8]) {
        let _ = self
            .writer
            .write_all(bytes)
            .and_then(|_| self.writer.flush());
    }

    /// Verarbeitet Programmausgabe, beantwortet Terminal-Anfragen und gibt
    /// OSC-52-Zwischenablagesequenzen für das Host-Terminal zurück.
    pub fn process(&mut self, bytes: &[u8]) -> Vec<String> {
        self.parser.process(bytes);
        let replies = std::mem::take(&mut self.parser.callbacks_mut().replies);
        if !replies.is_empty() {
            self.send(&replies);
        }
        std::mem::take(&mut self.parser.callbacks_mut().clipboard)
    }

    pub fn size(&self) -> (u16, u16) {
        let (rows, cols) = self.screen().size();
        (cols.get(), rows.get())
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if self.size() == (cols.max(1), rows.max(1)) {
            return;
        }
        self.parser.screen_mut().set_size(nz(rows), nz(cols));
        let _ = self.master.resize(pty_size(cols, rows));
    }

    pub fn scroll(&mut self, delta: isize) {
        let screen = self.parser.screen_mut();
        let now = screen.scrollback() as isize;
        screen.set_scrollback((now + delta).max(0) as usize);
    }

    pub fn wants_mouse(&self) -> bool {
        self.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None
    }

    /// Tastendruck: Umschalt+Bild↑/↓ blättert im Verlauf, alles andere geht ans Programm.
    pub fn key(&mut self, key: KeyEvent) {
        let page = self.screen().size().0.get() as isize / 2;
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::PageUp => return self.scroll(page),
                KeyCode::PageDown => return self.scroll(-page),
                _ => {}
            }
        }
        if !matches!(key.code, KeyCode::Modifier(_)) {
            self.parser.screen_mut().set_scrollback(0);
        }
        if let Some(bytes) = keys::encode_key(key, self.screen().application_cursor()) {
            self.send(&bytes);
        }
    }

    pub fn paste(&mut self, text: &str) {
        self.parser.screen_mut().set_scrollback(0);
        let bytes = keys::encode_paste(text, self.screen().bracketed_paste());
        self.send(&bytes);
    }

    /// Mausereignis an Position (`col`, `row`) relativ zur Pane. Programme, die keine Maus
    /// anfordern, bekommen nichts; das Mausrad blättert dann im Verlauf.
    pub fn mouse(&mut self, event: MouseEvent, col: u16, row: u16) {
        let screen = self.screen();
        if self.wants_mouse() {
            if let Some(bytes) = keys::encode_mouse(
                event,
                col,
                row,
                screen.mouse_protocol_mode(),
                screen.mouse_protocol_encoding(),
            ) {
                self.send(&bytes);
            }
            return;
        }
        if screen.alternate_screen() {
            // Vollbildprogramme ohne Mausmodus: Rad als Pfeiltasten (wie xterm „alternateScroll“).
            let arrow = match event.kind {
                MouseEventKind::ScrollUp => KeyCode::Up,
                MouseEventKind::ScrollDown => KeyCode::Down,
                _ => return,
            };
            for _ in 0..3 {
                self.key(KeyEvent::new(arrow, KeyModifiers::NONE));
            }
            return;
        }
        match event.kind {
            MouseEventKind::ScrollUp => self.scroll(3),
            MouseEventKind::ScrollDown => self.scroll(-3),
            _ => {}
        }
    }

    /// Exitcode, falls das Programm beendet ist.
    pub fn try_wait(&mut self) -> Option<u32> {
        self.child.try_wait().ok().flatten().map(|s| s.exit_code())
    }

    pub fn wait(&mut self) -> Option<u32> {
        self.child.wait().ok().map(|s| s.exit_code())
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        if self.try_wait().is_none() {
            self.kill();
        }
    }
}

/// Gibt OSC-52-Sequenzen an das äußere Terminal weiter.
pub fn forward_clipboard(clips: &[String]) {
    if clips.is_empty() {
        return;
    }
    let mut out = io::stdout();
    for clip in clips {
        let _ = out.write_all(clip.as_bytes());
    }
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_answers_queries_and_tracks_title() {
        let mut p = vt100::Parser::new_with_callbacks(nz(5), nz(20), 0, Host::default());
        p.process(b"ab\x1b[6n\x1b[c\x1b]2;vim notes.md\x07\x1b]52;c;aGFsbG8=\x07\x07");
        let host = p.callbacks();
        assert_eq!(host.replies, b"\x1b[1;3R\x1b[?62;22c");
        assert_eq!(host.title.as_deref(), Some("vim notes.md"));
        assert_eq!(host.clipboard, ["\x1b]52;c;aGFsbG8=\x07"]);
        assert!(host.bell);
    }

    #[test]
    fn program_names() {
        assert_eq!(program_name(&["/usr/bin/tfiles".into()]), "tfiles");
        assert_eq!(program_name(&["powershell.exe".into()]), "powershell");
        assert_eq!(program_name(&[]), "?");
    }

    #[test]
    fn spawns_and_reads_output() {
        let cmd: Vec<String> = if cfg!(windows) {
            vec!["cmd.exe".into(), "/c".into(), "echo tuilnux-ok".into()]
        } else {
            vec!["/bin/sh".into(), "-c".into(), "echo tuilnux-ok".into()]
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let mut pane =
            Pane::spawn(PaneOptions::new(cmd, 40, 5), move |b| tx.send(b).is_ok()).expect("spawn");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(Some(bytes)) => {
                    pane.process(&bytes);
                }
                Ok(None) => break,
                Err(_) => {}
            }
            if pane.screen().contents().contains("tuilnux-ok") {
                break;
            }
        }
        assert!(
            pane.screen().contents().contains("tuilnux-ok"),
            "{}",
            pane.screen().contents()
        );
    }
}
