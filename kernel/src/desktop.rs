//! Desktop di MIN-OS: organizzato come Windows 11, disegnato in Liquid Glass.
//!
//! Icone sul desktop, barra delle applicazioni, menu Start con ricerca, menu contestuale
//! (tasto destro) con sottomenu, finestre con riduci / ingrandisci / chiudi e app Impostazioni.

use crate::arch::{self, DateTime};
use crate::font;
use crate::gfx::{blend, mix, rgb, Canvas, BLACK, WHITE};
use crate::i18n;
use crate::i18n_desk::Dk;
use crate::input::{Events, Key};
use crate::apps::{self, Calc, Files, FilesAct, Note};
use crate::fat::{self, Dir};
use crate::keymap;
use crate::net;
use crate::storage;
use crate::ui::{self, check_mark, draw_logo, ease, Gui, Settings, Ui, ACCENTS};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// finestre
const TERM: usize = 0;
const INFO: usize = 1;
const SETT: usize = 2;
const BIN: usize = 3;
const FILES: usize = 4;
const NOTE: usize = 5;
const CALC: usize = 6;
const WEB: usize = 7;
const NWIN: usize = 8;

// icone disegnate
const IC_TERM: usize = 0;
const IC_INFO: usize = 1;
const IC_SETT: usize = 2;
const IC_POWER: usize = 3;
const IC_PC: usize = 4;
pub const IC_BIN: usize = 5;
const IC_PAINT: usize = 6;
const IC_GLOBE: usize = 7;
const IC_USER: usize = 8;
const IC_NET: usize = 9;
const IC_FOLDER: usize = 10;
const IC_NOTE: usize = 11;
const IC_CALC: usize = 12;
const IC_WEB: usize = 13;
const IC_UPD: usize = 14;

const TB_H: i32 = 52;
const TITLE_H: i32 = 42;
const MW: i32 = 340;
const IH: i32 = 34;
const SEP_H: i32 = 9;
const MPAD: i32 = 6;

/// Icone presenti sul desktop (in ordine "per tipo").
const DESK: [usize; 6] = [IC_PC, IC_BIN, IC_FOLDER, IC_WEB, IC_TERM, IC_SETT];

#[derive(Clone, Copy)]
struct R {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}
impl R {
    fn has(&self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }
}

#[derive(Clone, Copy)]
struct Win {
    open: bool,
    min: bool,
    max: bool,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    open_t: u64,
}

#[derive(Clone, Copy, PartialEq)]
enum MenuKind {
    Desktop,
    Taskbar,
    Icon(usize),
    DItem(usize),
    FilesBg,
    FilesItem(usize),
}

/// Icona del desktop: di sistema (Questo PC, Cestino, ...) o elemento della cartella Desktop.
#[derive(Clone, Copy, PartialEq)]
enum DIcon {
    Sys(usize),
    Item(usize),
}

#[derive(Clone, Copy, PartialEq)]
enum Tgt {
    Desk,
    Files,
}

#[derive(Clone, Copy, PartialEq)]
enum Act {
    None,
    IconSize(usize),
    ToggleIcons,
    SortName(bool),
    Refresh,
    Terminal,
    Page(usize),
    ShowDesktop,
    OpenIcon(usize),
    Props,
    NewFolder(Tgt),
    NewText(Tgt),
    OpenD(usize),
    RenameD(usize),
    DeleteSel,
    OpenF(usize),
    RenameF(usize),
    DeleteF(usize),
    FilesRefresh,
}

struct Item {
    label: String,
    act: Act,
    sub: Vec<Item>,
    check: bool,
    sep: bool,
}

fn item(label: &str, act: Act) -> Item {
    Item { label: String::from(label), act, sub: Vec::new(), check: false, sep: false }
}

struct Menu {
    x: i32,
    y: i32,
    kind: MenuKind,
    sub: Option<usize>,
    t: u64,
}

#[derive(Clone, Copy, PartialEq)]
enum Launch {
    App(usize),
    Page(usize),
}

#[derive(Clone, Copy, PartialEq)]
enum Layer {
    Menu,
    Notify,
    Start,
    Taskbar,
    Win(usize),
    Desk,
}

#[derive(Clone, Copy, PartialEq)]
enum Power {
    Reboot,
    Off,
}

struct Desktop {
    s: Settings,
    wins: [Win; NWIN],
    order: Vec<usize>,
    drag: Option<(usize, i32, i32)>,
    lines: Vec<String>,
    input: String,
    clock: DateTime,
    clock_t: u64,
    boot_t: u64,
    snap: Vec<u32>,
    action: Option<Power>,
    dsel: Vec<DIcon>,
    desk_dir: Option<Dir>,
    desk: Vec<fat::Entry>,
    desk_t: u64,
    band: Option<(i32, i32)>,
    drename: Option<(usize, apps::NameEdit)>,
    desk_focus: bool,
    menu: Option<Menu>,
    start: bool,
    start_t: u64,
    search: String,
    power: bool,
    page: usize,
    acc_focus: usize,
    last_click: (u64, i32, i32),
    flash_t: u64,
    frame_cost: u64,
    files: Files,
    note: Note,
    calc: Calc,
    web: crate::web::Web,
    upd: crate::update::Updater,
    cwd: Dir,
    cwd_path: Vec<String>,
    dbl: bool,
    hist: Vec<String>,
    hist_pos: Option<usize>,
    cur: usize,
    tscroll: usize,
    tsel: Option<((usize, usize), (usize, usize))>,
    tselecting: bool,
    toast: Option<(String, u64)>,
    /// serve un altro frame subito (es. un sottomenu appena aperto)
    dirty: bool,
}

pub fn run(gui: &mut Gui, s: Settings) -> ! {
    let (w, h) = (gui.w(), gui.h());
    let sw = (w - 60).min(900);
    let sh = (h - TB_H - 40).min(620);
    let now = arch::now_ms();
    let win = |x: i32, y: i32, ww: i32, hh: i32| Win { open: false, min: false, max: false, x, y, w: ww, h: hh, open_t: 0 };
    let mut d = Desktop {
        wins: [
            win((w - 680) / 2 + 40, 60, 680.min(w - 40), 430.min(h - TB_H - 80)),
            win(w - 540, 50, 500, 440.min(h - TB_H - 70)),
            win((w - sw) / 2, (h - TB_H - sh) / 2, sw, sh),
            win((w - 520) / 2 - 60, 120, 520, 400),
            win((w - 800) / 2 - 20, 50, 800.min(w - 40), 540.min(h - TB_H - 70)),
            win((w - 720) / 2 + 30, 70, 720.min(w - 40), 500.min(h - TB_H - 80)),
            win(w - 380, 60, 340, 520.min(h - TB_H - 70)),
            win((w - 960) / 2, 30, 960.min(w - 40), 640.min(h - TB_H - 50)),
        ],
        order: Vec::new(),
        drag: None,
        lines: Vec::new(),
        input: String::new(),
        clock: arch::rtc_now(),
        clock_t: now,
        boot_t: now,
        snap: Vec::new(),
        action: None,
        dsel: Vec::new(),
        desk_dir: None,
        desk: Vec::new(),
        desk_t: 0,
        band: None,
        drename: None,
        desk_focus: false,
        menu: None,
        start: false,
        start_t: 0,
        search: String::new(),
        power: false,
        page: 0,
        acc_focus: 0,
        last_click: (0, 0, 0),
        flash_t: 0,
        frame_cost: 0,
        files: Files::new(),
        note: Note::new(),
        calc: Calc::new(),
        web: crate::web::Web::new(),
        upd: crate::update::Updater::new(),
        cwd: 0,
        cwd_path: Vec::new(),
        dbl: false,
        hist: Vec::new(),
        hist_pos: None,
        cur: 0,
        tscroll: 0,
        tsel: None,
        tselecting: false,
        toast: None,
        dirty: false,
        s,
    };
    d.lines.push(String::from(d.s.tr().term_hello));
    if fat::mounted() {
        let first = fat::find(0, apps::DOCS).is_none();
        if let Some(docs) = fat::ensure_dir(apps::DOCS) {
            if first {
                let dk = d.dk();
                fat::write(docs, dk.welcome_name, dk.welcome_text.as_bytes());
            }
            d.cwd = docs;
            d.cwd_path = alloc::vec![String::from(apps::DOCS)];
        }
        d.desk_dir = fat::ensure_dir(apps::DESKTOP);
        d.reload_desk();
    }
    gui.input.layout = d.s.layout;
    crate::log!("desktop avviato per {}", d.s.user);

    let mut saved = storage::encode(&d.s);
    let mut last_draw = 0u64;
    let mut net_next = 0u64;
    loop {
        let ev = gui.next_events(&d.s);
        let now = arch::now_ms();
        // rete: avanza ogni pochi millisecondi, anche quando lo schermo e' fermo
        if now >= net_next {
            net_next = now + if d.web.busy() { 1 } else { 8 };
            if net::poll() {
                last_draw = 0;
            }
            let mut out = net::take_output();
            out.extend(net::take_pending());
            if !out.is_empty() {
                d.lines.extend(out);
                last_draw = 0;
            }
        }
        if now > d.clock_t + 1000 {
            d.clock_t = now;
            let c = arch::rtc_now();
            let changed = c.min != d.clock.min || (d.wins[INFO].open && !d.wins[INFO].min);
            d.clock = c;
            if changed {
                last_draw = 0;
            }
        }
        let dk_now = d.dk();
        if d.upd.tick(dk_now, now, d.boot_t) {
            last_draw = 0;
        }
        if d.drename.is_none() && d.band.is_none() && now > d.desk_t + 3000 && d.reload_desk() {
            last_draw = 0;
        }
        let animating = d.s.anim
            && (now < d.boot_t + 700
                || d.wins.iter().any(|w| w.open && now < w.open_t + 260)
                || (d.start && now < d.start_t + 240)
                || d.menu.as_ref().map(|m| now < m.t + 160).unwrap_or(false));
        let typing = d.start
            || match d.order.last() {
                Some(&TERM) => !d.wins[TERM].min,
                Some(&SETT) => d.page == 3 && !d.wins[SETT].min,
                Some(&NOTE) => !d.wins[NOTE].min,
                Some(&FILES) => !d.wins[FILES].min,
                Some(&WEB) => !d.wins[WEB].min,
                _ => false,
            };
        let typing = typing || d.drename.is_some();
        let tick = if d.web.busy() || d.upd.busy() { 200 } else if typing { 265 } else if now < d.flash_t + 200 { 50 } else { 30_000 };
        if last_draw != 0 && !ev.any && !animating && !d.dirty && now < last_draw + tick {
            arch::pause();
            continue;
        }
        last_draw = now;

        let before = (d.s.dark, d.s.accent, d.s.glass, d.s.layout, d.s.lang);
        d.dirty = false;
        d.frame(gui, &ev, now);
        gui.present();
        // salva su disco ogni modifica alle impostazioni
        let rec = storage::encode(&d.s);
        if rec != saved {
            storage::save_raw(&rec);
            saved = rec;
        }
        d.frame_cost = arch::now_ms() - now;
        if before != (d.s.dark, d.s.accent, d.s.glass, d.s.layout, d.s.lang) {
            gui.input.layout = d.s.layout;
            gui.apply(&d.s);
        }
        match d.action {
            Some(Power::Reboot) => arch::reboot(),
            Some(Power::Off) => power_off(gui, &d.s),
            None => {}
        }
    }
}

fn power_off(gui: &mut Gui, s: &Settings) -> ! {
    let (w, h) = (gui.w(), gui.h());
    gui.canvas.fill(BLACK);
    gui.screen.present(&gui.canvas);
    arch::shutdown();
    gui.canvas.fill(BLACK);
    draw_logo(&mut gui.canvas, w / 2 - 32, h / 2 - 80, 64, s.accent());
    gui.canvas.text_center(w / 2, h / 2 + 10, s.tr().off_msg, &font::MED, WHITE, 230);
    gui.screen.present(&gui.canvas);
    loop {
        arch::halt();
    }
}

fn app_icon(a: usize) -> usize {
    [IC_TERM, IC_INFO, IC_SETT, IC_BIN, IC_FOLDER, IC_NOTE, IC_CALC, IC_WEB][a]
}

fn icon_app(ic: usize) -> usize {
    match ic {
        IC_PC | IC_FOLDER => FILES,
        IC_BIN => BIN,
        IC_TERM => TERM,
        IC_WEB => WEB,
        _ => SETT,
    }
}

fn lower(s: &str) -> String {
    s.chars().flat_map(|c| c.to_lowercase()).collect()
}

