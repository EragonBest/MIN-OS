//! App Internet di MIN-OS: un piccolo browser testuale per siti http:// e https://
//! con link cliccabili, ricerca (FrogFind) e download nella cartella Download.
//!
//! HTTPS con TLS 1.3 (vedi net.rs); se un sito non lo accetta, la pagina viene
//! aperta in versione leggera attraverso FrogFind.

use crate::apps::{human, unique_name, DOWNLOADS};
use crate::fat;
use crate::font::{self, Font};
use crate::gfx::{mix, BLACK, WHITE};
use crate::i18n_desk::Dk;
use crate::input::Key;
use crate::net;
use crate::ui::{self, Ui};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const SEARCH: &str = "https://html.duckduckgo.com/html/?q=";
const PROXY: &str = "http://frogfind.com/read.php?a=";
const PARSE_LIMIT: usize = 2 * 1024 * 1024;

/// Siti della pagina iniziale (funzionano senza HTTPS).
const SITES: [&str; 6] = ["google.com", "duckduckgo.com", "it.m.wikipedia.org", "68k.news", "frogfind.com", "info.cern.ch"];

// ------------------------------------------------------------------ modello della pagina

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    P,
    H1,
    H2,
    H3,
    Li,
    Pre,
    Hr,
    Quote,
}

struct Run {
    text: String,
    link: Option<u16>,
    bold: bool,
    /// casella di testo di un modulo (indice in Page::fields)
    field: Option<u16>,
}

struct Block {
    kind: Kind,
    runs: Vec<Run>,
}

impl Block {
    fn has_text(&self) -> bool {
        self.kind == Kind::Hr || self.runs.iter().any(|r| r.field.is_some() || r.text.chars().any(|c| !c.is_whitespace()))
    }
}

#[derive(Default)]
struct Page {
    title: String,
    blocks: Vec<Block>,
    links: Vec<String>,
    forms: Vec<Form>,
    fields: Vec<Field>,
}

/// Modulo HTML (es. la casella di ricerca di Google o Wikipedia).
struct Form {
    action: String,
}

struct Field {
    form: Option<u16>,
    name: String,
    value: String,
    placeholder: String,
    hidden: bool,
    w: i32,
}

/// I pulsanti "invia" sono link speciali che iniziano con questo carattere.
const SUBMIT: char = '\u{1}';

struct Item {
    x: i32,
    w: i32,
    text: String,
    link: Option<u16>,
    bold: bool,
    field: Option<u16>,
}

struct LLine {
    y: i32,
    h: i32,
    kind: Kind,
    bullet: bool,
    items: Vec<Item>,
}

pub enum WebAct {
    None,
    OpenDownloads,
}

struct Loading {
    url: String,
    redirects: u8,
    download: bool,
    t: u64,
}

pub struct Web {
    pub url: String,
    addr: String,
    addr_focus: bool,
    /// casella di un modulo in cui si sta scrivendo
    focus: Option<u16>,
    addr_fresh: bool,
    hist: Vec<String>,
    loading: Option<Loading>,
    page: Page,
    raw: Vec<u8>,
    ctype: String,
    lines: Vec<LLine>,
    layout_w: i32,
    height: i32,
    scroll: i32,
    error: Option<String>,
    saved: Option<(String, u32, u64)>,
    msg: Option<(String, u64)>,
}

impl Web {
    pub fn new() -> Web {
        Web {
            url: String::new(),
            addr: String::new(),
            addr_focus: true,
            focus: None,
            addr_fresh: false,
            hist: Vec::new(),
            loading: None,
            page: Page::default(),
            raw: Vec::new(),
            ctype: String::new(),
            lines: Vec::new(),
            layout_w: 0,
            height: 0,
            scroll: 0,
            error: None,
            saved: None,
            msg: None,
        }
    }

    pub fn busy(&self) -> bool {
        self.loading.is_some()
    }

    /// Titolo da mostrare nella barra della finestra.
    pub fn title(&self) -> &str {
        &self.page.title
    }

    // ------------------------------------------------------------ navigazione

    /// Apre quello che l'utente ha scritto: indirizzo o ricerca.
    pub fn go(&mut self, input: &str, dk: &'static Dk) {
        let t = input.trim();
        if t.is_empty() {
            self.home();
            return;
        }
        let url = to_url(t);
        if !self.url.is_empty() && self.url != url {
            self.hist.push(self.url.clone());
        }
        self.start(url, false, dk);
    }

    fn home(&mut self) {
        if !self.url.is_empty() {
            self.hist.push(self.url.clone());
        }
        net::fetch_cancel(1);
        self.loading = None;
        self.url.clear();
        self.addr.clear();
        self.addr_focus = true;
        self.addr_fresh = false;
        self.page = Page::default();
        self.raw.clear();
        self.lines.clear();
        self.layout_w = 0;
        self.error = None;
        self.scroll = 0;
    }

    fn back(&mut self, dk: &'static Dk) {
        match self.hist.pop() {
            Some(u) if !u.is_empty() => {
                self.start(u, false, dk);
            }
            Some(_) | None => {
                let h = core::mem::take(&mut self.hist);
                self.home();
                self.hist = h;
                self.hist.pop();
            }
        }
    }

    fn start(&mut self, url: String, download: bool, dk: &'static Dk) {
        self.addr = url.clone();
        self.addr_focus = false;
        self.addr_fresh = false;
        self.error = None;
        match net::fetch(&url, dk, 1) {
            Ok(()) => self.loading = Some(Loading { url, redirects: 0, download, t: crate::arch::now_ms() }),
            Err(e) => {
                self.loading = None;
                self.url = url;
                self.show_error(e);
            }
        }
    }

    fn stop(&mut self) {
        net::fetch_cancel(1);
        self.loading = None;
        self.addr = self.url.clone();
    }

    fn show_error(&mut self, e: String) {
        self.error = Some(e);
        self.page = Page::default();
        self.raw.clear();
        self.lines.clear();
        self.layout_w = 0;
        self.scroll = 0;
    }

    /// Apre un file .html del disco (dall'app File).
    pub fn open_local(&mut self, name: &str, data: Vec<u8>) {
        net::fetch_cancel(1);
        self.loading = None;
        if !self.url.is_empty() {
            self.hist.push(self.url.clone());
        }
        self.url = format!("file:{}", name);
        self.addr = self.url.clone();
        self.addr_focus = false;
        self.error = None;
        self.ctype = String::from("text/html");
        self.set_page(data);
    }

    fn set_page(&mut self, body: Vec<u8>) {
        let text = decode_text(&body[..body.len().min(PARSE_LIMIT)]);
        let html = self.ctype.contains("html") || (self.ctype.is_empty() && looks_html(&text));
        self.page = if html { parse_html(&text, &self.url) } else { plain_page(&text) };
        if self.page.title.is_empty() {
            self.page.title = String::from(host_of(&self.url));
        }
        self.raw = body;
        self.focus = None;
        self.lines.clear();
        self.layout_w = 0;
        self.scroll = 0;
    }

