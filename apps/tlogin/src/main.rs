//! tlogin – Anmeldebildschirm von TUILNUX (greetd-Greeter).
//!
//! Unter Linux startet greetd `tlogin` auf dem TTY; nach erfolgreicher Anmeldung
//! übergibt tlogin den Sitzungsbefehl (Standard: `tuilnux`) an greetd und beendet sich.
//! Aussehen, Sperren und zweite Faktoren stehen in `/etc/tuilnux/login.toml`
//! (`tlogin setup`). Der Demo-Modus (Passwort `tuilnux`) läuft nur mit `--demo`.

mod console;
mod greetd;
mod logo;
mod setup;

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::{Datelike, Local};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Margin};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use tuilnux_config::{FactorMode, LoginSettings, LogoStyle, SecretDisplay, parse_hex};
use tuilnux_ui::{App, Flow, InputResult, LineInput, theme, tr, trf};

use greetd::{AuthMessageType, Backend, ErrorType, Request, Response};

const CURSOR: Color = Color::Rgb(0x9a, 0x9a, 0x9a);
const USER_LABEL: &str = "Username:";
const MAX_USER: usize = 64;
const MAX_SECRET: usize = 512;
/// Feste Länge bei `secret_display = "mask"` – verrät die Passwortlänge nicht.
const MASK_LEN: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Session {
    name: String,
    cmd: Vec<String>,
}

impl Session {
    fn parse(spec: &str) -> Option<Self> {
        let (name, cmd) = spec.split_once('=')?;
        let cmd: Vec<String> = cmd.split_whitespace().map(String::from).collect();
        (!name.trim().is_empty() && !cmd.is_empty()).then(|| Self {
            name: name.trim().to_string(),
            cmd,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Field {
    User,
    Secret { prompt: String, echo: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Info,
    Error,
}

struct Login {
    backend: Box<dyn Backend>,
    cfg: LoginSettings,
    host: String,
    user: LineInput,
    secret: LineInput,
    field: Field,
    message: Option<(String, Tone)>,
    help: bool,
    sessions: Vec<Session>,
    session: usize,
    ascii: bool,
    linux_console: bool,
    blink: bool,
    caps: bool,
    started: Option<Session>,
    launched: Instant,
    last_clear: Instant,
    last_input: Instant,
    failures: u32,
    lockouts: u32,
    locked_until: Option<Instant>,
    /// Wo der letzte Benutzername gespeichert wird (nur mit `remember_user`).
    state_dir: Option<PathBuf>,
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "arch".into())
}

fn valid_username(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_USER
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        && !name.starts_with('-')
}

/// PAM-Prompts auf kurze, einheitliche Labels bringen.
fn prompt_label(message: &str) -> String {
    let t = message.trim();
    let lower = t.to_lowercase();
    match lower.trim_end_matches(':').trim() {
        "" | "password" => return "Password:".into(),
        "new password" | "neues passwort" => return "Neues Passwort:".into(),
        "retype new password" | "neues passwort wiederholen" => {
            return "Wiederholen:".into();
        }
        "current password" | "(current) unix password" => return "Altes Passwort:".into(),
        _ => {}
    }
    if lower.contains("verification code") || lower.contains("one-time") {
        return "Code:".into();
    }
    if lower.contains("pin") {
        return "PIN:".into();
    }
    if t.ends_with(':') {
        t.into()
    } else {
        format!("{t}:")
    }
}

/// Englische PAM-Meldungen (pam_u2f, pam_faillock, pam_unix) auf Deutsch.
fn translate(message: &str) -> String {
    let m = message.trim();
    let lower = m.to_lowercase();
    if lower.contains("touch the device") || lower.contains("touch your") {
        return "Bitte Sicherheitsschlüssel berühren …".into();
    }
    if lower.contains("insert your u2f device") {
        return "Sicherheitsschlüssel einstecken, dann Enter".into();
    }
    if let Some(rest) = lower.strip_prefix("the account is locked due to ") {
        let n = rest.split_whitespace().next().unwrap_or("?");
        return trf("Konto gesperrt nach {} Fehlversuchen", &[&n]);
    }
    if lower.contains("left to unlock") {
        let n: String = m.chars().filter(char::is_ascii_digit).collect();
        let unit = if lower.contains("second") {
            "s"
        } else {
            "Min."
        };
        return trf("(noch {} {} bis zur Entsperrung)", &[&n, &tr(unit)]);
    }
    if lower.contains("password has expired") || lower.contains("required to change your password")
    {
        return "Passwort abgelaufen – bitte jetzt ändern".into();
    }
    if lower.contains("passwords don't match") || lower.contains("sorry, passwords do not match") {
        return "Passwörter stimmen nicht überein".into();
    }
    if lower.contains("authentication failure") {
        return "Anmeldung fehlgeschlagen".into();
    }
    m.to_string()
}

fn german_date() -> String {
    const DAYS: [&str; 7] = ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"];
    let now = Local::now();
    let day = tr(DAYS[now.weekday().num_days_from_monday() as usize]);
    let date = if tuilnux_ui::i18n::current() == "en" {
        now.format("%Y-%m-%d")
    } else {
        now.format("%d.%m.%Y")
    };
    format!("{day} {date} · {}", now.format("%H:%M"))
}

fn rgb(hex: &str, fallback: (u8, u8, u8)) -> Color {
    let (r, g, b) = parse_hex(hex).map_or(fallback, |[r, g, b]| (r, g, b));
    Color::Rgb(r, g, b)
}

/// Mischung zweier Farben (für gedämpfte Akzente).
fn mix(a: Color, b: Color, t: f32) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let m = |x: u8, y: u8| (f32::from(x) * (1.0 - t) + f32::from(y) * t).round() as u8;
            Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
        }
        _ => a,
    }
}

impl Login {
    fn new(
        backend: Box<dyn Backend>,
        sessions: Vec<Session>,
        user: Option<String>,
        cfg: LoginSettings,
    ) -> Self {
        let now = Instant::now();
        Self {
            backend,
            cfg,
            host: hostname(),
            user: LineInput::new(user.unwrap_or_default()),
            secret: LineInput::secret(),
            field: Field::User,
            message: None,
            help: false,
            sessions,
            session: 0,
            ascii: tuilnux_ui::ascii_console(),
            linux_console: std::env::var("TERM").is_ok_and(|t| t == "linux"),
            blink: true,
            caps: console::caps_lock().unwrap_or(false),
            started: None,
            launched: now,
            last_clear: now,
            last_input: now,
            failures: 0,
            lockouts: 0,
            locked_until: None,
            state_dir: None,
        }
    }