impl Desktop {
    fn t(&self) -> &'static i18n::Tr {
        self.s.tr()
    }
    fn dk(&self) -> &'static Dk {
        self.s.lang.dk()
    }

    fn app_name(&self, a: usize) -> &'static str {
        let (t, dk) = (self.t(), self.dk());
        [t.terminal, t.sysinfo, t.settings, dk.bin, dk.files, dk.notes, dk.calc, dk.internet][a]
    }

    fn icon_label(&self, ic: usize) -> &'static str {
        let (t, dk) = (self.t(), self.dk());
        match ic {
            IC_PC => dk.this_pc,
            IC_BIN => dk.bin,
            IC_FOLDER => dk.documents,
            IC_TERM => t.terminal,
            IC_WEB => dk.internet,
            _ => t.settings,
        }
    }

    // ------------------------------------------------------------ finestre

    fn open(&mut self, a: usize, now: u64) {
        let w = &mut self.wins[a];
        if !w.open {
            w.open = true;
            w.open_t = now;
        }
        if w.min {
            w.min = false;
            w.open_t = now;
        }
        self.order.retain(|&x| x != a);
        self.order.push(a);
    }

    fn open_icon(&mut self, ic: usize, now: u64) {
        let dk = self.dk();
        match ic {
            IC_PC => self.files.open_dir(String::new(), 0),
            IC_FOLDER => self.files.go_docs(dk),
            _ => {}
        }
        self.open(icon_app(ic), now);
    }

    fn close(&mut self, a: usize) {
        self.wins[a].open = false;
        self.wins[a].max = false;
        self.order.retain(|&x| x != a);
    }

    fn focused(&self) -> Option<usize> {
        self.order.iter().rev().copied().find(|&a| self.wins[a].open && !self.wins[a].min)
    }

    fn rect(&self, a: usize, sw: i32, sh: i32) -> (i32, i32, i32, i32, i32) {
        let w = &self.wins[a];
        if w.max {
            (0, 0, sw, sh - TB_H, 0)
        } else {
            (w.x, w.y, w.w, w.h, 18)
        }
    }

    fn win_at(&self, x: i32, y: i32, sw: i32, sh: i32) -> Option<usize> {
        self.order.iter().rev().copied().find(|&a| {
            let w = &self.wins[a];
            let (rx, ry, rw, rh, _) = self.rect(a, sw, sh);
            w.open && !w.min && x >= rx && y >= ry && x < rx + rw && y < ry + rh
        })
    }

    fn taskbar_click(&mut self, a: usize, now: u64) {
        let w = self.wins[a];
        if w.open && !w.min && self.focused() == Some(a) {
            self.wins[a].min = true;
        } else {
            self.open(a, now);
        }
    }

    fn launch(&mut self, l: Launch, now: u64) {
        self.start = false;
        self.power = false;
        self.search.clear();
        match l {
            Launch::App(a) => self.open(a, now),
            Launch::Page(p) => {
                self.page = p;
                self.open(SETT, now);
            }
        }
    }

    fn exec(&mut self, act: Act, now: u64) {
        match act {
            Act::None => {}
            Act::IconSize(n) => self.s.icon_size = n,
            Act::ToggleIcons => self.s.show_icons = !self.s.show_icons,
            Act::SortName(b) => self.s.sort_name = b,
            Act::Refresh => self.flash_t = now,
            Act::Terminal => self.open(TERM, now),
            Act::Page(p) => self.launch(Launch::Page(p), now),
            Act::ShowDesktop => {
                for w in self.wins.iter_mut() {
                    if w.open {
                        w.min = true;
                    }
                }
            }
            Act::OpenIcon(ic) => self.open_icon(ic, now),
            Act::Props => self.open(INFO, now),
            Act::NewFolder(t) | Act::NewText(t) => {
                let folder = matches!(act, Act::NewFolder(_));
                let dk = self.dk();
                match t {
                    Tgt::Files => self.files.create(folder, dk),
                    Tgt::Desk => {
                        if let Some(dd) = self.desk_dir {
                            if let Some(name) = apps::create_new(dd, folder, dk) {
                                self.reload_desk();
                                if let Some(i) = self.desk.iter().position(|e| e.name == name) {
                                    self.dsel = alloc::vec![DIcon::Item(i)];
                                    self.drename = Some((i, apps::NameEdit::new(&name)));
                                    self.desk_focus = true;
                                }
                            }
                        }
                    }
                }
            }
            Act::OpenD(i) => self.open_dicon(DIcon::Item(i), now),
            Act::RenameD(i) => {
                if let Some(e) = self.desk.get(i) {
                    self.drename = Some((i, apps::NameEdit::new(&e.name)));
                    self.desk_focus = true;
                }
            }
            Act::DeleteSel => self.delete_selected(),
            Act::OpenF(i) => {
                let dk = self.dk();
                if let FilesAct::Open(d, e) = self.files.open_idx(i, dk) {
                    self.open_file(d, e, now);
                }
            }
            Act::RenameF(i) => self.files.start_rename(i),
            Act::DeleteF(i) => self.files.trash_idx(i),
            Act::FilesRefresh => self.files.reload(),
        }
        self.menu = None;
    }

    // ------------------------------------------------------------ icone del desktop

    /// Rilegge la cartella Desktop. Ritorna true se e' cambiata.
    fn reload_desk(&mut self) -> bool {
        self.desk_t = arch::now_ms();
        let Some(dd) = self.desk_dir else { return false };
        let new = fat::list(dd);
        let changed = new.len() != self.desk.len() || new.iter().zip(self.desk.iter()).any(|(a, b)| a.name != b.name || a.size != b.size);
        if changed {
            self.desk = new;
            let n = self.desk.len();
            self.dsel.retain(|d| match d {
                DIcon::Item(i) => *i < n,
                _ => true,
            });
        }
        changed
    }

    fn icon_order(&self) -> Vec<DIcon> {
        let mut sys: Vec<usize> = DESK.to_vec();
        if self.s.sort_name {
            sys.sort_by(|a, b| lower(self.icon_label(*a)).cmp(&lower(self.icon_label(*b))));
        }
        let mut v: Vec<DIcon> = sys.into_iter().map(DIcon::Sys).collect();
        let mut items: Vec<usize> = (0..self.desk.len()).collect();
        if self.s.sort_name {
            items.sort_by(|a, b| lower(&self.desk[*a].name).cmp(&lower(&self.desk[*b].name)));
        }
        v.extend(items.into_iter().map(DIcon::Item));
        v
    }

    fn dicon_label(&self, d: DIcon) -> String {
        match d {
            DIcon::Sys(ic) => String::from(self.icon_label(ic)),
            DIcon::Item(i) => self.desk.get(i).map(|e| e.name.clone()).unwrap_or_default(),
        }
    }

    fn icon_cell(&self) -> (i32, i32, i32) {
        let sz = [64, 48, 36][self.s.icon_size];
        (sz, sz + 46, sz + 48)
    }

    fn icon_rects(&self, sh: i32) -> Vec<(DIcon, R)> {
        let (_, cw, ch) = self.icon_cell();
        let rows = ((sh - TB_H - 16) / ch).max(1);
        self.icon_order()
            .into_iter()
            .enumerate()
            .map(|(i, d)| {
                let (col, row) = (i as i32 / rows, i as i32 % rows);
                (d, R { x: 10 + col * (cw + 4), y: 10 + row * ch, w: cw, h: ch - 6 })
            })
            .collect()
    }

    fn icon_at(&self, x: i32, y: i32, sh: i32) -> Option<DIcon> {
        if !self.s.show_icons {
            return None;
        }
        self.icon_rects(sh).into_iter().find(|(_, r)| r.has(x, y)).map(|(d, _)| d)
    }

    fn open_dicon(&mut self, d: DIcon, now: u64) {
        match d {
            DIcon::Sys(ic) => self.open_icon(ic, now),
            DIcon::Item(i) => {
                let (Some(e), Some(dd)) = (self.desk.get(i).cloned(), self.desk_dir) else { return };
                if e.dir {
                    let dk = self.dk();
                    self.files.path = alloc::vec![(String::from(dk.desktop_name), dd)];
                    self.files.dir = dd;
                    self.files.open_dir(e.name.clone(), e.cluster);
                    self.open(FILES, now);
                } else {
                    self.open_file(dd, e, now);
                }
            }
        }
    }

    /// Apre un file con l'app giusta: le pagine .html nell'app Internet, il resto nel Blocco note.
    fn open_file(&mut self, dir: Dir, e: fat::Entry, now: u64) {
        let low = lower(&e.name);
        if low.ends_with(".html") || low.ends_with(".htm") {
            self.web.open_local(&e.name, fat::read(&e));
            self.open(WEB, now);
        } else {
            self.note.open(dir, &e);
            self.open(NOTE, now);
        }
    }

    fn delete_selected(&mut self) {
        let items: Vec<fat::Entry> = self
            .dsel
            .iter()
            .filter_map(|d| match d {
                DIcon::Item(i) => self.desk.get(*i).cloned(),
                _ => None,
            })
            .collect();
        for e in items.iter() {
            apps::trash(e);
        }
        self.dsel.clear();
        self.reload_desk();
    }

    fn commit_drename(&mut self, ok: bool) {
        if let (Some((i, ed)), Some(dd)) = (self.drename.take(), self.desk_dir) {
            if ok {
                if let Some(e) = self.desk.get(i).cloned() {
                    apps::rename(dd, &e, &ed.buf);
                }
            }
            self.reload_desk();
            if let Some(j) = self.desk.iter().position(|e| e.name == ed.buf.trim()) {
                self.dsel = alloc::vec![DIcon::Item(j)];
            }
        }
    }

    // ------------------------------------------------------------ menu contestuale

    fn menu_items(&self, kind: MenuKind) -> Vec<Item> {
        let dk = self.dk();
        match kind {
            MenuKind::Desktop => {
                let mut view = item(dk.view, Act::None);
                for (i, l) in dk.icons_lms.iter().enumerate() {
                    let mut it = item(l, Act::IconSize(i));
                    it.check = self.s.icon_size == i;
                    view.sub.push(it);
                }
                let mut show = item(dk.show_icons, Act::ToggleIcons);
                show.check = self.s.show_icons;
                show.sep = true;
                view.sub.push(show);
                let mut sort = item(dk.sort, Act::None);
                let mut a = item(dk.by_name, Act::SortName(true));
                a.check = self.s.sort_name;
                let mut b = item(dk.by_type, Act::SortName(false));
                b.check = !self.s.sort_name;
                sort.sub.push(a);
                sort.sub.push(b);
                let mut new = item(dk.new_menu, Act::None);
                new.sep = true;
                new.sub.push(item(dk.folder_word, Act::NewFolder(Tgt::Desk)));
                new.sub.push(item(dk.text_doc, Act::NewText(Tgt::Desk)));
                let mut term = item(dk.open_term, Act::Terminal);
                term.sep = true;
                let mut disp = item(dk.display, Act::Page(0));
                disp.sep = true;
                alloc::vec![view, sort, item(dk.refresh, Act::Refresh), new, term, disp, item(dk.personalize, Act::Page(1))]
            }
            MenuKind::Taskbar => {
                let mut sd = item(dk.show_desktop, Act::ShowDesktop);
                sd.sep = true;
                alloc::vec![item(dk.taskbar_set, Act::Page(1)), sd]
            }
            MenuKind::Icon(ic) => {
                let mut p = item(dk.properties, Act::Props);
                p.sep = true;
                alloc::vec![item(dk.open, Act::OpenIcon(ic)), p]
            }
            MenuKind::DItem(i) => {
                let mut r = item(dk.rename, Act::RenameD(i));
                r.sep = true;
                alloc::vec![item(dk.open, Act::OpenD(i)), r, item(dk.delete, Act::DeleteSel)]
            }
            MenuKind::FilesBg => {
                let mut new = item(dk.new_menu, Act::None);
                new.sub.push(item(dk.folder_word, Act::NewFolder(Tgt::Files)));
                new.sub.push(item(dk.text_doc, Act::NewText(Tgt::Files)));
                alloc::vec![new, item(dk.refresh, Act::FilesRefresh)]
            }
            MenuKind::FilesItem(i) => {
                let mut r = item(dk.rename, Act::RenameF(i));
                r.sep = true;
                alloc::vec![item(dk.open, Act::OpenF(i)), r, item(dk.delete, Act::DeleteF(i))]
            }
        }
    }

    fn items_h(items: &[Item]) -> i32 {
        MPAD * 2 + items.iter().map(|i| IH + if i.sep { SEP_H } else { 0 }).sum::<i32>()
    }

    fn item_y(items: &[Item], idx: usize) -> i32 {
        MPAD + items[..idx].iter().map(|i| IH + if i.sep { SEP_H } else { 0 }).sum::<i32>() + if items[idx].sep { SEP_H } else { 0 }
    }

    /// Rettangoli del menu principale e dell'eventuale sottomenu.
    fn menu_rects(&self, sw: i32, sh: i32) -> Option<(R, Option<R>, Vec<Item>)> {
        let m = self.menu.as_ref()?;
        let items = self.menu_items(m.kind);
        let mh = Self::items_h(&items);
        let mut x = m.x;
        let mut y = if m.kind == MenuKind::Taskbar { m.y - mh - 4 } else { m.y };
        if x + MW > sw - 4 {
            x = sw - MW - 4;
        }
        if y + mh > sh - TB_H - 4 {
            y = sh - TB_H - 4 - mh;
        }
        let main = R { x, y: y.max(4), w: MW, h: mh };
        let sub = m.sub.filter(|&i| i < items.len() && !items[i].sub.is_empty()).map(|i| {
            let sub_h = Self::items_h(&items[i].sub);
            let mut sx = main.x + MW - 8;
            if sx + MW > sw - 4 {
                sx = main.x - MW + 8;
            }
            let sy = (main.y + Self::item_y(&items, i) - MPAD).min(sh - TB_H - 4 - sub_h);
            R { x: sx, y: sy, w: MW, h: sub_h }
        });
        Some((main, sub, items))
    }

    // ------------------------------------------------------------ menu Start

    fn start_rect(sw: i32, sh: i32) -> R {
        let w = (sw - 40).min(640);
        let h = (sh - TB_H - 24).min(470);
        R { x: (sw - w) / 2, y: sh - TB_H - 12 - h, w, h }
    }

    fn start_entries(&self) -> Vec<(usize, &'static str, Launch)> {
        let (t, dk) = (self.t(), self.dk());
        alloc::vec![
            (IC_FOLDER, dk.files, Launch::App(FILES)),
            (IC_WEB, dk.internet, Launch::App(WEB)),
            (IC_NOTE, dk.notes, Launch::App(NOTE)),
            (IC_CALC, dk.calc, Launch::App(CALC)),
            (IC_TERM, t.terminal, Launch::App(TERM)),
            (IC_SETT, t.settings, Launch::App(SETT)),
            (IC_INFO, t.sysinfo, Launch::App(INFO)),
        ]
    }

    fn start_results(&self) -> Vec<(usize, &'static str, Launch)> {
        let q = lower(self.search.trim());
        let dk = self.dk();
        let mut all = self.start_entries();
        all.push((IC_BIN, dk.bin, Launch::App(BIN)));
        all.push((IC_PAINT, dk.pages[1], Launch::Page(1)));
        all.push((IC_GLOBE, dk.pages[2], Launch::Page(2)));
        all.push((IC_USER, dk.pages[3], Launch::Page(3)));
        all.push((IC_PC, dk.pages[4], Launch::Page(4)));
        all.push((IC_PC, dk.display, Launch::Page(0)));
        all.push((IC_NET, dk.pages[5], Launch::Page(5)));
        all.push((IC_UPD, dk.pages[6], Launch::Page(6)));
        all.into_iter().filter(|(_, l, _)| lower(l).contains(q.as_str())).collect()
    }

    // ------------------------------------------------------------ frame

    fn frame(&mut self, gui: &mut Gui, ev: &Events, now: u64) {
        let (sw, sh) = (gui.w(), gui.h());
        let (mx, my) = (gui.input.mx, gui.input.my);

        // ---- livello sotto il mouse (dall'alto verso il basso)
        let mrects = self.menu_rects(sw, sh);
        let in_menu = mrects.as_ref().map(|(a, b, _)| a.has(mx, my) || b.map(|r| r.has(mx, my)).unwrap_or(false)).unwrap_or(false);
        let srect = Self::start_rect(sw, sh);
        let layer = if in_menu {
            Layer::Menu
        } else if self.start && srect.has(mx, my) {
            Layer::Start
        } else if self.notify_rect(sw, sh).map(|r| r.has(mx, my)).unwrap_or(false) {
            Layer::Notify
        } else if my >= sh - TB_H {
            Layer::Taskbar
        } else if let Some(a) = self.win_at(mx, my, sw, sh) {
            Layer::Win(a)
        } else {
            Layer::Desk
        };

        let dbl = if ev.pressed {
            let (t, x, y) = self.last_click;
            // tolleranza: 450 ms + durata dell'ultimo frame (su macchine lente il clic arriva in ritardo)
            let d = ev.clicks >= 2 || (now < t + 450 + self.frame_cost && (mx - x).abs() < 6 && (my - y).abs() < 6);
            self.last_click = if d { (0, 0, 0) } else { (now, mx, my) };
            d
        } else {
            false
        };

        self.dbl = dbl;
        // ---- tastiera globale
        if ev.has(Key::Super) {
            self.start = !self.start;
            self.start_t = now;
            self.power = false;
            self.menu = None;
        }
        if ev.has(Key::Esc) {
            if self.menu.is_some() {
                self.menu = None;
            } else if self.power {
                self.power = false;
            } else if self.start {
                self.start = false;
            }
        }

        // ---- clic fuori dai menu: chiudili
        let start_btn = self.taskbar_start_rect(sw, sh);
        if (ev.pressed || ev.rpressed) && layer != Layer::Menu {
            self.menu = None;
            if layer != Layer::Start && !(layer == Layer::Taskbar && start_btn.has(mx, my)) {
                self.start = false;
                self.power = false;
            }
        }

        // ---- finestre: focus, trascinamento, doppio clic sulla barra del titolo
        if ev.pressed {
            if let Layer::Win(a) = layer {
                self.order.retain(|&x| x != a);
                self.order.push(a);
                let (x, y, w, _, _) = self.rect(a, sw, sh);
                if my < y + TITLE_H && mx < x + w - 46 * 3 {
                    if dbl {
                        self.wins[a].max = !self.wins[a].max;
                    } else {
                        self.drag = Some((a, mx - x, my - y));
                    }
                }
            }
            if matches!(layer, Layer::Win(_) | Layer::Taskbar) {
                self.desk_focus = false;
            }
            if layer == Layer::Desk {
                self.desk_focus = true;
                let hit = self.icon_at(mx, my, sh);
                if self.drename.is_some() {
                    let same = matches!((hit, &self.drename), (Some(DIcon::Item(i)), Some((j, _))) if i == *j);
                    if !same {
                        self.commit_drename(true);
                    }
                }
                match hit {
                    Some(d) => {
                        if ev.ctrl {
                            if let Some(p) = self.dsel.iter().position(|x| *x == d) {
                                self.dsel.remove(p);
                            } else {
                                self.dsel.push(d);
                            }
                        } else if !self.dsel.contains(&d) || dbl {
                            self.dsel = alloc::vec![d];
                        }
                        if dbl && self.drename.is_none() {
                            self.open_dicon(d, now);
                        }
                    }
                    None => {
                        if !ev.ctrl {
                            self.dsel.clear();
                        }
                        self.band = Some((mx, my));
                    }
                }
            }
        }
        // selezione a riquadro
        if let Some((bx, by)) = self.band {
            if ev.down {
                let r = R { x: bx.min(mx), y: by.min(my), w: (mx - bx).abs(), h: (my - by).abs() };
                self.dsel = self
                    .icon_rects(sh)
                    .into_iter()
                    .filter(|(_, ir)| ir.x < r.x + r.w && r.x < ir.x + ir.w && ir.y < r.y + r.h && r.y < ir.y + ir.h)
                    .map(|(d, _)| d)
                    .collect();
            } else {
                self.band = None;
            }
        }
        // tastiera sul desktop
        if self.desk_focus && !self.start && self.menu.is_none() {
            if let Some((_, ed)) = &mut self.drename {
                if let Some(ok) = ed.keys(&ev.keys) {
                    self.commit_drename(ok);
                }
            } else {
                for k in ev.keys.iter() {
                    match k {
                        Key::Delete => self.delete_selected(),
                        Key::F2 => {
                            if let Some(DIcon::Item(i)) = self.dsel.first().copied() {
                                self.exec(Act::RenameD(i), now);
                            }
                        }
                        Key::F5 => {
                            self.reload_desk();
                            self.flash_t = now;
                        }
                        Key::Enter => {
                            for d in self.dsel.clone() {
                                self.open_dicon(d, now);
                            }
                        }
                        Key::Ctrl('a') => self.dsel = self.icon_order(),
                        _ => {}
                    }
                }
            }
        }
        if ev.rpressed && self.menu.is_none() {
            match layer {
                Layer::Desk => {
                    self.desk_focus = true;
                    if self.drename.is_some() {
                        self.commit_drename(true);
                    }
                    let hit = self.icon_at(mx, my, sh);
                    let kind = match hit {
                        Some(d) => {
                            if !self.dsel.contains(&d) {
                                self.dsel = alloc::vec![d];
                            }
                            match d {
                                DIcon::Sys(ic) => MenuKind::Icon(ic),
                                DIcon::Item(i) => MenuKind::DItem(i),
                            }
                        }
                        None => {
                            self.dsel.clear();
                            MenuKind::Desktop
                        }
                    };
                    self.menu = Some(Menu { x: mx, y: my, kind, sub: None, t: now });
                }
                Layer::Taskbar => self.menu = Some(Menu { x: mx, y: my, kind: MenuKind::Taskbar, sub: None, t: now }),
                _ => {}
            }
        }
        if let Some((a, ox, oy)) = self.drag {
            if ev.down {
                let win = &mut self.wins[a];
                if win.max && ev.moved {
                    // trascinare una finestra ingrandita la ripristina sotto il mouse
                    win.max = false;
                    let nox = (ox * win.w / sw).clamp(40, win.w - 140);
                    self.drag = Some((a, nox, oy));
                    win.x = mx - nox;
                    win.y = my - oy;
                } else if !win.max {
                    win.x = (mx - ox).clamp(-win.w + 140, sw - 140);
                    win.y = (my - oy).clamp(0, sh - TB_H - TITLE_H);
                }
            } else {
                self.drag = None;
            }
        }

        // ---- disegno
        let Gui { canvas, wall, blur, theme, .. } = gui;
        canvas.copy_from(wall);
        let mut ui = Ui { c: canvas, blur, ev, th: theme, mx, my, now, block: layer != Layer::Desk };

        if self.s.show_icons && now >= self.flash_t + 140 {
            self.draw_icons(&mut ui, sh);
        }

        let focus = self.focused();
        let order = self.order.clone();
        for &a in order.iter() {
            if !self.wins[a].open || self.wins[a].min {
                continue;
            }
            ui.block = layer != Layer::Win(a);
            let typing = focus == Some(a) && !self.start && !self.desk_focus;
            self.draw_window(&mut ui, wall, a, focus == Some(a), typing, sw, sh, now);
        }

        ui.block = layer != Layer::Taskbar;
        self.draw_taskbar(&mut ui, sw, sh, now);

        if let Some(r) = self.notify_rect(sw, sh) {
            ui.block = layer != Layer::Notify;
            self.draw_notify(&mut ui, r, now);
        }

        if self.start {
            ui.block = layer != Layer::Start;
            self.draw_start(&mut ui, srect, now);
        }

        if let Some((main, sub, items)) = mrects {
            ui.block = layer != Layer::Menu;
            self.draw_menu(&mut ui, main, sub, &items, now);
        }

        // dissolvenza iniziale dal nero
        if self.s.anim && now < self.boot_t + 700 {
            let k = 256 - (ease((now - self.boot_t) as f32 / 700.0) * 256.0) as u32;
            ui.c.rect(0, 0, sw, sh, BLACK, k);
        }
    }

    // ------------------------------------------------------------ notifica aggiornamenti

    fn notify_rect(&self, sw: i32, sh: i32) -> Option<R> {
        self.upd.notify.as_ref()?;
        let w = 380.min(sw - 24);
        Some(R { x: sw - w - 12, y: sh - TB_H - 12 - 92, w, h: 92 })
    }

    fn draw_notify(&mut self, ui: &mut Ui, r: R, now: u64) {
        let dk = self.dk();
        let Some(v) = self.upd.notify.clone() else { return };
        ui.panel_strong(r.x, r.y, r.w, r.h, 16);
        icon(ui.c, IC_UPD, r.x + 16, r.y + 18, 48);
        let tc = ui.th.text;
        let title = ui::ellipsize(dk.upd_notify, r.w - 120, &font::BODY_B);
        ui.c.text(r.x + 78, r.y + 20, &title, &font::BODY_B, tc, 256);
        let sub = ui::ellipsize(&format!("MIN-OS {}  ·  {}", v, dk.upd_notify_d), r.w - 100, &font::BODY);
        ui.sub(r.x + 78, r.y + 46, &sub, &font::BODY);
        // chiudi
        let (bx, by) = (r.x + r.w - 34, r.y + 10);
        let hot = !ui.block && ui.hover(bx, by, 24, 24);
        if hot {
            ui.c.rrect(bx, by, 24, 24, 6, ui.th.fill, ui.th.fill_a + 22);
        }
        let (cx, cy) = ((bx + 12) as f32, (by + 12) as f32);
        ui.c.line(cx - 4.0, cy - 4.0, cx + 4.0, cy + 4.0, 1.4, tc, 220);
        ui.c.line(cx + 4.0, cy - 4.0, cx - 4.0, cy + 4.0, 1.4, tc, 220);
        if ui.click(bx, by, 24, 24) {
            self.upd.notify = None;
        } else if ui.click(r.x, r.y, r.w, r.h) {
            self.upd.notify = None;
            self.launch(Launch::Page(6), now);
        }
    }

    /// Pagina Impostazioni > Aggiornamenti (stile Windows Update).
    fn updates_page(&mut self, ui: &mut Ui, cx: i32, cy: i32, cw: i32) {
        use crate::update::St;
        let dk = self.dk();
        let tc = ui.th.text;
        let now = ui.now;
        let mut y = cy;
        // la pagina e' aperta: la notifica non serve piu'
        self.upd.notify = None;
        ui.tile(cx, y, cw, 110, 14, false);
        icon(ui.c, IC_UPD, cx + 20, y + 22, 64);
        let st = self.upd.st.clone();
        let (title, sub): (String, String) = match &st {
            St::Idle => (format!("MIN-OS {}", crate::VERSION), String::from(dk.upd_auto)),
            St::Checking => (String::from(dk.upd_checking), format!("github.com/{}", crate::REPO)),
            St::UpToDate => (String::from(dk.upd_uptodate), format!("MIN-OS {}", crate::VERSION)),
            St::Available { version, size, .. } => (String::from(dk.upd_available), format!("MIN-OS {}  ·  {}", version, apps::human(*size))),
            St::Downloading { version, .. } => (format!("{}: MIN-OS {}", dk.upd_downloading, version), String::new()),
            St::Installing => (String::from(dk.upd_installing), String::new()),
            St::Done(v) => (String::from(dk.upd_done), format!("{} MIN-OS {}", dk.upd_hint, v)),
            St::Restored(v) => (String::from(dk.upd_restored), format!("{} MIN-OS {}", dk.upd_hint, v)),
            St::Error(e) => (String::from(dk.upd_failed), e.clone()),
        };
        let bw = 200;
        let tw = cw - 120 - bw - 20;
        let t = ui::ellipsize(&title, tw, &font::MED_B);
        ui.text(cx + 104, y + 32, &t, &font::MED_B);
        if !sub.is_empty() {
            let s2 = ui::ellipsize(&sub, tw, &font::BODY);
            ui.sub(cx + 104, y + 62, &s2, &font::BODY);
        }
        let (bx, by) = (cx + cw - 20 - bw, y + 37);
        match &st {
            St::Checking | St::Installing => ui::spinner(ui.c, cx + cw - 50, y + 55, 12, ui.th.accent, now),
            St::Available { .. } => {
                if ui.primary(bx, by, bw, 38, dk.upd_install, false, true) {
                    self.upd.install(dk);
                }
            }
            St::Done(_) | St::Restored(_) => {
                if ui.primary(bx, by, bw, 38, dk.upd_restart, false, true) {
                    self.action = Some(Power::Reboot);
                }
            }
            St::Downloading { size, .. } => {
                // barra di avanzamento
                let got = self.upd.progress() as u32;
                let total = (*size).max(1);
                let pw = cw - 124 - 20;
                let (px, py) = (cx + 104, y + 70);
                ui.c.rrect(px, py, pw, 8, 4, ui.th.fill, ui.th.fill_a + 20);
                let fill = (pw as u64 * got.min(total) as u64 / total as u64) as i32;
                if fill > 0 {
                    ui.c.rrect(px, py, fill.max(8), 8, 4, ui.th.accent, 256);
                }
                let txt = format!("{} / {}", apps::human(got), apps::human(total));
                ui.c.text_right(cx + cw - 20, y + 30, &txt, &font::BODY, ui.th.sub, 256);
            }
            _ => {
                if ui.primary(bx, by, bw, 38, dk.upd_check, false, true) {
                    self.upd.check(dk, false);
                }
            }
        }
        y += 122;
        let last = match &self.upd.last_check {
            Some(c) => self.fmt_time(c),
            None => String::from(dk.upd_never),
        };
        let rows: [(&str, String); 3] = [
            (dk.upd_current, format!("MIN-OS {}  ·  build {}", crate::VERSION, crate::BUILD)),
            (dk.upd_last, last),
            (dk.upd_source, format!("github.com/{}", crate::REPO)),
        ];
        spec_list(ui, cx, y, cw, &rows);
        y += 3 * 40 + 20;
        if let St::Available { notes, .. } = &st {
            if !notes.is_empty() {
                card(ui, cx, y, cw, 64, dk.upd_news, "");
                let n = ui::ellipsize(notes, cw - 40, &font::BODY);
                ui.sub(cx + 20, y + 35, &n, &font::BODY);
                y += 76;
            }
        }
        if let Some(prev) = self.upd.backup_version() {
            let desc = if prev.is_empty() { String::from(dk.upd_restore_d) } else { format!("{} (MIN-OS {})", dk.upd_restore_d, prev) };
            card(ui, cx, y, cw, 64, dk.upd_restore, &desc);
            let enabled = !self.upd.busy();
            if ui.button(cx + cw - 20 - 150, y + 14, 150, 36, dk.upd_restore_btn, false, enabled) {
                self.upd.restore(dk);
            }
            y += 76;
        }
        let note = ui::ellipsize(dk.upd_signed, cw - 8, &font::BODY);
        ui.c.text(cx + 4, y + 4, &note, &font::BODY, tc, 160);
    }

    // ------------------------------------------------------------ icone

    fn draw_icons(&mut self, ui: &mut Ui, sh: i32) {
        let (sz, _, _) = self.icon_cell();
        for (d, r) in self.icon_rects(sh) {
            let hot = !ui.block && r.has(ui.mx, ui.my);
            if self.dsel.contains(&d) {
                ui.c.rrect(r.x, r.y, r.w, r.h, 10, mix(ui.th.accent, WHITE, 60), 90);
                ui.c.rrect_border(r.x, r.y, r.w, r.h, 10, mix(ui.th.accent, WHITE, 140), 200);
            } else if hot {
                ui.c.rrect(r.x, r.y, r.w, r.h, 10, WHITE, 34);
                ui.c.rrect_border(r.x, r.y, r.w, r.h, 10, WHITE, 60);
            }
            let (ix, iy) = (r.x + (r.w - sz) / 2, r.y + 8);
            match d {
                DIcon::Sys(ic) => icon(ui.c, ic, ix, iy, sz),
                DIcon::Item(i) => {
                    if self.desk[i].dir {
                        apps::draw_folder(ui.c, ix, iy, sz);
                    } else {
                        apps::draw_doc(ui.c, ix, iy, sz);
                    }
                }
            }
            let ly = r.y + 8 + sz + 7;
            if let (DIcon::Item(i), Some((j, ed))) = (d, &self.drename) {
                if i == *j {
                    let fw = (font::text_width(&ed.buf, &font::BODY) + 24).max(r.w + 20);
                    let fx = (r.x + r.w / 2 - fw / 2).max(4);
                    ed.draw(ui.c, fx, ly - 5, fw, 28, ui.th, ui.now);
                    continue;
                }
            }
            let label = ui::ellipsize(&self.dicon_label(d), r.w - 6, &font::BODY);
            let cx = r.x + r.w / 2;
            ui.c.text_center(cx + 1, ly + 1, &label, &font::BODY, BLACK, 170);
            ui.c.text_center(cx, ly, &label, &font::BODY, WHITE, 256);
        }
        if let Some((bx, by)) = self.band {
            let (mx, my) = (ui.mx, ui.my);
            let (x, y, w, h) = (bx.min(mx), by.min(my), (mx - bx).abs(), (my - by).abs());
            if w > 2 || h > 2 {
                ui.c.rect(x, y, w, h, ui.th.accent, 60);
                ui.c.rrect_border(x, y, w, h, 2, mix(ui.th.accent, WHITE, 80), 230);
            }
        }
    }

    // ------------------------------------------------------------ barra delle applicazioni

    fn taskbar_apps(&self) -> Vec<usize> {
        let mut v = alloc::vec![FILES, WEB, TERM, SETT];
        for a in [INFO, NOTE, CALC, BIN] {
            if self.wins[a].open {
                v.push(a);
            }
        }
        v
    }

    fn taskbar_start_rect(&self, sw: i32, sh: i32) -> R {
        let n = 1 + self.taskbar_apps().len() as i32;
        let total = n * 44 + (n - 1) * 6;
        R { x: (sw - total) / 2, y: sh - TB_H + 4, w: 44, h: 44 }
    }

    fn draw_taskbar(&mut self, ui: &mut Ui, sw: i32, sh: i32, now: u64) {
        let ty = sh - TB_H;
        ui.panel_strong(-30, ty, sw + 60, TB_H + 30, 0);
        let dk = self.dk();
        let apps = self.taskbar_apps();
        let st = self.taskbar_start_rect(sw, sh);
        let focus = self.focused();
        let mut tip: Option<(i32, &'static str)> = None;
        let mut tip_net: Option<(i32, String)> = None;

        // pulsante Start
        let hot = ui.hover(st.x, st.y, st.w, st.h) && !ui.block;
        if hot || self.start {
            ui.c.rrect(st.x, st.y, st.w, st.h, 10, ui.th.fill, ui.th.fill_a + 22);
            ui.c.rrect_border(st.x, st.y, st.w, st.h, 10, WHITE, 40);
        }
        draw_logo(ui.c, st.x + 9, st.y + 9, 26, ui.th.accent);
        if ui.click(st.x, st.y, st.w, st.h) {
            self.start = !self.start;
            self.start_t = now;
            self.power = false;
            self.search.clear();
        }

        for (i, &a) in apps.iter().enumerate() {
            let bx = st.x + (i as i32 + 1) * 50;
            let by = st.y;
            let hot = ui.hover(bx, by, 44, 44) && !ui.block;
            let w = self.wins[a];
            let active = w.open && !w.min && focus == Some(a);
            if hot || active {
                ui.c.rrect(bx, by, 44, 44, 10, ui.th.fill, ui.th.fill_a + if active { 26 } else { 18 });
                ui.c.rrect_border(bx, by, 44, 44, 10, WHITE, 40);
            }
            icon(ui.c, app_icon(a), bx + 8, by + 8, 28);
            if w.open {
                let pw = if active { 16 } else { 6 };
                let col = if active { ui.th.accent } else { ui.th.sub };
                ui.c.rrect(bx + 22 - pw / 2, ty + TB_H - 5, pw, 3, 1, col, 256);
            }
            if hot {
                tip = Some((bx + 22, self.app_name(a)));
            }
            if ui.click(bx, by, 44, 44) {
                self.taskbar_click(a, now);
            }
        }

        // area di notifica: lingua, ora e data, "mostra desktop"
        let time = self.fmt_time(&self.clock);
        let date = format!("{:02}/{:02}/{}", self.clock.day, self.clock.month, self.clock.year);
        let cw = font::text_width(&time, &font::BODY).max(font::text_width(&date, &font::BODY));
        let cx = sw - 22 - cw;
        let tc = ui.th.text;
        let hot = ui.hover(cx - 10, ty + 4, cw + 20, 44) && !ui.block;
        if hot {
            ui.c.rrect(cx - 10, ty + 4, cw + 20, 44, 10, ui.th.fill, ui.th.fill_a + 18);
        }
        ui.c.text_right(sw - 22, ty + 8, &time, &font::BODY, tc, 256);
        ui.c.text_right(sw - 22, ty + 28, &date, &font::BODY, tc, 256);
        let ni = net::info();
        if ni.present {
            let nx = cx - 104;
            let hot = ui.hover(nx, ty + 8, 40, 36) && !ui.block;
            if hot {
                ui.c.rrect(nx, ty + 8, 40, 36, 10, ui.th.fill, ui.th.fill_a + 18);
                let label = match ni.ip {
                    Some(ip) => format!("{}  ·  {}", dk.ethernet, ip.address()),
                    None => String::from(dk.disconnected),
                };
                tip_net = Some((nx + 20, label));
            }
            let a = if ni.ip.is_some() { 256 } else { 90 };
            wifi(ui.c, (nx + 20) as f32, (ty + 24) as f32, 22.0, tc, a);
            if ui.click(nx, ty + 8, 40, 36) {
                self.launch(Launch::Page(5), now);
            }
        }
        let code = self.s.layout.code();
        let lx = cx - 56;
        let hot = ui.hover(lx, ty + 8, 40, 36) && !ui.block;
        if hot {
            ui.c.rrect(lx, ty + 8, 40, 36, 10, ui.th.fill, ui.th.fill_a + 18);
        }
        ui.c.text_center(lx + 20, ty + 18, code, &font::BODY_B, tc, 230);
        if ui.click(lx, ty + 8, 40, 36) {
            // clic sulla lingua: passa al layout successivo
            let i = keymap::ALL.iter().position(|l| *l == self.s.layout).unwrap_or(0);
            self.s.layout = keymap::ALL[(i + 1) % keymap::ALL.len()];
        }
        if ui.click(cx - 10, ty + 4, cw + 20, 44) {
            self.launch(Launch::Page(2), now);
        }
        let sep = if ui.th.dark { WHITE } else { BLACK };
        ui.c.rect(sw - 8, ty + 14, 1, TB_H - 28, sep, 50);
        if ui.click(sw - 7, ty, 7, TB_H) {
            self.exec(Act::ShowDesktop, now);
        }

        if let Some((x, label)) = tip_net {
            let lw = font::text_width(&label, &font::BODY) + 26;
            let lx = (x - lw / 2).min(sw - lw - 8);
            ui.panel_strong(lx, ty - 44, lw, 32, 12);
            let tc = ui.th.text;
            ui.c.text(lx + 13, ty - 36, &label, &font::BODY, tc, 256);
        }
        if let Some((x, label)) = tip {
            let lw = font::text_width(label, &font::BODY) + 26;
            ui.panel_strong(x - lw / 2, ty - 44, lw, 32, 12);
            let tc = ui.th.text;
            ui.c.text_center(x, ty - 36, label, &font::BODY, tc, 256);
        }
    }

    fn fmt_time(&self, c: &DateTime) -> String {
        if self.s.clock24 {
            format!("{:02}:{:02}", c.hour, c.min)
        } else {
            let h = if c.hour % 12 == 0 { 12 } else { c.hour % 12 };
            format!("{}:{:02} {}", h, c.min, if c.hour < 12 { "AM" } else { "PM" })
        }
    }

    // ------------------------------------------------------------ menu Start

    fn draw_start(&mut self, ui: &mut Ui, r: R, now: u64) {
        let dk = self.dk();
        let t = self.t();
        let p = if self.s.anim { ((now.saturating_sub(self.start_t)) as f32 / 220.0).min(1.0) } else { 1.0 };
        let e = ease(p);
        let r = R { y: r.y + ((1.0 - e) * 30.0) as i32, ..r };
        let fading = p < 1.0;
        if fading {
            ui::snapshot(ui.c, r.x - 30, r.y - 30, r.w + 60, r.h + 60, &mut self.snap);
        }
        ui.panel_strong(r.x, r.y, r.w, r.h, 26);

        // tastiera: la ricerca riceve tutto
        let mut enter = false;
        for k in ui.ev.keys.iter() {
            match k {
                Key::Char(c) if self.search.chars().count() < 30 => self.search.push(*c),
                Key::Backspace => {
                    self.search.pop();
                }
                Key::Enter => enter = true,
                _ => {}
            }
        }
        ui.field(r.x + 28, r.y + 24, r.w - 56, 44, &self.search, dk.search, true);
        // lente d'ingrandimento
        let (gx, gy) = ((r.x + r.w - 54) as f32, (r.y + 44) as f32);
        let tc = ui.th.sub;
        ui.c.circle_border((gx) as i32, (gy - 2.0) as i32, 7, tc, 220);
        ui.c.line(gx + 5.0, gy + 3.0, gx + 10.0, gy + 8.0, 2.0, tc, 220);

        let mut launch: Option<Launch> = None;
        let (ix, iw) = (r.x + 28, r.w - 56);
        if self.search.trim().is_empty() {
            ui.text(ix + 8, r.y + 92, dk.pinned, &font::BODY_B);
            let entries = self.start_entries();
            let cols = 6;
            let cw = iw / cols;
            for (i, (ic, label, l)) in entries.iter().enumerate() {
                let cx = ix + (i as i32 % cols) * cw;
                let cy = r.y + 124 + (i as i32 / cols) * 96;
                let hot = !ui.block && ui.hover(cx + 2, cy, cw - 4, 90);
                if hot {
                    ui.c.rrect(cx + 2, cy, cw - 4, 90, 12, ui.th.fill, ui.th.fill_a + 18);
                }
                icon(ui.c, *ic, cx + (cw - 44) / 2, cy + 10, 44);
                let lab = ui::ellipsize(label, cw - 10, &font::BODY);
                let tc = ui.th.text;
                ui.c.text_center(cx + cw / 2, cy + 62, &lab, &font::BODY, tc, 256);
                if ui.click(cx + 2, cy, cw - 4, 90) {
                    launch = Some(*l);
                }
            }
            let ry = r.y + 124 + 96 + 26;
            ui.text(ix + 8, ry, dk.recommended, &font::BODY_B);
            let recs = [(IC_PAINT, Launch::Page(1)), (IC_TERM, Launch::App(TERM))];
            let rw = (iw - 12) / 2;
            for (i, (ic, l)) in recs.iter().enumerate() {
                let (bx, by) = (ix + i as i32 * (rw + 12), ry + 32);
                let hot = !ui.block && ui.hover(bx, by, rw, 64);
                if hot {
                    ui.c.rrect(bx, by, rw, 64, 12, ui.th.fill, ui.th.fill_a + 18);
                }
                icon(ui.c, *ic, bx + 10, by + 12, 40);
                let (title, sub) = dk.rec[i];
                let tt = ui::ellipsize(title, rw - 72, &font::BODY_B);
                let ss = ui::ellipsize(sub, rw - 72, &font::BODY);
                ui.text(bx + 62, by + 13, &tt, &font::BODY_B);
                ui.sub(bx + 62, by + 35, &ss, &font::BODY);
                if ui.click(bx, by, rw, 64) {
                    launch = Some(*l);
                }
            }
        } else {
            ui.text(ix + 8, r.y + 92, dk.results, &font::BODY_B);
            let res = self.start_results();
            for (i, (ic, label, l)) in res.iter().enumerate().take(6) {
                let by = r.y + 122 + i as i32 * 54;
                let hot = !ui.block && ui.hover(ix, by, iw, 50);
                if hot || i == 0 {
                    ui.c.rrect(ix, by, iw, 50, 12, ui.th.fill, ui.th.fill_a + if hot { 22 } else { 12 });
                }
                icon(ui.c, *ic, ix + 10, by + 9, 32);
                ui.text(ix + 56, by + 16, label, &font::BODY_B);
                if ui.click(ix, by, iw, 50) {
                    launch = Some(*l);
                }
            }
            if enter {
                if let Some((_, _, l)) = res.first() {
                    launch = Some(*l);
                }
            }
        }

        // barra in basso: utente e accensione
        let by = r.y + r.h - 68;
        let sep = if ui.th.dark { WHITE } else { BLACK };
        ui.c.rect(r.x + 1, by, r.w - 2, 1, sep, 26);
        let (ax, ay) = (r.x + 50, by + 34);
        avatar(ui, ax, ay, 18, &self.s.user);
        ui.text(ax + 30, ay - 9, &self.s.user, &font::BODY_B);
        let (px, py) = (r.x + r.w - 72, by + 14);
        let hot = !ui.block && ui.hover(px, py, 40, 40);
        if hot || self.power {
            ui.c.rrect(px, py, 40, 40, 10, ui.th.fill, ui.th.fill_a + 22);
        }
        let tc = ui.th.text;
        ring_arc(ui.c, (px + 20) as f32, (py + 21) as f32, 9.0, 2.2, tc);
        ui.c.line((px + 20) as f32, (py + 9) as f32, (px + 20) as f32, (py + 18) as f32, 2.2, tc, 256);
        if ui.click(px, py, 40, 40) {
            self.power = !self.power;
        }

        if fading {
            ui::fade_to_snapshot(ui.c, r.x - 30, r.y - 30, r.w + 60, r.h + 60, &self.snap, (e * 256.0) as u32);
        }

        if self.power {
            let (ww, hh) = (220, 120);
            let (qx, qy) = (px + 40 - ww, py - hh - 8);
            ui.panel_strong(qx, qy, ww, hh, 16);
            if ui.button(qx + 12, qy + 12, ww - 24, 44, t.restart, false, true) {
                self.action = Some(Power::Reboot);
            }
            let red = 0xFF453A;
            let hot = !ui.block && ui.hover(qx + 12, qy + 64, ww - 24, 44);
            ui.c.rrect(qx + 12, qy + 64, ww - 24, 44, 22, red, if hot { 256 } else { 215 });
            ui.c.text_center(qx + ww / 2, qy + 77, t.shutdown, &font::BODY_B, WHITE, 256);
            if ui.click(qx + 12, qy + 64, ww - 24, 44) {
                self.action = Some(Power::Off);
            }
        }

        if let Some(l) = launch {
            self.launch(l, now);
        }
    }

    // ------------------------------------------------------------ menu contestuale

    fn draw_menu(&mut self, ui: &mut Ui, main: R, sub: Option<R>, items: &[Item], now: u64) {
        let t0 = self.menu.as_ref().map(|m| m.t).unwrap_or(0);
        let p = if self.s.anim { ((now.saturating_sub(t0)) as f32 / 140.0).min(1.0) } else { 1.0 };
        let e = ease(p);
        let fading = p < 1.0;
        let full = R { x: main.x.min(sub.map(|s| s.x).unwrap_or(main.x)) - 30, y: main.y - 30, w: MW * 2 + 60, h: main.h.max(sub.map(|s| s.y + s.h - main.y).unwrap_or(0)) + 60 };
        if fading {
            ui::snapshot(ui.c, full.x, full.y, full.w, full.h, &mut self.snap);
        }
        let mut chosen: Option<Act> = None;
        let mut hover_parent: Option<Option<usize>> = None;
        let in_sub = sub.map(|s| s.has(ui.mx, ui.my)).unwrap_or(false);

        // menu principale
        let (hov, clk) = draw_menu_panel(ui, main, items);
        if !in_sub {
            if let Some(i) = hov {
                hover_parent = Some(if items[i].sub.is_empty() { None } else { Some(i) });
            }
        }
        if let Some(i) = clk {
            if items[i].sub.is_empty() {
                chosen = Some(items[i].act);
            } else {
                hover_parent = Some(Some(i));
            }
        }
        // sottomenu
        if let (Some(sr), Some(pi)) = (sub, self.menu.as_ref().and_then(|m| m.sub)) {
            let (_, clk) = draw_menu_panel(ui, sr, &items[pi].sub);
            if let Some(i) = clk {
                chosen = Some(items[pi].sub[i].act);
            }
        }
        if fading {
            ui::fade_to_snapshot(ui.c, full.x, full.y, full.w, full.h, &self.snap, (e * 256.0) as u32);
        }
        if let (Some(h), Some(m)) = (hover_parent, self.menu.as_mut()) {
            if m.sub != h {
                m.sub = h;
                self.dirty = true;
            }
        }
        if let Some(a) = chosen {
            self.exec(a, now);
        }
    }

    // ------------------------------------------------------------ finestre

    #[allow(clippy::too_many_arguments)]
    fn draw_window(&mut self, ui: &mut Ui, wall: &Canvas, a: usize, focused: bool, typing: bool, sw: i32, sh: i32, now: u64) {
        let p = if self.s.anim { ((now.saturating_sub(self.wins[a].open_t)) as f32 / 240.0).min(1.0) } else { 1.0 };
        let e = ease(p);
        let (x, y0, w, h, r) = self.rect(a, sw, sh);
        let y = y0 + ((1.0 - e) * 18.0) as i32;
        let fading = p < 1.0;
        if fading {
            ui::snapshot(ui.c, x - 40, y - 40, w + 80, h + 80, &mut self.snap);
        }
        ui.panel_strong(x, y, w, h, r);

        // barra del titolo: icona + nome a sinistra, pulsanti a destra (come Windows)
        icon(ui.c, app_icon(a), x + 14, y + 11, 20);
        let tc = ui.th.text;
        ui.c.text(x + 44, y + 13, self.app_name(a), &font::BODY, tc, if focused { 256 } else { 160 });
        let mut act = 0;
        for i in 0..3 {
            let bx = x + w - 46 * (3 - i) - 6;
            let (by, bw, bh) = (y + 6, 42, 30);
            let hot = !ui.block && ui.hover(bx, by, bw, bh);
            if hot {
                if i == 2 {
                    ui.c.rrect(bx, by, bw, bh, 8, 0xE5383B, 256);
                } else {
                    ui.c.rrect(bx, by, bw, bh, 8, ui.th.fill, ui.th.fill_a + 22);
                }
            }
            let col = if hot && i == 2 { WHITE } else { tc };
            let (cx, cy) = ((bx + bw / 2) as f32, (by + bh / 2) as f32);
            match i {
                0 => ui.c.line(cx - 5.5, cy, cx + 5.5, cy, 1.4, col, 256),
                1 => {
                    if self.wins[a].max {
                        ui.c.rrect_border(cx as i32 - 4, cy as i32 - 6, 10, 10, 2, col, 230);
                        ui.c.rrect(cx as i32 - 7, cy as i32 - 3, 10, 10, 2, mix(ui.th.strong.tint, WHITE, 0), 256);
                        ui.c.rrect_border(cx as i32 - 7, cy as i32 - 3, 10, 10, 2, col, 256);
                    } else {
                        ui.c.rrect_border(cx as i32 - 5, cy as i32 - 5, 11, 11, 2, col, 256);
                    }
                }
                _ => {
                    ui.c.line(cx - 5.0, cy - 5.0, cx + 5.0, cy + 5.0, 1.4, col, 256);
                    ui.c.line(cx + 5.0, cy - 5.0, cx - 5.0, cy + 5.0, 1.4, col, 256);
                }
            }
            if ui.click(bx, by, bw, bh) {
                act = i + 1;
            }
        }

        let (cx, cy, cw, ch) = (x, y + TITLE_H, w, h - TITLE_H);
        match a {
            TERM => self.terminal(ui, cx + 18, cy + 10, cw - 36, ch - 22, typing),
            INFO => self.info(ui, cx + 22, cy + 16, cw - 44),
            SETT => self.settings(ui, wall, cx, cy, cw, ch, typing, now),
            FILES => {
                let dk = self.dk();
                match self.files.draw(ui, cx, cy, cw, ch, typing, self.dbl, dk) {
                    FilesAct::Open(dir, e) => self.open_file(dir, e, now),
                    FilesAct::Context(o) => {
                        let kind = o.map(MenuKind::FilesItem).unwrap_or(MenuKind::FilesBg);
                        self.menu = Some(Menu { x: ui.mx, y: ui.my, kind, sub: None, t: now });
                        self.dirty = true;
                    }
                    FilesAct::None => {}
                }
            }
            NOTE => {
                let dk = self.dk();
                self.note.draw(ui, cx, cy, cw, ch, typing, dk);
            }
            CALC => self.calc.draw(ui, cx, cy, cw, ch, typing),
            WEB => {
                let dk = self.dk();
                if let crate::web::WebAct::OpenDownloads = self.web.draw(ui, cx, cy, cw, ch, typing, dk) {
                    self.files.go_root_dir(apps::DOWNLOADS, dk.downloads);
                    self.open(FILES, now);
                }
            }
            _ => {
                let dk = self.dk();
                apps::draw_bin(ui, cx, cy, cw, ch, dk);
            }
        }
        if fading {
            ui::fade_to_snapshot(ui.c, x - 40, y - 40, w + 80, h + 80, &self.snap, (e * 256.0) as u32);
        }
        match act {
            1 => self.wins[a].min = true,
            2 => self.wins[a].max = !self.wins[a].max,
            3 => self.close(a),
            _ => {}
        }
    }

    /// Completamento con Tab: comandi (prima parola) o nomi di file della cartella corrente.
    fn complete(&mut self) {
        const CMDS: &[&str] = &[
            "help", "info", "clear", "echo", "time", "theme", "accent", "lang", "ls", "cd", "pwd", "cat", "mkdir", "rm", "touch", "edit", "ip", "ping", "dns",
            "get", "internet", "settings", "reboot", "shutdown", "whoami", "uname", "hostname",
        ];
        let before: String = self.input.chars().take(self.cur).collect();
        let after: String = self.input.chars().skip(self.cur).collect();
        let start = before.rfind(' ').map(|i| i + 1).unwrap_or(0);
        let word = &before[start..];
        let (dir_part, prefix) = match word.rfind('/') {
            Some(i) => (&word[..=i], &word[i + 1..]),
            None => ("", word),
        };
        let mut cands: Vec<String> = Vec::new();
        if start == 0 {
            cands.extend(CMDS.iter().filter(|c| c.starts_with(prefix)).map(|c| alloc::format!("{} ", c)));
        } else if fat::mounted() {
            if let Some(d) = fat::resolve(self.cwd, if dir_part.is_empty() { "." } else { dir_part }) {
                let low = lower(prefix);
                for e in fat::list(d) {
                    if lower(&e.name).starts_with(&low) {
                        cands.push(alloc::format!("{}{}", e.name, if e.dir { "/" } else { " " }));
                    }
                }
            }
        }
        let done = match cands.len() {
            0 => return,
            1 => cands[0].clone(),
            _ => {
                // prefisso comune + elenco delle possibilita'
                let mut common: Vec<char> = cands[0].chars().collect();
                for c in cands.iter().skip(1) {
                    let n = common.iter().zip(c.chars()).take_while(|(a, b)| a.to_lowercase().eq(b.to_lowercase())).count();
                    common.truncate(n);
                }
                let prompt = alloc::format!("{}@{} {} $ {}", self.s.user, self.s.pc, self.pwd(), self.input);
                self.lines.push(prompt);
                self.lines.push(cands.iter().map(|c| c.trim_end()).collect::<Vec<_>>().join("   "));
                common.iter().collect()
            }
        };
        let mut new_before: String = before[..start].into();
        new_before.push_str(dir_part);
        new_before.push_str(&done);
        self.cur = new_before.chars().count();
        new_before.push_str(&after);
        self.input = new_before;
    }

    fn paste(&mut self) {
        let text = crate::clip::get().replace('\r', "").replace('\n', " ").replace('\t', " ");
        let before: String = self.input.chars().take(self.cur).collect();
        let after: String = self.input.chars().skip(self.cur).collect();
        let room = 400usize.saturating_sub(self.input.chars().count());
        let ins: String = text.chars().take(room).collect();
        self.cur += ins.chars().count();
        self.input = before + &ins + &after;
    }

    fn terminal(&mut self, ui: &mut Ui, x: i32, y: i32, w: i32, h: i32, focused: bool) {
        let dk = self.dk();
        let f = font::BODY;
        let lh = 21;
        let cw = font::mono_width(&f);
        let cols = (w / cw).max(10) as usize;
        let rows = ((h - 8) / lh).max(2) as usize;
        let n_in = self.input.chars().count();
        self.cur = self.cur.min(n_in);
        let mut copy = false;
        if focused {
            for k in ui.ev.keys.iter() {
                match *k {
                    Key::Char(c) => {
                        if n_in < 400 {
                            let mut v: Vec<char> = self.input.chars().collect();
                            v.insert(self.cur, c);
                            self.input = v.into_iter().collect();
                            self.cur += 1;
                        }
                        self.tsel = None;
                        self.tscroll = 0;
                    }
                    Key::Backspace if self.cur > 0 => {
                        let mut v: Vec<char> = self.input.chars().collect();
                        v.remove(self.cur - 1);
                        self.input = v.into_iter().collect();
                        self.cur -= 1;
                    }
                    Key::Delete if self.cur < self.input.chars().count() => {
                        let mut v: Vec<char> = self.input.chars().collect();
                        v.remove(self.cur);
                        self.input = v.into_iter().collect();
                    }
                    Key::Left => self.cur = self.cur.saturating_sub(1),
                    Key::Right => self.cur = (self.cur + 1).min(self.input.chars().count()),
                    Key::Home => self.cur = 0,
                    Key::End => self.cur = self.input.chars().count(),
                    Key::Up if !self.hist.is_empty() => {
                        let p = match self.hist_pos {
                            None => self.hist.len() - 1,
                            Some(p) => p.saturating_sub(1),
                        };
                        self.hist_pos = Some(p);
                        self.input = self.hist[p].clone();
                        self.cur = self.input.chars().count();
                    }
                    Key::Down => {
                        if let Some(p) = self.hist_pos {
                            if p + 1 < self.hist.len() {
                                self.hist_pos = Some(p + 1);
                                self.input = self.hist[p + 1].clone();
                            } else {
                                self.hist_pos = None;
                                self.input.clear();
                            }
                            self.cur = self.input.chars().count();
                        }
                    }
                    Key::Tab => self.complete(),
                    Key::PageUp => self.tscroll += rows / 2,
                    Key::PageDown => self.tscroll = self.tscroll.saturating_sub(rows / 2),
                    Key::Enter => {
                        let cmd = core::mem::take(&mut self.input);
                        if !cmd.trim().is_empty() && self.hist.last() != Some(&cmd) {
                            self.hist.push(cmd.clone());
                            if self.hist.len() > 100 {
                                self.hist.remove(0);
                            }
                        }
                        self.hist_pos = None;
                        self.cur = 0;
                        self.tscroll = 0;
                        self.tsel = None;
                        self.run_cmd(&cmd);
                    }
                    Key::Ctrl('c') => {
                        if self.tsel.is_some() {
                            copy = true;
                        } else {
                            let p = alloc::format!("{}@{} {} $ {}^C", self.s.user, self.s.pc, self.pwd(), self.input);
                            self.lines.push(p);
                            self.input.clear();
                            self.cur = 0;
                        }
                    }
                    Key::Ctrl('v') => self.paste(),
                    Key::Ctrl('l') => self.lines.clear(),
                    Key::Ctrl('a') => self.tsel = Some(((0, 0), (usize::MAX, usize::MAX))),
                    _ => {}
                }
            }
        }

        // ---- righe da mostrare (con a capo automatico); l'ultima e' il prompt attivo
        let prompt = alloc::format!("{}@{} {} $ ", self.s.user, self.s.pc, self.pwd());
        let mut wrapped: Vec<(String, usize)> = Vec::new(); // (testo, lunghezza del prompt colorato)
        for l in self.lines.iter() {
            let pl = l.find(" $ ").filter(|_| l.starts_with(&alloc::format!("{}@", self.s.user))).map(|i| l[..i + 3].chars().count()).unwrap_or(0);
            let chars: Vec<char> = l.chars().collect();
            if chars.is_empty() {
                wrapped.push((String::new(), 0));
            }
            for (k, chunk) in chars.chunks(cols).enumerate() {
                let p = pl.saturating_sub(k * cols).min(chunk.len());
                wrapped.push((chunk.iter().collect(), p));
            }
        }
        let active_start = wrapped.len();
        let full: Vec<char> = prompt.chars().chain(self.input.chars()).collect();
        let ppl = prompt.chars().count();
        for (k, chunk) in full.chunks(cols).enumerate() {
            wrapped.push((chunk.iter().collect(), ppl.saturating_sub(k * cols).min(chunk.len())));
        }
        if full.len() % cols == 0 {
            wrapped.push((String::new(), 0));
        }
        let total = wrapped.len();
        let area = (x - 8, y - 6, w + 16, h + 6);
        let hover = !ui.block && ui.hover(area.0, area.1, area.2, area.3);
        if hover && ui.ev.wheel != 0 {
            let d = ui.ev.wheel * 3;
            self.tscroll = if d < 0 { self.tscroll + (-d) as usize } else { self.tscroll.saturating_sub(d as usize) };
        }
        self.tscroll = self.tscroll.min(total.saturating_sub(rows));
        let start = total.saturating_sub(rows + self.tscroll);

        // ---- selezione con il mouse
        let pos = |mx: i32, my: i32| -> (usize, usize) {
            let r = (((my - y - 2).max(0)) / lh) as usize + start;
            let c = (((mx - x + cw / 2).max(0)) / cw) as usize;
            (r.min(total.saturating_sub(1)), c.min(cols))
        };
        if hover && ui.ev.pressed {
            let p = pos(ui.mx, ui.my);
            self.tsel = Some((p, p));
            self.tselecting = true;
        }
        if self.tselecting {
            if ui.ev.down {
                if let Some((a, _)) = self.tsel {
                    self.tsel = Some((a, pos(ui.mx, ui.my)));
                }
                if ui.my < y && ui.ev.moved {
                    self.tscroll += 1;
                }
            } else {
                self.tselecting = false;
                if let Some((a, b)) = self.tsel {
                    if a == b {
                        self.tsel = None;
                    }
                }
            }
        }
        if hover && ui.ev.rpressed {
            if self.tsel.is_some() {
                copy = true;
            } else {
                self.paste();
            }
        }
        let norm = self.tsel.map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
        if copy {
            if let Some((a, b)) = norm {
                let mut out: Vec<String> = Vec::new();
                for r in a.0..=b.0.min(total - 1) {
                    let line: Vec<char> = wrapped[r].0.chars().collect();
                    let c0 = if r == a.0 { a.1.min(line.len()) } else { 0 };
                    let c1 = if r == b.0 { b.1.min(line.len()) } else { line.len() };
                    out.push(line[c0..c1.max(c0)].iter().collect());
                }
                crate::clip::set(&out.join("\n"));
                self.toast = Some((String::from(dk.copied), ui.now));
                self.tsel = None;
            }
        }

        // ---- disegno
        ui.c.rrect(area.0, area.1, area.2, area.3, 14, BLACK, if ui.th.dark { 90 } else { 26 });
        let tc = ui.th.text;
        let acc = if ui.th.dark { mix(ui.th.accent, WHITE, 110) } else { ui.th.accent };
        for (vi, r) in (start..total).take(rows).enumerate() {
            let ly = y + 2 + vi as i32 * lh;
            if let Some((a, b)) = norm {
                if r >= a.0 && r <= b.0 {
                    let len = wrapped[r].0.chars().count();
                    let c0 = if r == a.0 { a.1.min(len) } else { 0 };
                    let c1 = if r == b.0 { b.1.min(len) } else { len.max(1) };
                    if c1 > c0 {
                        ui.c.rect(x + c0 as i32 * cw, ly - 2, (c1 - c0) as i32 * cw, lh, ui.th.accent, 150);
                    }
                }
            }
            let (text, pl) = &wrapped[r];
            let head: String = text.chars().take(*pl).collect();
            let tail: String = text.chars().skip(*pl).collect();
            ui.c.text_mono(x, ly, &head, &f, acc, 256);
            ui.c.text_mono(x + *pl as i32 * cw, ly, &tail, &f, tc, 245);
        }
        // cursore
        let ci = ppl + self.cur;
        let crow = active_start + ci / cols;
        if focused && (ui.now / 530) % 2 == 0 && crow >= start && crow < start + rows {
            let cx = x + (ci % cols) as i32 * cw;
            let cy = y + 2 + (crow - start) as i32 * lh;
            ui.c.rect(cx, cy, 2, 17, acc, 256);
        }
        // barra di scorrimento
        if total > rows {
            let track = h - 8;
            let th = (track * rows as i32 / total as i32).max(24);
            let ty = y + (track - th) * (total - rows - self.tscroll) as i32 / (total - rows) as i32;
            ui.c.rrect(x + w + 2, ty, 4, th, 2, tc, 90);
        }
        if let Some((m, t)) = &self.toast {
            if ui.now < t + 1500 {
                let tw = font::text_width(m, &font::BODY) + 28;
                ui.c.rrect(x + w - tw, y, tw, 30, 15, ui.th.accent, 230);
                ui.c.text(x + w - tw + 14, y + 7, m, &font::BODY, WHITE, 256);
            }
        }
    }

    fn run_cmd(&mut self, cmd: &str) {
        let t = self.t();
        self.lines.push(format!("{}@{} {} $ {}", self.s.user, self.s.pc, self.pwd(), cmd));
        let mut parts = cmd.trim().splitn(2, ' ');
        let name = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim();
        if net::command(name, arg, self.dk()) {
            return;
        }
        if self.fs_command(name, arg) {
            return;
        }
        match name {
            "" => {}
            "help" => self.lines.push(String::from(t.term_help)),
            "clear" | "cls" => self.lines.clear(),
            "echo" => self.lines.push(String::from(arg)),
            "whoami" => self.lines.push(self.s.user.clone()),
            "hostname" => self.lines.push(self.s.pc.clone()),
            "uname" | "ver" => self.lines.push(format!("MIN-OS {} x86_64 (Rust)", crate::VERSION)),
            "time" | "date" => {
                let c = arch::rtc_now();
                self.lines.push(format!(
                    "{} {} {} {}  {:02}:{:02}:{:02}",
                    t.days[arch::weekday(&c)],
                    c.day,
                    t.months[(c.month as usize).clamp(1, 12) - 1],
                    c.year,
                    c.hour,
                    c.min,
                    c.sec
                ));
            }
            "info" | "neofetch" => {
                let sys = SYS.get();
                self.lines.push(format!("MIN-OS {}  ·  kernel Rust  ·  x86_64", crate::VERSION));
                self.lines.push(format!("CPU: {}  ·  RAM: {} MB", sys.0.trim(), sys.2));
                self.lines.push(format!("{}: {}", t.uptime, uptime()));
            }
            "theme" => match arg {
                "dark" => self.s.dark = true,
                "light" => self.s.dark = false,
                _ => self.lines.push(String::from("theme dark | theme light")),
            },
            "accent" => match arg.parse::<usize>() {
                Ok(n) if (1..=6).contains(&n) => self.s.accent = n - 1,
                _ => self.lines.push(String::from("accent 1..6")),
            },
            "lang" => match i18n::ALL.iter().find(|l| l.code() == arg) {
                Some(l) => {
                    self.s.lang = *l;
                    self.lines.push(String::from(l.tr().term_hello));
                }
                None => self.lines.push(String::from("lang it | en | es | fr | de")),
            },
            "settings" => self.launch(Launch::App(SETT), arch::now_ms()),
            "internet" | "browse" | "web" => {
                if !arg.is_empty() {
                    self.web.go(arg, self.dk());
                }
                self.launch(Launch::App(WEB), arch::now_ms());
            }
            "reboot" => self.action = Some(Power::Reboot),
            "shutdown" | "poweroff" => self.action = Some(Power::Off),
            _ => self.lines.push(format!("{} {}", t.term_unknown, name)),
        }
        if self.lines.len() > 200 {
            self.lines.drain(0..50);
        }
    }

    fn pwd(&self) -> String {
        let mut p = String::from("/");
        p.push_str(&self.cwd_path.join("/"));
        p
    }

    /// Cartella + nome finale di un percorso relativo alla cartella corrente.
    fn split_path<'a>(&self, path: &'a str) -> Option<(Dir, &'a str)> {
        match path.rfind('/') {
            Some(i) => Some((fat::resolve(self.cwd, if i == 0 { "/" } else { &path[..i] })?, &path[i + 1..])),
            None => Some((self.cwd, path)),
        }
    }

    /// Comandi per i file. Ritorna false se il comando non e' di questo tipo.
    fn fs_command(&mut self, name: &str, arg: &str) -> bool {
        if !matches!(name, "ls" | "dir" | "cd" | "pwd" | "cat" | "type" | "mkdir" | "rm" | "del" | "touch" | "edit" | "notepad" | "echo") {
            return false;
        }
        if name == "echo" && !arg.contains('>') {
            return false;
        }
        let dk = self.dk();
        if !fat::mounted() {
            self.lines.push(String::from(dk.no_fs));
            return true;
        }
        let err = |s: &mut Self, what: &str| s.lines.push(format!("{}: {}", what, arg));
        match name {
            "pwd" => self.lines.push(self.pwd()),
            "ls" | "dir" => {
                let dir = if arg.is_empty() { Some(self.cwd) } else { fat::resolve(self.cwd, arg) };
                match dir {
                    Some(d) => {
                        let items = fat::list(d);
                        for e in items.iter() {
                            if e.dir {
                                self.lines.push(format!("  {}/", e.name));
                            } else {
                                self.lines.push(format!("  {:<32} {:>8}", e.name, apps::human(e.size)));
                            }
                        }
                        self.lines.push(format!("{} {}, {} {}", items.len(), dk.items, apps::human(fat::free_bytes().min(u32::MAX as u64) as u32), dk.free_word));
                    }
                    None => err(self, "?"),
                }
            }
            "cd" => {
                let target = if arg.is_empty() { "/" } else { arg };
                match fat::resolve(self.cwd, target) {
                    Some(d) => {
                        if target.starts_with('/') {
                            self.cwd_path.clear();
                        }
                        for part in target.split('/').filter(|p| !p.is_empty() && *p != ".") {
                            if part == ".." {
                                self.cwd_path.pop();
                            } else {
                                let real = fat::find(self.cwd_path_dir(), part).map(|e| e.name).unwrap_or_else(|| String::from(part));
                                self.cwd_path.push(real);
                            }
                        }
                        self.cwd = d;
                    }
                    None => err(self, "?"),
                }
            }
            "cat" | "type" => match self.split_path(arg).and_then(|(d, n)| fat::find(d, n)) {
                Some(e) if !e.dir => {
                    let data = fat::read(&e);
                    let text = String::from_utf8_lossy(&data);
                    for l in text.lines().take(300) {
                        self.lines.push(l.replace('\r', "").replace('\t', "    "));
                    }
                }
                _ => err(self, "?"),
            },
            "mkdir" => match self.split_path(arg) {
                Some((d, n)) if fat::mkdir(d, n).is_some() => {}
                _ => err(self, dk.save_error),
            },
            "rm" | "del" => match self.split_path(arg).and_then(|(d, n)| fat::find(d, n)) {
                Some(e) => {
                    fat::delete(&e);
                }
                None => err(self, "?"),
            },
            "touch" => match self.split_path(arg) {
                Some((d, n)) if fat::find(d, n).is_some() || fat::write(d, n, b"") => {}
                _ => err(self, dk.save_error),
            },
            "edit" | "notepad" => {
                let now = arch::now_ms();
                if arg.is_empty() {
                    self.note = Note::new();
                } else if let Some((d, n)) = self.split_path(arg) {
                    match fat::find(d, n) {
                        Some(e) if !e.dir => self.note.open(d, &e),
                        _ => {
                            self.note = Note::new();
                            self.note.dir = Some(d);
                            self.note.name = String::from(n);
                        }
                    }
                }
                self.open(NOTE, now);
            }
            _ => {
                // echo testo > file
                let (text, file) = arg.split_once('>').unwrap_or((arg, ""));
                let mut data = String::from(text.trim());
                data.push('\n');
                match self.split_path(file.trim()) {
                    Some((d, n)) if !n.is_empty() && fat::write(d, n, data.as_bytes()) => {}
                    _ => err(self, dk.save_error),
                }
            }
        }
        true
    }

    fn cwd_path_dir(&self) -> Dir {
        fat::resolve(0, &self.cwd_path.join("/")).unwrap_or(0)
    }

    fn spec_rows(&self) -> [(&'static str, String); 9] {
        let (t, dk) = (self.t(), self.dk());
        let sys = SYS.get();
        [
            (dk.device, self.s.pc.clone()),
            (t.cpu, String::from(sys.0.trim())),
            ("Core", format!("{}", sys.1)),
            (t.ram, format!("{} MB", sys.2)),
            (t.screen, format!("{} × {}", sys.3, sys.4)),
            ("Disco", crate::block::info().map(|(k, mb)| format!("{}  ·  {} MB  ·  {} {}", k, mb, apps::human(fat::free_bytes().min(u32::MAX as u64) as u32), dk.free_word)).unwrap_or_else(|| String::from("-"))),
            (dk.ethernet, net::info().ip.map(|c| format!("{}", c.address())).unwrap_or_else(|| String::from("-"))),
            (dk.version, format!("MIN-OS {} (x86_64)", crate::VERSION)),
            (t.uptime, uptime()),
        ]
    }

    fn info(&mut self, ui: &mut Ui, x: i32, y: i32, w: i32) {
        draw_logo(ui.c, x, y, 56, ui.th.accent);
        ui.text(x + 74, y + 4, "MIN-OS", &font::LARGE);
        ui.sub(x + 74, y + 34, &format!("{}  ·  x86_64  ·  Rust", crate::VERSION), &font::BODY);
        spec_list(ui, x, y + 78, w, &self.spec_rows());
    }

    // ------------------------------------------------------------ Impostazioni (stile Windows 11)

    #[allow(clippy::too_many_arguments)]
    fn settings(&mut self, ui: &mut Ui, wall: &Canvas, x: i32, y: i32, w: i32, h: i32, typing: bool, now: u64) {
        let dk = self.dk();
        let side = 250.min(w / 3);
        // barra laterale
        let (ax, ay) = (x + 44, y + 44);
        avatar(ui, ax, ay, 26, &self.s.user);
        let name = ui::ellipsize(&self.s.user, side - 90, &font::BODY_B);
        ui.text(ax + 38, ay - 20, &name, &font::BODY_B);
        let lacc = ui::ellipsize(dk.local_acc, side - 90, &font::BODY);
        ui.sub(ax + 38, ay + 2, &lacc, &font::BODY);
        let icons = [IC_PC, IC_PAINT, IC_GLOBE, IC_USER, IC_INFO, IC_NET, IC_UPD];
        for (row, &i) in [0usize, 5, 1, 2, 3, 4, 6].iter().enumerate() {
            let (ix, iy, iw, ih) = (x + 12, y + 96 + row as i32 * 44, side - 24, 40);
            let sel = self.page == i;
            let hot = !ui.block && ui.hover(ix, iy, iw, ih);
            if sel || hot {
                ui.c.rrect(ix, iy, iw, ih, 10, ui.th.fill, ui.th.fill_a + if sel { 20 } else { 10 });
            }
            if sel {
                ui.c.rrect(ix, iy + 11, 4, 18, 2, ui.th.accent, 256);
            }
            icon(ui.c, icons[i], ix + 14, iy + 9, 22);
            let tc = ui.th.text;
            let lab = ui::ellipsize(dk.pages[i], iw - 60, &font::BODY);
            ui.c.text(ix + 48, iy + 12, &lab, if sel { &font::BODY_B } else { &font::BODY }, tc, 256);
            if ui.click(ix, iy, iw, ih) {
                self.page = i;
            }
        }
        let sep = if ui.th.dark { WHITE } else { BLACK };
        ui.c.rect(x + side, y + 12, 1, h - 24, sep, 20);

        // contenuto
        let (cx, cw) = (x + side + 24, w - side - 48);
        ui.text(cx, y + 18, dk.pages[self.page], &font::TITLE);
        let mut cy = y + 72;
        match self.page {
            0 => {
                let sys = SYS.get();
                card(ui, cx, cy, cw, 64, self.t().screen, dk.display_d);
                let res = format!("{} × {}", sys.3, sys.4);
                let tc = ui.th.text;
                ui.c.text_right(cx + cw - 20, cy + 24, &res, &font::BODY_B, tc, 256);
                cy += 72;
                card(ui, cx, cy, cw, 108, dk.perf, dk.perf_d);
                let t = self.t();
                if let Some(i) = seg(ui, cx + 18, cy + 60, cw - 36, 34, &t.presets, self.s.preset) {
                    self.s.preset = i;
                }
                cy += 116;
                card(ui, cx, cy, cw, 64, self.t().anim, dk.anim_d);
                if ui.toggle(cx + cw - 72, cy + 17, self.s.anim, true) {
                    self.s.anim = !self.s.anim;
                }
                cy += 72;
                card(ui, cx, cy, cw, 64, self.t().power, dk.power_d);
                let bw = 120;
                if ui.button(cx + cw - 20 - bw * 2 - 8, cy + 14, bw, 36, self.t().restart, false, true) {
                    self.action = Some(Power::Reboot);
                }
                if ui.primary(cx + cw - 20 - bw, cy + 14, bw, 36, self.t().shutdown, false, true) {
                    self.action = Some(Power::Off);
                }
                cy += 72;
                card(ui, cx, cy, cw, 64, dk.reset, dk.reset_d);
                if ui.button(cx + cw - 20 - 160, cy + 14, 160, 36, dk.reset_btn, false, true) {
                    storage::erase();
                    self.action = Some(Power::Reboot);
                }
            }
            1 => {
                // anteprima dello sfondo
                let (pw, ph) = (220, 138);
                ui.tile(cx, cy, cw, ph + 24, 14, false);
                thumbnail(ui.c, wall, cx + 12, cy + 12, pw, ph);
                let tc = ui.th.text;
                ui.c.rrect(cx + 12 + pw / 2 - 40, cy + 12 + ph - 14, 80, 8, 4, tc, 120);
                ui.text(cx + pw + 36, cy + 50, dk.wallpaper, &font::MED_B);
                let wd = ui::ellipsize(dk.wallpaper_d, cw - pw - 60, &font::BODY);
                ui.sub(cx + pw + 36, cy + 80, &wd, &font::BODY);
                cy += ph + 32;
                let t = self.t();
                card(ui, cx, cy, cw, 64, t.theme, dk.theme_d);
                if let Some(i) = seg(ui, cx + cw - 20 - 220, cy + 15, 220, 34, &[t.dark, t.light], if self.s.dark { 0 } else { 1 }) {
                    self.s.dark = i == 0;
                }
                cy += 72;
                card(ui, cx, cy, cw, 64, t.accent, dk.accent_d);
                for (i, &c) in ACCENTS.iter().enumerate() {
                    if ui.swatch(cx + cw - 36 - (5 - i as i32) * 38, cy + 32, 12, c, self.s.accent == i) {
                        self.s.accent = i;
                    }
                }
                cy += 72;
                card(ui, cx, cy, cw, 64, t.glass, dk.glass_d);
                if ui.toggle(cx + cw - 72, cy + 17, self.s.glass, true) {
                    self.s.glass = !self.s.glass;
                }
                cy += 72;
                card(ui, cx, cy, cw, 64, dk.icons, dk.icons_d);
                let cur = 2 - self.s.icon_size;
                if let Some(i) = seg(ui, cx + cw - 20 - 260, cy + 15, 260, 34, &dk.size_sml, cur) {
                    self.s.icon_size = 2 - i;
                }
            }
            2 => {
                let t = self.t();
                ui.tile(cx, cy, cw, 84, 14, false);
                let time = self.fmt_time(&self.clock);
                ui.text(cx + 20, cy + 14, &time, &font::TITLE);
                let full = format!(
                    "{} {} {} {}",
                    t.days[arch::weekday(&self.clock)],
                    self.clock.day,
                    t.months[(self.clock.month as usize).clamp(1, 12) - 1],
                    self.clock.year
                );
                ui.sub(cx + 20, cy + 54, &full, &font::BODY);
                let tc = ui.th.sub;
                ui.c.text_right(cx + cw - 20, cy + 32, dk.datetime, &font::BODY_B, tc, 256);
                cy += 92;
                card(ui, cx, cy, cw, 64, dk.clock24, dk.clock24_d);
                if ui.toggle(cx + cw - 72, cy + 17, self.s.clock24, true) {
                    self.s.clock24 = !self.s.clock24;
                }
                cy += 72;
                card(ui, cx, cy, cw, 108, t.language, dk.lang_d);
                let names: Vec<&str> = i18n::ALL.iter().map(|l| l.tr().name).collect();
                let cur = i18n::ALL.iter().position(|l| *l == self.s.lang).unwrap_or(0);
                if let Some(i) = seg(ui, cx + 18, cy + 60, cw - 36, 34, &names, cur) {
                    self.s.lang = i18n::ALL[i];
                }
                cy += 116;
                card(ui, cx, cy, cw, 108, t.keyboard, dk.kbd_d);
                let codes: Vec<&str> = keymap::ALL.iter().map(|l| l.code()).collect();
                let cur = keymap::ALL.iter().position(|l| *l == self.s.layout).unwrap_or(0);
                if let Some(i) = seg(ui, cx + 18, cy + 60, cw - 36, 34, &codes, cur) {
                    self.s.layout = keymap::ALL[i];
                }
            }
            3 => {
                let t = self.t();
                ui.tile(cx, cy, cw, 96, 14, false);
                avatar(ui, cx + 56, cy + 48, 32, &self.s.user);
                ui.text(cx + 104, cy + 26, &self.s.user, &font::LARGE);
                ui.sub(cx + 104, cy + 56, dk.local_acc, &font::BODY);
                cy += 112;
                ui.sub(cx + 2, cy, dk.acc_d, &font::BODY);
                cy += 30;
                let fw = cw.min(460);
                ui.text(cx, cy, t.user, &font::BODY_B);
                if ui.field(cx, cy + 26, fw, 48, &self.s.user, "", typing && self.acc_focus == 0) {
                    self.acc_focus = 0;
                }
                ui.text(cx, cy + 96, t.pc, &font::BODY_B);
                if ui.field(cx, cy + 122, fw, 48, &self.s.pc, "", typing && self.acc_focus == 1) {
                    self.acc_focus = 1;
                }
                if typing {
                    for k in ui.ev.keys.iter() {
                        let target = if self.acc_focus == 0 { &mut self.s.user } else { &mut self.s.pc };
                        match k {
                            Key::Char(c) if target.chars().count() < 20 && !(self.acc_focus == 1 && *c == ' ') => target.push(*c),
                            Key::Backspace if target.chars().count() > 1 => {
                                target.pop();
                            }
                            Key::Tab | Key::Enter | Key::Down | Key::Up => self.acc_focus = 1 - self.acc_focus,
                            _ => {}
                        }
                    }
                }
            }
            6 => self.updates_page(ui, cx, cy, cw),
            5 => {
                let ni = net::info();
                ui.tile(cx, cy, cw, 96, 14, false);
                icon(ui.c, IC_NET, cx + 20, cy + 20, 56);
                if !ni.present {
                    ui.text(cx + 96, cy + 22, dk.ethernet, &font::LARGE);
                    ui.sub(cx + 96, cy + 54, dk.no_adapter, &font::BODY);
                } else {
                    ui.text(cx + 96, cy + 22, dk.ethernet, &font::LARGE);
                    let st = match ni.ip {
                        Some(ip) => format!("{}  ·  {}", dk.connected, ip.address()),
                        None => String::from(if ni.link { dk.waiting_dhcp } else { dk.disconnected }),
                    };
                    let st = ui::ellipsize(&st, cw - 260, &font::BODY);
                    ui.sub(cx + 96, cy + 54, &st, &font::BODY);
                    let (label, col) = if ni.ip.is_some() { (dk.connected, 0x30D158) } else { (dk.disconnected, 0x8E8E93) };
                    let pw = font::text_width(label, &font::BODY_B) + 36;
                    ui.c.rrect(cx + cw - 20 - pw, cy + 32, pw, 32, 16, col, 60);
                    ui.c.circle(cx + cw - 20 - pw + 16, cy + 48, 5, col, 256);
                    let tc = ui.th.text;
                    ui.c.text(cx + cw - 20 - pw + 28, cy + 40, label, &font::BODY_B, tc, 256);
                    cy += 108;
                    let opt = |v: Option<String>| v.unwrap_or_else(|| String::from("-"));
                    let dns = if ni.dns.is_empty() { String::from("-") } else { ni.dns.iter().map(|a| format!("{}", a)).collect::<Vec<_>>().join(", ") };
                    let rows: [(&str, String); 6] = [
                        (dk.adapter, String::from(ni.name)),
                        (dk.mac_addr, net::mac_string(&ni.mac)),
                        (dk.ip_addr, opt(ni.ip.map(|c| format!("{}", c)))),
                        (dk.gateway, opt(ni.gateway.map(|g| format!("{}", g)))),
                        (dk.dns_srv, dns),
                        (dk.packets, format!("{} / {}", ni.rx, ni.tx)),
                    ];
                    spec_list(ui, cx, cy, cw, &rows);
                    ui.sub(cx + 4, cy + 6 * 40 + 22, dk.net_d, &font::BODY);
                }
            }
            _ => {
                ui.tile(cx, cy, cw, 88, 14, false);
                draw_logo(ui.c, cx + 18, cy + 16, 56, ui.th.accent);
                ui.text(cx + 92, cy + 20, &self.s.pc, &font::LARGE);
                ui.sub(cx + 92, cy + 52, &format!("MIN-OS {}", crate::VERSION), &font::BODY);
                cy += 100;
                spec_list(ui, cx, cy, cw, &self.spec_rows());
            }
        }
        let _ = now;
    }
}