    /// Controlla il download in corso (chiamata a ogni frame).
    fn poll(&mut self, dk: &'static Dk, now: u64) {
        let Some(ld) = &self.loading else { return };
        if net::fetch_owner() != 1 {
            // il download e' stato preso dagli aggiornamenti di sistema
            self.loading = None;
            self.addr = self.url.clone();
            self.msg = Some((String::from(dk.page_error), now));
            return;
        }
        let Some(res) = net::fetch_take(1) else { return };
        let (url, redirects, download) = (ld.url.clone(), ld.redirects, ld.download);
        self.loading = None;
        match res {
            Err((e, tls_failed)) => {
                // il sito non accetta la nostra connessione sicura: versione leggera via FrogFind
                if tls_failed && scheme(&url).0 == "https://" && !download && net::fetch(&proxied(&url), dk, 1).is_ok() {
                    let p = proxied(&url);
                    self.addr = p.clone();
                    self.msg = Some((String::from(dk.lite_mode), now + 6000));
                    self.loading = Some(Loading { url: p, redirects, download, t: now });
                    return;
                }
                self.url = url;
                self.addr = self.url.clone();
                self.show_error(e);
            }
            Ok(r) => {
                if (300..400).contains(&r.status) && !r.location.is_empty() && redirects < 6 {
                    let next = to_url(&resolve(&url, &r.location));
                    if net::fetch(&next, dk, 1).is_ok() {
                        self.addr = next.clone();
                        self.loading = Some(Loading { url: next, redirects: redirects + 1, download, t: now });
                        return;
                    }
                }
                let textual = r.ctype.is_empty() || r.ctype.starts_with("text/") || r.ctype.contains("html") || r.ctype.contains("xml") || r.ctype.contains("json");
                if download || !textual {
                    // file da scaricare: la pagina corrente resta com'era
                    let name = file_name(&url, &r.ctype);
                    self.save(&name, &r.body, dk, now);
                    self.addr = self.url.clone();
                    return;
                }
                if r.truncated {
                    self.msg = Some((format!("{} ({})", dk.page_error, human(r.body.len() as u32)), now));
                }
                self.url = url;
                self.addr = self.url.clone();
                self.ctype = r.ctype;
                self.error = None;
                self.set_page(r.body);
            }
        }
    }

    fn save(&mut self, name: &str, data: &[u8], dk: &Dk, now: u64) {
        let ok = fat::mounted()
            && match fat::ensure_dir(DOWNLOADS) {
                Some(d) => {
                    let n = unique_name(d, name);
                    if fat::write(d, &n, data) {
                        self.saved = Some((n, data.len() as u32, now));
                        true
                    } else {
                        false
                    }
                }
                None => false,
            };
        if !ok {
            self.msg = Some((String::from(if fat::mounted() { dk.save_error } else { dk.no_fs }), now));
        }
    }

    fn download_current(&mut self, dk: &'static Dk, now: u64) {
        if self.url.is_empty() || self.raw.is_empty() || self.url.starts_with("file:") {
            return;
        }
        let name = file_name(&self.url, &self.ctype);
        let data = core::mem::take(&mut self.raw);
        self.save(&name, &data, dk, now);
        self.raw = data;
    }

    // ------------------------------------------------------------ impaginazione

    fn layout(&mut self, width: i32) {
        self.lines.clear();
        let mut y = 0;
        let mut prev: Option<Kind> = None;
        for b in self.page.blocks.iter() {
            let gap = match (prev, b.kind) {
                (None, _) => 0,
                (_, Kind::H1) => 22,
                (_, Kind::H2) => 18,
                (_, Kind::H3) => 14,
                (Some(Kind::Li), Kind::Li) => 4,
                _ => 10,
            };
            y += gap;
            prev = Some(b.kind);
            if b.kind == Kind::Hr {
                self.lines.push(LLine { y, h: 14, kind: Kind::Hr, bullet: false, items: Vec::new() });
                y += 14;
                continue;
            }
            let (lh, indent) = match b.kind {
                Kind::H1 => (34, 0),
                Kind::H2 => (29, 0),
                Kind::H3 => (25, 0),
                Kind::Li => (23, 24),
                Kind::Quote => (23, 22),
                Kind::Pre => (20, 10),
                _ => (23, 0),
            };
            let maxw = (width - indent).max(60);
            let mut line = LLine { y, h: lh, kind: b.kind, bullet: b.kind == Kind::Li, items: Vec::new() };
            let mut x = 0;
            let pre = b.kind == Kind::Pre;
            for run in b.runs.iter() {
                let f = block_font(b.kind, run.bold);
                let sp = font::char_width(' ', &f);
                if let Some(fid) = run.field {
                    // casella di testo: un blocco unico che non va a capo
                    let fw = self.page.fields.get(fid as usize).map(|f| f.w).unwrap_or(260).min(maxw);
                    if x > 0 && x + fw > maxw {
                        y += lh;
                        self.lines.push(core::mem::replace(&mut line, LLine { y, h: lh, kind: b.kind, bullet: false, items: Vec::new() }));
                        x = 0;
                    }
                    line.items.push(Item { x, w: fw, text: String::new(), link: None, bold: false, field: Some(fid) });
                    x += fw + 18;
                    continue;
                }
                let pieces: Vec<&str> = if pre { run.text.split_inclusive('\n').collect() } else { run.text.split_inclusive(|c| c == ' ' || c == '\n').collect() };
                for piece in pieces {
                    let newline = piece.ends_with('\n');
                    let word = piece.trim_end_matches('\n');
                    let core_w = if pre { mono_w(word.trim_end()) } else { adv(word.trim_end(), &f) };
                    let trailing = if pre { 0 } else { (word.len() - word.trim_end().len()) as i32 * sp };
                    if !word.is_empty() {
                        if x > 0 && x + core_w > maxw {
                            y += lh;
                            self.lines.push(core::mem::replace(&mut line, LLine { y, h: lh, kind: b.kind, bullet: false, items: Vec::new() }));
                            x = 0;
                        }
                        let word = if x == 0 && !pre { word.trim_start() } else { word };
                        if core_w > maxw {
                            // parola troppo lunga: va a capo carattere per carattere
                            let mut chunk = String::new();
                            let mut cw = 0;
                            for c in word.chars() {
                                let w = if pre { mono_w("a") } else { font::char_width(c, &f) };
                                if x + cw + w > maxw && !chunk.is_empty() {
                                    push_item(&mut line, x, cw, &chunk, run.link, run.bold, sp);
                                    y += lh;
                                    self.lines.push(core::mem::replace(&mut line, LLine { y, h: lh, kind: b.kind, bullet: false, items: Vec::new() }));
                                    x = 0;
                                    chunk.clear();
                                    cw = 0;
                                }
                                chunk.push(c);
                                cw += w;
                            }
                            if !chunk.is_empty() {
                                push_item(&mut line, x, cw, &chunk, run.link, run.bold, sp);
                                x += cw;
                            }
                        } else if !word.trim_end().is_empty() {
                            push_item(&mut line, x, core_w, word.trim_end(), run.link, run.bold, sp);
                            x += core_w + trailing;
                        } else {
                            x += trailing;
                        }
                    }
                    if newline {
                        y += lh;
                        self.lines.push(core::mem::replace(&mut line, LLine { y, h: lh, kind: b.kind, bullet: false, items: Vec::new() }));
                        x = 0;
                    }
                }
            }
            if !line.items.is_empty() || line.bullet {
                y += lh;
                self.lines.push(line);
            }
        }
        // le righe con una casella di testo sono un po' piu' alte
        let mut shift = 0;
        for l in self.lines.iter_mut() {
            l.y += shift;
            if l.items.iter().any(|i| i.field.is_some()) {
                l.y += 6;
                l.h += 10;
                shift += 16;
            }
        }
        self.height = y + shift;
        self.layout_w = width;
    }