    fn rgb_accent(&self) -> Color {
        rgb(&self.cfg.accent, (0x22, 0xc8, 0xd6))
    }

    fn rgb_bg(&self) -> Color {
        rgb(&self.cfg.background, (0, 0, 0))
    }

    fn rgb_dim(&self) -> Color {
        mix(self.rgb_accent(), self.rgb_bg(), 0.5)
    }

    // Die Linux-Konsole kennt nur 16 Farben und rundet 24-Bit-Werte grob; dort werden
    // stattdessen Palettenplätze auf die exakten Farben gesetzt (siehe `console_palette`).
    fn accent(&self) -> Color {
        if self.linux_console {
            Color::Cyan
        } else {
            self.rgb_accent()
        }
    }

    fn bg(&self) -> Color {
        if self.ascii {
            Color::Reset
        } else if self.linux_console {
            Color::Black
        } else {
            self.rgb_bg()
        }
    }

    fn dim(&self) -> Color {
        if self.linux_console {
            Color::LightCyan
        } else {
            self.rgb_dim()
        }
    }

    /// Escape-Folge, die die Konsolenpalette auf die Login-Farben setzt (0, 6, 14).
    fn console_palette(&self) -> String {
        let hex = |c: Color| match c {
            Color::Rgb(r, g, b) => format!("{r:02x}{g:02x}{b:02x}"),
            _ => "000000".into(),
        };
        format!(
            "\x1b]P0{}\x1b]P6{}\x1b]PE{}",
            hex(self.rgb_bg()),
            hex(self.rgb_accent()),
            hex(self.rgb_dim())
        )
    }

    fn text(&self, s: &str) -> String {
        if self.ascii {
            logo::ascii(s)
        } else if self.linux_console {
            logo::light(s)
        } else {
            s.to_string()
        }
    }

    fn info(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.message = Some((tr(&text).to_string(), Tone::Info));
    }

    fn error(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.message = Some((tr(&text).to_string(), Tone::Error));
    }

    /// Restliche Sperrzeit in Sekunden.
    fn locked_left(&self) -> Option<u64> {
        let left = self.locked_until?.checked_duration_since(Instant::now())?;
        Some(left.as_secs() + u64::from(left.subsec_nanos() > 0))
    }

    /// Zählt einen Fehlversuch; nach `max_attempts` folgt eine Pause, die sich
    /// mit jeder weiteren Sperre verdoppelt.
    fn register_failure(&mut self) {
        self.failures += 1;
        let max = self.cfg.max_attempts;
        if max == 0 || !self.failures.is_multiple_of(max) || self.cfg.lockout_seconds == 0 {
            return;
        }
        self.lockouts += 1;
        let secs = (u64::from(self.cfg.lockout_seconds) << (self.lockouts - 1).min(10)).min(3600);
        self.locked_until = Some(Instant::now() + Duration::from_secs(secs));
    }

    fn call(&mut self, request: Request) -> Option<Response> {
        match self.backend.request(request) {
            Ok(r) => Some(r),
            Err(e) => {
                self.error(trf("Verbindung zu greetd fehlgeschlagen: {}", &[&e]));
                self.reset_to_user();
                None
            }
        }
    }

    fn cancel(&mut self) {
        let _ = self.backend.request(Request::CancelSession);
    }

    fn reset_to_user(&mut self) {
        self.field = Field::User;
        self.secret.wipe();
    }

    /// Alles vergessen (Inaktivität, Strg+C).
    fn clear_all(&mut self) {
        if self.field != Field::User {
            self.cancel();
        }
        self.user.wipe();
        self.reset_to_user();
        self.help = false;
        self.message = None;
    }

    fn redraw(&mut self, terminal: &mut DefaultTerminal) {
        let _ = terminal.draw(|f| self.draw(f));
    }

    fn submit_user(&mut self, terminal: &mut DefaultTerminal) {
        let name = self.user.text().trim().to_string();
        match name.as_str() {
            "" => return,
            "help" | "hilfe" | "?" => {
                self.help = !self.help;
                self.user.wipe();
                return;
            }
            "reboot" | "neustart" => return self.power("reboot"),
            "poweroff" | "shutdown" | "ausschalten" => return self.power("poweroff"),
            _ => {}
        }
        if !valid_username(&name) {
            self.error("Ungültiger Benutzername");
            return;
        }
        self.help = false;
        self.info("Verbinde …");
        self.redraw(terminal);
        if let Some(resp) = self.call(Request::CreateSession { username: name }) {
            self.message = None;
            self.advance(terminal, resp, true);
        }
    }

    fn submit_secret(&mut self, terminal: &mut DefaultTerminal) {
        let response = self.secret.take_text();
        self.info("Anmeldung wird geprüft …");
        self.redraw(terminal);
        if let Some(resp) = self.call(Request::PostAuthMessageResponse {
            response: Some(response),
        }) {
            self.message = None;
            self.advance(terminal, resp, false);
        }
    }

