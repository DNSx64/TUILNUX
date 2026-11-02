//! IPC zwischen der TUILNUX-Sitzung und Programmen im Terminal.
//!
//! Protokoll: eine JSON-Zeile pro Anfrage, eine JSON-Zeile als Antwort.
//! Transport: Unix-Socket (Rechte 0600) unter Linux, sonst TCP auf 127.0.0.1.
//! Jede Anfrage trägt das Sitzungs-Token, das nur an Kindprozesse der Sitzung
//! weitergegeben wird.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

pub const SOCK_ENV: &str = "TUILNUX_SOCK";
pub const TOKEN_ENV: &str = "TUILNUX_TOKEN";
pub const WINDOW_ENV: &str = "TUILNUX_WINDOW";

const MAX_LINE: usize = 64 * 1024;
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Programm in einem neuen Fenster starten.
    Open {
        command: Vec<String>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        cwd: Option<String>,
        /// Innengröße (Spalten, Zeilen).
        #[serde(default)]
        size: Option<(u16, u16)>,
    },
    /// Programm im Hintergrund starten (eigenes Terminal, aber ohne Fenster);
    /// `Focus` mit der ID zeigt seine Ausgabe (`tab --shell NAME`).
    Launch {
        command: Vec<String>,
        #[serde(default)]
        cwd: Option<String>,
    },
    List,
    Focus {
        id: usize,
    },
    /// Fenster schließen (Prozess beenden).
    Close {
        id: usize,
    },
    Minimize {
        id: usize,
    },
    Maximize {
        id: usize,
    },
    /// Meldung in der Statusleiste anzeigen.
    Notify {
        text: String,
    },
    Logout,
    /// Einstellungen (tuilnux.toml) neu laden.
    Reload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: usize,
    pub title: String,
    pub command: String,
    pub pid: Option<u32>,
    pub focused: bool,
    pub minimized: bool,
    pub cpu: f32,
    pub memory: u64,
    /// Grafisches Programm unter labwc (kein Terminalfenster).
    #[serde(default)]
    pub gui: bool,
    /// Im Hintergrund gestartet (`tuilnux launch`): das Terminal des Programms,
    /// erreichbar mit `tab --shell NAME`.
    #[serde(default)]
    pub background: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Opened { id: usize },
    Tasks { tasks: Vec<TaskInfo> },
    Error { message: String },
}

impl Response {
    pub fn error(message: impl Into<String>) -> Self {
        Response::Error {
            message: message.into(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    token: String,
    #[serde(flatten)]
    request: Request,
}

/// Zufälliges Token ohne zusätzliche Abhängigkeit (SipHash mit zufälligem Schlüssel).
pub fn random_token() -> String {
    let mut out = String::new();
    for i in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u32(std::process::id());
        if let Ok(t) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            h.write_u128(t.as_nanos());
        }
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Verarbeitet eine Verbindung: Zeile lesen, prüfen, an die Sitzung geben, antworten.
fn serve<S: Read + Write>(
    stream: S,
    writer: impl Write,
    token: &str,
    handler: &(dyn Fn(Request, Sender<Response>) + Send + Sync),
) -> io::Result<()> {
    let mut reader = BufReader::new(stream).take(MAX_LINE as u64);
    let mut writer = writer;
    let mut line = String::new();
    while {
        line.clear();
        reader.read_line(&mut line)? > 0
    } {
        if !line.ends_with('\n') && line.len() >= MAX_LINE {
            let reply = Response::error("Anfrage zu groß");
            writeln!(writer, "{}", serde_json::to_string(&reply)?)?;
            return Ok(());
        }
        reader.set_limit(MAX_LINE as u64);
        let reply = match serde_json::from_str::<Envelope>(line.trim()) {
            Err(e) => Response::error(format!("ungültige Anfrage: {e}")),
            Ok(env) if !same(&env.token, token) => Response::error("falsches Token"),
            Ok(env) => {
                let (tx, rx) = mpsc::channel();
                handler(env.request, tx);
                rx.recv_timeout(REPLY_TIMEOUT)
                    .unwrap_or_else(|_| Response::error("Sitzung antwortet nicht"))
            }
        };
        writeln!(writer, "{}", serde_json::to_string(&reply)?)?;
        writer.flush()?;
    }
    Ok(())
}

pub type Handler = Arc<dyn Fn(Request, Sender<Response>) + Send + Sync>;

/// Laufender IPC-Server. Beim Drop wird die Socket-Datei entfernt.
pub struct Server {
    address: String,
    token: String,
    #[cfg_attr(not(unix), allow(dead_code))]
    path: Option<PathBuf>,
    /// Mit [`Server::publish`] geschriebene Sitzungsdatei.
    published: Option<PathBuf>,
}

impl Server {
    /// Adresse im Format `unix:/pfad` oder `tcp:127.0.0.1:port`.
    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// Umgebungsvariablen für Kindprozesse.
    pub fn env(&self) -> [(String, String); 2] {
        [
            (SOCK_ENV.into(), self.address.clone()),
            (TOKEN_ENV.into(), self.token.clone()),
        ]
    }

    pub fn start(
        handler: impl Fn(Request, Sender<Response>) + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let handler: Handler = Arc::new(handler);
        let token = random_token();
        Self::start_platform(handler, token)
    }

    #[cfg(unix)]
    fn start_platform(handler: Handler, token: String) -> io::Result<Self> {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;
        use std::sync::atomic::{AtomicU32, Ordering};

        // Mehrere Server pro Prozess (z. B. parallele Tests) brauchen eigene Pfade.
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        let path = dir.join(format!("tuilnux-{}-{n}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let t = token.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (handler, token) = (handler.clone(), t.clone());
                thread::spawn(move || {
                    if let Ok(writer) = stream.try_clone() {
                        let _ = serve(stream, writer, &token, &*handler);
                    }
                });
            }
        });
        Ok(Self {
            address: format!("unix:{}", path.display()),
            token,
            path: Some(path),
            published: None,
        })
    }

    #[cfg(not(unix))]
    fn start_platform(handler: Handler, token: String) -> io::Result<Self> {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let t = token.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (handler, token) = (handler.clone(), t.clone());
                thread::spawn(move || {
                    if let Ok(writer) = stream.try_clone() {
                        let _ = serve(stream, writer, &token, &*handler);
                    }
                });
            }
        });
        Ok(Self {
            address: format!("tcp:{addr}"),
            token,
            path: None,
            published: None,
        })
    }
}