    // ------------------------------------------------------------ disegno

    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, ui: &mut Ui, x: i32, y: i32, w: i32, h: i32, focused: bool, dk: &'static Dk) -> WebAct {
        let now = ui.now;
        self.poll(dk, now);
        let mut act = WebAct::None;
        let tc = ui.th.text;
        let sc = ui.th.sub;

        // ---- barra degli strumenti
        let ty = y + 10;
        let bh = 38;
        let mut bx = x + 14;
        let can_back = !self.hist.is_empty() || !self.url.is_empty();
        for i in 0..3 {
            let enabled = match i {
                0 => can_back,
                1 => self.loading.is_some() || !self.url.is_empty(),
                _ => true,
            };
            let hot = enabled && !ui.block && ui.hover(bx, ty, 38, bh);
            ui.tile(bx, ty, 38, bh, 12, hot);
            let (cx, cy) = ((bx + 19) as f32, (ty + bh / 2) as f32);
            let a = if enabled { 256 } else { 90 };
            match i {
                0 => {
                    ui.c.line(cx + 3.0, cy - 6.0, cx - 3.0, cy, 2.2, tc, a);
                    ui.c.line(cx - 3.0, cy, cx + 3.0, cy + 6.0, 2.2, tc, a);
                }
                1 if self.loading.is_some() => {
                    ui.c.line(cx - 5.0, cy - 5.0, cx + 5.0, cy + 5.0, 2.0, tc, a);
                    ui.c.line(cx + 5.0, cy - 5.0, cx - 5.0, cy + 5.0, 2.0, tc, a);
                }
                1 => {
                    // ricarica: arco con freccia
                    let r = 6.5f32;
                    let mut px = cx + r;
                    let mut py = cy;
                    for s in 1..=10 {
                        let ang = s as f32 * 0.52;
                        let (sn, cs) = sincos(ang);
                        let (nx, ny) = (cx + r * cs, cy + r * sn);
                        ui.c.line(px, py, nx, ny, 2.0, tc, a);
                        px = nx;
                        py = ny;
                    }
                    ui.c.line(cx + r, cy, cx + r - 4.0, cy - 3.0, 2.0, tc, a);
                    ui.c.line(cx + r, cy, cx + r + 3.0, cy - 4.0, 2.0, tc, a);
                }
                _ => {
                    // casa
                    ui.c.line(cx - 7.0, cy - 1.0, cx, cy - 8.0, 2.0, tc, a);
                    ui.c.line(cx, cy - 8.0, cx + 7.0, cy - 1.0, 2.0, tc, a);
                    ui.c.rrect_border(cx as i32 - 5, cy as i32 - 2, 11, 10, 2, tc, a);
                }
            }
            if enabled && ui.click(bx, ty, 38, bh) {
                match i {
                    0 => self.back(dk),
                    1 if self.loading.is_some() => self.stop(),
                    1 => {
                        let u = self.url.clone();
                        if u.starts_with("file:") {
                        } else {
                            self.start(u, false, dk);
                        }
                    }
                    _ => self.home(),
                }
            }
            bx += 44;
        }
        let dl_w = (font::text_width(dk.download, &font::BODY_B) + 34).max(100);
        let go_w = (font::text_width(dk.go, &font::BODY_B) + 34).max(64);
        let fx = bx + 4;
        let fw = (x + w - 14 - dl_w - 8 - go_w - 8 - fx).max(120);
        let shown = if self.addr_focus { tail_fit(&self.addr, fw - 40, &font::MED) } else { ui::ellipsize(&pretty(&self.addr), fw - 40, &font::MED) };
        if ui.field(fx, ty, fw, bh, &shown, dk.search_hint, focused && self.addr_focus && !self.addr_fresh) {
            if !self.addr_focus {
                self.addr_fresh = !self.addr.is_empty();
            }
            self.addr_focus = true;
        }
        if focused && self.addr_focus && self.addr_fresh {
            let tw = font::text_width(&shown, &font::MED);
            ui.c.rrect(fx + 13, ty + 8, tw + 6, bh - 16, 4, ui.th.accent, 110);
            ui.c.rrect_border(fx - 2, ty - 2, fw + 4, bh + 4, 14, ui.th.accent, 140);
        }
        let gx = fx + fw + 8;
        if ui.primary(gx, ty, go_w, bh, dk.go, false, true) {
            let a = self.addr.clone();
            self.go(&a, dk);
        }
        let can_dl = !self.url.is_empty() && !self.raw.is_empty() && self.loading.is_none() && !self.url.starts_with("file:");
        if ui.button(gx + go_w + 8, ty, dl_w, bh, dk.download, false, can_dl) {
            self.download_current(dk, now);
        }

        // ---- barra di avanzamento
        let ay = ty + bh + 10;
        if let Some(ld) = &self.loading {
            let track_w = w - 28;
            ui.c.rrect(x + 14, ay - 4, track_w, 3, 1, ui.th.fill, ui.th.fill_a + 10);
            let seg = track_w / 4;
            let p = ((now - ld.t) % 1200) as i32 * (track_w + seg) / 1200 - seg;
            let (sx, ex) = ((x + 14 + p).max(x + 14), (x + 14 + p + seg).min(x + 14 + track_w));
            if ex > sx {
                ui.c.rrect(sx, ay - 4, ex - sx, 3, 1, ui.th.accent, 256);
            }
        }

        // ---- contenuto
        let (cx, cy, cw, ch) = (x + 12, ay + 2, w - 24, y + h - ay - 40);
        ui.c.rrect(cx, cy, cw, ch, 14, if ui.th.dark { BLACK } else { WHITE }, if ui.th.dark { 70 } else { 150 });
        let pad = 24;
        let (px, pw) = (cx + pad, cw - pad * 2 - 10);
        let content_hover = !ui.block && ui.hover(cx, cy, cw, ch);
        if content_hover && ui.ev.pressed && !self.url.is_empty() {
            self.addr_focus = false;
            self.addr_fresh = false;
        }