// ------------------------------------------------------------------ widget del desktop

fn draw_menu_panel(ui: &mut Ui, r: R, items: &[Item]) -> (Option<usize>, Option<usize>) {
    ui.panel_strong(r.x, r.y, r.w, r.h, 14);
    let mut hov = None;
    let mut clk = None;
    let mut iy = r.y + MPAD;
    for (i, it) in items.iter().enumerate() {
        if it.sep {
            let sep = if ui.th.dark { WHITE } else { BLACK };
            ui.c.rect(r.x + 12, iy + SEP_H / 2, r.w - 24, 1, sep, 30);
            iy += SEP_H;
        }
        let (ix, iw) = (r.x + MPAD, r.w - MPAD * 2);
        let hot = !ui.block && ui.hover(ix, iy, iw, IH);
        if hot {
            hov = Some(i);
            ui.c.rrect(ix, iy, iw, IH, 8, ui.th.fill, ui.th.fill_a + 22);
        }
        let tc = ui.th.text;
        if it.check {
            check_mark(ui.c, ix + 20, iy + IH / 2, 11.0, ui.th.accent);
        }
        let lab = ui::ellipsize(&it.label, iw - 70, &font::BODY);
        ui.c.text(ix + 40, iy + 9, &lab, &font::BODY, tc, 256);
        if !it.sub.is_empty() {
            let (cx, cy) = ((ix + iw - 18) as f32, (iy + IH / 2) as f32);
            ui.c.line(cx - 2.0, cy - 4.5, cx + 2.5, cy, 1.6, tc, 220);
            ui.c.line(cx + 2.5, cy, cx - 2.0, cy + 4.5, 1.6, tc, 220);
        }
        if ui.click(ix, iy, iw, IH) {
            clk = Some(i);
        }
        iy += IH;
    }
    (hov, clk)
}

