//! App di MIN-OS: Esplora file, Blocco note, Calcolatrice e Cestino.

use crate::fat::{self, Dir, Entry};
use crate::font;
use crate::gfx::{mix, rgb, Canvas, BLACK, WHITE};
use crate::i18n_desk::Dk;
use crate::input::Key;
use crate::ui::{self, Ui};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Nomi delle cartelle di sistema sul disco (uguali in tutte le lingue).
pub const DOCS: &str = "Documents";
pub const TRASH: &str = "Recycle";
pub const DESKTOP: &str = "Desktop";
pub const DOWNLOADS: &str = "Downloads";

pub fn human(n: u32) -> String {
    if n < 1024 {
        format!("{} B", n)
    } else if n < 1024 * 1024 {
        format!("{} KB", n.div_ceil(1024))
    } else {
        format!("{}.{} MB", n / (1024 * 1024), (n % (1024 * 1024)) * 10 / (1024 * 1024))
    }
}

/// Nome da mostrare per le cartelle di sistema.
pub fn display_name(e: &Entry, dk: &Dk) -> String {
    if e.dir && e.name == DOCS {
        String::from(dk.documents)
    } else if e.dir && e.name == DESKTOP {
        String::from(dk.desktop_name)
    } else if e.dir && e.name == DOWNLOADS {
        String::from(dk.downloads)
    } else {
        e.name.clone()
    }
}

pub fn draw_folder(c: &mut Canvas, x: i32, y: i32, s: i32) {
    let h = s * 3 / 4;
    let top = y + (s - h) / 2;
    c.rrect(x, top - s / 10, s * 4 / 10, s / 5, s / 12, rgb(230, 170, 40), 256);
    c.rrect_vgrad(x, top, s, h, s / 8, rgb(255, 214, 90), rgb(245, 175, 40), 256);
    c.rrect(x, top, s, h / 3, s / 8, WHITE, 40);
}

pub fn draw_doc(c: &mut Canvas, x: i32, y: i32, s: i32) {
    let w = s * 3 / 4;
    let px = x + (s - w) / 2;
    c.rrect(px, y, w, s, s / 8, rgb(250, 250, 252), 256);
    c.rrect_border(px, y, w, s, s / 8, rgb(170, 175, 190), 200);
    for i in 0..3 {
        let ly = y + s * (35 + i * 18) / 100;
        c.rect(px + w / 5, ly, w * 3 / 5, (s / 16).max(1), rgb(150, 160, 185), 256);
    }
}

// ------------------------------------------------------------------ operazioni comuni

/// Nome libero in una cartella: "Nuova cartella", "Nuova cartella (2)", ...
pub fn unique_name(dir: Dir, base: &str) -> String {
    if fat::find(dir, base).is_none() {
        return String::from(base);
    }
    let (stem, ext) = match base.rfind('.') {
        Some(i) if i > 0 => (&base[..i], &base[i..]),
        _ => (base, ""),
    };
    for n in 2..1000 {
        let cand = format!("{} ({}){}", stem, n, ext);
        if fat::find(dir, &cand).is_none() {
            return cand;
        }
    }
    String::from(base)
}

/// Sposta un elemento nel Cestino (che e' una cartella nascosta del disco).
pub fn trash(e: &Entry) -> bool {
    match fat::ensure_dir(TRASH) {
        Some(bin) => {
            let name = unique_name(bin, &e.name);
            fat::move_to(e, bin, &name)
        }
        None => false,
    }
}

/// Rinomina: ritorna true se il nome e' cambiato.
pub fn rename(dir: Dir, e: &Entry, new: &str) -> bool {
    let new = new.trim();
    !new.is_empty() && new != e.name && fat::move_to(e, dir, new)
}

/// Crea una cartella o un documento di testo con nome libero; ritorna il nome.
pub fn create_new(dir: Dir, folder: bool, dk: &Dk) -> Option<String> {
    let name = unique_name(dir, if folder { dk.new_folder } else { dk.new_text_name });
    let ok = if folder { fat::mkdir(dir, &name).is_some() } else { fat::write(dir, &name, b"") };
    if ok {
        Some(name)
    } else {
        None
    }
}

/// Modifica di un nome come in Windows: all'inizio il nome (senza estensione) e' selezionato
/// e il primo tasto lo sostituisce; poi si scrive alla posizione del cursore.
#[derive(Clone)]
pub struct NameEdit {
    pub buf: String,
    /// None = nome selezionato; Some(n) = cursore dopo n caratteri
    pub cur: Option<usize>,
}