        if self.url.is_empty() && self.error.is_none() {
            // pagina iniziale
            let mut yy = cy + 34;
            ui.c.text_center(cx + cw / 2, yy, dk.home_title, &font::LARGE, tc, 256);
            yy += 38;
            ui.c.text_center(cx + cw / 2, yy, dk.home_desc, &font::BODY, sc, 256);
            yy += 40;
            let cols = if cw > 640 { 3 } else { 2 };
            let gap = 14;
            let cardw = ((cw - 60 - gap * (cols - 1)) / cols).min(230);
            let total = cardw * cols + gap * (cols - 1);
            let x0 = cx + (cw - total) / 2;
            for (i, s) in SITES.iter().enumerate() {
                let (col, row) = (i as i32 % cols, i as i32 / cols);
                let (kx, ky) = (x0 + col * (cardw + gap), yy + row * 84);
                if ky + 72 > cy + ch {
                    break;
                }
                if ui.card(kx, ky, cardw, 72, s, dk.sites[i], false) && !ui.block {
                    self.go(s, dk);
                }
            }
            let rows = (SITES.len() as i32 + cols - 1) / cols;
            let ny = yy + rows * 84 + 16;
            if ny + 20 < cy + ch {
                let note = ui::ellipsize(dk.https_proxy, cw - 40, &font::BODY);
                ui.c.text_center(cx + cw / 2, ny, &note, &font::BODY, sc, 200);
            }
        } else if let Some(e) = &self.error {
            let e = e.clone();
            ui.c.text_center(cx + cw / 2, cy + ch / 2 - 40, ":(", &font::TITLE, sc, 220);
            ui.c.text_center(cx + cw / 2, cy + ch / 2 + 6, dk.page_error, &font::MED_B, tc, 256);
            let e = ui::ellipsize(&e, cw - 40, &font::BODY);
            ui.c.text_center(cx + cw / 2, cy + ch / 2 + 38, &e, &font::BODY, sc, 256);
        } else {
            if self.layout_w != pw {
                self.layout(pw);
            }
            let view_h = ch - 24;
            let max_scroll = (self.height - view_h).max(0);
            // scorrimento
            if content_hover && ui.ev.wheel != 0 {
                self.scroll += ui.ev.wheel * 69;
            }
            if focused && !self.addr_focus && self.focus.is_none() {
                for k in ui.ev.keys.iter() {
                    match k {
                        Key::Down => self.scroll += 46,
                        Key::Up => self.scroll -= 46,
                        Key::PageDown | Key::Char(' ') => self.scroll += view_h - 40,
                        Key::PageUp => self.scroll -= view_h - 40,
                        Key::Home => self.scroll = 0,
                        Key::End => self.scroll = max_scroll,
                        _ => {}
                    }
                }
            }
            self.scroll = self.scroll.clamp(0, max_scroll);
            let top = cy + 12;
            let bottom = cy + ch - 12;
            let link_col = if ui.th.dark { mix(ui.th.accent, WHITE, 110) } else { mix(ui.th.accent, BLACK, 30) };
            // link sotto il mouse
            let mut hover_link: Option<u16> = None;
            let mut hover_field: Option<u16> = None;
            if content_hover {
                for l in self.lines.iter() {
                    let ly = top + l.y - self.scroll;
                    if ui.my >= ly && ui.my < ly + l.h {
                        let ind = indent_of(l.kind);
                        for it in l.items.iter() {
                            let ix = px + ind + it.x;
                            if ui.mx >= ix - 2 && ui.mx < ix + it.w + 2 {
                                if it.link.is_some() {
                                    hover_link = it.link;
                                }
                                if it.field.is_some() {
                                    hover_field = it.field;
                                }
                            }
                        }
                    }
                }
                if ui.ev.pressed && !ui.block {
                    self.focus = hover_field;
                    if hover_field.is_some() {
                        self.addr_focus = false;
                    }
                }
            }
            let quote_col = if ui.th.dark { WHITE } else { BLACK };
            for l in self.lines.iter() {
                let ly = top + l.y - self.scroll;
                if ly + l.h < top || ly > bottom - l.h + 4 {
                    continue;
                }
                let ind = indent_of(l.kind);
                match l.kind {
                    Kind::Hr => {
                        ui.c.rect(px, ly + 6, pw, 1, quote_col, 50);
                        continue;
                    }
                    Kind::Quote => ui.c.rect(px + 6, ly, 3, l.h, ui.th.accent, 150),
                    Kind::Pre => ui.c.rect(px, ly, pw, l.h, quote_col, if ui.th.dark { 22 } else { 12 }),
                    _ => {}
                }
                if l.bullet {
                    ui.c.circle(px + 10, ly + l.h / 2, 3, tc, 220);
                }
                let off = match l.kind {
                    Kind::H1 => 4,
                    Kind::H2 => 4,
                    Kind::H3 => 4,
                    Kind::Pre => 2,
                    _ => 3,
                };
                for it in l.items.iter() {
                    let ix = px + ind + it.x;
                    if let Some(fid) = it.field {
                        let fo = self.focus == Some(fid) && focused;
                        let (fy, fh) = (ly + 2, l.h - 4);
                        ui.c.rrect(ix, fy, it.w, fh, 9, if ui.th.dark { BLACK } else { WHITE }, if ui.th.dark { 120 } else { 220 });
                        ui.c.rrect_border(ix, fy, it.w, fh, 9, if fo { ui.th.accent } else { quote_col }, if fo { 256 } else { 70 });
                        if let Some(fd) = self.page.fields.get(fid as usize) {
                            let ty2 = fy + (fh - 16) / 2 - 1;
                            if fd.value.is_empty() {
                                let ph = ui::ellipsize(&fd.placeholder, it.w - 20, &font::BODY);
                                ui.c.text(ix + 10, ty2, &ph, &font::BODY, sc, 200);
                            } else {
                                let v = tail_fit(&fd.value, it.w - 22, &font::BODY);
                                ui.c.text(ix + 10, ty2, &v, &font::BODY, tc, 256);
                            }
                            if fo && (now / 530) % 2 == 0 {
                                let vw = if fd.value.is_empty() { 0 } else { font::text_width(&tail_fit(&fd.value, it.w - 22, &font::BODY), &font::BODY) };
                                ui.c.rect(ix + 11 + vw, ty2 - 1, 2, 19, ui.th.accent, 256);
                            }
                        }
                        continue;
                    }
                    let is_submit = it.link.and_then(|id| self.page.links.get(id as usize)).map(|t| t.starts_with(SUBMIT)).unwrap_or(false);
                    if is_submit {
                        // pulsante "invia" del modulo
                        let hot = it.link == hover_link;
                        let (by, bh2) = if l.h > 26 { (ly + 2, l.h - 4) } else { (ly, l.h) };
                        ui.c.rrect(ix - 10, by, it.w + 20, bh2, 9, ui.th.accent, if hot { 256 } else { 215 });
                        ui.c.text(ix, by + (bh2 - 16) / 2 - 1, &it.text, &font::BODY_B, WHITE, 256);
                        continue;
                    }
                    let f = block_font(l.kind, it.bold);
                    let col = if it.link.is_some() { link_col } else { tc };
                    if l.kind == Kind::Pre {
                        ui.c.text_mono(ix, ly + off, &it.text, &font::BODY, col, 256);
                    } else {
                        ui.c.text(ix, ly + off, &it.text, &f, col, 256);
                    }
                    if it.link.is_some() && it.link == hover_link {
                        ui.c.rect(ix, ly + off + f.height() + 1, it.w, 1, col, 230);
                    }
                }
            }
            // barra di scorrimento
            if max_scroll > 0 {
                let track = ch - 24;
                let th_h = (track * view_h / self.height.max(1)).max(30);
                let th_y = cy + 12 + (track - th_h) * self.scroll / max_scroll;
                ui.c.rrect(cx + cw - 10, th_y, 5, th_h, 2, tc, 90);
            }
            if let Some(id) = hover_link {
                if let Some(target) = self.page.links.get(id as usize).cloned().filter(|t| t.starts_with(SUBMIT)) {
                    if ui.ev.pressed && !ui.block {
                        let form = target[1..].parse::<u16>().unwrap_or(0);
                        self.submit(Some(form), dk);
                    }
                } else if let Some(target) = self.page.links.get(id as usize).cloned() {
                    // anteprima del link in basso
                    let t = ui::ellipsize(&pretty(&target), cw - 40, &font::BODY);
                    let tw = font::text_width(&t, &font::BODY);
                    ui.c.rrect(cx + 8, cy + ch - 34, tw + 20, 26, 8, ui.th.strong.tint, 235);
                    ui.c.text(cx + 18, cy + ch - 29, &t, &font::BODY, tc, 256);
                    if ui.ev.pressed && !ui.block {
                        self.follow(&target, dk);
                    }
                }
            }
        }