    /// Verarbeitet Antworten von greetd, bis eine Eingabe nötig ist oder die Sitzung startet.
    /// `fresh` ist wahr, solange seit CreateSession noch keine Antwort gesendet wurde
    /// (verhindert eine Endlosschleife, wenn greetd sofort mit AuthError antwortet).
    fn advance(&mut self, terminal: &mut DefaultTerminal, mut resp: Response, mut fresh: bool) {
        let mut pam_error: Option<String> = None;
        loop {
            resp = match resp {
                Response::AuthMessage {
                    auth_message_type: kind @ (AuthMessageType::Secret | AuthMessageType::Visible),
                    auth_message,
                } => {
                    self.field = Field::Secret {
                        prompt: prompt_label(&auth_message),
                        echo: kind == AuthMessageType::Visible,
                    };
                    self.secret.wipe();
                    return;
                }
                Response::AuthMessage {
                    auth_message_type: kind,
                    auth_message,
                } => {
                    let text = translate(&auth_message);
                    if kind == AuthMessageType::Error {
                        pam_error = Some(match pam_error.take() {
                            Some(prev) => format!("{prev} {text}"),
                            None => text.clone(),
                        });
                        self.error(pam_error.clone().unwrap_or_default());
                    } else {
                        self.info(text);
                    }
                    // Z. B. „Schlüssel berühren“: anzeigen, bevor greetd blockiert.
                    self.redraw(terminal);
                    fresh = false;
                    match self.call(Request::PostAuthMessageResponse { response: None }) {
                        Some(r) => r,
                        None => return,
                    }
                }
                Response::Success => {
                    self.start_session();
                    return;
                }
                Response::Error {
                    error_type: ErrorType::AuthError,
                    ..
                } if !fresh => {
                    // Fehlversuch: Sitzung neu anlegen, damit PAM erneut fragt.
                    self.register_failure();
                    self.cancel();
                    fresh = true;
                    let was_code =
                        matches!(&self.field, Field::Secret { prompt, .. } if prompt == "Code:");
                    let reason = pam_error.take().unwrap_or_else(|| {
                        if was_code {
                            "Falscher Code – bitte erneut versuchen".into()
                        } else {
                            "Falsches Passwort – bitte erneut versuchen".into()
                        }
                    });
                    let username = self.user.text().trim().to_string();
                    let next = self.call(Request::CreateSession { username });
                    self.error(reason);
                    match next {
                        Some(r) => r,
                        None => return,
                    }
                }
                Response::Error { description, .. } => {
                    self.error(pam_error.take().unwrap_or_else(|| translate(&description)));
                    self.cancel();
                    self.reset_to_user();
                    return;
                }
            };
        }
    }

    fn start_session(&mut self) {
        let session = self.sessions[self.session].clone();
        let mut env: Vec<String> = vec![
            "XDG_SESSION_DESKTOP=tuilnux".into(),
            "XDG_CURRENT_DESKTOP=TUILNUX".into(),
            "XDG_SESSION_TYPE=tty".into(),
        ];
        // Die Rahmen-Einstellung des Greeters gilt auch für die Sitzung.
        if let Ok(v) = std::env::var(tuilnux_ui::ASCII_ENV) {
            env.push(format!("{}={v}", tuilnux_ui::ASCII_ENV));
        }
        let request = Request::StartSession {
            cmd: session.cmd.clone(),
            env,
        };
        match self.call(request) {
            Some(Response::Success) => {
                self.info(trf("Willkommen! Starte {} …", &[&session.name]));
                self.remember_user();
                self.failures = 0;
                self.lockouts = 0;
                self.started = Some(session);
            }
            Some(Response::Error { description, .. }) => {
                self.error(trf("Sitzung konnte nicht starten: {}", &[&description]));
                self.cancel();
                self.reset_to_user();
            }
            Some(_) => {
                self.error("Unerwartete Antwort von greetd");
                self.cancel();
                self.reset_to_user();
            }
            None => {}
        }
    }

    fn remember_user(&self) {
        let (true, Some(dir)) = (self.cfg.remember_user, &self.state_dir) else {
            return;
        };
        let name = self.user.text().trim();
        if valid_username(name) {
            let _ = std::fs::write(dir.join(setup::LAST_USER_FILE), format!("{name}\n"));
        }
    }

    fn power(&mut self, action: &str) {
        self.user.wipe();
        if !self.cfg.allow_power {
            self.error("Ausschalten/Neustart ist am Anmeldebildschirm gesperrt");
            return;
        }
        if self.backend.is_demo() || !cfg!(unix) {
            self.info(format!("Demo: würde jetzt „systemctl {action}“ ausführen"));
            return;
        }
        match std::process::Command::new("systemctl").arg(action).status() {
            Ok(s) if s.success() => self.info("Wird ausgeführt …"),
            Ok(_) => self.error(trf("systemctl {} wurde abgelehnt", &[&action])),
            Err(e) => self.error(trf("systemctl nicht gefunden: {}", &[&e])),
        }
    }

    /// Anzeige des Geheimnisses je nach Einstellung, dazu die Cursorposition.
    fn shown_secret(&self) -> (String, usize) {
        let n = self.secret.text().chars().count();
        match &self.field {
            Field::Secret { echo: true, .. } => {
                (self.secret.text().to_string(), self.secret.cursor())
            }
            _ => match self.cfg.secret_display {
                SecretDisplay::Dots => (self.text(&"•".repeat(n)), self.secret.cursor()),
                SecretDisplay::Mask if n > 0 => (self.text(&"•".repeat(MASK_LEN)), MASK_LEN),
                SecretDisplay::Mask | SecretDisplay::Hidden => (String::new(), 0),
            },
        }
    }