/// Scheda delle Impostazioni: titolo e descrizione a sinistra, controllo a destra.
fn card(ui: &mut Ui, x: i32, y: i32, w: i32, h: i32, title: &str, desc: &str) {
    ui.tile(x, y, w, h, 14, false);
    let ty = if h > 70 { y + 14 } else { y + (h - 38) / 2 };
    ui.text(x + 20, ty, title, &font::BODY_B);
    let d = ui::ellipsize(desc, w / 2, &font::BODY);
    ui.sub(x + 20, ty + 21, &d, &font::BODY);
}

/// Controllo segmentato: ritorna l'indice cliccato.
fn seg(ui: &mut Ui, x: i32, y: i32, w: i32, h: i32, labels: &[&str], sel: usize) -> Option<usize> {
    let n = labels.len() as i32;
    let gap = 6;
    let bw = (w - gap * (n - 1)) / n;
    let mut out = None;
    for (i, l) in labels.iter().enumerate() {
        let bx = x + i as i32 * (bw + gap);
        let lab = ui::ellipsize(l, bw - 16, &font::BODY_B);
        let clicked = if i == sel { ui.primary(bx, y, bw, h, &lab, false, true) } else { ui.button(bx, y, bw, h, &lab, false, true) };
        if clicked {
            out = Some(i);
        }
    }
    out
}

