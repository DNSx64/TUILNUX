//! Grafik (Renderer, NVIDIA & Co.) und Wayland-Unterstützung der GUI-Apps.
//! Wird zu `export`-Zeilen in `session.conf` (vor dem Start von labwc gelesen)
//! und zur labwc-`environment`-Datei.

use std::fmt::Write as _;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::Choice;

choice!(
    /// Renderer von labwc/wlroots.
    Renderer [Auto] {
        Auto = "auto", "automatisch";
        Gles2 = "gles2", "OpenGL ES 2";
        Vulkan = "vulkan", "Vulkan";
        Pixman = "pixman", "Software (pixman)";
    }
);

choice!(
    /// Grafiktreiber, für den die Umgebung eingerichtet wird.
    Gpu [Auto] {
        Auto = "auto", "automatisch erkennen";
        Nvidia = "nvidia", "NVIDIA (proprietär/open)";
        Amd = "amd", "AMD";
        Intel = "intel", "Intel";
        Other = "other", "andere / virtuell";
    }
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GraphicsSettings {
    pub renderer: Renderer,
    pub gpu: Gpu,
    /// Hardware-Mauszeiger (aus = Software-Cursor, hilft bei NVIDIA/VMs).
    pub hardware_cursors: bool,
    /// NVIDIA: GBM-Backend und GLX-Hersteller setzen.
    pub nvidia_gbm: bool,
    /// NVIDIA: Videobeschleunigung über nvidia-vaapi-driver.
    pub nvidia_vaapi: bool,
    /// VA-API-Treiber (`LIBVA_DRIVER_NAME`), leer = automatisch.
    pub vaapi_driver: String,
    /// Nur diese DRM-Geräte benutzen, z. B. `/dev/dri/card1` (leer = alle).
    pub drm_device: String,
    /// Variable Bildwiederholrate (FreeSync/G-Sync) erlauben.
    pub adaptive_sync: bool,
    /// Tearing für Vollbild-Spiele erlauben (geringere Latenz).
    pub allow_tearing: bool,
    /// OpenGL in Software (für Problemfälle).
    pub software_gl: bool,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Self {
            renderer: Renderer::Auto,
            gpu: Gpu::Auto,
            hardware_cursors: true,
            nvidia_gbm: true,
            nvidia_vaapi: false,
            vaapi_driver: String::new(),
            drm_device: String::new(),
            adaptive_sync: false,
            allow_tearing: false,
            software_gl: false,
        }
    }
}

fn plain_ok(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:".contains(c))
}

impl GraphicsSettings {
    pub fn validate(&self) -> Result<()> {
        if !plain_ok(&self.drm_device) {
            bail!(
                "graphics.drm_device = \"{}\" ist kein Gerätepfad",
                self.drm_device
            );
        }
        if !plain_ok(&self.vaapi_driver) {
            bail!(
                "graphics.vaapi_driver = \"{}\" ist ungültig",
                self.vaapi_driver
            );
        }
        Ok(())
    }

    /// sh-Zeilen für `session.conf`.
    pub fn exports(&self) -> String {
        let mut out = String::new();
        if self.renderer != Renderer::Auto {
            let _ = writeln!(out, "export WLR_RENDERER={}", self.renderer.key());
        }
        if !self.hardware_cursors {
            out.push_str("export WLR_NO_HARDWARE_CURSORS=1\n");
        }
        if !self.drm_device.is_empty() {
            let _ = writeln!(out, "export WLR_DRM_DEVICES={}", self.drm_device);
        }
        if self.software_gl {
            out.push_str("export LIBGL_ALWAYS_SOFTWARE=1\n");
        }
        if !self.vaapi_driver.is_empty() {
            let _ = writeln!(out, "export LIBVA_DRIVER_NAME={}", self.vaapi_driver);
        }
        let mut nvidia = String::new();
        if self.nvidia_gbm {
            nvidia.push_str(
                "export GBM_BACKEND=nvidia-drm\nexport __GLX_VENDOR_LIBRARY_NAME=nvidia\n",
            );
        }
        if self.nvidia_vaapi && self.vaapi_driver.is_empty() {
            nvidia.push_str("export LIBVA_DRIVER_NAME=nvidia\nexport NVD_BACKEND=direct\n");
        }
        match self.gpu {
            Gpu::Nvidia => out.push_str(&nvidia),
            Gpu::Auto if !nvidia.is_empty() => {
                out.push_str("if [ -d /sys/module/nvidia_drm ]; then\n");
                for line in nvidia.lines() {
                    let _ = writeln!(out, "    {line}");
                }
                out.push_str("fi\n");
            }
            _ => {}
        }
        out
    }
}

