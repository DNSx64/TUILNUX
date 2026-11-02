//! Befehle, mit denen Programme in der Shell die laufende Sitzung steuern.

use anyhow::{Context, Result, bail};
use tuilnux_ipc::{Request, Response, TaskInfo, request_env};

use crate::draw::format_memory;

pub const CLIENT_COMMANDS: &[&str] = &[
    "open", "list", "ls", "focus", "close", "min", "minimize", "max", "maximize", "notify",
    "logout", "tab", "reload", "launch",
];

/// Programmname eines Tasks: Dateiname des ersten Wortes im Befehl.
fn program(command: &str) -> &str {
    let first = command.split_whitespace().next().unwrap_or("");
    first.rsplit(['/', '\\']).next().unwrap_or(first)
}

/// Sucht den Task zu `tab NAME`: Nummer, Titel oder Programmname, ohne Groß/klein.
/// Genaue Treffer gehen vor Anfängen, Anfänge vor Teilwörtern; bei Gleichstand
/// zuerst minimierte, dann nicht fokussierte Fenster, dann die kleinste Nummer.
pub fn find_task<'a>(tasks: &'a [TaskInfo], query: &str) -> Result<&'a TaskInfo> {
    if let Ok(id) = query.parse::<usize>() {
        return tasks
            .iter()
            .find(|t| t.id == id)
            .with_context(|| format!("kein Fenster mit Nummer {id}"));
    }
    let q = query.to_lowercase();
    let rank = |t: &TaskInfo| -> Option<u8> {
        if t.id == 0 {
            return (q == "desktop").then_some(0);
        }
        let names = [t.title.to_lowercase(), program(&t.command).to_lowercase()];
        if names.contains(&q) {
            Some(0)
        } else if names.iter().any(|n| n.starts_with(&q)) {
            Some(1)
        } else if names.iter().any(|n| n.contains(&q)) || t.command.to_lowercase().contains(&q) {
            Some(2)
        } else {
            None
        }
    };
    tasks
        .iter()
        .filter_map(|t| rank(t).map(|r| (r, !t.minimized, t.focused, t.id, t)))
        .min_by_key(|&(r, visible, focused, id, _)| (r, visible, focused, id))
        .map(|(.., t)| t)
        .with_context(|| {
            let names: Vec<String> = tasks
                .iter()
                .filter(|t| t.id > 0)
                .map(|t| format!("{} {}", t.id, t.title))
                .collect();
            if names.is_empty() {
                format!("kein Task „{query}“ – es ist kein Fenster offen")
            } else {
                format!("kein Task „{query}“ – offen: {}", names.join(", "))
            }
        })
}

/// `tab [NAME|NR]`: minimierten Task wiederherstellen und nach vorne holen.
/// `tab --shell [NAME]`: Terminal eines Hintergrundprogramms anzeigen.
fn tab(args: &[String]) -> Result<()> {
    let Response::Tasks { tasks } = request_env(Request::List)? else {
        bail!("unerwartete Antwort der Sitzung");
    };
    let shell = args.first().is_some_and(|a| a == "--shell" || a == "-s");
    let args = if shell { &args[1..] } else { args };
    let (tasks, others): (Vec<TaskInfo>, Vec<TaskInfo>) = tasks
        .into_iter()
        .partition(|t| t.id == 0 || t.background == shell);
    match args.first().map(String::as_str) {
        None => {
            for t in tasks.iter().filter(|t| t.id > 0) {
                let status = if t.focused {
                    "aktiv"
                } else if t.background && t.minimized {
                    "Hintergrund"
                } else if t.minimized {
                    "minimiert"
                } else {
                    "offen"
                };
                println!(
                    "{:>3}  {:<24} {:<9} {}",
                    t.id,
                    t.title,
                    status,
                    program(&t.command)
                );
            }
            if tasks.len() <= 1 {
                if shell {
                    println!(
                        "Keine Programme im Hintergrund. Starten z. B. mit: firefox (oder launch BEFEHL)"
                    );
                } else {
                    println!("Keine Fenster offen. Aufruf: tab PROGRAMMNAME");
                }
            }
            if !shell && !others.is_empty() {
                println!(
                    "Dazu {} im Hintergrund – anzeigen mit tab --shell",
                    others.len()
                );
            }
            Ok(())
        }
        Some("--names") => {
            let mut names: Vec<String> = Vec::new();
            for t in tasks.iter().filter(|t| t.id > 0) {
                for n in [t.title.as_str(), program(&t.command)] {
                    if !n.is_empty()
                        && !n.contains(char::is_whitespace)
                        && !names.iter().any(|x| x == n)
                    {
                        names.push(n.to_string());
                    }
                }
            }
            for n in names {
                println!("{n}");
            }
            Ok(())
        }
        Some(_) => {
            let query = args.join(" ");
            let id = match find_task(&tasks, &query) {
                Ok(t) => t.id,
                Err(e) => {
                    let mut all = others.clone();
                    all.retain(|t| t.id > 0);
                    if find_task(&all, &query).is_ok() {
                        if shell {
                            bail!("„{query}“ läuft nicht im Hintergrund – tab {query}");
                        }
                        bail!("„{query}“ läuft im Hintergrund – tab --shell {query}");
                    }
                    return Err(e);
                }
            };
            match request_env(Request::Focus { id })? {
                Response::Error { message } => bail!(message),
                _ => Ok(()),
            }
        }
    }
}