fn spec_list(ui: &mut Ui, x: i32, y: i32, w: i32, rows: &[(&str, String)]) {
    let rh = 40;
    ui.tile(x, y, w, rh * rows.len() as i32 + 8, 14, false);
    for (i, (k, v)) in rows.iter().enumerate() {
        let yy = y + 4 + i as i32 * rh;
        if i > 0 {
            let sep = if ui.th.dark { WHITE } else { BLACK };
            ui.c.rect(x + 16, yy, w - 32, 1, sep, 22);
        }
        let sc = ui.th.sub;
        ui.c.text(x + 18, yy + 12, k, &font::BODY, sc, 256);
        let v = ui::ellipsize(v, w - 190, &font::BODY_B);
        let tc = ui.th.text;
        ui.c.text_right(x + w - 18, yy + 12, &v, &font::BODY_B, tc, 256);
    }
}

fn avatar(ui: &mut Ui, cx: i32, cy: i32, r: i32, name: &str) {
    let a = ui.th.accent;
    ui.c.rrect_dgrad(cx - r, cy - r, r * 2, r * 2, r, mix(a, WHITE, 70), mix(a, rgb(255, 80, 160), 110), 256);
    ui.c.circle_border(cx, cy, r, WHITE, 120);
    let mut initial = String::new();
    if let Some(c) = name.chars().next() {
        for u in c.to_uppercase() {
            initial.push(u);
        }
    }
    let f = if r >= 26 { &font::TITLE } else if r >= 16 { &font::MED_B } else { &font::BODY_B };
    ui.c.text_center(cx, cy - f.height() / 2 - 1, &initial, f, WHITE, 256);
}