        // ---- tastiera della barra indirizzi
        if focused {
            for k in ui.ev.keys.iter() {
                if let Some(fid) = self.focus.filter(|_| !self.addr_focus) {
                    let Some(fd) = self.page.fields.get_mut(fid as usize) else {
                        self.focus = None;
                        continue;
                    };
                    match k {
                        Key::Char(c) if fd.value.chars().count() < 300 => fd.value.push(*c),
                        Key::Backspace => {
                            fd.value.pop();
                        }
                        Key::Ctrl('v') => {
                            let p = crate::clip::get();
                            fd.value.push_str(p.lines().next().unwrap_or("").trim());
                        }
                        Key::Ctrl('a') => fd.value.clear(),
                        Key::Enter => {
                            let form = fd.form;
                            self.submit(form, dk);
                            break;
                        }
                        Key::Esc | Key::Tab => self.focus = None,
                        _ => {}
                    }
                    continue;
                }
                if self.addr_focus {
                    match k {
                        Key::Char(c) => {
                            if self.addr_fresh {
                                self.addr.clear();
                                self.addr_fresh = false;
                            }
                            if self.addr.chars().count() < 400 {
                                self.addr.push(*c);
                            }
                        }
                        Key::Backspace => {
                            if self.addr_fresh {
                                self.addr.clear();
                                self.addr_fresh = false;
                            } else {
                                self.addr.pop();
                            }
                        }
                        Key::Enter => {
                            let a = self.addr.clone();
                            self.go(&a, dk);
                        }
                        Key::Esc => {
                            self.addr = self.url.clone();
                            self.addr_fresh = false;
                            if !self.url.is_empty() {
                                self.addr_focus = false;
                            }
                        }
                        Key::Ctrl('a') => self.addr_fresh = !self.addr.is_empty(),
                        Key::Ctrl('c') => {
                            crate::clip::set(&self.addr);
                            self.msg = Some((String::from(dk.copied), now));
                        }
                        Key::Ctrl('v') => {
                            if self.addr_fresh {
                                self.addr.clear();
                                self.addr_fresh = false;
                            }
                            let p = crate::clip::get();
                            self.addr.push_str(p.lines().next().unwrap_or("").trim());
                        }
                        Key::Left | Key::Right | Key::Home | Key::End => self.addr_fresh = false,
                        _ => {}
                    }
                } else {
                    match k {
                        Key::Backspace => self.back(dk),
                        Key::Ctrl('l') => {
                            self.addr_focus = true;
                            self.addr_fresh = !self.addr.is_empty();
                        }
                        Key::F5 if !self.url.is_empty() && !self.url.starts_with("file:") => {
                            let u = self.url.clone();
                            self.start(u, false, dk);
                        }
                        Key::Ctrl('s') => self.download_current(dk, now),
                        Key::Esc if self.loading.is_some() => self.stop(),
                        _ => {}
                    }
                }
            }
        }

        // ---- barra di stato / download completato
        let sy = y + h - 32;
        if let Some((name, size, t)) = self.saved.clone() {
            if now < t + 12_000 {
                let bw = (font::text_width(dk.open_folder, &font::BODY_B) + 30).max(120);
                let text = format!("{} {} ({})", dk.downloaded, name, human(size));
                let text = ui::ellipsize(&text, w - bw - 80, &font::BODY);
                ui.c.circle(x + 26, sy + 12, 9, ui.th.accent, 256);
                ui.c.line((x + 26) as f32, (sy + 7) as f32, (x + 26) as f32, (sy + 16) as f32, 1.8, WHITE, 256);
                ui.c.line((x + 22) as f32, (sy + 12) as f32, (x + 26) as f32, (sy + 16) as f32, 1.8, WHITE, 256);
                ui.c.line((x + 30) as f32, (sy + 12) as f32, (x + 26) as f32, (sy + 16) as f32, 1.8, WHITE, 256);
                ui.c.text(x + 44, sy + 4, &text, &font::BODY, tc, 256);
                if ui.button(x + w - 14 - bw, sy - 3, bw, 30, dk.open_folder, false, true) {
                    act = WebAct::OpenDownloads;
                    self.saved = None;
                }
                return act;
            }
            self.saved = None;
        }
        let status = if let Some(ld) = &self.loading {
            let got = net::fetch_progress(1).unwrap_or(0);
            let host = String::from(host_of(&ld.url));
            if got > 0 {
                format!("{} {}  ·  {}", dk.loading, host, human(got as u32))
            } else {
                format!("{} {}", dk.loading, host)
            }
        } else if let Some((m, t)) = &self.msg {
            if now < t + 3000 {
                m.clone()
            } else {
                String::new()
            }
        } else if !self.raw.is_empty() {
            format!("{}  ·  {}", self.page.title, human(self.raw.len() as u32))
        } else {
            String::new()
        };
        if self.loading.is_some() {
            ui::spinner(ui.c, x + 26, sy + 12, 8, ui.th.accent, now);
        }
        let status = ui::ellipsize(&status, w - 70, &font::BODY);
        ui.c.text(x + if self.loading.is_some() { 44 } else { 20 }, sy + 4, &status, &font::BODY, sc, 230);
        act
    }

    /// Invia un modulo come richiesta GET: indirizzo?nome=valore&...
    fn submit(&mut self, form: Option<u16>, dk: &'static Dk) {
        let form = form.or(if self.page.forms.len() == 1 { Some(0) } else { None });
        let action = form.and_then(|f| self.page.forms.get(f as usize)).map(|f| f.action.clone()).unwrap_or_default();
        let base = if action.is_empty() { self.url.clone() } else { action };
        let base = String::from(base.split(['?', '#']).next().unwrap_or(""));
        let mut q = String::new();
        for f in self.page.fields.iter().filter(|f| f.form == form && !f.name.is_empty()) {
            if !q.is_empty() {
                q.push('&');
            }
            q.push_str(&pct_encode(&f.name));
            q.push('=');
            q.push_str(&pct_encode(&f.value));
        }
        self.focus = None;
        let url = if q.is_empty() { base } else { format!("{}?{}", base, q) };
        self.follow(&url, dk);
    }

    fn follow(&mut self, target: &str, dk: &'static Dk) {
        if target.starts_with("file:") {
            return;
        }
        let u = to_url(target);
        if !self.url.is_empty() && self.url != u {
            self.hist.push(self.url.clone());
        }
        self.start(u, false, dk);
    }
}

fn push_item(line: &mut LLine, x: i32, w: i32, text: &str, link: Option<u16>, bold: bool, sp: i32) {
    if let Some(last) = line.items.last_mut() {
        let gap = x - (last.x + last.w);
        if last.field.is_none() && last.link == link && last.bold == bold && (gap == 0 || gap == sp) {
            // stessa formattazione: unisce le parole in un unico pezzo
            if gap > 0 {
                last.text.push(' ');
            }
            last.text.push_str(text);
            last.w = x + w - last.x;
            return;
        }
    }
    line.items.push(Item { x, w, text: String::from(text), link, bold, field: None });
}

fn indent_of(k: Kind) -> i32 {
    match k {
        Kind::Li => 24,
        Kind::Quote => 22,
        Kind::Pre => 10,
        _ => 0,
    }
}

fn block_font(k: Kind, bold: bool) -> Font {
    match k {
        Kind::H1 => font::LARGE,
        Kind::H2 => font::MED_B,
        Kind::H3 => font::BODY_B,
        _ if bold => font::BODY_B,
        _ => font::BODY,
    }
}

/// Avanzamento reale del testo (come lo disegna Canvas::text).
fn adv(s: &str, f: &Font) -> i32 {
    s.chars().map(|c| font::char_width(c, f)).sum()
}

fn mono_w(s: &str) -> i32 {
    s.chars().count() as i32 * font::mono_width(&font::BODY)
}

