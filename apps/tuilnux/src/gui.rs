//! Grafische Programme (Wayland-Fenster unter labwc) in Taskleiste, Taskmanager und `tab`.
//!
//! Ein Hintergrund-Thread verfolgt über das Protokoll `wlr-foreign-toplevel-management`
//! alle Fenster des Compositors; die Sitzung kann sie aktivieren, minimieren,
//! maximieren und schließen. Ohne Wayland (Konsole, andere Systeme) bleibt die Liste leer.

use std::fmt;

use anyhow::Result;

/// App-ID des Terminals, in dem die TUILNUX-Sitzung selbst läuft.
pub const DESKTOP_APP_ID: &str = "tuilnux-desktop";

/// Zustand eines Fensters nach einem `done`-Ereignis des Compositors.
#[derive(Debug, Clone, PartialEq)]
pub struct GuiState {
    pub handle: Handle,
    pub title: String,
    pub app_id: String,
    pub minimized: bool,
    pub maximized: bool,
    pub activated: bool,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum GuiEvent {
    Update(GuiState),
    Closed(Handle),
    /// Höhe des Bildschirms, auf dem der Desktop liegt, in logischen Pixeln.
    Output {
        height: u32,
    },
    /// Ein Bildschirm wurde angeschlossen oder entfernt.
    OutputsChanged,
}

/// Verweis auf ein Fenster des Compositors.
#[derive(Clone, PartialEq)]
pub struct Handle(Inner);

#[derive(Clone, PartialEq)]
enum Inner {
    #[cfg(target_os = "linux")]
    Wayland(imp::Toplevel),
    #[allow(dead_code)]
    Fake(u64),
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            #[cfg(target_os = "linux")]
            Inner::Wayland(t) => write!(f, "Handle({:?})", t.id()),
            Inner::Fake(n) => write!(f, "Handle(fake {n})"),
        }
    }
}

impl Handle {
    #[cfg(test)]
    pub fn fake(n: u64) -> Self {
        Self(Inner::Fake(n))
    }

    /// Wiederherstellen (falls minimiert) und nach vorne holen.
    pub fn activate(&self) {
        match &self.0 {
            #[cfg(target_os = "linux")]
            Inner::Wayland(t) => t.activate(),
            Inner::Fake(_) => {}
        }
    }

    pub fn minimize(&self) {
        match &self.0 {
            #[cfg(target_os = "linux")]
            Inner::Wayland(t) => t.minimize(),
            Inner::Fake(_) => {}
        }
    }

    pub fn set_maximized(&self, on: bool) {
        match &self.0 {
            #[cfg(target_os = "linux")]
            Inner::Wayland(t) => t.set_maximized(on),
            Inner::Fake(_) => {
                let _ = on;
            }
        }
    }

    /// Bittet das Programm, sich zu schließen (wie der Schließen-Knopf).
    pub fn close(&self) {
        match &self.0 {
            #[cfg(target_os = "linux")]
            Inner::Wayland(t) => t.close(),
            Inner::Fake(_) => {}
        }
    }
}

/// Ein grafisches Fenster mit seiner TUILNUX-Nummer.
#[derive(Debug, Clone, PartialEq)]
pub struct GuiWindow {
    pub id: usize,
    pub state: GuiState,
}

impl GuiWindow {
    /// Name für Taskleiste und Taskliste: Titel, sonst App-ID.
    pub fn title(&self) -> &str {
        if self.state.title.trim().is_empty() {
            &self.state.app_id
        } else {
            &self.state.title
        }
    }
}

/// Alle bekannten grafischen Fenster.
#[derive(Debug, Default)]
pub struct GuiWindows {
    list: Vec<GuiWindow>,
    /// Das Terminal der Sitzung hat gerade den Tastaturfokus des Compositors.
    pub desktop_active: bool,
    desktop: Option<Handle>,
    /// Fenster, das zuletzt den Fokus verloren hat, und wann.
    lost_focus: Option<(usize, std::time::Instant)>,
}