/// Miniatura dello sfondo (campionamento semplice).
fn thumbnail(c: &mut Canvas, src: &Canvas, x: i32, y: i32, w: i32, h: i32) {
    let r = 10;
    c.shape(x, y, w, h, r, |px, py, p, cov| {
        let sx = (px - x) * src.w / w;
        let sy = (py - y) * src.h / h;
        let v = src.px[(sy.clamp(0, src.h - 1) * src.w + sx.clamp(0, src.w - 1)) as usize];
        *p = blend(*p, v, cov);
    });
    c.rrect_border(x, y, w, h, r, WHITE, 90);
}

/// Icone disegnate interamente in codice.
pub fn icon(c: &mut Canvas, i: usize, x: i32, y: i32, s: i32) {
    let r = s * 28 / 100;
    let (c1, c2) = match i {
        IC_TERM => (rgb(70, 72, 84), rgb(18, 18, 24)),
        IC_INFO => (rgb(90, 200, 250), rgb(10, 100, 255)),
        IC_SETT => (rgb(190, 192, 204), rgb(110, 112, 128)),
        IC_POWER => (rgb(255, 120, 100), rgb(235, 40, 70)),
        IC_PC => (rgb(110, 170, 255), rgb(70, 60, 220)),
        IC_BIN => (rgb(150, 160, 178), rgb(78, 86, 104)),
        IC_PAINT => (rgb(255, 170, 90), rgb(240, 70, 140)),
        IC_NET => (rgb(90, 160, 255), rgb(30, 80, 230)),
        IC_FOLDER => (rgb(110, 190, 255), rgb(30, 110, 240)),
        IC_NOTE => (rgb(255, 214, 90), rgb(245, 160, 30)),
        IC_CALC => (rgb(70, 72, 84), rgb(20, 20, 26)),
        IC_GLOBE => (rgb(80, 220, 200), rgb(10, 130, 170)),
        IC_WEB => (rgb(120, 200, 255), rgb(70, 60, 230)),
        IC_UPD => (rgb(90, 220, 170), rgb(20, 140, 200)),
        _ => (rgb(120, 225, 140), rgb(30, 160, 90)),
    };
    if s >= 30 {
        c.shadow(x, y, s, s, r, s / 6, 80);
    }
    c.rrect_dgrad(x, y, s, s, r, c1, c2, 256);
    let hh = s / 2;
    c.shape(x, y, s, hh, r, |_, py, p, cov| {
        let t = ((hh - (py - y)) * 256 / hh.max(1)) as u32;
        *p = blend(*p, WHITE, cov * (t * 60 >> 8) >> 8);
    });
    c.rrect_border(x, y, s, s, r, WHITE, 120);
    let (cx, cy) = ((x + s / 2) as f32, (y + s / 2) as f32);
    let k = s as f32 / 58.0;
    let th = (3.4 * k).max(1.3);
    match i {
        IC_TERM => {
            c.line(cx - 14.0 * k, cy - 9.0 * k, cx - 4.0 * k, cy, th, rgb(120, 255, 160), 256);
            c.line(cx - 4.0 * k, cy, cx - 14.0 * k, cy + 9.0 * k, th, rgb(120, 255, 160), 256);
            c.line(cx + 1.0 * k, cy + 10.0 * k, cx + 14.0 * k, cy + 10.0 * k, th, WHITE, 256);
        }
        IC_INFO => {
            c.circle(cx as i32, (cy - 11.0 * k) as i32, ((3.4 * k) as i32).max(1), WHITE, 256);
            c.line(cx, cy - 2.0 * k, cx, cy + 13.0 * k, (5.6 * k).max(1.8), WHITE, 256);
        }
        IC_SETT => {
            for &(dx, dy) in ui::SPIN.iter().step_by(2) {
                c.line(cx + dx * 9.0 * k, cy + dy * 9.0 * k, cx + dx * 17.0 * k, cy + dy * 17.0 * k, (6.0 * k).max(1.6), WHITE, 256);
            }
            c.circle(cx as i32, cy as i32, ((13.0 * k) as i32).max(3), WHITE, 256);
            c.circle(cx as i32, cy as i32, ((6.0 * k) as i32).max(1), rgb(130, 132, 148), 256);
        }
        IC_POWER => {
            ring_arc(c, cx, cy, 14.0 * k, th, WHITE);
            c.line(cx, cy - 17.0 * k, cx, cy - 4.0 * k, th, WHITE, 256);
        }
        IC_PC => {
            let (w2, h2) = ((17.0 * k) as i32, (11.0 * k) as i32);
            c.rrect(cx as i32 - w2, (cy - 14.0 * k) as i32, w2 * 2, h2 * 2, (3.0 * k) as i32, WHITE, 245);
            c.rrect_vgrad(cx as i32 - w2 + 3, (cy - 14.0 * k) as i32 + 3, w2 * 2 - 6, h2 * 2 - 6, (2.0 * k) as i32, rgb(90, 140, 255), rgb(60, 40, 170), 256);
            c.line(cx, cy + 8.0 * k, cx, cy + 13.0 * k, th, WHITE, 256);
            c.line(cx - 8.0 * k, cy + 14.0 * k, cx + 8.0 * k, cy + 14.0 * k, th, WHITE, 256);
        }
        IC_BIN => {
            let p = [(-10.0, -6.0), (-8.0, 15.0), (8.0, 15.0), (10.0, -6.0)];
            for j in 0..3 {
                c.line(cx + p[j].0 * k, cy + p[j].1 * k, cx + p[j + 1].0 * k, cy + p[j + 1].1 * k, th, WHITE, 256);
            }
            c.line(cx - 14.0 * k, cy - 10.0 * k, cx + 14.0 * k, cy - 10.0 * k, th, WHITE, 256);
            c.line(cx - 4.0 * k, cy - 14.0 * k, cx + 4.0 * k, cy - 14.0 * k, th, WHITE, 256);
            c.line(cx - 3.0 * k, cy - 1.0 * k, cx - 3.0 * k, cy + 9.0 * k, th * 0.7, WHITE, 220);
            c.line(cx + 3.0 * k, cy - 1.0 * k, cx + 3.0 * k, cy + 9.0 * k, th * 0.7, WHITE, 220);
        }
        IC_PAINT => {
            c.circle(cx as i32, cy as i32, ((16.0 * k) as i32).max(4), WHITE, 245);
            let dots = [(-6.0, -6.0, rgb(10, 132, 255)), (6.0, -6.0, rgb(48, 209, 88)), (-7.0, 5.0, rgb(255, 55, 95)), (5.0, 7.0, rgb(255, 180, 0))];
            for (dx, dy, col) in dots {
                c.circle((cx + dx * k) as i32, (cy + dy * k) as i32, ((3.6 * k) as i32).max(1), col, 256);
            }
        }
        IC_GLOBE => {
            ellipse(c, cx, cy, 15.0 * k, 15.0 * k, th * 0.8, WHITE);
            ellipse(c, cx, cy, 6.5 * k, 15.0 * k, th * 0.7, WHITE);
            c.line(cx - 15.0 * k, cy, cx + 15.0 * k, cy, th * 0.7, WHITE, 256);
        }
        IC_NET => wifi(c, cx, cy + 2.0 * k, 34.0 * k, WHITE, 256),
        IC_UPD => {
            // due frecce in cerchio
            let r = 14.0 * k;
            let steps = 14;
            for half in 0..2 {
                let a0 = if half == 0 { 0.35f32 } else { 3.49 };
                let mut prev: Option<(f32, f32)> = None;
                for s in 0..=steps {
                    let a = a0 + s as f32 * 2.45 / steps as f32;
                    let (sn, cs) = sincos(a);
                    let p = (cx + r * cs, cy + r * sn);
                    if let Some(q) = prev {
                        c.line(q.0, q.1, p.0, p.1, th, WHITE, 256);
                    }
                    prev = Some(p);
                }
                if let Some((ex, ey)) = prev {
                    let a = a0 + 2.45;
                    let (sn, cs) = sincos(a);
                    // punta della freccia (tangente al cerchio)
                    let (tx, ty) = (-sn, cs);
                    let (nx, ny) = (cs, sn);
                    let l = 6.0 * k;
                    c.line(ex, ey, ex - tx * l + nx * l, ey - ty * l + ny * l, th, WHITE, 256);
                    c.line(ex, ey, ex - tx * l - nx * l, ey - ty * l - ny * l, th, WHITE, 256);
                }
            }
        }
        IC_WEB => {
            ellipse(c, cx, cy, 16.0 * k, 16.0 * k, th * 0.8, WHITE);
            ellipse(c, cx, cy, 7.0 * k, 16.0 * k, th * 0.7, WHITE);
            c.line(cx - 16.0 * k, cy, cx + 16.0 * k, cy, th * 0.7, WHITE, 256);
            c.line(cx - 13.5 * k, cy - 8.0 * k, cx + 13.5 * k, cy - 8.0 * k, th * 0.55, WHITE, 220);
            c.line(cx - 13.5 * k, cy + 8.0 * k, cx + 13.5 * k, cy + 8.0 * k, th * 0.55, WHITE, 220);
        }
        IC_FOLDER => {
            let (w2, h2) = ((32.0 * k) as i32, (24.0 * k) as i32);
            let (fx, fy) = (cx as i32 - w2 / 2, (cy - 9.0 * k) as i32);
            c.rrect(fx, fy - (4.0 * k) as i32, w2 * 2 / 5, (8.0 * k) as i32, (2.5 * k) as i32, rgb(255, 225, 140), 256);
            c.rrect_vgrad(fx, fy, w2, h2, (4.0 * k) as i32, rgb(255, 220, 110), rgb(250, 180, 50), 256);
        }
        IC_NOTE => {
            let (w2, h2) = ((26.0 * k) as i32, (32.0 * k) as i32);
            let (nx, ny) = (cx as i32 - w2 / 2, cy as i32 - h2 / 2);
            c.rrect(nx, ny, w2, h2, (4.0 * k) as i32, WHITE, 250);
            for i in 0..4 {
                let ly = ny + (8.0 * k) as i32 + i * (6.0 * k) as i32;
                c.rect(nx + (5.0 * k) as i32, ly, w2 - (10.0 * k) as i32, (2.0 * k).max(1.0) as i32, rgb(200, 150, 40), 256);
            }
        }
        IC_CALC => {
            let b = (11.0 * k) as i32;
            let g = (4.0 * k) as i32;
            let ox = cx as i32 - b - g / 2;
            let oy = cy as i32 - b - g / 2;
            for (i, col) in [rgb(165, 165, 170), rgb(255, 159, 10), rgb(90, 90, 98), rgb(255, 159, 10)].iter().enumerate() {
                let (bx, by) = (ox + (i as i32 % 2) * (b + g), oy + (i as i32 / 2) * (b + g));
                c.rrect(bx, by, b, b, b / 2, *col, 256);
            }
        }
        _ => {
            c.circle(cx as i32, (cy - 6.0 * k) as i32, ((7.5 * k) as i32).max(2), WHITE, 250);
            let (bw, bh) = ((28.0 * k) as i32, (12.0 * k) as i32);
            c.shape(cx as i32 - bw / 2, (cy + 4.0 * k) as i32, bw, bh * 2, bh, |_, py, p, cov| {
                if py < (cy + 4.0 * k) as i32 + bh {
                    *p = blend(*p, WHITE, cov * 250 >> 8);
                }
            });
        }
    }
}

