//! Zustand der Linux-Konsole (Feststelltaste).

/// `Some(true)`, wenn die Feststelltaste an ist; `None` außerhalb einer Linux-Konsole.
#[cfg(target_os = "linux")]
pub fn caps_lock() -> Option<bool> {
    // linux/kd.h: KDGKBLED liefert die Tastatur-Flags (nicht nur die Lämpchen).
    const KDGKBLED: libc::c_ulong = 0x4B64;
    const KDGETLED: libc::c_ulong = 0x4B31;
    const LED_CAP: u8 = 0x04;
    for request in [KDGKBLED, KDGETLED] {
        let mut flags: libc::c_char = 0;
        // SAFETY: beide ioctls schreiben genau ein char in `flags`.
        let r = unsafe { libc::ioctl(0, request as _, &mut flags) };
        if r == 0 {
            return Some(flags as u8 & LED_CAP != 0);
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
pub fn caps_lock() -> Option<bool> {
    None
}