impl Server {
    /// Veröffentlicht Adresse und Token in [`session_file`], damit auch Programme
    /// die Sitzung finden, die nicht von ihr gestartet wurden (Tastenkürzel des
    /// Compositors wie Super+F). Die Datei liegt im privaten `$XDG_RUNTIME_DIR`.
    pub fn publish(&mut self) -> io::Result<()> {
        let path = session_file()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "$XDG_RUNTIME_DIR fehlt"))?;
        self.publish_to(path)
    }

    fn publish_to(&mut self, path: PathBuf) -> io::Result<()> {
        write_private(&path, &format!("{}\n{}\n", self.address, self.token))?;
        self.published = Some(path);
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
        // Nur entfernen, wenn keine neuere Sitzung die Datei übernommen hat.
        if let Some(path) = &self.published
            && read_session(path).is_some_and(|(address, _)| address == self.address)
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// `$XDG_RUNTIME_DIR/tuilnux/session`: Adresse und Token der laufenden Sitzung.
pub fn session_file() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_dir())
        .map(|p| p.join("tuilnux").join("session"))
}

/// Schreibt `content` atomar in eine nur für den Benutzer lesbare Datei (0600,
/// Ordner 0700).
fn write_private(path: &std::path::Path, content: &str) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "kein Ordner"))?;
    let tmp = dir.join(format!(".session-{}", std::process::id()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let _ = std::fs::remove_file(&tmp);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(content.as_bytes())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)?;
        std::fs::write(&tmp, content)?;
    }
    std::fs::rename(&tmp, path)
}

fn read_session(path: &std::path::Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let address = lines.next()?.trim().to_string();
    let token = lines.next().unwrap_or_default().trim().to_string();
    (!address.is_empty()).then_some((address, token))
}

fn exchange<S: Read + Write>(mut stream: S, line: &str) -> io::Result<Response> {
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut reader = BufReader::new(stream).take(MAX_LINE as u64 * 16);
    let mut answer = String::new();
    reader.read_line(&mut answer)?;
    if answer.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "keine Antwort von der Sitzung",
        ));
    }
    serde_json::from_str(answer.trim()).map_err(io::Error::other)
}

/// Sendet eine Anfrage an die Sitzung unter `address`.
pub fn request(address: &str, token: &str, request: Request) -> io::Result<Response> {
    let line = serde_json::to_string(&Envelope {
        token: token.to_string(),
        request,
    })?;
    if let Some(addr) = address.strip_prefix("tcp:") {
        let stream = std::net::TcpStream::connect(addr)?;
        stream.set_read_timeout(Some(REPLY_TIMEOUT * 2))?;
        return exchange(stream, &line);
    }
    #[cfg(unix)]
    if let Some(path) = address.strip_prefix("unix:") {
        let stream = std::os::unix::net::UnixStream::connect(path)?;
        stream.set_read_timeout(Some(REPLY_TIMEOUT * 2))?;
        return exchange(stream, &line);
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("unbekannte Adresse: {address}"),
    ))
}