/// Grafikkarten im System (`/sys/class/drm/card*`), z. B. „card0: NVIDIA (nvidia)“.
pub fn detect_gpus() -> Vec<String> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/sys/class/drm") else {
        return out;
    };
    let mut cards: Vec<_> = dir
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("card") && !n.contains('-'))
        .collect();
    cards.sort();
    for card in cards {
        let dev = format!("/sys/class/drm/{card}/device");
        let vendor = std::fs::read_to_string(format!("{dev}/vendor")).unwrap_or_default();
        let driver = std::fs::read_link(format!("{dev}/driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "?".into());
        out.push(format!("{card}: {} ({driver})", vendor_name(vendor.trim())));
    }
    out
}

pub fn vendor_name(id: &str) -> &'static str {
    match id {
        "0x10de" => "NVIDIA",
        "0x1002" => "AMD",
        "0x8086" => "Intel",
        "0x15ad" => "VMware",
        "0x1af4" => "virtio",
        "0x1234" => "QEMU",
        "0x80ee" => "VirtualBox",
        _ => "unbekannt",
    }
}

/// Läuft der NVIDIA-Treiber mit Kernel-Modesetting (für Wayland nötig)?
pub fn nvidia_modeset() -> Option<bool> {
    let v = std::fs::read_to_string("/sys/module/nvidia_drm/parameters/modeset").ok()?;
    Some(v.trim() == "Y")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WaylandSettings {
    /// Firefox/Thunderbird nativ unter Wayland.
    pub firefox: bool,
    /// Qt-Apps nativ (Rückfall auf X11).
    pub qt: bool,
    /// GTK-Apps nativ (Rückfall auf X11).
    pub gtk: bool,
    /// Electron/Chromium (VS Code, Discord …) nativ.
    pub electron: bool,
    /// SDL-Spiele nativ (manche ältere Spiele brauchen X11).
    pub sdl: bool,
    /// Java-Apps (Fensterfehler unter Xwayland vermeiden).
    pub java: bool,
    /// Clutter-Apps nativ.
    pub clutter: bool,
}

impl Default for WaylandSettings {
    fn default() -> Self {
        Self {
            firefox: true,
            qt: true,
            gtk: true,
            electron: true,
            sdl: false,
            java: true,
            clutter: true,
        }
    }
}

impl WaylandSettings {
    /// `NAME=WERT`-Zeilen für die labwc-`environment`-Datei.
    pub fn env(&self) -> Vec<(&'static str, &'static str)> {
        let mut out = Vec::new();
        if self.firefox {
            out.push(("MOZ_ENABLE_WAYLAND", "1"));
        }
        if self.qt {
            out.push(("QT_QPA_PLATFORM", "wayland;xcb"));
            out.push(("QT_WAYLAND_DISABLE_WINDOWDECORATION", "1"));
        }
        if self.gtk {
            out.push(("GDK_BACKEND", "wayland,x11"));
        }
        if self.electron {
            out.push(("ELECTRON_OZONE_PLATFORM_HINT", "auto"));
        }
        if self.sdl {
            out.push(("SDL_VIDEODRIVER", "wayland"));
        }
        if self.java {
            out.push(("_JAVA_AWT_WM_NONREPARENTING", "1"));
        }
        if self.clutter {
            out.push(("CLUTTER_BACKEND", "wayland"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphics_exports() {
        let g = GraphicsSettings::default();
        let e = g.exports();
        assert!(e.contains("if [ -d /sys/module/nvidia_drm ]"));
        assert!(e.contains("    export GBM_BACKEND=nvidia-drm"));
        assert!(!e.contains("WLR_RENDERER"));

        let g = GraphicsSettings {
            renderer: Renderer::Vulkan,
            gpu: Gpu::Nvidia,
            hardware_cursors: false,
            nvidia_vaapi: true,
            drm_device: "/dev/dri/card1".into(),
            ..GraphicsSettings::default()
        };
        let e = g.exports();
        assert!(e.contains("export WLR_RENDERER=vulkan\n"));
        assert!(e.contains("export WLR_NO_HARDWARE_CURSORS=1\n"));
        assert!(e.contains("export WLR_DRM_DEVICES=/dev/dri/card1\n"));
        assert!(e.contains("export LIBVA_DRIVER_NAME=nvidia\n"));
        assert!(!e.contains("if ["));

        let g = GraphicsSettings {
            gpu: Gpu::Intel,
            ..GraphicsSettings::default()
        };
        assert!(!g.exports().contains("nvidia"));
    }

    #[test]
    fn graphics_validation() {
        let mut g = GraphicsSettings::default();
        assert!(g.validate().is_ok());
        g.drm_device = "/dev/dri/card0; rm".into();
        assert!(g.validate().is_err());
    }

    #[test]
    fn wayland_env() {
        let w = WaylandSettings::default();
        let env = w.env();
        assert!(env.contains(&("MOZ_ENABLE_WAYLAND", "1")));
        assert!(!env.iter().any(|(k, _)| *k == "SDL_VIDEODRIVER"));
        let none = WaylandSettings {
            firefox: false,
            qt: false,
            gtk: false,
            electron: false,
            sdl: false,
            java: false,
            clutter: false,
        };
        assert!(none.env().is_empty());
    }

    #[test]
    fn vendors() {
        assert_eq!(vendor_name("0x10de"), "NVIDIA");
        assert_eq!(vendor_name("0xffff"), "unbekannt");
    }
}