fn sincos(a: f32) -> (f32, f32) {
    // piccola approssimazione (non serve precisione per un'icona)
    let mut x = a;
    while x > core::f32::consts::PI {
        x -= 2.0 * core::f32::consts::PI;
    }
    let s = x - x * x * x / 6.0 + x * x * x * x * x / 120.0 - x * x * x * x * x * x * x / 5040.0;
    let c = 1.0 - x * x / 2.0 + x * x * x * x / 24.0 - x * x * x * x * x * x / 720.0;
    (s, c)
}

/// Mostra la fine del testo quando non entra (barra indirizzi in modifica).
fn tail_fit(s: &str, max_w: i32, f: &Font) -> String {
    if font::text_width(s, f) <= max_w {
        return String::from(s);
    }
    let chars: Vec<char> = s.chars().collect();
    let mut w = font::text_width("...", f);
    let mut start = chars.len();
    while start > 0 {
        let cw = font::char_width(chars[start - 1], f);
        if w + cw > max_w {
            break;
        }
        w += cw;
        start -= 1;
    }
    let mut out = String::from("...");
    out.extend(chars[start..].iter());
    out
}

/// Divide un indirizzo in schema ("http://" o "https://") e resto.
fn scheme(u: &str) -> (&'static str, &str) {
    if u.len() >= 8 && u[..8].eq_ignore_ascii_case("https://") {
        ("https://", &u[8..])
    } else if u.len() >= 7 && u[..7].eq_ignore_ascii_case("http://") {
        ("http://", &u[7..])
    } else {
        ("http://", u)
    }
}

/// Indirizzo da mostrare: senza "http://" e con i siti del proxy in chiaro.
fn pretty(u: &str) -> String {
    if let Some(rest) = u.strip_prefix(PROXY) {
        return pct_decode(rest);
    }
    String::from(u.strip_prefix("http://").unwrap_or(u))
}

fn host_of(u: &str) -> &str {
    if let Some(rest) = u.strip_prefix(PROXY) {
        let r = rest.strip_prefix("https%3A%2F%2F").or_else(|| rest.strip_prefix("https://")).unwrap_or(rest);
        return r.split(['/', '%']).next().unwrap_or(r);
    }
    let r = scheme(u).1;
    r.split(['/', '?', ':']).next().unwrap_or(r)
}

// ------------------------------------------------------------------ indirizzi

/// Trasforma il testo della barra in un indirizzo (o una ricerca).
pub fn to_url(t: &str) -> String {
    let t = t.trim();
    let low = t.to_ascii_lowercase();
    if low.starts_with("http://") || low.starts_with("https://") || low.starts_with("file:") {
        return String::from(t);
    }
    let looks_addr = !t.contains(' ') && (t.contains('.') || low.starts_with("localhost") || t.contains(':')) && !t.ends_with('.');
    if looks_addr {
        // come i browser veri: prima la versione sicura
        let host = low.split(['/', ':']).next().unwrap_or("");
        let local = host == "localhost" || host.parse::<smoltcp::wire::Ipv4Address>().is_ok() || t.contains(':');
        if local {
            format!("http://{}", t)
        } else {
            format!("https://{}", t)
        }
    } else {
        format!("{}{}", SEARCH, pct_encode(t))
    }
}

/// Versione leggera di un sito HTTPS tramite FrogFind (quando il TLS non riesce).
fn proxied(url: &str) -> String {
    format!("{}{}", PROXY, pct_encode(url))
}