    /// Eine Eingabezeile im Stil `Username: jonas█` mit selbst gezeichnetem Blockcursor.
    fn input_line(
        &self,
        label: &str,
        value: &str,
        cursor: Option<usize>,
        width: u16,
    ) -> Line<'static> {
        let label = tr(label);
        let label_w = tr(USER_LABEL).chars().count().max(label.chars().count());
        let mut spans = vec![Span::styled(
            format!("{label:<label_w$} "),
            Style::new().fg(self.accent()),
        )];
        let room = (width as usize).saturating_sub(label_w + 2).max(1);
        let chars: Vec<char> = value.chars().collect();
        let end = cursor
            .map_or(chars.len(), |c| c + 1)
            .max(chars.len().min(room));
        let skip = end.saturating_sub(room);
        let text = Style::new().fg(theme::fg());
        match cursor {
            Some(c) => {
                let c = c - skip.min(c);
                let shown = &chars[skip.min(chars.len())..];
                let before: String = shown.iter().take(c).collect();
                let under = shown.get(c).map_or(" ".to_string(), |ch| ch.to_string());
                let after: String = shown
                    .iter()
                    .skip(c + 1)
                    .take(room.saturating_sub(c + 1))
                    .collect();
                let cursor_style = if self.blink {
                    Style::new().fg(self.bg()).bg(CURSOR)
                } else {
                    text
                };
                spans.push(Span::styled(before, text));
                spans.push(Span::styled(under, cursor_style));
                spans.push(Span::styled(after, text));
            }
            None => spans.push(Span::styled(
                chars.iter().take(room).collect::<String>(),
                Style::new().fg(theme::dim()),
            )),
        }
        Line::from(spans)
    }

    fn logo(&self, width: u16, height: u16) -> Vec<String> {
        let lines: Vec<String> = match self.cfg.logo {
            LogoStyle::None => return Vec::new(),
            LogoStyle::Thin | LogoStyle::Bold if width >= 52 && height >= 20 => logo::big()
                .into_iter()
                .zip(logo::ICON)
                .map(|(l, i)| format!("{l}   {i}"))
                .collect(),
            _ if width >= 20 && height >= 14 => logo::small(),
            _ => vec!["TUILNUX".into()],
        };
        lines
            .iter()
            .map(|l| match self.cfg.logo {
                LogoStyle::Bold => self.text(l),
                _ if self.ascii || self.linux_console => self.text(l),
                _ => logo::thin(l),
            })
            .collect()
    }

    fn help_lines(&self) -> Vec<String> {
        let mut v = vec!["help      Hilfe ein-/ausblenden".to_string()];
        if self.cfg.allow_power {
            v.push("reboot    Neustart  ·  poweroff  Ausschalten".into());
        }
        v.push(if self.cfg.allow_session_switch {
            "F2        Sitzung wechseln  ·  Esc  zurück".into()
        } else {
            "Esc       zurück".into()
        });
        v.push("Enter     bestätigen  ·  Strg+U  Feld leeren".into());
        if self.cfg.key.enabled() {
            v.push("Schlüssel einstecken, berühren, wenn er blinkt".into());
        }
        if self.cfg.otp.enabled() {
            v.push("Code      6 Ziffern aus der Authenticator-App".into());
        }
        v.into_iter().map(|l| tr(&l).to_string()).collect()
    }

    /// Hinweis unter der Box, abhängig vom Schritt.
    fn hint(&self) -> String {
        match &self.field {
            Field::Secret { prompt, .. } if prompt == "Code:" => {
                if self.cfg.otp == FactorMode::Alternative {
                    tr("Code aus der Authenticator-App – leer lassen für Passwort").into()
                } else {
                    tr("Code aus der Authenticator-App eingeben").into()
                }
            }
            _ => tr("type \"help\" for more information").into(),
        }
    }
}

impl App for Login {
    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let accent = self.accent();
        let dim = self.dim();
        frame.render_widget(Block::new().style(Style::new().bg(self.bg())), area);

