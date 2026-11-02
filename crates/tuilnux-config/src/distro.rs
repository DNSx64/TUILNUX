//! Distribution: Paketverwaltung, Paketnamen und Installationsordner.
//!
//! TUILNUX läuft auf Arch (pacman), Debian/Ubuntu (apt), Fedora (dnf) und
//! openSUSE (zypper). Hinweise wie „wlr-randr fehlt“ nennen den passenden Befehl.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageManager {
    Pacman,
    Apt,
    Dnf,
    Zypper,
}

/// Pakete, die TUILNUX zur Laufzeit nachinstalliert oder empfiehlt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Package {
    PamU2f,
    GoogleAuthenticator,
    Xwayland,
    WlrRandr,
}

impl PackageManager {
    pub const ALL: [PackageManager; 4] = [Self::Pacman, Self::Apt, Self::Dnf, Self::Zypper];

    pub fn binary(self) -> &'static str {
        match self {
            Self::Pacman => "pacman",
            Self::Apt => "apt-get",
            Self::Dnf => "dnf",
            Self::Zypper => "zypper",
        }
    }

    /// Erster Paketmanager, dessen Programm in /usr/bin oder /bin liegt.
    pub fn detect() -> Option<Self> {
        Self::detect_in(Path::new("/"))
    }

    pub fn detect_in(root: &Path) -> Option<Self> {
        Self::ALL.into_iter().find(|pm| {
            ["usr/bin", "bin"]
                .iter()
                .any(|dir| root.join(dir).join(pm.binary()).is_file())
        })
    }

    pub fn package(self, pkg: Package) -> &'static str {
        use Package::*;
        match (self, pkg) {
            (Self::Pacman, PamU2f) => "pam-u2f",
            (Self::Apt, PamU2f) => "libpam-u2f",
            (Self::Dnf, PamU2f) => "pam-u2f",
            (Self::Zypper, PamU2f) => "pam_u2f",
            (Self::Pacman | Self::Apt, GoogleAuthenticator) => "libpam-google-authenticator",
            (Self::Dnf, GoogleAuthenticator) => "google-authenticator",
            (Self::Zypper, GoogleAuthenticator) => "google-authenticator-libpam",
            (Self::Pacman, Xwayland) => "xorg-xwayland",
            (Self::Apt | Self::Zypper, Xwayland) => "xwayland",
            (Self::Dnf, Xwayland) => "xorg-x11-server-Xwayland",
            (_, WlrRandr) => "wlr-randr",
        }
    }

    /// Argumente für eine Installation ohne Rückfrage (ohne das Programm selbst).
    pub fn install_args(self) -> &'static [&'static str] {
        match self {
            Self::Pacman => &["-S", "--needed", "--noconfirm"],
            Self::Apt => &["install", "-y"],
            Self::Dnf => &["install", "-y"],
            Self::Zypper => &["--non-interactive", "install"],
        }
    }

    /// Befehl für Menschen, z. B. `sudo apt install wlr-randr`.
    pub fn hint(self, pkg: Package) -> String {
        let cmd = match self {
            Self::Pacman => "pacman -S",
            Self::Apt => "apt install",
            Self::Dnf => "dnf install",
            Self::Zypper => "zypper install",
        };
        format!("sudo {cmd} {}", self.package(pkg))
    }
}

/// Installationshinweis für das laufende System (Arch-Befehl, wenn unbekannt).
pub fn install_hint(pkg: Package) -> String {
    PackageManager::detect()
        .unwrap_or(PackageManager::Pacman)
        .hint(pkg)
}

/// Ordner der PAM-Module je nach Distribution (Arch, Fedora/openSUSE, Debian-Multiarch).
pub fn pam_module_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [
        "usr/lib/security",
        "lib/security",
        "usr/lib64/security",
        "lib64/security",
    ]
    .iter()
    .map(|d| root.join(d))
    .collect();
    for base in ["usr/lib", "lib"] {
        if let Ok(entries) = std::fs::read_dir(root.join(base)) {
            let mut multiarch: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with("-linux-gnu"))
                .map(|e| e.path().join("security"))
                .collect();
            multiarch.sort();
            dirs.extend(multiarch);
        }
    }
    dirs
}

/// Gemeinsame Daten (`share/tuilnux`): Paketinstallation unter /usr,
/// `install.sh` auf anderen Distributionen unter /usr/local.
pub fn share_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(prefix) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent()?.parent().map(Path::to_path_buf))
    {
        dirs.push(prefix.join("share/tuilnux"));
    }
    for d in ["/usr/share/tuilnux", "/usr/local/share/tuilnux"] {
        let d = PathBuf::from(d);
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs
}

/// Erste vorhandene Datei `name` in den gemeinsamen Datenordnern.
pub fn share_file(name: &str) -> Option<PathBuf> {
    share_dirs()
        .into_iter()
        .map(|d| d.join(name))
        .find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names_per_distro() {
        assert_eq!(PackageManager::Apt.package(Package::PamU2f), "libpam-u2f");
        assert_eq!(
            PackageManager::Dnf.package(Package::Xwayland),
            "xorg-x11-server-Xwayland"
        );
        assert_eq!(
            PackageManager::Zypper.hint(Package::GoogleAuthenticator),
            "sudo zypper install google-authenticator-libpam"
        );
        assert_eq!(
            PackageManager::Pacman.hint(Package::WlrRandr),
            "sudo pacman -S wlr-randr"
        );
    }

    #[test]
    fn detects_package_manager_and_pam_dirs() {
        let root = std::env::temp_dir().join(format!("tuilnux-distro-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(PackageManager::detect_in(&root), None);
        std::fs::create_dir_all(root.join("usr/bin")).unwrap();
        std::fs::write(root.join("usr/bin/apt-get"), "").unwrap();
        assert_eq!(PackageManager::detect_in(&root), Some(PackageManager::Apt));
        std::fs::create_dir_all(root.join("usr/lib/x86_64-linux-gnu/security")).unwrap();
        let dirs = pam_module_dirs(&root);
        assert!(dirs.contains(&root.join("usr/lib/x86_64-linux-gnu/security")));
        assert!(dirs.contains(&root.join("usr/lib64/security")));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