fn pct_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else if b == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(core::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Risolve un link relativo rispetto alla pagina corrente.
pub fn resolve(base: &str, href: &str) -> String {
    let href = href.trim();
    let low = href.to_ascii_lowercase();
    if low.starts_with("http://") || low.starts_with("https://") {
        return String::from(href);
    }
    let (sch, b) = scheme(base);
    if let Some(rest) = href.strip_prefix("//") {
        return format!("{}{}", sch, rest);
    }
    let (host, path) = match b.find('/') {
        Some(i) => (&b[..i], &b[i..]),
        None => (b.split('?').next().unwrap_or(b), "/"),
    };
    if href.starts_with('/') {
        return format!("{}{}{}", sch, host, href);
    }
    if href.starts_with('?') {
        let p = path.split('?').next().unwrap_or("/");
        return format!("{}{}{}{}", sch, host, p, href);
    }
    let p = path.split('?').next().unwrap_or("/");
    let dir = match p.rfind('/') {
        Some(i) => &p[..=i],
        None => "/",
    };
    let mut parts: Vec<&str> = Vec::new();
    let joined = format!("{}{}", dir, href);
    let (jp, query) = match joined.find('?') {
        Some(i) => (String::from(&joined[..i]), String::from(&joined[i..])),
        None => (joined.clone(), String::new()),
    };
    let ends_slash = jp.ends_with('/') || jp.ends_with("/.") || jp.ends_with("/..");
    for seg in jp.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let mut out = format!("{}{}/{}", sch, host, parts.join("/"));
    if ends_slash && !parts.is_empty() {
        out.push('/');
    }
    out.push_str(&query);
    out
}

/// Nome di file per un download, preso dall'indirizzo.
fn file_name(url: &str, ctype: &str) -> String {
    let src = if let Some(rest) = url.strip_prefix(PROXY) { pct_decode(rest) } else { String::from(url) };
    let no_q = src.split(['?', '#']).next().unwrap_or("");
    let after_scheme = no_q.split("://").nth(1).unwrap_or(no_q);
    let host = after_scheme.split('/').next().unwrap_or("download");
    let last = after_scheme.splitn(2, '/').nth(1).and_then(|p| p.rsplit('/').next()).unwrap_or("");
    let mut name: String = pct_decode(last)
        .chars()
        .map(|c| if c.is_alphanumeric() || " .-_()[]".contains(c) { c } else { '_' })
        .collect();
    name = String::from(name.trim().trim_matches('.'));
    if name.is_empty() {
        name = String::from(host);
    }
    if name.chars().count() > 80 {
        name = name.chars().take(80).collect();
    }
    if !name.contains('.') || name == host {
        let ext = if ctype.contains("html") {
            ".html"
        } else if ctype.starts_with("text/plain") {
            ".txt"
        } else if ctype.contains("png") {
            ".png"
        } else if ctype.contains("jpeg") {
            ".jpg"
        } else if ctype.contains("gif") {
            ".gif"
        } else if ctype.contains("pdf") {
            ".pdf"
        } else if ctype.contains("zip") {
            ".zip"
        } else if ctype.contains("json") {
            ".json"
        } else {
            ".bin"
        };
        name.push_str(ext);
    }
    name
}

// ------------------------------------------------------------------ HTML -> pagina

fn decode_text(b: &[u8]) -> String {
    match core::str::from_utf8(b) {
        Ok(s) => String::from(s),
        Err(e) if e.valid_up_to() + 4 >= b.len() => String::from_utf8_lossy(b).into_owned(),
        // non e' UTF-8: quasi sempre Latin-1 / Windows-1252
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
}

fn looks_html(s: &str) -> bool {
    let head: String = s.chars().take(600).collect::<String>().to_ascii_lowercase();
    head.contains("<html") || head.contains("<!doctype html") || head.contains("<body") || head.contains("<p>")
}

fn plain_page(text: &str) -> Page {
    let mut p = Page::default();
    let mut cur = String::new();
    for (i, l) in text.lines().enumerate() {
        if i >= 20_000 {
            break;
        }
        cur.push_str(&fix_text(&l.replace('\t', "    ")));
        cur.push('\n');
        if cur.len() > 4000 {
            p.blocks.push(Block { kind: Kind::Pre, runs: alloc::vec![Run { text: core::mem::take(&mut cur), link: None, bold: false, field: None }] });
        }
    }
    if !cur.is_empty() {
        p.blocks.push(Block { kind: Kind::Pre, runs: alloc::vec![Run { text: cur, link: None, bold: false, field: None }] });
    }
    p
}

/// Converte i caratteri che il font non ha in equivalenti semplici.
fn fix_char(c: char) -> Option<char> {
    Some(match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{2032}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{2033}' => '"',
        '\u{2013}' | '\u{2014}' | '\u{2212}' | '\u{2010}' | '\u{2011}' => '-',
        '\u{2022}' | '\u{25CF}' | '\u{2219}' => '·',
        '\u{2026}' => '…',
        '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2009}' => ' ',
        '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{00AD}' => return None,
        '\u{2192}' => '>',
        '\u{2190}' => '<',
        '\r' => return None,
        c => c,
    })
}

fn fix_text(s: &str) -> String {
    s.chars().filter_map(fix_char).collect()
}

fn entity(name: &str) -> Option<char> {
    if let Some(n) = name.strip_prefix('#') {
        let v = if let Some(h) = n.strip_prefix('x').or_else(|| n.strip_prefix('X')) { u32::from_str_radix(h, 16).ok()? } else { n.parse().ok()? };
        // i numeri 128-159 sono Windows-1252 nelle pagine vecchie
        let v = match v {
            146 | 145 => 0x2019,
            147 | 148 => 0x201C,
            150 | 151 => 0x2013,
            133 => 0x2026,
            v => v,
        };
        return char::from_u32(v);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "copy" => '©',
        "reg" => '®',
        "trade" => 'T',
        "mdash" | "ndash" | "minus" => '-',
        "hellip" => '…',
        "laquo" => '«',
        "raquo" => '»',
        "lsquo" | "rsquo" | "sbquo" => '\'',
        "ldquo" | "rdquo" | "bdquo" => '"',
        "bull" | "middot" => '·',
        "euro" => '€',
        "pound" => '£',
        "deg" => '°',
        "times" => '×',
        "divide" => '÷',
        "agrave" => 'à',
        "aacute" => 'á',
        "egrave" => 'è',
        "eacute" => 'é',
        "igrave" => 'ì',
        "iacute" => 'í',
        "ograve" => 'ò',
        "oacute" => 'ó',
        "ugrave" => 'ù',
        "uacute" => 'ú',
        "Agrave" => 'À',
        "Egrave" => 'È',
        "Eacute" => 'É',
        "ccedil" => 'ç',
        "ntilde" => 'ñ',
        "auml" => 'ä',
        "ouml" => 'ö',
        "uuml" => 'ü',
        "Auml" => 'Ä',
        "Ouml" => 'Ö',
        "Uuml" => 'Ü',
        "szlig" => 'ß',
        "iexcl" => '¡',
        "iquest" => '¿',
        "sect" => '§',
        "para" => '¶',
        _ => return None,
    })
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return fix_text(s);
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let end = after.char_indices().take(10).find(|(_, c)| *c == ';' || c.is_whitespace() || *c == '&').map(|(j, c)| (j, c));
        match end {
            Some((j, ';')) => match entity(&after[..j]) {
                Some(c) => {
                    out.push(c);
                    rest = &after[j + 1..];
                }
                None => {
                    out.push('&');
                    rest = after;
                }
            },
            _ => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    fix_text(&out)
}

/// Valore di un attributo di un tag (href, alt, ...).
fn attr(tag: &str, name: &str) -> Option<String> {
    let low = tag.to_ascii_lowercase();
    let lb = low.as_bytes();
    let mut from = 0;
    while let Some(p) = low[from..].find(name) {
        let i = from + p;
        from = i + name.len();
        if i == 0 || !lb[i - 1].is_ascii_whitespace() {
            continue;
        }
        let mut j = i + name.len();
        while j < lb.len() && lb[j] == b' ' {
            j += 1;
        }
        if j >= lb.len() || lb[j] != b'=' {
            continue;
        }
        j += 1;
        while j < lb.len() && lb[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= lb.len() {
            return None;
        }
        let q = lb[j];
        let v = if q == b'"' || q == b'\'' {
            let end = tag[j + 1..].find(q as char).map(|e| j + 1 + e).unwrap_or(tag.len());
            &tag[j + 1..end]
        } else {
            let end = tag[j..].find(|c: char| c.is_whitespace() || c == '>').map(|e| j + e).unwrap_or(tag.len());
            &tag[j..end]
        };
        return Some(decode_entities(v));
    }
    None
}

struct Parser<'a> {
    page: Page,
    cur: Block,
    bold: u32,
    link: Option<u16>,
    pre: u32,
    in_title: bool,
    base: &'a str,
    form: Option<u16>,
}

impl<'a> Parser<'a> {
    fn brk(&mut self, kind: Kind) {
        let old = core::mem::replace(&mut self.cur, Block { kind, runs: Vec::new() });
        if old.has_text() {
            self.page.blocks.push(old);
        }
    }

    fn text(&mut self, raw: &str) {
        let t = decode_entities(raw);
        if self.in_title {
            self.page.title.push_str(&t);
            return;
        }
        let t = if self.pre > 0 {
            t
        } else {
            let mut s = String::new();
            let mut sp = false;
            for c in t.chars() {
                if c.is_whitespace() {
                    sp = true;
                } else {
                    if sp {
                        let at_start = self.cur.runs.is_empty() && s.is_empty();
                        let after_space = s.is_empty() && self.cur.runs.last().map(|r| r.text.ends_with(' ') || r.text.ends_with('\n')).unwrap_or(true);
                        if !at_start && !after_space {
                            s.push(' ');
                        }
                    }
                    sp = false;
                    s.push(c);
                }
            }
            if sp && !s.is_empty() {
                s.push(' ');
            } else if sp && s.is_empty() {
                let ok = self.cur.runs.last().map(|r| !r.text.ends_with(' ') && !r.text.ends_with('\n')).unwrap_or(false);
                if ok {
                    s.push(' ');
                }
            }
            s
        };
        if t.is_empty() {
            return;
        }
        self.push(t);
    }

    fn push(&mut self, t: String) {
        let (link, bold) = (self.link, self.bold > 0);
        if let Some(r) = self.cur.runs.last_mut() {
            if r.field.is_none() && r.link == link && r.bold == bold {
                r.text.push_str(&t);
                return;
            }
        }
        self.cur.runs.push(Run { text: t, link, bold, field: None });
    }

    fn tag(&mut self, raw: &str) {
        let close = raw.starts_with('/');
        let body = raw.trim_start_matches('/');
        let name: String = body.chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        match name.as_str() {
            "h1" => self.brk(if close { Kind::P } else { Kind::H1 }),
            "h2" => self.brk(if close { Kind::P } else { Kind::H2 }),
            "h3" | "h4" | "h5" | "h6" => self.brk(if close { Kind::P } else { Kind::H3 }),
            "li" | "dt" => self.brk(if close { Kind::P } else { Kind::Li }),
            "pre" => {
                if close {
                    self.pre = self.pre.saturating_sub(1);
                    self.brk(Kind::P);
                } else {
                    self.pre += 1;
                    self.brk(Kind::Pre);
                }
            }
            "blockquote" => self.brk(if close { Kind::P } else { Kind::Quote }),
            "hr" => {
                self.brk(Kind::Hr);
                self.cur.runs.clear();
                let hr = core::mem::replace(&mut self.cur, Block { kind: Kind::P, runs: Vec::new() });
                self.page.blocks.push(hr);
            }
            "br" => {
                if self.cur.has_text() {
                    self.push(String::from("\n"));
                }
            }
            "p" | "div" | "section" | "article" | "header" | "footer" | "nav" | "main" | "aside" | "table" | "tr" | "ul" | "ol" | "dl" | "dd" | "center" | "figure" | "figcaption" | "address" | "fieldset" | "details" | "summary" | "body" | "caption" | "legend" => {
                if self.pre == 0 {
                    let k = if self.cur.kind == Kind::Quote && !close { Kind::Quote } else { Kind::P };
                    self.brk(k);
                }
            }
            "td" | "th" if !close => {
                if self.cur.has_text() {
                    self.push(String::from("   "));
                }
            }
            "b" | "strong" | "th" => {
                if close {
                    self.bold = self.bold.saturating_sub(1);
                } else {
                    self.bold += 1;
                }
            }
            "a" => {
                if close {
                    self.link = None;
                } else if let Some(h) = attr(body, "href") {
                    let hl = h.to_ascii_lowercase();
                    if !(hl.starts_with('#') || hl.starts_with("javascript:") || hl.starts_with("mailto:") || hl.starts_with("tel:") || hl.is_empty()) && self.page.links.len() < 60_000 {
                        let target = resolve(self.base, &h);
                        self.link = Some(self.page.links.len() as u16);
                        self.page.links.push(target);
                    }
                }
            }
            "img" => {
                if let Some(alt) = attr(body, "alt") {
                    let alt = String::from(alt.trim());
                    if !alt.is_empty() && alt.chars().count() < 80 {
                        self.push(format!("[{}] ", alt));
                    }
                }
            }
            "title" => self.in_title = !close,
            "form" => {
                if self.pre == 0 {
                    self.brk(Kind::P);
                }
                if close {
                    self.form = None;
                } else if self.page.forms.len() < 200 {
                    let action = attr(body, "action").map(|a| resolve(self.base, &a)).unwrap_or_default();
                    self.form = Some(self.page.forms.len() as u16);
                    self.page.forms.push(Form { action });
                }
            }
            "input" | "textarea" if !close => {
                let kind = if name == "textarea" { String::from("text") } else { attr(body, "type").unwrap_or_default().to_ascii_lowercase() };
                let fname = attr(body, "name").unwrap_or_default();
                let value = attr(body, "value").unwrap_or_default();
                match kind.as_str() {
                    "hidden" => self.page.fields.push(Field { form: self.form, name: fname, value, placeholder: String::new(), hidden: true, w: 0 }),
                    "submit" | "image" => {
                        if let Some(f) = self.form {
                            let label = if value.trim().is_empty() { attr(body, "alt").unwrap_or_else(|| String::from("OK")) } else { value };
                            let saved = self.link;
                            self.link = Some(self.page.links.len() as u16);
                            self.page.links.push(format!("{}{}", SUBMIT, f));
                            self.push(format!("{} ", label.trim()));
                            self.link = saved;
                        }
                    }
                    "" | "text" | "search" | "email" | "url" | "tel" | "number" => {
                        let size = attr(body, "size").and_then(|s| s.parse::<i32>().ok()).unwrap_or(32);
                        let placeholder = attr(body, "placeholder").or_else(|| attr(body, "aria-label")).or_else(|| attr(body, "title")).unwrap_or_default();
                        let id = self.page.fields.len() as u16;
                        self.page.fields.push(Field { form: self.form, name: fname, value, placeholder, hidden: false, w: (size * 9).clamp(180, 420) });
                        self.cur.runs.push(Run { text: String::new(), link: None, bold: false, field: Some(id) });
                        self.push(String::from(" "));
                    }
                    _ => {}
                }
            }
            "button" => {
                let t = attr(body, "type").unwrap_or_default().to_ascii_lowercase();
                if close {
                    if self.link.and_then(|l| self.page.links.get(l as usize)).map(|s| s.starts_with(SUBMIT)).unwrap_or(false) {
                        self.link = None;
                    }
                } else if let (Some(f), true) = (self.form, t != "button" && t != "reset") {
                    self.link = Some(self.page.links.len() as u16);
                    self.page.links.push(format!("{}{}", SUBMIT, f));
                }
            }
            "option" if !close => self.push(String::from(" ")),
            _ => {}
        }
    }
}

fn parse_html(src: &str, base: &str) -> Page {
    let mut p = Parser { page: Page::default(), cur: Block { kind: Kind::P, runs: Vec::new() }, bold: 0, link: None, pre: 0, in_title: false, base, form: None };
    let bytes = src.as_bytes();
    let low_needed = |i: usize, pat: &str| -> bool { bytes.len() >= i + pat.len() && bytes[i..i + pat.len()].eq_ignore_ascii_case(pat.as_bytes()) };
    let mut i = 0;
    let mut text_start = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        // testo prima del tag
        if i > text_start {
            p.text(&src[text_start..i]);
        }
        if low_needed(i, "<!--") {
            i = src[i + 4..].find("-->").map(|e| i + 4 + e + 3).unwrap_or(bytes.len());
            text_start = i;
            continue;
        }
        // un '<' che non apre un tag e' testo normale
        let next = bytes.get(i + 1).copied().unwrap_or(b' ');
        if !(next.is_ascii_alphabetic() || next == b'/' || next == b'!' || next == b'?') {
            text_start = i;
            i += 1;
            // il '<' resta nel testo: avanza fino al prossimo
            while i < bytes.len() && bytes[i] != b'<' {
                i += 1;
            }
            continue;
        }
        let end = match src[i + 1..].find('>') {
            Some(e) => i + 1 + e,
            None => bytes.len(),
        };
        let raw = &src[i + 1..end.min(bytes.len())];
        let name: String = raw.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        i = (end + 1).min(bytes.len());
        text_start = i;
        if !raw.starts_with('/') && matches!(name.as_str(), "script" | "style" | "svg" | "template" | "select" | "head" | "iframe" | "object") && !raw.ends_with('/') {
            if name == "head" {
                // nell'intestazione interessa solo il titolo
                let close = find_ci(src, i, "</head").unwrap_or(bytes.len());
                if let Some(ts) = find_ci(src, i, "<title") {
                    if ts < close {
                        if let Some(te) = src[ts..].find('>') {
                            let tstart = ts + te + 1;
                            let tend = find_ci(src, tstart, "</title").unwrap_or(close);
                            p.page.title = decode_entities(src[tstart..tend.max(tstart)].trim());
                        }
                    }
                }
                i = close;
            } else {
                let pat = format!("</{}", name);
                i = find_ci(src, i, &pat).unwrap_or(bytes.len());
            }
            text_start = i;
            continue;
        }
        p.tag(raw);
    }
    if bytes.len() > text_start {
        p.text(&src[text_start..]);
    }
    p.brk(Kind::P);
    let t = String::from(p.page.title.split_whitespace().collect::<Vec<_>>().join(" ").trim());
    p.page.title = t;
    p.page
}

fn find_ci(s: &str, from: usize, pat: &str) -> Option<usize> {
    let b = s.as_bytes();
    let pb = pat.as_bytes();
    if pb.is_empty() || from >= b.len() {
        return None;
    }
    let mut i = from;
    while i + pb.len() <= b.len() {
        if b[i..i + pb.len()].eq_ignore_ascii_case(pb) {
            return Some(i);
        }
        i += 1;
    }
    None
}