/// Sendet eine Anfrage an die Sitzung, in der dieser Prozess läuft (aus der
/// Umgebung, sonst aus der von der Sitzung veröffentlichten [`session_file`]).
pub fn request_env(req: Request) -> io::Result<Response> {
    let (address, token) = match std::env::var(SOCK_ENV) {
        Ok(address) => (address, std::env::var(TOKEN_ENV).unwrap_or_default()),
        Err(_) => session_file()
            .and_then(|p| read_session(&p))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "keine TUILNUX-Sitzung gefunden ($TUILNUX_SOCK fehlt)",
                )
            })?,
    };
    match request(&address, &token, req) {
        // Veraltete Sitzungsdatei (Sitzung abgestürzt): wie keine Sitzung behandeln.
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) && std::env::var_os(SOCK_ENV).is_none() =>
        {
            Err(io::Error::new(io::ErrorKind::NotFound, e))
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_format_is_stable() {
        let line = serde_json::to_string(&Envelope {
            token: "t".into(),
            request: Request::Focus { id: 3 },
        })
        .unwrap();
        assert_eq!(line, r#"{"token":"t","cmd":"focus","id":3}"#);
        let env: Envelope =
            serde_json::from_str(r#"{"token":"t","cmd":"open","command":["ttop"]}"#).unwrap();
        assert_eq!(
            env.request,
            Request::Open {
                command: vec!["ttop".into()],
                title: None,
                cwd: None,
                size: None
            }
        );
        assert_eq!(
            serde_json::to_string(&Response::Opened { id: 2 }).unwrap(),
            r#"{"status":"opened","id":2}"#
        );
    }

    #[test]
    fn tokens_are_random() {
        let (a, b) = (random_token(), random_token());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(same(&a, &a.clone()) && !same(&a, &b) && !same(&a, "x"));
    }

    #[test]
    fn round_trip_over_real_socket() {
        let server = Server::start(|req, reply| {
            let _ = reply.send(match req {
                Request::List => Response::Tasks {
                    tasks: vec![TaskInfo {
                        id: 1,
                        title: "ttop".into(),
                        command: "ttop".into(),
                        pid: Some(42),
                        focused: true,
                        minimized: false,
                        background: false,
                        cpu: 1.5,
                        memory: 1024,
                        gui: false,
                    }],
                },
                Request::Open { .. } => Response::Opened { id: 7 },
                _ => Response::Ok,
            });
        })
        .unwrap();
        let (addr, token) = (server.address().to_string(), server.token().to_string());
        match request(&addr, &token, Request::List).unwrap() {
            Response::Tasks { tasks } => assert_eq!(tasks[0].title, "ttop"),
            other => panic!("{other:?}"),
        }
        let open = Request::Open {
            command: vec!["tfiles".into()],
            title: None,
            cwd: None,
            size: Some((80, 24)),
        };
        assert_eq!(
            request(&addr, &token, open).unwrap(),
            Response::Opened { id: 7 }
        );
        assert!(matches!(
            request(&addr, "falsch", Request::List).unwrap(),
            Response::Error { .. }
        ));
        assert!(request("nirgendwo", &token, Request::List).is_err());
    }

    #[test]
    fn published_session_file_is_private_and_cleaned_up() {
        let dir = std::env::temp_dir().join(format!("tuilnux-ipc-test-{}", random_token()));
        let path = dir.join("tuilnux").join("session");
        let mut server = Server::start(|_req, reply| {
            let _ = reply.send(Response::Ok);
        })
        .unwrap();
        server.publish_to(path.clone()).unwrap();
        let (address, token) = read_session(&path).unwrap();
        assert_eq!(
            (address.as_str(), token.as_str()),
            (server.address(), server.token())
        );
        assert_eq!(
            request(&address, &token, Request::List).unwrap(),
            Response::Ok
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode =
                |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
        }
        // Eine neuere Sitzung hat die Datei übernommen: nicht löschen.
        let mut newer = Server::start(|_req, _reply| {}).unwrap();
        newer.publish_to(path.clone()).unwrap();
        drop(server);
        assert!(path.exists());
        drop(newer);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn silent_session_times_out_gracefully() {
        // Der Handler antwortet nie: der Client bekommt trotzdem eine Fehlermeldung.
        let server = Server::start(|_req, reply| drop(reply)).unwrap();
        let reply = request(server.address(), server.token(), Request::Logout).unwrap();
        assert!(matches!(reply, Response::Error { .. }));
    }
}
