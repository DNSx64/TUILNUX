//! Spielt eine [`Animation`] direkt im laufenden Terminal ab – ohne Vollbild,
//! damit die Ausgabe nach dem Ende im Scrollback stehen bleibt (z. B. aus ~/.bashrc).

use std::io::{self, Write};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::{Print, ResetColor, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::{QueueableCommand, cursor};

use crate::Animation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Once,
    Loop,
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub mode: Mode,
    pub speed: f32,
    /// Am Ende den letzten Frame stehen lassen (sonst wird der Bereich geleert).
    pub keep_last: bool,
}

fn write_frame(
    out: &mut impl Write,
    anim: &Animation,
    index: usize,
    width: usize,
) -> io::Result<()> {
    let frame = &anim.frames[index];
    for row in 0..anim.height {
        out.queue(cursor::MoveToColumn(0))?;
        if let Some(line) = frame.lines.get(row) {
            let visible: String = line.chars().take(width).collect();
            if let Some(c) = frame.line_color(row) {
                out.queue(SetForegroundColor(crossterm::style::Color::Rgb {
                    r: c.0,
                    g: c.1,
                    b: c.2,
                }))?;
            }
            out.queue(Print(visible))?;
            out.queue(ResetColor)?;
        }
        out.queue(Clear(ClearType::UntilNewLine))?;
        if row + 1 < anim.height {
            out.queue(cursor::MoveDown(1))?;
        }
    }
    if anim.height > 1 {
        out.queue(cursor::MoveUp((anim.height - 1) as u16))?;
    }
    out.flush()
}

fn interrupted(deadline: Instant) -> io::Result<bool> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Ok(false);
        }
        if event::poll(deadline - now)?
            && let Event::Key(k) = event::read()?
        {
            if k.kind == KeyEventKind::Release {
                continue;
            }
            let ctrl_c =
                k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL);
            if ctrl_c || matches!(k.code, KeyCode::Char('q') | KeyCode::Esc) {
                return Ok(true);
            }
        }
    }
}

/// Spielt die Animation ab. Gibt `true` zurück, wenn sie abgebrochen wurde.
pub fn play(anim: &Animation, opts: Options) -> io::Result<bool> {
    let mut out = io::stdout();
    let (cols, _) = terminal::size().unwrap_or((80, 24));
    let width = cols as usize;

    // Platz reservieren: Zeilen ausgeben (scrollt ggf.) und zurück an den Anfang.
    out.queue(cursor::Hide)?;
    for _ in 0..anim.height.saturating_sub(1) {
        out.queue(Print("\n"))?;
    }
    if anim.height > 1 {
        out.queue(cursor::MoveUp((anim.height - 1) as u16))?;
    }
    out.flush()?;

    terminal::enable_raw_mode()?;
    let result = (|| -> io::Result<bool> {
        loop {
            for i in 0..anim.frames.len() {
                write_frame(&mut out, anim, i, width)?;
                let wait = anim.frames[i].duration.div_f32(opts.speed.max(0.05));
                if interrupted(Instant::now() + wait.min(Duration::from_secs(60)))? {
                    return Ok(true);
                }
            }
            if opts.mode == Mode::Once {
                return Ok(false);
            }
        }
    })();
    terminal::disable_raw_mode()?;

    let aborted = result?;
    if !opts.keep_last || aborted {
        for row in 0..anim.height {
            out.queue(cursor::MoveToColumn(0))?;
            out.queue(Clear(ClearType::UntilNewLine))?;
            if row + 1 < anim.height {
                out.queue(cursor::MoveDown(1))?;
            }
        }
        if anim.height > 1 {
            out.queue(cursor::MoveUp((anim.height - 1) as u16))?;
        }
    } else if anim.height > 1 {
        out.queue(cursor::MoveDown((anim.height - 1) as u16))?;
        out.queue(Print("\r\n"))?;
    } else {
        out.queue(Print("\r\n"))?;
    }
    out.queue(cursor::Show)?;
    out.flush()?;
    Ok(aborted)
}