        let width = area.width.saturating_sub(4).min(64);
        let logo = self.logo(width, area.height);
        let help: Vec<String> = if self.help {
            self.help_lines()
        } else {
            vec![self.hint()]
        };
        let notice = self.cfg.notice.trim();
        let notice_h: u16 = if notice.is_empty() {
            0
        } else {
            (notice.chars().count() as u16 / width.max(1) + 1).min(4) + 1
        };
        let caps_h: u16 = u16::from(self.cfg.caps_warning && self.caps);
        let help_h = help.len() as u16;
        let logo_h = logo.len() as u16;
        let logo_gap: u16 = if logo_h > 0 { 2 } else { 0 };
        let content_h = logo_h + logo_gap + 4 + 1 + help_h + caps_h + 2 + notice_h;
        let [_, column, _] = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(width),
            Constraint::Fill(1),
        ])
        .areas(area);
        let [_, content, _, footer] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(content_h),
            Constraint::Fill(2),
            Constraint::Length(1),
        ])
        .areas(column);
        let [
            logo_area,
            _,
            box_area,
            _,
            hint_area,
            caps_area,
            message_area,
            notice_area,
        ] = Layout::vertical([
            Constraint::Length(logo_h),
            Constraint::Length(logo_gap),
            Constraint::Length(4),
            Constraint::Length(1),
            Constraint::Length(help_h),
            Constraint::Length(caps_h),
            Constraint::Length(2),
            Constraint::Length(notice_h),
        ])
        .areas(content);

        let mut logo_style = Style::new().fg(accent);
        if self.cfg.logo == LogoStyle::Bold {
            logo_style = logo_style.add_modifier(Modifier::BOLD);
        }
        let logo_lines: Vec<Line> = logo
            .into_iter()
            .map(|l| Line::styled(l, logo_style))
            .collect();
        frame.render_widget(
            Paragraph::new(logo_lines).alignment(ratatui::layout::Alignment::Center),
            logo_area,
        );

        // Eingabebox mit „user@host:~$“ im Rahmen, wie ein Shell-Prompt.
        let typed = self.user.text().trim();
        let (who, who_style) = if typed.is_empty() {
            ("user".to_string(), Style::new().fg(dim))
        } else {
            (typed.to_string(), Style::new().fg(accent))
        };
        let mut title = vec![Span::raw(" "), Span::styled(who, who_style)];
        if self.cfg.show_host {
            title.push(Span::styled(
                format!("@{}", self.host),
                Style::new().fg(accent),
            ));
        }
        title.push(Span::styled(":~$ ", Style::new().fg(accent)));
        let mut block = Block::new()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(accent))
            .title(Line::from(title));
        if self.ascii {
            block = block.border_set(ratatui::symbols::border::Set {
                top_left: "+",
                top_right: "+",
                bottom_left: "+",
                bottom_right: "+",
                vertical_left: "|",
                vertical_right: "|",
                horizontal_top: "-",
                horizontal_bottom: "-",
            });
        }
        let inner = block.inner(box_area).inner(Margin::new(1, 0));
        frame.render_widget(block, box_area);
        let [user_row, secret_row] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);

        let locked = self.locked_left();
        let secret_label = match &self.field {
            Field::Secret { prompt, .. } => prompt.clone(),
            Field::User => "Password:".into(),
        };
        let on_user = self.field == Field::User;
        let user_line = self.input_line(
            USER_LABEL,
            self.user.text(),
            (on_user && locked.is_none()).then(|| self.user.cursor()),
            inner.width,
        );
        frame.render_widget(user_line, user_row);
        let (secret_shown, secret_cursor) = self.shown_secret();
        let secret_line = self.input_line(
            &secret_label,
            &secret_shown,
            (!on_user && locked.is_none()).then_some(secret_cursor),
            inner.width,
        );
        frame.render_widget(secret_line, secret_row);

        let hint_style = Style::new().fg(dim);
        let hint: Vec<Line> = help
            .into_iter()
            .map(|l| Line::styled(l, hint_style))
            .collect();
        frame.render_widget(Paragraph::new(hint), hint_area);

        if caps_h > 0 {
            frame.render_widget(
                Line::styled(
                    self.text(tr("⚠ Feststelltaste ist an")),
                    Style::new().fg(theme::warn()),
                ),
                caps_area,
            );
        }

        let message = match locked {
            Some(secs) => Some((
                trf("Zu viele Fehlversuche – bitte {} s warten", &[&secs]),
                Tone::Error,
            )),
            None => self.message.clone(),
        };
        if let Some((text, tone)) = message {
            let color = if tone == Tone::Error {
                theme::error()
            } else {
                accent
            };
            frame.render_widget(
                Paragraph::new(Line::styled(text, Style::new().fg(color)))
                    .wrap(Wrap { trim: true }),
                message_area,
            );
        }

        if notice_h > 0 {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    notice.to_string(),
                    Style::new().fg(theme::fg()),
                ))
                .wrap(Wrap { trim: true }),
                notice_area.inner(Margin::new(0, 0)),
            );
        }

        let mut spans = Vec::new();
        if self.cfg.allow_session_switch {
            let session = &self.sessions[self.session];
            spans.push(Span::styled("F2 ", Style::new().fg(accent)));
            spans.push(Span::styled(
                trf("Sitzung: {}  ", &[&session.name]),
                hint_style,
            ));
        }
        let factors: Vec<&str> = [
            (self.cfg.key.enabled(), "Schlüssel"),
            (self.cfg.otp.enabled(), "Code"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(tr(name)))
        .collect();
        if !factors.is_empty() {
            spans.push(Span::styled(format!("{} ", factors.join("+")), hint_style));
        }
        if self.backend.is_demo() {
            spans.push(Span::styled(
                format!("DEMO: {}", greetd::DEMO_PASSWORD),
                Style::new().fg(theme::warn()),
            ));
        }
        if self.cfg.show_clock {
            let date = self.text(&german_date());
            let used: usize = spans
                .iter()
                .map(|s| s.content.chars().count())
                .sum::<usize>()
                + date.chars().count();
            spans.push(Span::raw(
                " ".repeat((footer.width as usize).saturating_sub(used)),
            ));
            spans.push(Span::styled(date, hint_style));
        }
        frame.render_widget(Line::from(spans), footer);
    }

    fn on_key(&mut self, key: KeyEvent, terminal: &mut DefaultTerminal) -> io::Result<Flow> {
        self.blink = true;
        self.last_input = Instant::now();
        if let Some(caps) = console::caps_lock() {
            self.caps = caps;
        }
        if tuilnux_ui::is_ctrl(&key, 'c') && self.backend.is_demo() {
            return Ok(Flow::Quit);
        }
        if self.locked_left().is_some() {
            return Ok(Flow::Continue);
        }
        if key.code == KeyCode::F(2) {
            if self.cfg.allow_session_switch {
                self.session = (self.session + 1) % self.sessions.len();
            }
            return Ok(Flow::Continue);
        }
        if tuilnux_ui::is_ctrl(&key, 'c') {
            self.clear_all();
            return Ok(Flow::Continue);
        }
        let typing =
            matches!(key.code, KeyCode::Char(_)) && !key.modifiers.contains(KeyModifiers::CONTROL);
        match self.field {
            Field::User => match key.code {
                KeyCode::Tab | KeyCode::Down => self.submit_user(terminal),
                KeyCode::Esc => {
                    self.help = false;
                    self.message = None;
                }
                _ if typing && self.user.text().chars().count() >= MAX_USER => {}
                _ => {
                    if self.user.handle_key(key) == InputResult::Submit {
                        self.submit_user(terminal);
                    }
                }
            },
            Field::Secret { .. } => match key.code {
                KeyCode::Esc | KeyCode::Up | KeyCode::BackTab => {
                    self.cancel();
                    self.reset_to_user();
                    self.message = None;
                }
                // Bei verdeckter Länge kein Springen im Feld.
                KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End
                    if self.cfg.secret_display != SecretDisplay::Dots => {}
                _ if typing && self.secret.text().chars().count() >= MAX_SECRET => {}
                _ => {
                    if self.secret.handle_key(key) == InputResult::Submit {
                        self.submit_secret(terminal);
                    }
                }
            },
        }
        Ok(if self.started.is_some() {
            Flow::Quit
        } else {
            Flow::Continue
        })
    }

    fn on_paste(&mut self, text: String) {
        if self.locked_left().is_some() {
            return;
        }
        self.last_input = Instant::now();
        let (input, max) = if self.field == Field::User {
            (&mut self.user, MAX_USER)
        } else {
            (&mut self.secret, MAX_SECRET)
        };
        for c in text.chars().filter(|c| !c.is_control()) {
            if input.text().chars().count() >= max {
                break;
            }
            input.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let mut text = text;
        tuilnux_ui::wipe_string(&mut text);
    }

    fn tick(&mut self) {
        self.blink = !self.blink;
        if let Some(caps) = console::caps_lock() {
            self.caps = caps;
        }
        if let Some(until) = self.locked_until
            && Instant::now() >= until
        {
            self.locked_until = None;
            self.info("Du kannst es erneut versuchen");
        }
        let idle = self.cfg.idle_timeout;
        let busy = self.field != Field::User || !self.user.text().is_empty() || self.help;
        if idle > 0 && busy && self.last_input.elapsed() >= Duration::from_secs(u64::from(idle)) {
            self.clear_all();
            self.info("Eingaben wegen Inaktivität gelöscht");
        }
    }

    fn tick_rate(&self) -> Duration {
        Duration::from_millis(530)
    }

    // Auf der Linux-Konsole schreiben systemd-Startmeldungen und Kernel-Logs
    // direkt in den Bildschirm; ratatui zeichnet aber nur geänderte Zellen neu.
    fn wants_clear(&mut self) -> bool {
        if !self.linux_console {
            return false;
        }
        let interval = if self.launched.elapsed() < Duration::from_secs(20) {
            Duration::from_millis(500)
        } else {
            Duration::from_secs(5)
        };
        if self.last_clear.elapsed() < interval {
            return false;
        }
        self.last_clear = Instant::now();
        true
    }
}