/// Simbolo della rete: un punto e due archi.
fn wifi(c: &mut Canvas, cx: f32, cy: f32, s: f32, col: u32, a: u32) {
    let base = cy + s * 0.32;
    c.circle(cx as i32, base as i32, ((s * 0.07) as i32).max(1), col, a);
    let (cs, sn) = (0.984_808, 0.173_648);
    for r in [s * 0.38, s * 0.68] {
        // da 225 a 315 gradi (in alto)
        let (mut x, mut y) = (-0.707_107f32, -0.707_107f32);
        for _ in 0..9 {
            let (nx, ny) = (x * cs - y * sn, x * sn + y * cs);
            c.line(cx + x * r, base + y * r, cx + nx * r, base + ny * r, (s * 0.1).max(1.4), col, a);
            x = nx;
            y = ny;
        }
    }
}

/// Ellisse (contorno) fatta di piccoli segmenti.
fn ellipse(c: &mut Canvas, cx: f32, cy: f32, rx: f32, ry: f32, thick: f32, col: u32) {
    let (cs, sn) = (0.984_808, 0.173_648);
    let (mut x, mut y) = (1.0f32, 0.0f32);
    for _ in 0..36 {
        let (nx, ny) = (x * cs - y * sn, x * sn + y * cs);
        c.line(cx + x * rx, cy + y * ry, cx + nx * rx, cy + ny * ry, thick, col, 256);
        x = nx;
        y = ny;
    }
}