fn id_arg(args: &[String], cmd: &str) -> Result<usize> {
    let raw = args
        .first()
        .with_context(|| format!("Aufruf: tuilnux {cmd} NR (Nummer aus `tuilnux list`)"))?;
    raw.parse()
        .with_context(|| format!("„{raw}“ ist keine Fensternummer"))
}

pub fn parse_size(s: &str) -> Result<(u16, u16)> {
    let (c, r) = s
        .split_once(['x', 'X'])
        .with_context(|| format!("Größe „{s}“: erwartet SPALTENxZEILEN, z. B. 100x30"))?;
    let (c, r): (u16, u16) = (c.trim().parse()?, r.trim().parse()?);
    if c == 0 || r == 0 {
        bail!("Größe „{s}“ muss größer als 0 sein");
    }
    Ok((c, r))
}

/// Zerlegt `open [--title T] [--size CxR] [--cwd DIR] [--] BEFEHL …`.
pub fn parse_open(args: &[String]) -> Result<Request> {
    let mut title = None;
    let mut size = None;
    let mut cwd = std::env::current_dir()
        .ok()
        .map(|d| d.display().to_string());
    let mut i = 0;
    while i < args.len() {
        let value = |i: usize| {
            args.get(i + 1)
                .cloned()
                .with_context(|| format!("{} braucht einen Wert", args[i]))
        };
        match args[i].as_str() {
            "--" => {
                i += 1;
                break;
            }
            "-t" | "--title" => {
                title = Some(value(i)?);
                i += 2;
            }
            "-s" | "--size" => {
                size = Some(parse_size(&value(i)?)?);
                i += 2;
            }
            "-C" | "--cwd" => {
                cwd = Some(value(i)?);
                i += 2;
            }
            a if a.starts_with('-') && a.len() > 1 => bail!("unbekannte Option {a}"),
            _ => break,
        }
    }
    let command = args[i..].to_vec();
    if command.is_empty() {
        bail!("Aufruf: tuilnux open [--title T] [--size 100x30] [--] BEFEHL [ARGUMENTE…]");
    }
    Ok(Request::Open {
        command,
        title,
        cwd,
        size,
    })
}

/// `launch [--] BEFEHL …`: Programm im Hintergrund starten.
pub fn parse_launch(args: &[String]) -> Result<Request> {
    let command = match args.first().map(String::as_str) {
        Some("--") => args[1..].to_vec(),
        _ => args.to_vec(),
    };
    if command.is_empty() {
        bail!("Aufruf: tuilnux launch [--] BEFEHL [ARGUMENTE…]");
    }
    Ok(Request::Launch {
        command,
        cwd: std::env::current_dir()
            .ok()
            .map(|d| d.display().to_string()),
    })
}

pub fn parse(cmd: &str, args: &[String]) -> Result<Request> {
    Ok(match cmd {
        "open" => parse_open(args)?,
        "launch" => parse_launch(args)?,
        "list" | "ls" => Request::List,
        "focus" => Request::Focus {
            id: id_arg(args, cmd)?,
        },
        "close" => Request::Close {
            id: id_arg(args, cmd)?,
        },
        "min" | "minimize" => Request::Minimize {
            id: id_arg(args, cmd)?,
        },
        "max" | "maximize" => Request::Maximize {
            id: id_arg(args, cmd)?,
        },
        "notify" => {
            if args.is_empty() {
                bail!("Aufruf: tuilnux notify TEXT");
            }
            Request::Notify {
                text: args.join(" "),
            }
        }
        "logout" => Request::Logout,
        "reload" => Request::Reload,
        other => bail!("unbekannter Befehl „{other}“ (tuilnux help)"),
    })
}