impl NameEdit {
    pub fn new(name: &str) -> NameEdit {
        NameEdit { buf: String::from(name), cur: None }
    }

    fn stem_len(&self) -> usize {
        match self.buf.rfind('.') {
            Some(i) if i > 0 => self.buf[..i].chars().count(),
            _ => self.buf.chars().count(),
        }
    }

    /// Ritorna Some(true) per confermare, Some(false) per annullare.
    pub fn keys(&mut self, keys: &[Key]) -> Option<bool> {
        for k in keys {
            let mut v: Vec<char> = self.buf.chars().collect();
            match *k {
                Key::Char(c) if !"\\/:*?\"<>|".contains(c) => {
                    let pos = match self.cur {
                        None => {
                            let n = self.stem_len();
                            v.drain(..n);
                            0
                        }
                        Some(p) => p,
                    };
                    if v.len() < 60 {
                        v.insert(pos, c);
                        self.cur = Some(pos + 1);
                    }
                }
                Key::Backspace => match self.cur {
                    None => {
                        let n = self.stem_len();
                        v.drain(..n);
                        self.cur = Some(0);
                    }
                    Some(p) if p > 0 => {
                        v.remove(p - 1);
                        self.cur = Some(p - 1);
                    }
                    _ => {}
                },
                Key::Delete => {
                    if let Some(p) = self.cur {
                        if p < v.len() {
                            v.remove(p);
                        }
                    }
                }
                Key::Left => self.cur = Some(self.cur.unwrap_or(self.stem_len()).saturating_sub(1)),
                Key::Right => self.cur = Some((self.cur.unwrap_or(self.stem_len()) + 1).min(v.len())),
                Key::Home => self.cur = Some(0),
                Key::End => self.cur = Some(v.len()),
                Key::Enter => return Some(true),
                Key::Esc => return Some(false),
                _ => {}
            }
            self.buf = v.into_iter().collect();
        }
        None
    }