/// Arco circolare aperto in alto (simbolo di accensione).
fn ring_arc(c: &mut Canvas, cx: f32, cy: f32, r: f32, thick: f32, col: u32) {
    let (cs, sn) = (0.984_808, 0.173_648);
    let (mut x, mut y) = (1.0f32, 0.0f32);
    for _ in 0..36 {
        let (nx, ny) = (x * cs - y * sn, x * sn + y * cs);
        if (y + ny) * 0.5 > -0.80 || ((x + nx) * 0.5).abs() > 0.62 {
            c.line(cx + x * r, cy + y * r, cx + nx * r, cy + ny * r, thick, col, 256);
        }
        x = nx;
        y = ny;
    }
}

fn uptime() -> String {
    let s = arch::now_ms() / 1000;
    format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// Informazioni di sistema accessibili da tutto il desktop.
pub struct SysCell(core::cell::UnsafeCell<(String, u32, u64, i32, i32)>);
unsafe impl Sync for SysCell {}
impl SysCell {
    pub fn get(&self) -> &(String, u32, u64, i32, i32) {
        unsafe { &*self.0.get() }
    }
    pub fn set(&self, v: (String, u32, u64, i32, i32)) {
        unsafe { *self.0.get() = v }
    }
}
pub static SYS: SysCell = SysCell(core::cell::UnsafeCell::new((String::new(), 0, 0, 0, 0)));

fn sincos(a: f32) -> (f32, f32) {
    let mut x = a;
    while x > core::f32::consts::PI {
        x -= 2.0 * core::f32::consts::PI;
    }
    let x2 = x * x;
    let s = x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0 * (1.0 - x2 / 72.0))));
    let c = 1.0 - x2 / 2.0 * (1.0 - x2 / 12.0 * (1.0 - x2 / 30.0 * (1.0 - x2 / 56.0)));
    (s, c)
}