const HELP: &str = "\
tlogin – Anmeldebildschirm von TUILNUX (greetd-Greeter)

Aufruf: tlogin [Optionen]
  --cmd <befehl>           Standardsitzung (Standard: tuilnux)
  --session <Name=befehl>  weitere Sitzung, mehrfach möglich (F2 wechselt)
  --user <name>            Benutzername vorausfüllen
  --config <datei>         Einstellungen statt /etc/tuilnux/login.toml
  --demo                   ohne greetd testen (Passwort: tuilnux)

  tlogin setup …           Einstellungen systemweit übernehmen (tlogin setup --help)

greetd-Konfiguration (/etc/greetd/config.toml):
  [default_session]
  command = \"tlogin --cmd tuilnux-session\"
  user = \"greeter\"";

/// Lädt die Einstellungen; Fehler ergeben die Standardwerte plus Meldung.
fn load_settings(file: Option<&PathBuf>) -> (LoginSettings, Option<String>) {
    let loaded = match file {
        Some(f) => std::fs::read_to_string(f)
            .map_err(anyhow::Error::from)
            .and_then(|s| LoginSettings::parse(&s))
            .map_err(|e| format!("{}: {e:#}", f.display())),
        None => LoginSettings::load_system().map_err(|e| format!("{e:#}")),
    };
    match loaded {
        Ok(s) => (s, None),
        Err(e) => (
            LoginSettings::default(),
            Some(format!(
                "Einstellungen fehlerhaft, Standardwerte aktiv – {e}"
            )),
        ),
    }
}

/// Ohne greetd nur mit ausdrücklichem `--demo` – sonst könnte ein falsch
/// konfigurierter Start mit dem bekannten Demo-Passwort eine Sitzung öffnen.
fn backend(demo: bool) -> anyhow::Result<Box<dyn Backend>> {
    if demo {
        return Ok(Box::new(greetd::Demo::default()));
    }
    if std::env::var_os("GREETD_SOCK").is_none() {
        anyhow::bail!(
            "GREETD_SOCK ist nicht gesetzt – tlogin läuft unter greetd.\n\
             Zum Ausprobieren: tlogin --demo"
        );
    }
    Ok(Box::new(greetd::connect()?))
}

