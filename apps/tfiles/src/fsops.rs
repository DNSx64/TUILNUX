//! Dateisystem-Operationen von tfiles (ohne UI, damit sie testbar sind).

use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};

pub type RawEntry = (String, PathBuf, bool, bool, u64);

/// Entfernt das `\\?\`-Präfix, das `canonicalize` unter Windows erzeugt.
pub fn clean_path(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(stripped) if !stripped.starts_with("UNC") => PathBuf::from(stripped),
        _ => path,
    }
}

pub fn read_dir(dir: &Path, show_hidden: bool) -> io::Result<Vec<RawEntry>> {
    let mut entries: Vec<RawEntry> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !show_hidden && name.starts_with('.') {
                return None;
            }
            let path = e.path();
            let is_link = e.file_type().is_ok_and(|t| t.is_symlink());
            let meta = fs::metadata(&path).ok();
            let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
            let size = meta.map_or(0, |m| m.len());
            Some((name, path, is_dir, is_link, size))
        })
        .collect();
    entries.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    Ok(entries)
}

pub fn read_head(path: &Path, max_lines: usize) -> io::Result<Vec<String>> {
    let file = fs::File::open(path)?;
    io::BufReader::new(file)
        .lines()
        .take(max_lines)
        .map(|l| l.map(|s| s.replace('\t', "    ")))
        .collect()
}

pub fn validate_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        Err("Name darf nicht leer sein")
    } else if name == "." || name == ".." {
        Err("ungültiger Name")
    } else if name.contains('/') || name.contains('\\') || name.contains('\0') {
        Err("Name darf keine / oder \\ enthalten")
    } else {
        Ok(())
    }
}

/// Freier Zielname im Ordner: `datei.txt`, `datei (2).txt`, `datei (3).txt` …
pub fn unique_target(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map_or(name.to_string(), |s| s.to_string_lossy().into_owned());
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("infinite iterator")
}

fn copy_recursive(src: &Path, dest: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(src)?;
    if meta.is_dir() {
        fs::create_dir(dest)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dest.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(src, dest).map(|_| ())
    }
}

fn file_name(src: &Path) -> io::Result<String> {
    src.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "kein Dateiname"))
}

pub fn copy_into(src: &Path, dir: &Path) -> io::Result<PathBuf> {
    if dir.starts_with(src) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Ordner kann nicht in sich selbst kopiert werden",
        ));
    }
    let dest = unique_target(dir, &file_name(src)?);
    copy_recursive(src, &dest)?;
    Ok(dest)
}

pub fn move_into(src: &Path, dir: &Path) -> io::Result<PathBuf> {
    if src.parent() == Some(dir) {
        return Ok(src.to_path_buf());
    }
    if dir.starts_with(src) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Ordner kann nicht in sich selbst verschoben werden",
        ));
    }
    let dest = unique_target(dir, &file_name(src)?);
    if fs::rename(src, &dest).is_err() {
        // Anderes Dateisystem: kopieren und Original entfernen.
        copy_recursive(src, &dest)?;
        if fs::symlink_metadata(src)?.is_dir() {
            fs::remove_dir_all(src)?;
        } else {
            fs::remove_file(src)?;
        }
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tfiles-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn copy_and_move_directories() {
        let root = temp_dir("copy");
        let src = root.join("src");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub").join("a.txt"), "hallo").unwrap();
        let target = root.join("target");
        fs::create_dir(&target).unwrap();

        let copied = copy_into(&src, &target).unwrap();
        assert_eq!(
            fs::read_to_string(copied.join("sub").join("a.txt")).unwrap(),
            "hallo"
        );
        let second = copy_into(&src, &target).unwrap();
        assert_eq!(second.file_name().unwrap(), "src (2)");

        let moved = move_into(&src, &target).unwrap();
        assert_eq!(moved.file_name().unwrap(), "src (3)");
        assert!(!src.exists());
        assert!(copy_into(&target, &target.join("src")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn listing_puts_dirs_first_and_hides_dotfiles() {
        let root = temp_dir("list");
        fs::write(root.join("b.txt"), "").unwrap();
        fs::write(root.join(".hidden"), "").unwrap();
        fs::create_dir(root.join("Zeta")).unwrap();
        let names: Vec<String> = read_dir(&root, false)
            .unwrap()
            .into_iter()
            .map(|e| e.0)
            .collect();
        assert_eq!(names, ["Zeta", "b.txt"]);
        assert_eq!(read_dir(&root, true).unwrap().len(), 3);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unique_names_keep_extension() {
        let root = temp_dir("unique");
        fs::write(root.join("bild.png"), "").unwrap();
        assert_eq!(unique_target(&root, "bild.png"), root.join("bild (2).png"));
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("..").is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