pub fn run(cmd: &str, args: &[String]) -> Result<()> {
    if cmd == "tab" {
        return tab(args);
    }
    let request = parse(cmd, args)?;
    let launched = match &request {
        Request::Launch { command, .. } => Some(program(&command[0]).to_string()),
        _ => None,
    };
    match request_env(request)? {
        Response::Ok => {}
        Response::Opened { id } => match launched {
            Some(name) => {
                eprintln!("[{id}] {name} läuft im Hintergrund – Ausgabe: tab --shell {name}")
            }
            None => println!("{id}"),
        },
        Response::Tasks { tasks } => {
            println!(
                "{:>3}  {:<20} {:>7} {:>6} {:>6}  {:<9} BEFEHL",
                "NR", "TITEL", "PID", "CPU", "RAM", "STATUS"
            );
            for t in tasks {
                let status = if t.focused {
                    "aktiv"
                } else if t.background && t.minimized {
                    "Hintergr."
                } else if t.minimized {
                    "minimiert"
                } else if t.id == 0 {
                    "Desktop"
                } else {
                    "offen"
                };
                let title: String = t.title.chars().take(20).collect();
                println!(
                    "{:>3}  {:<20} {:>7} {:>5.1}% {:>6}  {:<9} {}",
                    t.id,
                    title,
                    t.pid.map_or("-".into(), |p| p.to_string()),
                    t.cpu,
                    format_memory(t.memory),
                    status,
                    t.command
                );
            }
        }
        Response::Error { message } => bail!(message),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn open_arguments() {
        let Request::Open {
            command,
            title,
            size,
            cwd,
        } = parse_open(&s(&[
            "--title", "Dateien", "-s", "100x30", "--", "tfiles", "-a",
        ]))
        .unwrap()
        else {
            panic!()
        };
        assert_eq!(command, ["tfiles", "-a"]);
        assert_eq!(title.as_deref(), Some("Dateien"));
        assert_eq!(size, Some((100, 30)));
        assert!(cwd.is_some());
        // Optionen des Programms gehören dem Programm.
        let Request::Open { command, .. } = parse_open(&s(&["ls", "--title", "x"])).unwrap() else {
            panic!()
        };
        assert_eq!(command, ["ls", "--title", "x"]);
        assert!(parse_open(&s(&[])).is_err());
        assert!(parse_open(&s(&["--size", "0x3", "ttop"])).is_err());
        assert!(parse_open(&s(&["--bogus", "ttop"])).is_err());
    }

    #[test]
    fn simple_commands() {
        assert_eq!(
            parse("focus", &s(&["2"])).unwrap(),
            Request::Focus { id: 2 }
        );
        assert_eq!(parse("ls", &[]).unwrap(), Request::List);
        assert!(parse("close", &[]).is_err());
        assert!(parse("close", &s(&["zwei"])).is_err());
        assert_eq!(
            parse("notify", &s(&["Build", "fertig"])).unwrap(),
            Request::Notify {
                text: "Build fertig".into()
            }
        );
        assert!(
            CLIENT_COMMANDS
                .iter()
                .all(|c| matches!(*c, "open" | "notify" | "tab") || parse(c, &s(&["1"])).is_ok())
        );
        let Request::Launch { command, cwd } =
            parse("launch", &s(&["--", "firefox", "-P"])).unwrap()
        else {
            panic!()
        };
        assert_eq!(command, ["firefox", "-P"]);
        assert!(cwd.is_some());
        assert!(parse("launch", &s(&["--"])).is_err());
    }

    fn task(id: usize, title: &str, command: &str, minimized: bool, focused: bool) -> TaskInfo {
        TaskInfo {
            id,
            title: title.into(),
            command: command.into(),
            pid: None,
            focused,
            minimized,
            cpu: 0.0,
            memory: 0,
            gui: false,
            background: false,
        }
    }

    #[test]
    fn tab_finds_tasks_by_name() {
        let tasks = vec![
            task(0, "Desktop", "/bin/bash", false, false),
            task(1, "Dateien", "/usr/bin/tfiles", false, true),
            task(2, "ttop", "ttop", true, false),
            task(3, "notizen.txt", "tedit notizen.txt", false, false),
            task(4, "Dateien", "tfiles /tmp", true, false),
        ];
        let id = |q: &str| find_task(&tasks, q).map(|t| t.id).ok();
        assert_eq!(id("ttop"), Some(2));
        assert_eq!(id("TTOP"), Some(2));
        assert_eq!(id("tt"), Some(2), "Anfang des Namens genügt");
        // Zwei Dateibrowser: der minimierte wird geholt.
        assert_eq!(id("tfiles"), Some(4));
        assert_eq!(id("dateien"), Some(4));
        assert_eq!(id("tedit"), Some(3), "Programmname statt Titel");
        assert_eq!(id("notizen"), Some(3));
        assert_eq!(id("ted"), Some(3), "Anfang schlägt Teilwort");
        assert_eq!(id("3"), Some(3));
        assert_eq!(id("desktop"), Some(0));
        assert_eq!(id("bash"), None, "Der Desktop zählt nur als „desktop“");
        assert!(
            find_task(&tasks, "firefox")
                .unwrap_err()
                .to_string()
                .contains("1 Dateien")
        );
        assert_eq!(program("/usr/bin/tfiles -a"), "tfiles");
    }
}