    /// Disegna il campo (testo, selezione o cursore) in un rettangolo.
    pub fn draw(&self, c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, th: &crate::ui::Theme, now: u64) {
        c.rrect(x, y, w, h, 6, if th.dark { BLACK } else { WHITE }, 230);
        c.rrect_border(x, y, w, h, 6, th.accent, 256);
        let f = &font::BODY;
        let ty = y + (h - 16) / 2;
        let tx = x + 7;
        match self.cur {
            None => {
                let stem: String = self.buf.chars().take(self.stem_len()).collect();
                let sw = font::text_width(&stem, f);
                c.rect(tx - 1, ty - 2, sw + 2, 20, th.accent, 200);
                let ww = c.text(tx, ty, &stem, f, WHITE, 256);
                let rest: String = self.buf.chars().skip(self.stem_len()).collect();
                c.text(tx + ww + 1, ty, &rest, f, th.text, 256);
            }
            Some(p) => {
                c.text(tx, ty, &self.buf, f, th.text, 256);
                let before: String = self.buf.chars().take(p).collect();
                if (now / 530) % 2 == 0 {
                    c.rect(tx + font::text_width(&before, f) + 1, ty - 1, 2, 18, th.accent, 256);
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Esplora file

pub enum FilesAct {
    None,
    Open(Dir, Entry),
    /// tasto destro: elemento (se c'e') e posizione del mouse
    Context(Option<usize>),
}

pub struct Files {
    pub dir: Dir,
    pub path: Vec<(String, Dir)>,
    entries: Vec<Entry>,
    sel: Option<usize>,
    scroll: usize,
    pub renaming: Option<(String, NameEdit)>,
    loaded: bool,
    pub msg: Option<String>,
}

impl Files {
    pub fn new() -> Files {
        Files { dir: 0, path: Vec::new(), entries: Vec::new(), sel: None, scroll: 0, renaming: None, loaded: false, msg: None }
    }

    pub fn reload(&mut self) {
        self.entries = fat::list(self.dir);
        if self.dir == 0 {
            // la cartella EFI (bootloader), il cestino e System (copie per gli aggiornamenti) restano nascosti
            self.entries.retain(|e| !(e.dir && (e.name.eq_ignore_ascii_case("EFI") || e.name == TRASH || e.name == "System")));
        }
        self.loaded = true;
        if let Some(s) = self.sel {
            if s >= self.entries.len() {
                self.sel = None;
            }
        }
    }

    pub fn open_dir(&mut self, name: String, dir: Dir) {
        if dir == 0 {
            self.path.clear();
        } else {
            self.path.push((name, dir));
        }
        self.dir = dir;
        self.sel = None;
        self.scroll = 0;
        self.renaming = None;
        self.reload();
    }

    /// Apre una cartella della radice (Documenti, Desktop, ...).
    pub fn go_root_dir(&mut self, disk_name: &str, shown: &str) {
        self.path.clear();
        self.dir = 0;
        if let Some(d) = fat::ensure_dir(disk_name) {
            self.open_dir(String::from(shown), d);
        } else {
            self.reload();
        }
    }

    pub fn go_docs(&mut self, dk: &Dk) {
        self.go_root_dir(DOCS, dk.documents);
    }

    fn up(&mut self) {
        self.path.pop();
        self.dir = self.path.last().map(|p| p.1).unwrap_or(0);
        self.sel = None;
        self.scroll = 0;
        self.renaming = None;
        self.reload();
    }

    pub fn create(&mut self, folder: bool, dk: &Dk) {
        if let Some(name) = create_new(self.dir, folder, dk) {
            self.reload();
            self.sel = self.entries.iter().position(|e| e.name == name);
            self.renaming = Some((name.clone(), NameEdit::new(&name)));
        } else {
            self.msg = Some(String::from(dk.save_error));
        }
    }

    pub fn start_rename(&mut self, i: usize) {
        if let Some(e) = self.entries.get(i) {
            self.sel = Some(i);
            self.renaming = Some((e.name.clone(), NameEdit::new(&e.name)));
        }
    }

    pub fn trash_idx(&mut self, i: usize) {
        if let Some(e) = self.entries.get(i).cloned() {
            trash(&e);
            self.reload();
        }
    }

    pub fn open_idx(&mut self, i: usize, dk: &Dk) -> FilesAct {
        let Some(e) = self.entries.get(i).cloned() else { return FilesAct::None };
        if e.dir {
            let name = display_name(&e, dk);
            self.open_dir(name, e.cluster);
            FilesAct::None
        } else {
            FilesAct::Open(self.dir, e)
        }
    }

    fn finish_rename(&mut self, ok: bool) {
        if let Some((orig, ed)) = self.renaming.take() {
            if ok {
                if let Some(e) = self.entries.iter().find(|e| e.name == orig).cloned() {
                    rename(self.dir, &e, &ed.buf);
                }
            }
            self.reload();
            self.sel = self.entries.iter().position(|e| e.name == ed.buf.trim() || e.name == orig);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, ui: &mut Ui, x: i32, y: i32, w: i32, h: i32, focused: bool, dbl: bool, dk: &Dk) -> FilesAct {
        if !self.loaded {
            self.reload();
        }
        let mut act = FilesAct::None;
        if !fat::mounted() {
            ui.sub(x + 20, y + 20, dk.no_fs, &font::BODY);
            return act;
        }
        let tc = ui.th.text;
        let sc = ui.th.sub;
        // ---- barra degli strumenti
        let ty = y + 12;
        let can_up = !self.path.is_empty() || self.dir != 0;
        let hot = !ui.block && ui.hover(x + 16, ty, 36, 34) && can_up;
        ui.tile(x + 16, ty, 36, 34, 10, hot);
        let (cx, cy) = ((x + 34) as f32, (ty + 17) as f32);
        let a = if can_up { 256 } else { 90 };
        ui.c.line(cx + 3.0, cy - 6.0, cx - 3.0, cy, 2.2, tc, a);
        ui.c.line(cx - 3.0, cy, cx + 3.0, cy + 6.0, 2.2, tc, a);
        if can_up && ui.click(x + 16, ty, 36, 34) {
            self.up();
        }
        let bw = [150, 120, 110];
        let labels = [dk.new_folder, dk.new_file, dk.delete];
        let mut bx = x + w - 16;
        let mut clicked = [false; 3];
        for i in (0..3).rev() {
            bx -= bw[i];
            let enabled = i < 2 || self.sel.is_some();
            let lab = ui::ellipsize(labels[i], bw[i] - 16, &font::BODY_B);
            clicked[i] = ui.button(bx, ty, bw[i], 34, &lab, false, enabled);
            bx -= 8;
        }
        let mut crumbs = String::from("MIN-OS");
        for (n, _) in self.path.iter() {
            crumbs.push_str("  >  ");
            crumbs.push_str(n);
        }
        let crumbs = ui::ellipsize(&crumbs, bx - x - 80, &font::BODY_B);
        ui.c.text(x + 66, ty + 9, &crumbs, &font::BODY_B, tc, 256);
        if clicked[0] {
            self.create(true, dk);
        }
        if clicked[1] {
            self.create(false, dk);
        }
        if clicked[2] {
            if let Some(i) = self.sel {
                self.trash_idx(i);
            }
        }

        // ---- intestazione colonne
        let hy = ty + 50;
        let (c_size, c_mod) = (x + w - 330, x + w - 190);
        ui.c.text(x + 56, hy, dk.col_name, &font::BODY, sc, 220);
        ui.c.text(c_size, hy, dk.col_size, &font::BODY, sc, 220);
        ui.c.text(c_mod, hy, dk.col_modified, &font::BODY, sc, 220);
        let sep = if ui.th.dark { WHITE } else { BLACK };
        ui.c.rect(x + 16, hy + 24, w - 32, 1, sep, 26);

        // ---- elenco
        let ly = hy + 30;
        let rh = 36;
        let rows = ((y + h - 40 - ly) / rh).max(1) as usize;
        let list_hover = !ui.block && ui.hover(x + 12, ly, w - 24, rows as i32 * rh);
        if list_hover && ui.ev.wheel != 0 {
            let n = self.entries.len().saturating_sub(rows);
            let d = ui.ev.wheel * 3;
            self.scroll = if d < 0 { self.scroll.saturating_sub((-d) as usize) } else { (self.scroll + d as usize).min(n) };
        } else if let Some(s) = self.sel {
            if ui.ev.any && !ui.ev.moved {
                if s < self.scroll {
                    self.scroll = s;
                } else if s >= self.scroll + rows {
                    self.scroll = s + 1 - rows;
                }
            }
        }
        if self.entries.is_empty() {
            ui.c.text_center(x + w / 2, ly + 60, dk.empty_folder, &font::BODY, sc, 220);
        }
        let mut open: Option<usize> = None;
        let mut row_clicked = false;
        let renaming_name = self.renaming.as_ref().map(|r| r.0.clone());
        for (vi, i) in (self.scroll..self.entries.len()).take(rows).enumerate() {
            let e = self.entries[i].clone();
            let ry = ly + vi as i32 * rh;
            let hot = !ui.block && ui.hover(x + 12, ry, w - 24, rh - 2);
            let sel = self.sel == Some(i);
            if sel {
                ui.c.rrect(x + 12, ry, w - 24, rh - 2, 10, ui.th.accent, if ui.th.dark { 90 } else { 60 });
            } else if hot {
                ui.c.rrect(x + 12, ry, w - 24, rh - 2, 10, ui.th.fill, ui.th.fill_a + 16);
            }
            if e.dir {
                draw_folder(ui.c, x + 24, ry + 7, 22);
            } else {
                draw_doc(ui.c, x + 24, ry + 7, 22);
            }
            if renaming_name.as_deref() == Some(e.name.as_str()) {
                if let Some((_, ed)) = &self.renaming {
                    ed.draw(ui.c, x + 52, ry + 3, c_size - x - 64, rh - 8, ui.th, ui.now);
                }
            } else {
                let name = ui::ellipsize(&display_name(&e, dk), c_size - x - 80, &font::BODY);
                ui.c.text(x + 56, ry + 10, &name, &font::BODY, tc, 256);
            }
            let size = if e.dir { String::from("-") } else { human(e.size) };
            ui.c.text(c_size, ry + 10, &size, &font::BODY, sc, 256);
            ui.c.text(c_mod, ry + 10, &e.modified(), &font::BODY, sc, 256);
            if ui.click(x + 12, ry, w - 24, rh - 2) {
                row_clicked = true;
                if dbl && self.sel == Some(i) && self.renaming.is_none() {
                    open = Some(i);
                }
                self.sel = Some(i);
            }
            if hot && ui.ev.rpressed {
                self.sel = Some(i);
                act = FilesAct::Context(Some(i));
            }
        }
        if list_hover && ui.ev.pressed && !row_clicked {
            self.sel = None;
        }
        if list_hover && ui.ev.rpressed && matches!(act, FilesAct::None) {
            act = FilesAct::Context(None);
        }
        // clic altrove: conferma il nuovo nome
        if self.renaming.is_some() && ui.ev.pressed && !ui.block {
            let still = self.sel.and_then(|s| self.entries.get(s)).map(|e| Some(e.name.clone()) == renaming_name).unwrap_or(false);
            if !still || !row_clicked {
                self.finish_rename(true);
            }
        }
        if focused {
            if let Some((_, ed)) = &mut self.renaming {
                if let Some(ok) = ed.keys(&ui.ev.keys) {
                    self.finish_rename(ok);
                }
            } else {
                let n = self.entries.len();
                for k in ui.ev.keys.iter() {
                    match k {
                        Key::Down if n > 0 => self.sel = Some(self.sel.map(|s| (s + 1).min(n - 1)).unwrap_or(0)),
                        Key::Up if n > 0 => self.sel = Some(self.sel.map(|s| s.saturating_sub(1)).unwrap_or(0)),
                        Key::Enter => open = self.sel,
                        Key::Delete => {
                            if let Some(i) = self.sel {
                                self.trash_idx(i);
                            }
                        }
                        Key::F2 => {
                            if let Some(i) = self.sel {
                                self.start_rename(i);
                            }
                        }
                        Key::F5 => self.reload(),
                        Key::Backspace if can_up => self.up(),
                        _ => {}
                    }
                }
            }
        }
        if let Some(i) = open {
            if let FilesAct::Open(d, e) = self.open_idx(i, dk) {
                act = FilesAct::Open(d, e);
            }
        }

        // ---- barra di stato
        let status = match &self.msg {
            Some(m) => m.clone(),
            None => format!("{} {}   ·   {} {}", self.entries.len(), dk.items, human(fat::free_bytes().min(u32::MAX as u64) as u32), dk.free_word),
        };
        ui.c.text(x + 20, y + h - 28, &status, &font::BODY, sc, 230);
        act
    }
}

// ------------------------------------------------------------------ Cestino

pub fn draw_bin(ui: &mut Ui, x: i32, y: i32, w: i32, h: i32, dk: &Dk) {
    let bin = if fat::mounted() { fat::find(0, TRASH).filter(|e| e.dir).map(|e| e.cluster) } else { None };
    let items = bin.map(fat::list).unwrap_or_default();
    let tc = ui.th.text;
    let sc = ui.th.sub;
    if items.is_empty() {
        let cy = y + h / 2 - 70;
        crate::desktop::icon(ui.c, crate::desktop::IC_BIN, x + w / 2 - 36, cy, 72);
        ui.c.text_center(x + w / 2, cy + 96, dk.bin_empty, &font::LARGE, tc, 256);
        return;
    }
    let bw = 180;
    if ui.button(x + w - bw - 20, y + 14, bw, 36, dk.empty_bin, false, true) {
        for e in items.iter() {
            fat::delete(e);
        }
        return;
    }
    ui.c.text(x + 20, y + 24, &format!("{} {}", items.len(), dk.items), &font::BODY_B, tc, 256);
    for (i, e) in items.iter().enumerate().take(((h - 80) / 40) as usize) {
        let ry = y + 64 + i as i32 * 40;
        if e.dir {
            draw_folder(ui.c, x + 24, ry + 8, 22);
        } else {
            draw_doc(ui.c, x + 24, ry + 8, 22);
        }
        let name = ui::ellipsize(&e.name, w - 300, &font::BODY);
        ui.c.text(x + 56, ry + 11, &name, &font::BODY, tc, 256);
        ui.c.text(x + w - 220, ry + 11, &e.modified(), &font::BODY, sc, 256);
        if ui.button(x + w - 110, ry + 2, 90, 32, dk.restore, false, true) {
            if let Some(docs) = fat::ensure_dir(DOCS) {
                let mut name = e.name.clone();
                let mut n = 2;
                while fat::find(docs, &name).is_some() {
                    name = format!("{} ({})", e.name, n);
                    n += 1;
                }
                fat::move_to(e, docs, &name);
            }
            return;
        }
    }
}

// ------------------------------------------------------------------ Blocco note

pub struct Note {
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
    top: usize,
    left: usize,
    pub dir: Option<Dir>,
    pub name: String,
    modified: bool,
    name_focus: bool,
    status: Option<(String, u64)>,
}

impl Note {
    pub fn new() -> Note {
        Note { lines: alloc::vec![Vec::new()], row: 0, col: 0, top: 0, left: 0, dir: None, name: String::new(), modified: false, name_focus: false, status: None }
    }

    pub fn open(&mut self, dir: Dir, e: &Entry) {
        let data = fat::read(e);
        let text = String::from_utf8_lossy(&data).replace('\r', "").replace('\t', "    ");
        self.lines = text.split('\n').map(|l| l.chars().collect()).collect();
        if self.lines.is_empty() {
            self.lines.push(Vec::new());
        }
        self.row = 0;
        self.col = 0;
        self.top = 0;
        self.left = 0;
        self.dir = Some(dir);
        self.name = e.name.clone();
        self.modified = false;
    }

    fn text(&self) -> String {
        let mut s = String::new();
        for (i, l) in self.lines.iter().enumerate() {
            if i > 0 {
                s.push('\n');
            }
            s.extend(l.iter());
        }
        s
    }

    fn save(&mut self, dk: &Dk, now: u64) {
        if !fat::mounted() {
            self.status = Some((String::from(dk.no_fs), now));
            return;
        }
        if self.name.trim().is_empty() {
            self.name = String::from(dk.untitled);
        }
        let dir = match self.dir {
            Some(d) => d,
            None => fat::ensure_dir(DOCS).unwrap_or(0),
        };
        let ok = fat::write(dir, self.name.trim(), self.text().as_bytes());
        if ok {
            self.dir = Some(dir);
            self.modified = false;
            self.status = Some((String::from(dk.saved), now));
        } else {
            self.status = Some((String::from(dk.save_error), now));
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, ui: &mut Ui, x: i32, y: i32, w: i32, h: i32, focused: bool, dk: &Dk) {
        let now = ui.now;
        let tc = ui.th.text;
        let sc = ui.th.sub;
        // ---- barra degli strumenti
        let ty = y + 12;
        if ui.button(x + 16, ty, 96, 36, dk.new_doc, false, true) {
            *self = Note::new();
        }
        if ui.primary(x + 120, ty, 96, 36, dk.save, false, true) {
            self.save(dk, now);
        }
        let fx = x + 228;
        let fw = (w - 228 - 180).max(120);
        let shown_name = if self.name.is_empty() && !self.name_focus { "" } else { &self.name };
        if ui.field(fx, ty - 3, fw, 42, shown_name, dk.untitled, focused && self.name_focus) {
            self.name_focus = true;
        }
        let status = match &self.status {
            Some((m, t)) if now < t + 2500 => m.clone(),
            _ => String::from(if self.modified { dk.unsaved } else { "" }),
        };
        let st = ui::ellipsize(&status, 160, &font::BODY);
        ui.c.text_right(x + w - 18, ty + 11, &st, &font::BODY, sc, 256);

        // ---- area di testo
        let (ax, ay, aw, ah) = (x + 12, ty + 52, w - 24, y + h - ty - 64);
        ui.c.rrect(ax, ay, aw, ah, 12, if ui.th.dark { BLACK } else { WHITE }, if ui.th.dark { 80 } else { 150 });
        let f = font::BODY;
        let cw = font::mono_width(&f);
        let lh = 21;
        let gutter = 46;
        let cols = ((aw - gutter - 16) / cw).max(8) as usize;
        let rows = ((ah - 16) / lh).max(1) as usize;
        if ui.click(ax, ay, aw, ah) {
            self.name_focus = false;
            let r = ((ui.my - ay - 8) / lh).max(0) as usize + self.top;
            self.row = r.min(self.lines.len() - 1);
            let c = ((ui.mx - ax - gutter + cw / 2) / cw).max(0) as usize + self.left;
            self.col = c.min(self.lines[self.row].len());
        }
        if focused {
            for k in ui.ev.keys.iter() {
                if self.name_focus {
                    match k {
                        Key::Char(c) if self.name.chars().count() < 60 => self.name.push(*c),
                        Key::Backspace => {
                            self.name.pop();
                        }
                        Key::Enter | Key::Tab | Key::Esc => self.name_focus = false,
                        Key::Ctrl('s') => self.save(dk, now),
                        _ => {}
                    }
                    continue;
                }
                let line_len = self.lines[self.row].len();
                match *k {
                    Key::Ctrl('s') => self.save(dk, now),
                    Key::Char(c) => {
                        self.lines[self.row].insert(self.col, c);
                        self.col += 1;
                        self.modified = true;
                    }
                    Key::Tab => {
                        for _ in 0..4 {
                            self.lines[self.row].insert(self.col, ' ');
                        }
                        self.col += 4;
                        self.modified = true;
                    }
                    Key::Enter => {
                        let rest = self.lines[self.row].split_off(self.col);
                        self.lines.insert(self.row + 1, rest);
                        self.row += 1;
                        self.col = 0;
                        self.modified = true;
                    }
                    Key::Backspace => {
                        if self.col > 0 {
                            self.col -= 1;
                            self.lines[self.row].remove(self.col);
                            self.modified = true;
                        } else if self.row > 0 {
                            let cur = self.lines.remove(self.row);
                            self.row -= 1;
                            self.col = self.lines[self.row].len();
                            self.lines[self.row].extend(cur);
                            self.modified = true;
                        }
                    }
                    Key::Delete => {
                        if self.col < line_len {
                            self.lines[self.row].remove(self.col);
                            self.modified = true;
                        } else if self.row + 1 < self.lines.len() {
                            let next = self.lines.remove(self.row + 1);
                            self.lines[self.row].extend(next);
                            self.modified = true;
                        }
                    }
                    Key::Left => {
                        if self.col > 0 {
                            self.col -= 1;
                        } else if self.row > 0 {
                            self.row -= 1;
                            self.col = self.lines[self.row].len();
                        }
                    }
                    Key::Right => {
                        if self.col < line_len {
                            self.col += 1;
                        } else if self.row + 1 < self.lines.len() {
                            self.row += 1;
                            self.col = 0;
                        }
                    }
                    Key::Up if self.row > 0 => {
                        self.row -= 1;
                        self.col = self.col.min(self.lines[self.row].len());
                    }
                    Key::Down if self.row + 1 < self.lines.len() => {
                        self.row += 1;
                        self.col = self.col.min(self.lines[self.row].len());
                    }
                    Key::Home => self.col = 0,
                    Key::End => self.col = line_len,
                    _ => {}
                }
            }
        }
        // scorrimento per tenere il cursore visibile
        if self.row < self.top {
            self.top = self.row;
        } else if self.row >= self.top + rows {
            self.top = self.row + 1 - rows;
        }
        if self.col < self.left {
            self.left = self.col;
        } else if self.col >= self.left + cols {
            self.left = self.col + 1 - cols;
        }
        for (vi, r) in (self.top..self.lines.len()).take(rows).enumerate() {
            let ly = ay + 8 + vi as i32 * lh;
            let num = format!("{}", r + 1);
            ui.c.text_right(ax + gutter - 12, ly, &num, &f, sc, 140);
            let visible: String = self.lines[r].iter().skip(self.left).take(cols).collect();
            ui.c.text_mono(ax + gutter, ly, &visible, &f, tc, 256);
        }
        if focused && !self.name_focus && (now / 530) % 2 == 0 {
            let cx = ax + gutter + (self.col - self.left) as i32 * cw;
            let cy = ay + 8 + (self.row - self.top) as i32 * lh;
            ui.c.rect(cx, cy - 1, 2, 19, ui.th.accent, 256);
        }
    }
}

// ------------------------------------------------------------------ Calcolatrice

pub struct Calc {
    disp: String,
    acc: Option<f64>,
    op: Option<char>,
    fresh: bool,
}

const KEYS: [[&str; 4]; 5] = [["AC", "±", "%", "÷"], ["7", "8", "9", "×"], ["4", "5", "6", "-"], ["1", "2", "3", "+"], ["0", "0", ",", "="]];

impl Calc {
    pub fn new() -> Calc {
        Calc { disp: String::from("0"), acc: None, op: None, fresh: true }
    }

    fn value(&self) -> f64 {
        self.disp.replace(',', ".").parse::<f64>().unwrap_or(0.0)
    }

    fn show(&mut self, v: f64) {
        if !v.is_finite() {
            self.disp = String::from("Errore");
            return;
        }
        let r = (v * 1e9) as i128;
        let s = if (v as i128) as f64 == v && v.abs() < 1e15 {
            format!("{}", v as i128)
        } else {
            let mut t = format!("{:.9}", r as f64 / 1e9);
            while t.ends_with('0') {
                t.pop();
            }
            if t.ends_with('.') {
                t.pop();
            }
            t
        };
        self.disp = s.replace('.', ",");
    }

    fn apply(&mut self) {
        let b = self.value();
        if let (Some(a), Some(op)) = (self.acc, self.op) {
            let r = match op {
                '+' => a + b,
                '-' => a - b,
                '×' => a * b,
                _ => {
                    if b == 0.0 {
                        f64::NAN
                    } else {
                        a / b
                    }
                }
            };
            self.show(r);
        }
    }

    fn press(&mut self, k: &str) {
        match k {
            "AC" => *self = Calc::new(),
            "±" => {
                let v = -self.value();
                self.show(v);
            }
            "%" => {
                let v = self.value() / 100.0;
                self.show(v);
            }
            "+" | "-" | "×" | "÷" => {
                if !self.fresh {
                    self.apply();
                }
                self.acc = Some(self.value());
                self.op = k.chars().next();
                self.fresh = true;
            }
            "=" => {
                self.apply();
                self.acc = None;
                self.op = None;
                self.fresh = true;
            }
            "," => {
                if self.fresh {
                    self.disp = String::from("0,");
                    self.fresh = false;
                } else if !self.disp.contains(',') {
                    self.disp.push(',');
                }
            }
            "⌫" => {
                if !self.fresh {
                    self.disp.pop();
                    if self.disp.is_empty() || self.disp == "-" {
                        self.disp = String::from("0");
                        self.fresh = true;
                    }
                }
            }
            d => {
                if self.fresh || self.disp == "0" {
                    self.disp = String::from(d);
                    self.fresh = false;
                } else if self.disp.len() < 14 {
                    self.disp.push_str(d);
                }
            }
        }
    }

    pub fn draw(&mut self, ui: &mut Ui, x: i32, y: i32, w: i32, h: i32, focused: bool) {
        if focused {
            for k in ui.ev.keys.iter() {
                match k {
                    Key::Char(c @ '0'..='9') => self.press(&String::from(*c)),
                    Key::Char('+') => self.press("+"),
                    Key::Char('-') => self.press("-"),
                    Key::Char('*') | Key::Char('x') => self.press("×"),
                    Key::Char('/') => self.press("÷"),
                    Key::Char(',') | Key::Char('.') => self.press(","),
                    Key::Char('%') => self.press("%"),
                    Key::Char('=') | Key::Enter => self.press("="),
                    Key::Backspace => self.press("⌫"),
                    Key::Esc | Key::Delete => self.press("AC"),
                    _ => {}
                }
            }
        }
        let tc = ui.th.text;
        // display
        let pad = 18;
        let gap = 12;
        let bs = ((w - pad * 2 - gap * 3) / 4).min((h - 150 - gap * 4) / 5).max(30);
        let gw = bs * 4 + gap * 3;
        let gx = x + (w - gw) / 2;
        let gy = y + h - pad - bs * 5 - gap * 4;
        if let (Some(a), Some(op)) = (self.acc, self.op) {
            let mut c = Calc::new();
            c.show(a);
            let s = format!("{} {}", c.disp, op);
            let sc = ui.th.sub;
            ui.c.text_right(gx + gw - 6, gy - 120, &s, &font::MED, sc, 256);
        }
        let big = if font::text_width(&self.disp, &font::HERO) <= gw { font::HERO } else { font::TITLE };
        ui.c.text_right(gx + gw - 4, gy - 16 - big.height(), &self.disp, &big, tc, 256);
        // tasti
        for (r, row) in KEYS.iter().enumerate() {
            let mut cidx = 0;
            while cidx < 4 {
                let label = row[cidx];
                let wide = label == "0" && cidx == 0;
                let bx = gx + cidx as i32 * (bs + gap);
                let by = gy + r as i32 * (bs + gap);
                let bw = if wide { bs * 2 + gap } else { bs };
                let is_op = cidx == 3;
                let is_fn = r == 0 && cidx < 3;
                let active_op = is_op && self.op == label.chars().next() && self.fresh;
                let hot = !ui.block && ui.hover(bx, by, bw, bs);
                let (fill, txt) = if is_op {
                    if active_op {
                        (WHITE, rgb(255, 159, 10))
                    } else {
                        (rgb(255, 159, 10), WHITE)
                    }
                } else if is_fn {
                    (if ui.th.dark { rgb(165, 165, 170) } else { rgb(210, 210, 216) }, if ui.th.dark { BLACK } else { tc })
                } else {
                    (if ui.th.dark { rgb(52, 52, 58) } else { WHITE }, tc)
                };
                let fill = if hot { mix(fill, WHITE, 40) } else { fill };
                ui.c.shadow(bx, by, bw, bs, bs / 2, 8, 50);
                ui.c.rrect(bx, by, bw, bs, bs / 2, fill, if is_op || is_fn { 256 } else { 230 });
                ui.c.shape(bx, by, bw, bs / 2, bs / 2, |_, _, p, cov| *p = crate::gfx::blend(*p, WHITE, cov * 22 >> 8));
                ui.c.rrect_border(bx, by, bw, bs, bs / 2, WHITE, 70);
                let f = if bs >= 56 { font::TITLE } else { font::LARGE };
                let lw = font::text_width(label, &f);
                let lx = if wide { bx + bs / 2 - lw / 2 } else { bx + bw / 2 - lw / 2 };
                ui.c.text(lx, by + (bs - f.height()) / 2, label, &f, txt, 256);
                if ui.click(bx, by, bw, bs) {
                    self.press(label);
                }
                cidx += if wide { 2 } else { 1 };
            }
        }
    }
}
