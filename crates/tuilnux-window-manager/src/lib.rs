//! Fensterlogik von TUILNUX: schwebende Fenster über dem Vollbild-Terminal.
//!
//! Die Logik ist rein (keine Ein-/Ausgabe) und damit unabhängig vom Ausgabeweg testbar.
//! Heute zeichnet sie der TUI-Manager in einem Terminal, später kann der
//! Smithay-Compositor dieselbe Logik verwenden.

use ratatui::layout::Rect;

pub const MIN_WIDTH: u16 = 20;
pub const MIN_HEIGHT: u16 = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: usize,
    pub title: String,
    /// Äußeres Rechteck inklusive Rahmen.
    pub rect: Rect,
    pub minimized: bool,
    pub maximized: bool,
    restore: Rect,
}

impl Window {
    /// Innenbereich ohne Rahmen.
    pub fn inner(&self) -> Rect {
        inner(self.rect)
    }
}

pub fn inner(rect: Rect) -> Rect {
    Rect::new(
        rect.x + 1,
        rect.y + 1,
        rect.width.saturating_sub(2),
        rect.height.saturating_sub(2),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Minimize,
    Maximize,
    Close,
}

impl Button {
    pub const ALL: [Button; 3] = [Button::Minimize, Button::Maximize, Button::Close];

    /// Spalte des Knopfes in der Titelzeile: `… _ □ x ┐`.
    pub fn x(self, rect: Rect) -> u16 {
        let right = rect.right();
        match self {
            Button::Close => right.saturating_sub(3),
            Button::Maximize => right.saturating_sub(5),
            Button::Minimize => right.saturating_sub(7),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Desktop,
    Window(usize),
}

/// Rahmenkante, an der eine Größenänderung angefasst wurde.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Bottom,
    BottomLeft,
    BottomRight,
}

impl Edge {
    fn left(self) -> bool {
        matches!(self, Edge::Left | Edge::BottomLeft)
    }

    fn right(self) -> bool {
        matches!(self, Edge::Right | Edge::BottomRight)
    }

    fn bottom(self) -> bool {
        matches!(self, Edge::Bottom | Edge::BottomLeft | Edge::BottomRight)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Desktop,
    Title(usize),
    Button(usize, Button),
    /// Rahmen oder Ecke zum Ziehen der Größe.
    Resize(usize, Edge),
    /// Rahmen ohne besondere Funktion.
    Border(usize),
    /// Innenbereich, Koordinaten relativ zum Inhalt.
    Content {
        id: usize,
        col: u16,
        row: u16,
    },
}

impl Hit {
    pub fn window(self) -> Option<usize> {
        match self {
            Hit::Desktop => None,
            Hit::Title(id)
            | Hit::Button(id, _)
            | Hit::Resize(id, _)
            | Hit::Border(id)
            | Hit::Content { id, .. } => Some(id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drag {
    Move {
        id: usize,
        dx: u16,
        dy: u16,
    },
    Resize {
        id: usize,
        edge: Edge,
        /// Rechteck und Mausposition beim Anfassen.
        start: Rect,
        x: u16,
        y: u16,
    },
}

#[derive(Debug, Clone)]
pub struct Wm {
    /// Z-Reihenfolge: das letzte Fenster liegt oben.
    windows: Vec<Window>,
    focus: Focus,
    area: Rect,
    next_id: usize,
    opened: usize,
    drag: Option<Drag>,
    /// Innengröße neuer Fenster ohne Größenangabe (None = 2/3 des Bildschirms).
    default_size: Option<(u16, u16)>,
    /// Neue Fenster mittig statt versetzt öffnen.
    center: bool,
}

impl Wm {
    pub fn new(area: Rect) -> Self {
        Self {
            windows: Vec::new(),
            focus: Focus::Desktop,
            area,
            next_id: 1,
            opened: 0,
            drag: None,
            default_size: None,
            center: false,
        }
    }

    /// Größe (Innenmaß, None = automatisch) und Platzierung neuer Fenster.
    pub fn set_defaults(&mut self, size: Option<(u16, u16)>, center: bool) {
        self.default_size = size;
        self.center = center;
    }

    /// Vergibt eine Nummer ohne Fenster (z. B. für grafische Programme unter labwc).
    pub fn alloc_id(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn area(&self) -> Rect {
        self.area
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn focused(&self) -> Option<usize> {
        match self.focus {
            Focus::Desktop => None,
            Focus::Window(id) => Some(id),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    pub fn len(&self) -> usize {
        self.windows.len()
    }

    pub fn get(&self, id: usize) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    fn get_mut(&mut self, id: usize) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    /// Alle Fenster von unten nach oben.
    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    /// Sichtbare Fenster von unten nach oben (zum Zeichnen).
    pub fn visible(&self) -> impl Iterator<Item = &Window> {
        self.windows.iter().filter(|w| !w.minimized)
    }

    /// Fenster nach Nummer sortiert (für Taskleiste und Taskmanager).
    pub fn by_id(&self) -> Vec<&Window> {
        let mut v: Vec<&Window> = self.windows.iter().collect();
        v.sort_by_key(|w| w.id);
        v
    }

    pub fn set_title(&mut self, id: usize, title: &str) {
        if let Some(w) = self.get_mut(id) {
            w.title = title.to_string();
        }
    }

    /// Öffnet ein neues Fenster (kaskadiert) und fokussiert es.
    pub fn open(&mut self, title: &str, size: Option<(u16, u16)>) -> usize {
        let id = self.alloc_id();
        self.open_with_id(id, title, size);
        id
    }

    /// Wie [`Wm::open`], aber mit einer vorher per [`Wm::alloc_id`] vergebenen ID
    /// (z. B. für ein Hintergrundprogramm, das erst später ein Fenster bekommt).
    pub fn open_with_id(&mut self, id: usize, title: &str, size: Option<(u16, u16)>) {
        let area = self.area;
        let (w, h) = size.or(self.default_size).map_or(
            (
                (area.width * 2 / 3).max(MIN_WIDTH),
                (area.height * 2 / 3).max(MIN_HEIGHT),
            ),
            // Gewünschte Größe meint den Inhalt, der Rahmen kommt dazu.
            |(c, r)| (c.saturating_add(2), r.saturating_add(2)),
        );
        let step = (self.opened % 8) as u16;
        self.opened += 1;
        let rect = if self.center {
            let (w, h) = (w.min(area.width), h.min(area.height));
            Rect::new(
                area.x + (area.width - w) / 2,
                area.y + (area.height - h) / 2,
                w,
                h,
            )
        } else {
            Rect::new(area.x + 2 + step * 3, area.y + 1 + step, w, h)
        };
        let rect = self.clamp(rect);
        self.windows.push(Window {
            id,
            title: title.to_string(),
            rect,
            minimized: false,
            maximized: false,
            restore: rect,
        });
        self.focus = Focus::Window(id);
    }

    /// Schließt ein Fenster; der Fokus wandert zum obersten sichtbaren Fenster.
    pub fn close(&mut self, id: usize) -> bool {
        let before = self.windows.len();
        self.windows.retain(|w| w.id != id);
        if matches!(self.drag, Some(Drag::Move { id: d, .. } | Drag::Resize { id: d, .. }) if d == id)
        {
            self.drag = None;
        }
        if self.focus == Focus::Window(id) {
            self.focus_top();
        }
        if self.windows.is_empty() {
            self.opened = 0;
        }
        self.windows.len() != before
    }

    fn focus_top(&mut self) {
        self.focus = self
            .windows
            .iter()
            .rev()
            .find(|w| !w.minimized)
            .map_or(Focus::Desktop, |w| Focus::Window(w.id));
    }

    /// Fokussiert ein Fenster, holt es nach oben und stellt es ggf. wieder her.
    pub fn focus_window(&mut self, id: usize) -> bool {
        let Some(pos) = self.windows.iter().position(|w| w.id == id) else {
            return false;
        };
        let mut w = self.windows.remove(pos);
        w.minimized = false;
        self.windows.push(w);
        self.focus = Focus::Window(id);
        true
    }

    pub fn focus_desktop(&mut self) {
        self.focus = Focus::Desktop;
    }

    /// Zyklisch: Desktop → Fenster 1 → Fenster 2 → … → Desktop.
    pub fn focus_next(&mut self) {
        let ids: Vec<usize> = self.by_id().iter().map(|w| w.id).collect();
        let next = match self.focus {
            Focus::Desktop => ids.first().copied(),
            Focus::Window(cur) => ids.iter().copied().find(|&id| id > cur),
        };
        match next {
            Some(id) => {
                self.focus_window(id);
            }
            None => self.focus_desktop(),
        }
    }

    pub fn minimize(&mut self, id: usize) {
        if let Some(w) = self.get_mut(id) {
            w.minimized = true;
            if self.focus == Focus::Window(id) {
                self.focus_top();
            }
        }
    }

    pub fn toggle_maximize(&mut self, id: usize) {
        let area = self.area;
        if let Some(w) = self.get_mut(id) {
            if w.maximized {
                w.maximized = false;
                w.rect = w.restore;
            } else {
                w.restore = w.rect;
                w.maximized = true;
                w.rect = area;
            }
        }
        if let Some(w) = self.get(id) {
            let rect = self.clamp(w.rect);
            self.get_mut(id).unwrap().rect = rect;
        }
    }

    /// Hält ein Rechteck innerhalb des Arbeitsbereichs und über der Mindestgröße.
    fn clamp(&self, r: Rect) -> Rect {
        let a = self.area;
        let w = r.width.clamp(MIN_WIDTH.min(a.width), a.width.max(1));
        let h = r.height.clamp(MIN_HEIGHT.min(a.height), a.height.max(1));
        let x = r.x.clamp(a.x, a.right().saturating_sub(w).max(a.x));
        let y = r.y.clamp(a.y, a.bottom().saturating_sub(h).max(a.y));
        Rect::new(x, y, w, h)
    }

    fn set_rect(&mut self, id: usize, rect: Rect) -> bool {
        let rect = self.clamp(rect);
        match self.get_mut(id) {
            Some(w) if w.rect != rect => {
                w.rect = rect;
                w.maximized = false;
                w.restore = rect;
                true
            }
            _ => false,
        }
    }

    pub fn move_by(&mut self, id: usize, dx: i32, dy: i32) -> bool {
        let Some(w) = self.get(id) else { return false };
        let r = w.rect;
        let x = (r.x as i32 + dx).max(0) as u16;
        let y = (r.y as i32 + dy).max(0) as u16;
        self.set_rect(id, Rect { x, y, ..r })
    }

    pub fn resize_by(&mut self, id: usize, dw: i32, dh: i32) -> bool {
        let Some(w) = self.get(id) else { return false };
        let r = w.rect;
        let width = (r.width as i32 + dw).max(1) as u16;
        let height = (r.height as i32 + dh).max(1) as u16;
        self.set_rect(id, Rect { width, height, ..r })
    }

    /// Neue Bildschirmgröße: alle Fenster einpassen, maximierte füllen den Bereich.
    pub fn set_area(&mut self, area: Rect) {
        self.area = area;
        for i in 0..self.windows.len() {
            let w = &self.windows[i];
            let rect = if w.maximized {
                area
            } else {
                self.clamp(w.rect)
            };
            let restore = self.clamp(w.restore);
            let w = &mut self.windows[i];
            w.rect = rect;
            w.restore = restore;
        }
    }

    pub fn hit_test(&self, x: u16, y: u16) -> Hit {
        for w in self.windows.iter().rev().filter(|w| !w.minimized) {
            let r = w.rect;
            if x < r.x || x >= r.right() || y < r.y || y >= r.bottom() {
                continue;
            }
            if y == r.y {
                return Button::ALL
                    .into_iter()
                    .find(|b| b.x(r) == x)
                    .map_or(Hit::Title(w.id), |b| Hit::Button(w.id, b));
            }
            if !w.maximized {
                let bottom = y == r.bottom() - 1;
                // Ecken sind zwei Zellen breit, damit man sie leichter trifft.
                let left = x <= r.x + u16::from(bottom);
                let right = x + 1 + u16::from(bottom) >= r.right();
                let edge = match (left, right, bottom) {
                    (true, _, true) => Some(Edge::BottomLeft),
                    (_, true, true) => Some(Edge::BottomRight),
                    (_, _, true) => Some(Edge::Bottom),
                    (true, _, false) => Some(Edge::Left),
                    (_, true, false) => Some(Edge::Right),
                    _ => None,
                };
                if let Some(edge) = edge {
                    return Hit::Resize(w.id, edge);
                }
            }
            let inner = w.inner();
            if x >= inner.x && x < inner.right() && y >= inner.y && y < inner.bottom() {
                return Hit::Content {
                    id: w.id,
                    col: x - inner.x,
                    row: y - inner.y,
                };
            }
            return Hit::Border(w.id);
        }
        Hit::Desktop
    }

    /// Maustaste gedrückt: fokussiert, startet Ziehen/Größenänderung, löst Knöpfe aus.
    pub fn mouse_down(&mut self, x: u16, y: u16) -> Hit {
        let hit = self.hit_test(x, y);
        self.drag = None;
        match hit {
            Hit::Desktop => self.focus_desktop(),
            Hit::Button(id, Button::Close) => {
                // Das Schließen übernimmt der Aufrufer (Kindprozess beenden).
                self.focus_window(id);
            }
            Hit::Button(id, Button::Minimize) => self.minimize(id),
            Hit::Button(id, Button::Maximize) => {
                self.focus_window(id);
                self.toggle_maximize(id);
            }
            Hit::Title(id) => {
                self.focus_window(id);
                let r = self.get(id).unwrap().rect;
                self.drag = Some(Drag::Move {
                    id,
                    dx: x - r.x,
                    dy: y - r.y,
                });
            }
            Hit::Resize(id, edge) => {
                self.focus_window(id);
                let start = self.get(id).unwrap().rect;
                self.drag = Some(Drag::Resize {
                    id,
                    edge,
                    start,
                    x,
                    y,
                });
            }
            Hit::Border(id) | Hit::Content { id, .. } => {
                self.focus_window(id);
            }
        }
        hit
    }

    /// Maus mit gedrückter Taste bewegt. Liefert `true`, wenn sich etwas geändert hat.
    pub fn mouse_drag(&mut self, x: u16, y: u16) -> bool {
        match self.drag {
            Some(Drag::Move { id, dx, dy }) => {
                let Some(w) = self.get(id) else { return false };
                let mut r = w.rect;
                let mut dx = dx;
                if w.maximized {
                    // Aus maximiertem Zustand herausziehen: alte Größe wiederherstellen.
                    r = w.restore;
                    dx = dx.min(r.width.saturating_sub(4));
                    self.drag = Some(Drag::Move { id, dx, dy });
                }
                r.x = x.saturating_sub(dx);
                r.y = y.saturating_sub(dy);
                self.set_rect(id, r)
            }
            Some(Drag::Resize {
                id,
                edge,
                start,
                x: x0,
                y: y0,
            }) => {
                if self.get(id).is_none() {
                    return false;
                }
                let a = self.area;
                let (dx, dy) = (x as i32 - x0 as i32, y as i32 - y0 as i32);
                let mut r = start;
                if edge.right() {
                    let max = a.right().saturating_sub(start.x) as i32;
                    r.width = (start.width as i32 + dx).clamp(MIN_WIDTH as i32, max.max(1)) as u16;
                }
                if edge.left() {
                    // Die rechte Kante bleibt stehen.
                    let right = start.right() as i32;
                    let left = (start.x as i32 + dx)
                        .clamp(a.x as i32, (right - MIN_WIDTH as i32).max(a.x as i32));
                    r.x = left as u16;
                    r.width = (right - left) as u16;
                }
                if edge.bottom() {
                    let max = a.bottom().saturating_sub(start.y) as i32;
                    r.height =
                        (start.height as i32 + dy).clamp(MIN_HEIGHT as i32, max.max(1)) as u16;
                }
                self.set_rect(id, r)
            }
            None => false,
        }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn mouse_up(&mut self) {
        self.drag = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wm() -> Wm {
        Wm::new(Rect::new(0, 0, 120, 39))
    }

    #[test]
    fn open_cascades_and_focuses() {
        let mut wm = wm();
        let a = wm.open("tfiles", None);
        let b = wm.open("ttop", Some((60, 20)));
        assert_eq!((a, b), (1, 2));
        assert_eq!(wm.focus(), Focus::Window(2));
        let (ra, rb) = (wm.get(a).unwrap().rect, wm.get(b).unwrap().rect);
        assert_eq!((ra.x, ra.y), (2, 1));
        assert_eq!((rb.x, rb.y, rb.width, rb.height), (5, 2, 62, 22));
        assert_eq!(wm.get(b).unwrap().inner(), Rect::new(6, 3, 60, 20));
    }

    #[test]
    fn huge_windows_are_clamped() {
        let mut wm = wm();
        let id = wm.open("big", Some((500, 500)));
        assert_eq!(wm.get(id).unwrap().rect, Rect::new(0, 0, 120, 39));
    }

    #[test]
    fn hit_test_regions() {
        let mut wm = wm();
        let id = wm.open("x", Some((30, 10)));
        let r = wm.get(id).unwrap().rect; // 2,1 32x12
        assert_eq!(wm.hit_test(0, 0), Hit::Desktop);
        assert_eq!(wm.hit_test(r.x + 3, r.y), Hit::Title(id));
        assert_eq!(
            wm.hit_test(r.right() - 3, r.y),
            Hit::Button(id, Button::Close)
        );
        assert_eq!(
            wm.hit_test(r.right() - 5, r.y),
            Hit::Button(id, Button::Maximize)
        );
        assert_eq!(
            wm.hit_test(r.right() - 7, r.y),
            Hit::Button(id, Button::Minimize)
        );
        assert_eq!(
            wm.hit_test(r.right() - 1, r.bottom() - 1),
            Hit::Resize(id, Edge::BottomRight)
        );
        assert_eq!(
            wm.hit_test(r.right() - 2, r.bottom() - 1),
            Hit::Resize(id, Edge::BottomRight)
        );
        assert_eq!(
            wm.hit_test(r.x, r.bottom() - 1),
            Hit::Resize(id, Edge::BottomLeft)
        );
        assert_eq!(
            wm.hit_test(r.x + 5, r.bottom() - 1),
            Hit::Resize(id, Edge::Bottom)
        );
        assert_eq!(wm.hit_test(r.x, r.y + 3), Hit::Resize(id, Edge::Left));
        assert_eq!(
            wm.hit_test(r.right() - 1, r.y + 3),
            Hit::Resize(id, Edge::Right)
        );
        assert_eq!(
            wm.hit_test(r.x + 1, r.y + 1),
            Hit::Content { id, col: 0, row: 0 }
        );
        // Maximierte Fenster haben keine Ziehkanten.
        wm.toggle_maximize(id);
        let r = wm.get(id).unwrap().rect;
        assert_eq!(wm.hit_test(r.x, r.y + 3), Hit::Border(id));
    }

    #[test]
    fn topmost_window_wins_and_click_raises() {
        let mut wm = wm();
        let a = wm.open("a", Some((40, 10)));
        let b = wm.open("b", Some((40, 10)));
        // Überlappung: b liegt oben.
        assert_eq!(wm.hit_test(10, 5).window(), Some(b));
        // Klick auf einen nur von a sichtbaren Punkt holt a nach oben.
        wm.mouse_down(3, 2);
        assert_eq!(wm.focus(), Focus::Window(a));
        assert_eq!(wm.windows().last().unwrap().id, a);
        assert_eq!(wm.hit_test(10, 5).window(), Some(a));
        wm.mouse_down(100, 30);
        assert_eq!(wm.focus(), Focus::Desktop);
    }

    #[test]
    fn drag_moves_and_resizes() {
        let mut wm = wm();
        let id = wm.open("x", Some((30, 10)));
        wm.mouse_down(10, 1);
        assert!(wm.dragging());
        assert!(wm.mouse_drag(20, 5));
        assert_eq!(wm.get(id).unwrap().rect, Rect::new(12, 5, 32, 12));
        // Nicht aus dem Bildschirm schieben.
        wm.mouse_drag(500, 500);
        let r = wm.get(id).unwrap().rect;
        assert_eq!((r.right(), r.bottom()), (120, 39));
        wm.mouse_up();
        assert!(!wm.mouse_drag(0, 0));

        let r = wm.get(id).unwrap().rect;
        wm.mouse_down(r.right() - 1, r.bottom() - 1);
        wm.mouse_drag(r.x + 1, r.y + 1);
        let small = wm.get(id).unwrap().rect;
        assert_eq!((small.width, small.height), (MIN_WIDTH, MIN_HEIGHT));
        wm.mouse_up();
    }

    #[test]
    fn resize_from_every_edge() {
        let mut wm = wm();
        let id = wm.open("x", Some((30, 10)));
        wm.move_by(id, 20, 5);
        let r = wm.get(id).unwrap().rect; // 22,6 32x12
        assert_eq!(r, Rect::new(22, 6, 32, 12));

        // Rechte Kante: nur die Breite ändert sich.
        wm.mouse_down(r.right() - 1, r.y + 4);
        wm.mouse_drag(r.right() + 9, r.y + 8);
        assert_eq!(wm.get(id).unwrap().rect, Rect::new(22, 6, 42, 12));
        wm.mouse_up();

        // Linke Kante: die rechte Kante bleibt stehen.
        let r = wm.get(id).unwrap().rect;
        wm.mouse_down(r.x, r.y + 4);
        wm.mouse_drag(r.x - 10, r.y + 4);
        assert_eq!(wm.get(id).unwrap().rect, Rect::new(12, 6, 52, 12));
        // Nicht unter die Mindestbreite und nicht über die rechte Kante hinaus.
        wm.mouse_drag(200, r.y + 4);
        assert_eq!(
            wm.get(id).unwrap().rect,
            Rect::new(64 - MIN_WIDTH, 6, MIN_WIDTH, 12)
        );
        wm.mouse_drag(0, r.y);
        assert_eq!(wm.get(id).unwrap().rect, Rect::new(0, 6, 64, 12));
        wm.mouse_up();

        // Untere Kante: nur die Höhe.
        let r = wm.get(id).unwrap().rect;
        wm.mouse_down(r.x + 10, r.bottom() - 1);
        wm.mouse_drag(r.x + 30, r.bottom() + 4);
        assert_eq!(wm.get(id).unwrap().rect, Rect::new(0, 6, 64, 17));
        wm.mouse_drag(r.x + 30, 500);
        assert_eq!(wm.get(id).unwrap().rect.bottom(), 39);
        wm.mouse_up();

        // Ecke unten links: Breite und Höhe, rechte Kante bleibt.
        let r = wm.get(id).unwrap().rect;
        wm.mouse_down(r.x, r.bottom() - 1);
        wm.mouse_drag(r.x + 4, r.bottom() - 6);
        assert_eq!(wm.get(id).unwrap().rect, Rect::new(4, 6, 60, r.height - 5));
        wm.mouse_up();
    }

    #[test]
    fn minimize_maximize_and_restore() {
        let mut wm = wm();
        let a = wm.open("a", Some((30, 10)));
        let b = wm.open("b", Some((30, 10)));
        wm.minimize(b);
        assert_eq!(wm.focus(), Focus::Window(a));
        assert_eq!(wm.visible().count(), 1);
        wm.focus_window(b);
        assert!(!wm.get(b).unwrap().minimized);
        let before = wm.get(b).unwrap().rect;
        wm.toggle_maximize(b);
        assert_eq!(wm.get(b).unwrap().rect, wm.area());
        wm.set_area(Rect::new(0, 0, 80, 23));
        assert_eq!(wm.get(b).unwrap().rect, Rect::new(0, 0, 80, 23));
        wm.set_area(Rect::new(0, 0, 120, 39));
        wm.toggle_maximize(b);
        assert_eq!(wm.get(b).unwrap().rect, before);
        // Titelleisten-Knöpfe per Maus.
        let r = wm.get(b).unwrap().rect;
        wm.mouse_down(Button::Minimize.x(r), r.y);
        assert!(wm.get(b).unwrap().minimized);
    }

    #[test]
    fn focus_cycle_and_close() {
        let mut wm = wm();
        let a = wm.open("a", None);
        let b = wm.open("b", None);
        wm.focus_desktop();
        wm.focus_next();
        assert_eq!(wm.focused(), Some(a));
        wm.focus_next();
        assert_eq!(wm.focused(), Some(b));
        wm.focus_next();
        assert_eq!(wm.focused(), None);
        wm.focus_window(a);
        assert!(wm.close(a));
        assert_eq!(wm.focused(), Some(b));
        assert!(!wm.close(a));
        wm.close(b);
        assert!(wm.is_empty());
        assert_eq!(wm.focus(), Focus::Desktop);
    }

    #[test]
    fn keyboard_move_and_resize() {
        let mut wm = wm();
        let id = wm.open("x", Some((30, 10)));
        wm.move_by(id, -10, -10);
        assert_eq!(wm.get(id).unwrap().rect.x, 0);
        wm.move_by(id, 4, 2);
        wm.resize_by(id, 6, 3);
        assert_eq!(wm.get(id).unwrap().rect, Rect::new(4, 2, 38, 15));
    }

    #[test]
    fn tiny_screens_do_not_panic() {
        let mut wm = Wm::new(Rect::new(0, 0, 10, 3));
        let id = wm.open("x", None);
        let r = wm.get(id).unwrap().rect;
        assert!(r.width <= 10 && r.height <= 3);
        wm.mouse_down(0, 0);
        wm.mouse_drag(9, 2);
        wm.set_area(Rect::new(0, 0, 1, 1));
        wm.hit_test(0, 0);
    }
}