impl GuiWindows {
    /// Holt das Terminal der Sitzung nach vorne, falls ein grafisches Fenster aktiv ist.
    pub fn raise_desktop(&mut self) {
        if self.active().is_none() {
            return;
        }
        if let Some(h) = &self.desktop {
            h.activate();
        }
        for w in &mut self.list {
            w.state.activated = false;
        }
        self.desktop_active = true;
    }

    /// Übernimmt ein Ereignis; neue Fenster erhalten eine Nummer von `alloc`.
    /// Gibt die Nummer eines neu erschienenen Fensters zurück.
    pub fn apply(&mut self, event: GuiEvent, alloc: impl FnOnce() -> usize) -> Option<usize> {
        match event {
            GuiEvent::Update(state) if state.app_id == DESKTOP_APP_ID => {
                self.desktop_active = state.activated;
                self.desktop = Some(state.handle);
                None
            }
            GuiEvent::Update(state) => {
                if state.activated {
                    self.desktop_active = false;
                }
                if let Some(w) = self
                    .list
                    .iter_mut()
                    .find(|w| w.state.handle == state.handle)
                {
                    if w.state.activated && !state.activated && !state.minimized {
                        self.lost_focus = Some((w.id, std::time::Instant::now()));
                    }
                    w.state = state;
                    None
                } else {
                    let id = alloc();
                    self.list.push(GuiWindow { id, state });
                    Some(id)
                }
            }
            GuiEvent::Closed(handle) => {
                self.list.retain(|w| w.state.handle != handle);
                None
            }
            GuiEvent::Output { .. } | GuiEvent::OutputsChanged => None,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &GuiWindow> {
        self.list.iter()
    }

    pub fn get(&self, id: usize) -> Option<&GuiWindow> {
        self.list.iter().find(|w| w.id == id)
    }

    /// Nummer des Fensters, das gerade den Fokus im Compositor hat.
    pub fn active(&self) -> Option<usize> {
        self.list
            .iter()
            .find(|w| w.state.activated && !w.state.minimized)
            .map(|w| w.id)
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Aktiv, oder hat den Fokus gerade erst verloren – etwa weil ein Klick
    /// auf die Statusleiste zuerst das Terminal fokussiert hat.
    pub fn was_active(&self, id: usize) -> bool {
        self.active() == Some(id)
            || self.active().is_none()
                && self.get(id).is_some_and(|w| !w.state.minimized)
                && self.lost_focus.is_some_and(|(last, at)| {
                    last == id && at.elapsed() < std::time::Duration::from_millis(800)
                })
    }

    /// Markiert das Fenster sofort als aktiv (der Compositor bestätigt kurz danach).
    pub fn activate(&mut self, id: usize) -> bool {
        let Some(w) = self.list.iter_mut().find(|w| w.id == id) else {
            return false;
        };
        w.state.handle.activate();
        for other in &mut self.list {
            other.state.activated = other.id == id;
        }
        if let Some(w) = self.list.iter_mut().find(|w| w.id == id) {
            w.state.minimized = false;
        }
        self.desktop_active = false;
        true
    }

    pub fn minimize(&mut self, id: usize) -> bool {
        let Some(w) = self.list.iter_mut().find(|w| w.id == id) else {
            return false;
        };
        w.state.handle.minimize();
        w.state.minimized = true;
        w.state.activated = false;
        self.lost_focus = None;
        true
    }

    pub fn toggle_maximize(&mut self, id: usize) -> bool {
        let Some(w) = self.list.iter_mut().find(|w| w.id == id) else {
            return false;
        };
        w.state.maximized = !w.state.maximized;
        w.state.handle.set_maximized(w.state.maximized);
        true
    }

    pub fn close(&self, id: usize) -> bool {
        let Some(w) = self.get(id) else {
            return false;
        };
        w.state.handle.close();
        true
    }
}

/// Hält am unteren Bildschirmrand einen Streifen für die Statusleiste frei: eine
/// unsichtbare, für Klicks durchlässige Layer-Shell-Fläche mit exklusiver Zone.
/// Maximierte und neue GUI-Fenster bleiben dadurch oberhalb der Leiste.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct BarReserve {
    #[cfg(target_os = "linux")]
    inner: imp::Reserve,
}

impl BarReserve {
    /// Höhe des Streifens in Pixeln; 0 gibt ihn frei. Ohne Änderung passiert nichts.
    pub fn set(&self, height: u32) {
        #[cfg(target_os = "linux")]
        self.inner.set(height);
        #[cfg(not(target_os = "linux"))]
        let _ = height;
    }
}

/// Startet die Beobachtung, wenn ein Wayland-Compositor erreichbar ist.
/// `send` gibt `false` zurück, sobald die Sitzung beendet ist.
pub fn start(send: impl Fn(GuiEvent) -> bool + Send + 'static) -> Result<BarReserve> {
    #[cfg(target_os = "linux")]
    {
        imp::start(Box::new(send)).map(|inner| BarReserve { inner })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = send;
        anyhow::bail!("grafische Fenster gibt es nur unter Linux/Wayland")
    }
}

/// Wiederherstellen/Hochholen von außen (z. B. TUILNUX-Desktop nach vorne).
#[cfg(target_os = "linux")]
mod imp {
    use std::collections::HashMap;
    use std::os::fd::AsFd;
    use std::sync::{Arc, Mutex};

    use anyhow::{Context, Result, bail};
    use wayland_client::backend::ObjectId;
    use wayland_client::protocol::{
        wl_buffer, wl_compositor, wl_output, wl_region, wl_registry, wl_seat, wl_shm, wl_shm_pool,
        wl_surface,
    };
    use wayland_client::{
        Connection, Dispatch, Proxy, QueueHandle, delegate_noop, event_created_child,
    };
    use wayland_protocols_wlr::foreign_toplevel::v1::client::{
        zwlr_foreign_toplevel_handle_v1 as handle, zwlr_foreign_toplevel_manager_v1 as manager,
    };
    use wayland_protocols_wlr::layer_shell::v1::client::{
        zwlr_layer_shell_v1 as layer_shell, zwlr_layer_surface_v1 as layer_surface,
    };

    use super::{GuiEvent, GuiState, Handle, Inner};

    struct Shared {
        conn: Connection,
        qh: QueueHandle<Client>,
        seat: Mutex<Option<wl_seat::WlSeat>>,
        bar: Mutex<Bar>,
    }

    /// Unsichtbare Fläche, die per exklusiver Zone die Statusleiste freihält.
    #[derive(Default)]
    struct Bar {
        compositor: Option<wl_compositor::WlCompositor>,
        shm: Option<wl_shm::WlShm>,
        layer_shell: Option<layer_shell::ZwlrLayerShellV1>,
        surface: Option<(wl_surface::WlSurface, layer_surface::ZwlrLayerSurfaceV1)>,
        height: u32,
        /// Bildschirm des Desktops (sonst wählt der Compositor).
        output: Option<wl_output::WlOutput>,
    }

    pub struct Reserve(Arc<Shared>);

    impl Reserve {
        pub fn set(&self, height: u32) {
            let shared = &self.0;
            let Ok(mut bar) = shared.bar.lock() else {
                return;
            };
            if bar.height == height {
                return;
            }
            bar.height = height;
            if height == 0 {
                if let Some((surface, layer)) = bar.surface.take() {
                    layer.destroy();
                    surface.destroy();
                }
            } else if let Some((surface, layer)) = &bar.surface {
                layer.set_size(0, height);
                layer.set_exclusive_zone(height as i32);
                surface.commit();
            } else if let (Some(compositor), Some(shell)) = (&bar.compositor, &bar.layer_shell) {
                let qh = &shared.qh;
                let surface = compositor.create_surface(qh, ());
                // Leerer Eingabebereich: Klicks gehen an das Terminal darunter.
                let region = compositor.create_region(qh, ());
                surface.set_input_region(Some(&region));
                region.destroy();
                let layer = shell.get_layer_surface(
                    &surface,
                    bar.output.as_ref(),
                    layer_shell::Layer::Bottom,
                    "tuilnux-bar".into(),
                    qh,
                    (),
                );
                layer.set_anchor(
                    layer_surface::Anchor::Bottom
                        | layer_surface::Anchor::Left
                        | layer_surface::Anchor::Right,
                );
                layer.set_size(0, height);
                layer.set_exclusive_zone(height as i32);
                surface.commit();
                bar.surface = Some((surface, layer));
            }
            let _ = shared.conn.flush();
        }
    }

    /// Vollständig transparenter Puffer (mit Nullen gefüllte Datei = ARGB 0).
    fn transparent_buffer(
        shared: &Shared,
        shm: &wl_shm::WlShm,
        width: u32,
        height: u32,
    ) -> std::io::Result<wl_buffer::WlBuffer> {
        let stride = width * 4;
        let size = stride * height;
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = dir.join(format!("tuilnux-bar-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        let _ = std::fs::remove_file(&path);
        file.set_len(u64::from(size))?;
        let pool = shm.create_pool(file.as_fd(), size as i32, &shared.qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            wl_shm::Format::Argb8888,
            &shared.qh,
            (),
        );
        pool.destroy();
        let _ = shared.conn.flush();
        Ok(buffer)
    }

    #[derive(Clone)]
    pub struct Toplevel {
        proxy: handle::ZwlrForeignToplevelHandleV1,
        shared: Arc<Shared>,
    }

    impl PartialEq for Toplevel {
        fn eq(&self, other: &Self) -> bool {
            self.proxy == other.proxy
        }
    }

    impl Toplevel {
        pub fn id(&self) -> ObjectId {
            self.proxy.id()
        }

        fn flush(&self) {
            let _ = self.shared.conn.flush();
        }

        pub fn activate(&self) {
            self.proxy.unset_minimized();
            if let Some(seat) = self.shared.seat.lock().ok().and_then(|s| s.clone()) {
                self.proxy.activate(&seat);
            }
            self.flush();
        }

        pub fn minimize(&self) {
            self.proxy.set_minimized();
            self.flush();
        }

        pub fn set_maximized(&self, on: bool) {
            if on {
                self.proxy.set_maximized();
            } else {
                self.proxy.unset_maximized();
            }
            self.flush();
        }

        pub fn close(&self) {
            self.proxy.close();
            self.flush();
        }
    }

    #[derive(Default)]
    struct Pending {
        title: String,
        app_id: String,
        minimized: bool,
        maximized: bool,
        activated: bool,
        outputs: Vec<wl_output::WlOutput>,
    }

    struct Output {
        global: u32,
        proxy: wl_output::WlOutput,
        height: i32,
        scale: i32,
    }

    struct Client {
        shared: Arc<Shared>,
        send: Box<dyn Fn(GuiEvent) -> bool + Send>,
        pending: HashMap<ObjectId, Pending>,
        manager: bool,
        outputs: Vec<Output>,
        /// Bildschirm, auf dem das Desktop-Terminal liegt.
        desktop_output: Option<wl_output::WlOutput>,
        /// Nach dem ersten Roundtrip: neue/entfernte Bildschirme melden.
        ready: bool,
        stop: bool,
    }

    impl Client {
        fn emit(&mut self, event: GuiEvent) {
            if !(self.send)(event) {
                self.stop = true;
            }
        }

        /// Bildschirm des Desktops, sonst der erste.
        fn main_output(&self) -> Option<&Output> {
            self.desktop_output
                .as_ref()
                .and_then(|d| self.outputs.iter().find(|o| &o.proxy == d))
                .or(self.outputs.first())
        }

        fn emit_height(&mut self) {
            if let Some(o) = self.main_output()
                && o.height > 0
            {
                let height = (o.height / o.scale.max(1)) as u32;
                self.emit(GuiEvent::Output { height });
            }
        }

        /// Desktop auf `output` gewechselt: Höhe melden, Leiste dorthin verlegen.
        fn set_desktop_output(&mut self, output: Option<wl_output::WlOutput>) {
            if output.is_none() || self.desktop_output == output {
                return;
            }
            self.desktop_output = output.clone();
            self.emit_height();
            let height = {
                let Ok(mut bar) = self.shared.bar.lock() else {
                    return;
                };
                bar.output = output;
                if let Some((surface, layer)) = bar.surface.take() {
                    layer.destroy();
                    surface.destroy();
                }
                std::mem::take(&mut bar.height)
            };
            if height > 0 {
                Reserve(self.shared.clone()).set(height);
            }
        }
    }

    impl Dispatch<wl_registry::WlRegistry, ()> for Client {
        fn event(
            state: &mut Self,
            registry: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            let (name, interface, version) = match event {
                wl_registry::Event::Global {
                    name,
                    interface,
                    version,
                } => (name, interface, version),
                wl_registry::Event::GlobalRemove { name } => {
                    let Some(i) = state.outputs.iter().position(|o| o.global == name) else {
                        return;
                    };
                    let gone = state.outputs.remove(i);
                    if state.desktop_output.as_ref() == Some(&gone.proxy) {
                        state.desktop_output = None;
                        let first = state.outputs.first().map(|o| o.proxy.clone());
                        state.set_desktop_output(first);
                    }
                    gone.proxy.release();
                    state.emit(GuiEvent::OutputsChanged);
                    return;
                }
                _ => return,
            };
            match interface.as_str() {
                "zwlr_foreign_toplevel_manager_v1" if !state.manager => {
                    registry.bind::<manager::ZwlrForeignToplevelManagerV1, _, _>(
                        name,
                        version.min(3),
                        qh,
                        (),
                    );
                    state.manager = true;
                }
                "wl_output" => {
                    // Version 3 für release().
                    let proxy =
                        registry.bind::<wl_output::WlOutput, _, _>(name, version.min(3), qh, ());
                    state.outputs.push(Output {
                        global: name,
                        proxy,
                        height: 0,
                        scale: 1,
                    });
                    if state.ready {
                        state.emit(GuiEvent::OutputsChanged);
                    }
                }
                "wl_compositor" => {
                    if let Ok(mut bar) = state.shared.bar.lock() {
                        bar.compositor = Some(registry.bind(name, version.min(4), qh, ()));
                    }
                }
                "wl_shm" => {
                    if let Ok(mut bar) = state.shared.bar.lock() {
                        bar.shm = Some(registry.bind(name, 1, qh, ()));
                    }
                }
                "zwlr_layer_shell_v1" => {
                    if let Ok(mut bar) = state.shared.bar.lock() {
                        bar.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
                    }
                }
                "wl_seat" => {
                    if let Ok(mut seat) = state.shared.seat.lock()
                        && seat.is_none()
                    {
                        *seat = Some(registry.bind::<wl_seat::WlSeat, _, _>(name, 1, qh, ()));
                    }
                }
                _ => {}
            }
        }
    }

    delegate_noop!(Client: ignore wl_compositor::WlCompositor);
    delegate_noop!(Client: ignore wl_shm::WlShm);
    delegate_noop!(Client: ignore wl_shm_pool::WlShmPool);
    delegate_noop!(Client: ignore wl_surface::WlSurface);
    delegate_noop!(Client: ignore wl_region::WlRegion);
    delegate_noop!(Client: ignore layer_shell::ZwlrLayerShellV1);

    impl Dispatch<wl_buffer::WlBuffer, ()> for Client {
        fn event(
            _: &mut Self,
            buffer: &wl_buffer::WlBuffer,
            event: wl_buffer::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            // Der Inhalt bleibt sichtbar; der Compositor braucht den Puffer nicht mehr.
            if let wl_buffer::Event::Release = event {
                buffer.destroy();
            }
        }
    }

    impl Dispatch<layer_surface::ZwlrLayerSurfaceV1, ()> for Client {
        fn event(
            state: &mut Self,
            layer: &layer_surface::ZwlrLayerSurfaceV1,
            event: layer_surface::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            let shared = state.shared.clone();
            let Ok(mut bar) = shared.bar.lock() else {
                return;
            };
            let ours = bar.surface.as_ref().is_some_and(|(_, l)| l == layer);
            match event {
                layer_surface::Event::Configure {
                    serial,
                    width,
                    height,
                } => {
                    layer.ack_configure(serial);
                    if let (true, Some(shm), Some((surface, _))) = (ours, &bar.shm, &bar.surface)
                        && width > 0
                        && height > 0
                        && let Ok(buffer) = transparent_buffer(&shared, shm, width, height)
                    {
                        surface.attach(Some(&buffer), 0, 0);
                        surface.damage(0, 0, width as i32, height as i32);
                        surface.commit();
                    }
                    let _ = shared.conn.flush();
                }
                layer_surface::Event::Closed => {
                    layer.destroy();
                    if ours {
                        if let Some((surface, _)) = bar.surface.take() {
                            surface.destroy();
                        }
                        // Beim nächsten `set` neu anlegen.
                        bar.height = 0;
                    }
                }
                _ => {}
            }
        }
    }

    impl Dispatch<wl_seat::WlSeat, ()> for Client {
        fn event(
            _: &mut Self,
            _: &wl_seat::WlSeat,
            _: wl_seat::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<wl_output::WlOutput, ()> for Client {
        fn event(
            state: &mut Self,
            proxy: &wl_output::WlOutput,
            event: wl_output::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            let Some(out) = state.outputs.iter_mut().find(|o| &o.proxy == proxy) else {
                return;
            };
            match event {
                wl_output::Event::Mode { flags, height, .. }
                    if flags
                        .into_result()
                        .is_ok_and(|f| f.contains(wl_output::Mode::Current)) =>
                {
                    out.height = height;
                }
                wl_output::Event::Scale { factor } => out.scale = factor,
                wl_output::Event::Done => {
                    let main = state.main_output().is_some_and(|o| &o.proxy == proxy);
                    if main {
                        state.emit_height();
                    }
                }
                _ => {}
            }
        }
    }

    impl Dispatch<manager::ZwlrForeignToplevelManagerV1, ()> for Client {
        fn event(
            state: &mut Self,
            _: &manager::ZwlrForeignToplevelManagerV1,
            event: manager::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                manager::Event::Toplevel { toplevel } => {
                    state.pending.insert(toplevel.id(), Pending::default());
                }
                manager::Event::Finished => state.stop = true,
                _ => {}
            }
        }

        event_created_child!(Client, manager::ZwlrForeignToplevelManagerV1, [
            manager::EVT_TOPLEVEL_OPCODE => (handle::ZwlrForeignToplevelHandleV1, ()),
        ]);
    }

    impl Dispatch<handle::ZwlrForeignToplevelHandleV1, ()> for Client {
        fn event(
            state: &mut Self,
            proxy: &handle::ZwlrForeignToplevelHandleV1,
            event: handle::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            let toplevel = || {
                Handle(Inner::Wayland(Toplevel {
                    proxy: proxy.clone(),
                    shared: state.shared.clone(),
                }))
            };
            match event {
                handle::Event::Title { title } => {
                    state.pending.entry(proxy.id()).or_default().title = title;
                }
                handle::Event::AppId { app_id } => {
                    state.pending.entry(proxy.id()).or_default().app_id = app_id;
                }
                handle::Event::State { state: raw } => {
                    let values: Vec<u32> = raw
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|c| u32::from_ne_bytes(*c))
                        .collect();
                    let has = |s: handle::State| values.contains(&(s as u32));
                    let p = state.pending.entry(proxy.id()).or_default();
                    p.minimized = has(handle::State::Minimized);
                    p.maximized = has(handle::State::Maximized);
                    p.activated = has(handle::State::Activated);
                }
                handle::Event::OutputEnter { output } => {
                    let p = state.pending.entry(proxy.id()).or_default();
                    if !p.outputs.contains(&output) {
                        p.outputs.push(output);
                    }
                }
                handle::Event::OutputLeave { output } => {
                    let p = state.pending.entry(proxy.id()).or_default();
                    p.outputs.retain(|o| o != &output);
                }
                handle::Event::Done => {
                    let handle = toplevel();
                    let p = state.pending.entry(proxy.id()).or_default();
                    let update = GuiState {
                        handle,
                        title: p.title.clone(),
                        app_id: p.app_id.clone(),
                        minimized: p.minimized,
                        maximized: p.maximized,
                        activated: p.activated,
                    };
                    let desktop = (p.app_id == super::DESKTOP_APP_ID)
                        .then(|| p.outputs.first().cloned())
                        .flatten();
                    state.emit(GuiEvent::Update(update));
                    state.set_desktop_output(desktop);
                }
                handle::Event::Closed => {
                    let handle = toplevel();
                    state.pending.remove(&proxy.id());
                    state.emit(GuiEvent::Closed(handle));
                    proxy.destroy();
                }
                _ => {}
            }
        }
    }

    pub fn start(send: Box<dyn Fn(GuiEvent) -> bool + Send>) -> Result<Reserve> {
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            bail!("kein Wayland-Compositor (WAYLAND_DISPLAY fehlt)");
        }
        let conn = Connection::connect_to_env().context("Wayland-Verbindung")?;
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());
        let mut client = Client {
            shared: Arc::new(Shared {
                conn: conn.clone(),
                qh: qh.clone(),
                seat: Mutex::new(None),
                bar: Mutex::new(Bar::default()),
            }),
            send,
            pending: HashMap::new(),
            manager: false,
            outputs: Vec::new(),
            desktop_output: None,
            ready: false,
            stop: false,
        };
        queue.roundtrip(&mut client).context("Wayland-Registry")?;
        if !client.manager {
            bail!("der Compositor bietet zwlr_foreign_toplevel_manager_v1 nicht an");
        }
        client.ready = true;
        let reserve = Reserve(client.shared.clone());
        std::thread::Builder::new()
            .name("tuilnux-gui".into())
            .spawn(move || {
                while !client.stop {
                    if queue.blocking_dispatch(&mut client).is_err() {
                        break;
                    }
                }
            })?;
        Ok(reserve)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(n: u64, app_id: &str, title: &str, minimized: bool, activated: bool) -> GuiEvent {
        GuiEvent::Update(GuiState {
            handle: Handle::fake(n),
            title: title.into(),
            app_id: app_id.into(),
            minimized,
            maximized: false,
            activated,
        })
    }

    #[test]
    fn tracks_windows_and_skips_desktop() {
        let mut gui = GuiWindows::default();
        let mut next = 5;
        let mut alloc = || {
            next += 1;
            next - 1
        };
        assert_eq!(
            gui.apply(
                update(1, DESKTOP_APP_ID, "tuilnux", false, true),
                &mut alloc
            ),
            None
        );
        assert!(gui.desktop_active && gui.is_empty());
        assert_eq!(
            gui.apply(
                update(2, "firefox", "Mozilla Firefox", false, true),
                &mut alloc
            ),
            Some(5)
        );
        assert!(!gui.desktop_active);
        assert_eq!(gui.active(), Some(5));
        assert_eq!(
            gui.apply(update(2, "firefox", "", true, false), &mut alloc),
            None
        );
        assert_eq!(gui.get(5).unwrap().title(), "firefox");
        assert_eq!(gui.active(), None);
        assert!(gui.activate(5));
        assert_eq!(gui.active(), Some(5));
        assert!(gui.minimize(5) && gui.get(5).unwrap().state.minimized);
        assert!(!gui.activate(9));
        gui.apply(GuiEvent::Closed(Handle::fake(2)), &mut alloc);
        assert!(gui.is_empty());
    }

    #[test]
    fn bar_click_after_focus_loss_still_counts_as_active() {
        let mut gui = GuiWindows::default();
        gui.apply(update(2, "firefox", "", false, true), || 1);
        gui.apply(update(2, "firefox", "", false, false), || 1);
        gui.apply(update(1, DESKTOP_APP_ID, "", false, true), || 9);
        assert_eq!(gui.active(), None);
        assert!(gui.was_active(1));
        gui.minimize(1);
        assert!(!gui.was_active(1));
    }
}
