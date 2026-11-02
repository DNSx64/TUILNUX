//! greetd-IPC: JSON-Nachrichten mit vorangestellter u32-Länge (native Byte-Reihenfolge)
//! über den Unix-Socket aus `$GREETD_SOCK`. Dazu ein Demo-Backend zum Testen ohne greetd.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

const MAX_MESSAGE: usize = 1 << 20;
pub const DEMO_PASSWORD: &str = "tuilnux";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    CreateSession { username: String },
    PostAuthMessageResponse { response: Option<String> },
    StartSession { cmd: Vec<String>, env: Vec<String> },
    CancelSession,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Success,
    Error {
        error_type: ErrorType,
        description: String,
    },
    AuthMessage {
        auth_message_type: AuthMessageType,
        auth_message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorType {
    AuthError,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMessageType {
    Visible,
    Secret,
    Info,
    Error,
}

pub fn write_message<W: Write>(w: &mut W, request: &Request) -> io::Result<()> {
    let mut body = serde_json::to_vec(request)?;
    let result = w
        .write_all(&(body.len() as u32).to_ne_bytes())
        .and_then(|()| w.write_all(&body))
        .and_then(|()| w.flush());
    wipe_bytes(&mut body);
    result
}

/// Puffer mit Passwörtern vor dem Freigeben überschreiben.
fn wipe_bytes(buf: &mut Vec<u8>) {
    for b in buf.iter_mut() {
        // SAFETY: Zeiger auf ein gültiges, exklusiv geliehenes Byte.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    buf.clear();
}

/// Löscht ein mitgesendetes Passwort aus der Anfrage.
pub fn wipe_request(request: &mut Request) {
    if let Request::PostAuthMessageResponse {
        response: Some(secret),
    } = request
    {
        tuilnux_ui::wipe_string(secret);
    }
}

pub fn read_message<R: Read>(r: &mut R) -> io::Result<Response> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_ne_bytes(len) as usize;
    if len > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Antwort von greetd ist zu groß",
        ));
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub trait Backend {
    fn request(&mut self, request: Request) -> io::Result<Response>;
    fn is_demo(&self) -> bool {
        false
    }
}

/// Verbindung zu greetd über einen beliebigen Byte-Stream.
pub struct Greetd<S>(pub S);

impl<S: Read + Write> Backend for Greetd<S> {
    fn request(&mut self, mut request: Request) -> io::Result<Response> {
        let sent = write_message(&mut self.0, &request);
        wipe_request(&mut request);
        sent?;
        read_message(&mut self.0)
    }
}

#[cfg(unix)]
pub fn connect() -> io::Result<Greetd<std::os::unix::net::UnixStream>> {
    let path = std::env::var_os("GREETD_SOCK")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "GREETD_SOCK ist nicht gesetzt"))?;
    Ok(Greetd(std::os::unix::net::UnixStream::connect(path)?))
}

#[cfg(not(unix))]
pub fn connect() -> io::Result<Greetd<std::fs::File>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "greetd gibt es nur unter Linux",
    ))
}

/// Simuliert greetd: jeder Benutzername, Passwort `tuilnux`.
#[derive(Default)]
pub struct Demo {
    user: Option<String>,
    authenticated: bool,
}

impl Backend for Demo {
    fn request(&mut self, mut request: Request) -> io::Result<Response> {
        let correct = matches!(
            &request,
            Request::PostAuthMessageResponse { response: Some(r) } if r == DEMO_PASSWORD
        );
        wipe_request(&mut request);
        let error = |error_type, text: &str| Response::Error {
            error_type,
            description: text.to_string(),
        };
        Ok(match request {
            Request::CreateSession { .. } if self.user.is_some() => {
                error(ErrorType::Error, "Es läuft bereits eine Anmeldung")
            }
            Request::CreateSession { username } => {
                self.user = Some(username);
                Response::AuthMessage {
                    auth_message_type: AuthMessageType::Secret,
                    auth_message: "Password: ".into(),
                }
            }
            Request::PostAuthMessageResponse { .. } if self.user.is_none() => {
                error(ErrorType::Error, "Keine Anmeldung aktiv")
            }
            Request::PostAuthMessageResponse { .. } => {
                if correct {
                    self.authenticated = true;
                    Response::Success
                } else {
                    error(ErrorType::AuthError, "Authentifizierung fehlgeschlagen")
                }
            }
            Request::StartSession { .. } if !self.authenticated => {
                error(ErrorType::Error, "Nicht angemeldet")
            }
            Request::StartSession { .. } => Response::Success,
            Request::CancelSession => {
                *self = Self::default();
                Response::Success
            }
        })
    }

    fn is_demo(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn frame(json: &str) -> Vec<u8> {
        let mut v = (json.len() as u32).to_ne_bytes().to_vec();
        v.extend_from_slice(json.as_bytes());
        v
    }

    #[test]
    fn encodes_requests_like_greetd_expects() {
        let mut out = Vec::new();
        write_message(
            &mut out,
            &Request::CreateSession {
                username: "jonas".into(),
            },
        )
        .unwrap();
        assert_eq!(
            out,
            frame(r#"{"type":"create_session","username":"jonas"}"#)
        );

        out.clear();
        write_message(
            &mut out,
            &Request::PostAuthMessageResponse { response: None },
        )
        .unwrap();
        assert_eq!(
            out,
            frame(r#"{"type":"post_auth_message_response","response":null}"#)
        );

        out.clear();
        write_message(&mut out, &Request::CancelSession).unwrap();
        assert_eq!(out, frame(r#"{"type":"cancel_session"}"#));
    }

    #[test]
    fn decodes_responses() {
        let mut input = Cursor::new(
            [
                frame(r#"{"type":"auth_message","auth_message_type":"secret","auth_message":"Password: "}"#),
                frame(r#"{"type":"error","error_type":"auth_error","description":"nope"}"#),
                frame(r#"{"type":"success"}"#),
            ]
            .concat(),
        );
        assert_eq!(
            read_message(&mut input).unwrap(),
            Response::AuthMessage {
                auth_message_type: AuthMessageType::Secret,
                auth_message: "Password: ".into()
            }
        );
        assert_eq!(
            read_message(&mut input).unwrap(),
            Response::Error {
                error_type: ErrorType::AuthError,
                description: "nope".into()
            }
        );
        assert_eq!(read_message(&mut input).unwrap(), Response::Success);
        assert!(read_message(&mut input).is_err());
    }

    #[test]
    fn rejects_oversized_messages() {
        let mut input = Cursor::new(u32::MAX.to_ne_bytes().to_vec());
        assert!(read_message(&mut input).is_err());
    }

    #[test]
    fn demo_flow() {
        let mut demo = Demo::default();
        let create = || Request::CreateSession {
            username: "jonas".into(),
        };
        assert!(matches!(
            demo.request(create()).unwrap(),
            Response::AuthMessage { .. }
        ));
        let wrong = demo
            .request(Request::PostAuthMessageResponse {
                response: Some("falsch".into()),
            })
            .unwrap();
        assert!(matches!(
            wrong,
            Response::Error {
                error_type: ErrorType::AuthError,
                ..
            }
        ));
        demo.request(Request::CancelSession).unwrap();
        demo.request(create()).unwrap();
        let ok = demo
            .request(Request::PostAuthMessageResponse {
                response: Some(DEMO_PASSWORD.into()),
            })
            .unwrap();
        assert_eq!(ok, Response::Success);
        let start = Request::StartSession {
            cmd: vec!["tterm".into()],
            env: vec![],
        };
        assert_eq!(demo.request(start).unwrap(), Response::Success);
    }
}