fn main() -> anyhow::Result<()> {
    let all: Vec<String> = std::env::args().skip(1).collect();
    if all.first().map(String::as_str) == Some("setup") {
        return setup::run(&all[1..]);
    }
    let mut args = all.into_iter();
    let mut default_cmd = vec!["tuilnux".to_string()];
    let mut extra = Vec::new();
    let mut user = None;
    let mut demo = false;
    let mut config = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{HELP}");
                return Ok(());
            }
            "--demo" => demo = true,
            "--cmd" => {
                let cmd = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--cmd braucht einen Befehl"))?;
                default_cmd = cmd.split_whitespace().map(String::from).collect();
                anyhow::ensure!(!default_cmd.is_empty(), "--cmd ist leer");
            }
            "--session" => {
                let spec = args.next().unwrap_or_default();
                let session = Session::parse(&spec).ok_or_else(|| {
                    anyhow::anyhow!("--session erwartet Name=befehl, nicht „{spec}“")
                })?;
                extra.push(session);
            }
            "--user" => user = args.next(),
            "--config" => {
                config =
                    Some(PathBuf::from(args.next().ok_or_else(|| {
                        anyhow::anyhow!("--config braucht eine Datei")
                    })?));
            }
            other => anyhow::bail!("Unbekannte Option „{other}“ (siehe --help)"),
        }
    }
    let mut sessions = vec![Session {
        name: "TUILNUX".into(),
        cmd: default_cmd,
    }];
    if extra.is_empty() {
        extra.push(Session {
            name: "Terminal".into(),
            cmd: vec!["tterm".into()],
        });
        extra.push(Session {
            name: "Bash".into(),
            cmd: vec!["bash".into(), "-l".into()],
        });
    }
    sessions.extend(extra);

    let backend = backend(demo)?;
    let is_demo = backend.is_demo();
    let (cfg, cfg_error) = load_settings(config.as_ref());
    tuilnux_ui::i18n::init(&cfg.language);
    let state_dir = PathBuf::from(setup::STATE_DIR);
    if user.is_none() && cfg.remember_user && !is_demo {
        user = std::fs::read_to_string(state_dir.join(setup::LAST_USER_FILE))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| valid_username(s));
    }
    let mut login = Login::new(backend, sessions, user, cfg);
    if !is_demo {
        login.state_dir = Some(state_dir);
    }
    if let Some(e) = cfg_error {
        login.error(e);
    }
    let palette = login.linux_console;
    if palette {
        use std::io::Write;
        let mut out = io::stdout();
        let _ = write!(out, "{}", login.console_palette());
        let _ = out.flush();
    }
    let result = tuilnux_ui::run(&mut login);
    if palette {
        use std::io::Write;
        let mut out = io::stdout();
        let _ = write!(out, "\x1b]R");
        let _ = out.flush();
    }
    result?;

    // Im Demo-Modus die Sitzung selbst starten, damit man den Ablauf ausprobieren kann.
    if let (true, Some(session)) = (is_demo, login.started.take()) {
        let program = tuilnux_ui::tool_path(&session.cmd[0]);
        if let Err(e) = std::process::Command::new(&program)
            .args(&session.cmd[1..])
            .status()
        {
            eprintln!(
                "Demo: {} konnte nicht gestartet werden: {e}",
                program.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::{CrosstermBackend, TestBackend};
    use ratatui::layout::Rect;
    use ratatui::{Terminal, TerminalOptions, Viewport};

    fn login_with(cfg: LoginSettings) -> Login {
        let sessions = vec![
            Session {
                name: "TUILNUX".into(),
                cmd: vec!["tterm".into()],
            },
            Session {
                name: "Bash".into(),
                cmd: vec!["bash".into()],
            },
        ];
        let mut l = Login::new(Box::new(greetd::Demo::default()), sessions, None, cfg);
        l.host = "arch".into();
        l.linux_console = false;
        l.ascii = false;
        l.caps = false;
        l
    }

    fn login() -> Login {
        login_with(LoginSettings::default())
    }

    /// Terminal ohne Fläche, damit Zwischen-Redraws im Test nichts ausgeben.
    fn dummy_terminal() -> DefaultTerminal {
        Terminal::with_options(
            CrosstermBackend::new(io::stdout()),
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, 0, 0)),
            },
        )
        .unwrap()
    }

    fn key(l: &mut Login, t: &mut DefaultTerminal, code: KeyCode) -> Flow {
        l.on_key(KeyEvent::new(code, KeyModifiers::NONE), t)
            .unwrap()
    }

    fn typ(l: &mut Login, t: &mut DefaultTerminal, text: &str) -> Flow {
        let mut flow = Flow::Continue;
        for c in text.chars() {
            let code = if c == '\n' {
                KeyCode::Enter
            } else {
                KeyCode::Char(c)
            };
            flow = key(l, t, code);
        }
        flow
    }

    fn screen(l: &mut Login, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| l.draw(f)).unwrap();
        let buf = t.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn full_login_flow() {
        let mut l = login();
        let mut t = dummy_terminal();
        typ(&mut l, &mut t, "jonas\n");
        assert!(matches!(l.field, Field::Secret { .. }));
        typ(&mut l, &mut t, "falsch\n");
        assert!(matches!(l.field, Field::Secret { .. }));
        assert_eq!(l.message.as_ref().unwrap().1, Tone::Error);
        assert!(l.secret.text().is_empty());
        let s = screen(&mut l, 100, 30);
        assert!(s.contains("jonas@arch:~$"), "{s}");
        assert!(s.contains("Falsches Passwort"), "{s}");
        let flow = typ(&mut l, &mut t, "tuilnux\n");
        assert_eq!(flow, Flow::Quit);
        assert_eq!(l.started.as_ref().unwrap().cmd, ["tterm"]);
    }

    #[test]
    fn help_and_session_switch() {
        let mut l = login();
        let mut t = dummy_terminal();
        typ(&mut l, &mut t, "help\n");
        assert!(l.help);
        assert!(screen(&mut l, 100, 30).contains("poweroff"));
        key(&mut l, &mut t, KeyCode::F(2));
        assert_eq!(l.session, 1);
        typ(&mut l, &mut t, "jonas\n");
        key(&mut l, &mut t, KeyCode::Esc);
        assert_eq!(l.field, Field::User);
        assert_eq!(l.user.text(), "jonas");
    }

    #[test]
    fn design_like_the_template() {
        let mut l = login();
        let s = screen(&mut l, 100, 30);
        assert!(s.contains("└───┘"), "dünnes Logo fehlt:\n{s}");
        assert!(s.contains("╱╲"), "Arch-Symbol fehlt:\n{s}");
        assert!(s.contains("user@arch:~$"), "{s}");
        assert!(s.contains("Username:") && s.contains("Password:"), "{s}");
        assert!(s.contains("type \"help\" for more information"), "{s}");
        let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
        t.draw(|f| l.draw(f)).unwrap();
        assert_eq!(t.backend().buffer()[(0, 0)].bg, Color::Rgb(0, 0, 0));
        let bold = login_with(LoginSettings {
            logo: LogoStyle::Bold,
            show_host: false,
            ..Default::default()
        });
        let s = screen(&mut { bold }, 100, 30);
        assert!(
            s.contains("┗━━━┛") && s.contains("user:~$") && !s.contains("@arch"),
            "{s}"
        );
        let mut none = login_with(LoginSettings {
            logo: LogoStyle::None,
            ..Default::default()
        });
        assert!(!screen(&mut none, 100, 30).contains('╱'));
    }

    #[test]
    fn renders_small_screens_and_masks_password() {
        let mut l = login();
        let mut t = dummy_terminal();
        typ(&mut l, &mut t, "jonas\ngeheim");
        let s = screen(&mut l, 100, 30);
        // Uhrzeit im Footer wird vollständig angezeigt.
        assert!(
            s.lines()
                .last()
                .unwrap()
                .trim_end()
                .chars()
                .last()
                .unwrap()
                .is_ascii_digit(),
            "{s}"
        );
        assert!(s.contains("Username: jonas"), "{s}");
        assert!(s.contains("Password: ••••••"), "{s}");
        assert!(!s.contains("geheim"));
        for (w, h) in [(80, 24), (40, 14), (12, 5), (1, 1)] {
            screen(&mut l, w, h);
        }
        l.ascii = true;
        let s = screen(&mut l, 80, 24);
        assert!(
            !s.contains('┃') && !s.contains('•') && !s.contains('┌'),
            "{s}"
        );
    }

    #[test]
    fn secret_display_modes() {
        let mut t = dummy_terminal();
        let mut mask = login_with(LoginSettings {
            secret_display: SecretDisplay::Mask,
            ..Default::default()
        });
        typ(&mut mask, &mut t, "jonas\nab");
        assert!(screen(&mut mask, 100, 30).contains("Password: ••••••••"));
        typ(&mut mask, &mut t, "cdefghijk");
        let s = screen(&mut mask, 100, 30);
        assert!(
            s.contains("Password: ••••••••") && !s.contains("•••••••••"),
            "{s}"
        );
        let mut hidden = login_with(LoginSettings {
            secret_display: SecretDisplay::Hidden,
            ..Default::default()
        });
        typ(&mut hidden, &mut t, "jonas\ngeheim");
        assert!(!screen(&mut hidden, 100, 30).contains('•'));
        typ(&mut hidden, &mut t, "\n");
        assert_eq!(hidden.secret.text(), "");
    }

    #[test]
    fn lockout_after_failures_doubles() {
        let mut l = login_with(LoginSettings {
            max_attempts: 2,
            lockout_seconds: 10,
            ..Default::default()
        });
        let mut t = dummy_terminal();
        typ(&mut l, &mut t, "jonas\nx\n");
        assert!(l.locked_left().is_none());
        typ(&mut l, &mut t, "y\n");
        let left = l.locked_left().unwrap();
        assert!((9..=10).contains(&left), "{left}");
        assert!(screen(&mut l, 100, 30).contains("Zu viele Fehlversuche"));
        // Gesperrt: Tasten (auch das richtige Passwort) werden ignoriert.
        assert_eq!(typ(&mut l, &mut t, "tuilnux\n"), Flow::Continue);
        assert!(l.started.is_none() && l.secret.text().is_empty());
        l.locked_until = Some(Instant::now());
        l.tick();
        typ(&mut l, &mut t, "a\nb\n");
        assert!((19..=20).contains(&l.locked_left().unwrap()));
    }

    #[test]
    fn idle_timeout_clears_everything() {
        let mut l = login();
        let mut t = dummy_terminal();
        typ(&mut l, &mut t, "jonas\ngeh");
        l.last_input = Instant::now() - Duration::from_secs(121);
        l.tick();
        assert_eq!(l.field, Field::User);
        assert!(l.user.text().is_empty() && l.secret.text().is_empty());
        assert!(l.message.as_ref().unwrap().0.contains("Inaktivität"));
    }

    #[test]
    fn power_and_session_switch_can_be_disabled() {
        let mut l = login_with(LoginSettings {
            allow_power: false,
            allow_session_switch: false,
            notice: "Nur für befugte Personen".into(),
            ..Default::default()
        });
        let mut t = dummy_terminal();
        typ(&mut l, &mut t, "reboot\n");
        assert_eq!(l.message.as_ref().unwrap().1, Tone::Error);
        key(&mut l, &mut t, KeyCode::F(2));
        assert_eq!(l.session, 0);
        typ(&mut l, &mut t, "help\n");
        let s = screen(&mut l, 100, 30);
        assert!(!s.contains("poweroff") && !s.contains("F2"), "{s}");
        assert!(s.contains("Nur für befugte Personen"), "{s}");
    }

    #[test]
    fn rejects_bad_usernames_and_limits_length() {
        let mut l = login();
        let mut t = dummy_terminal();
        typ(&mut l, &mut t, "../etc\n");
        assert_eq!(l.field, Field::User);
        assert_eq!(l.message.as_ref().unwrap().1, Tone::Error);
        l.user.wipe();
        typ(&mut l, &mut t, &"x".repeat(200));
        assert_eq!(l.user.text().len(), MAX_USER);
        let s = screen(&mut l, 40, 20);
        assert!(s.contains("Username: xxxx"), "{s}");
        assert!(valid_username("jonas.k-2") && !valid_username("-x") && !valid_username("a b"));
    }

    #[test]
    fn parses_sessions_prompts_and_messages() {
        assert_eq!(
            Session::parse("Sway=sway --unsupported-gpu"),
            Some(Session {
                name: "Sway".into(),
                cmd: vec!["sway".into(), "--unsupported-gpu".into()]
            })
        );
        assert_eq!(Session::parse("kaputt"), None);
        assert_eq!(prompt_label("Password: "), "Password:");
        assert_eq!(prompt_label("Verification code: "), "Code:");
        assert_eq!(prompt_label("Please enter the PIN: "), "PIN:");
        assert_eq!(prompt_label("New password: "), "Neues Passwort:");
        assert_eq!(prompt_label("Sonstwas"), "Sonstwas:");
        assert_eq!(
            translate("Please touch the device."),
            "Bitte Sicherheitsschlüssel berühren …"
        );
        assert_eq!(
            translate("The account is locked due to 5 failed logins."),
            "Konto gesperrt nach 5 Fehlversuchen"
        );
        assert_eq!(
            translate("(9 minutes left to unlock)"),
            "(noch 9 Min. bis zur Entsperrung)"
        );
    }

    #[test]
    fn demo_needs_flag_and_bad_config_falls_back() {
        if std::env::var_os("GREETD_SOCK").is_none() {
            assert!(backend(false).is_err());
        }
        assert!(backend(true).unwrap().is_demo());
        let file = std::env::temp_dir().join(format!("tlogin-cfg-{}.toml", std::process::id()));
        std::fs::write(&file, "accent = \"#ff0000\"\nlogo = \"small\"\n").unwrap();
        let (cfg, err) = load_settings(Some(&file));
        assert!(err.is_none());
        assert_eq!(cfg.accent, "#ff0000");
        std::fs::write(&file, "accent = 5").unwrap();
        let (cfg, err) = load_settings(Some(&file));
        assert_eq!(cfg, LoginSettings::default());
        assert!(err.unwrap().contains("Standardwerte"));
        let _ = std::fs::remove_file(file);
    }
}
