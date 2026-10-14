//! Interfaccia "immediate mode" in stile Liquid Glass: tema, widget, logo, cursore.

use crate::arch;
use crate::font::{self, Font};
use crate::gfx::{self, blend, mix, rgb, Canvas, Glass, Screen, BLACK, WHITE};
use crate::i18n::{Lang, Tr};
use crate::input::{Events, Input};
use crate::keymap::Layout;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

pub const ACCENTS: [u32; 6] = [
    rgb(94, 92, 230),  // indaco
    rgb(10, 132, 255), // blu
    rgb(191, 90, 242), // viola
    rgb(255, 55, 95),  // rosa
    rgb(255, 159, 10), // arancio
    rgb(48, 209, 88),  // verde
];

#[derive(Clone)]
pub struct Settings {
    pub lang: Lang,
    pub layout: Layout,
    pub user: String,
    pub pc: String,
    pub dark: bool,
    pub accent: usize,
    pub preset: usize,
    pub anim: bool,
    pub glass: bool,
    pub icon_size: usize,
    pub clock24: bool,
    pub show_icons: bool,
    pub sort_name: bool,
}

impl Settings {
    pub fn tr(&self) -> &'static Tr {
        self.lang.tr()
    }
    pub fn accent(&self) -> u32 {
        ACCENTS[self.accent]
    }
    /// intervallo minimo tra due frame (ms)
    pub fn frame_ms(&self) -> u64 {
        [0, 16, 33][self.preset]
    }
}

pub struct SysInfo {
    pub cpu: String,
    pub cores: u32,
    pub ram_mb: u64,
    pub w: i32,
    pub h: i32,
}

pub struct Theme {
    pub dark: bool,
    pub accent: u32,
    pub text: u32,
    pub sub: u32,
    pub glass: Glass,
    pub strong: Glass,
    pub fill: u32,
    pub fill_a: u32,
}

impl Theme {
    pub fn new(s: &Settings) -> Theme {
        let accent = s.accent();
        if s.dark {
            Theme {
                dark: true,
                accent,
                text: WHITE,
                sub: rgb(200, 204, 222),
                glass: Glass { tint: rgb(18, 20, 36), tint_a: 96, sheen: 46, rim_top: 170, rim_bottom: 44, refract: true, blur: s.glass },
                strong: Glass { tint: rgb(22, 24, 40), tint_a: 176, sheen: 30, rim_top: 140, rim_bottom: 36, refract: true, blur: s.glass },
                fill: WHITE,
                fill_a: 22,
            }
        } else {
            Theme {
                dark: false,
                accent,
                text: rgb(22, 24, 34),
                sub: rgb(80, 86, 104),
                glass: Glass { tint: WHITE, tint_a: 120, sheen: 70, rim_top: 245, rim_bottom: 110, refract: true, blur: s.glass },
                strong: Glass { tint: rgb(250, 251, 255), tint_a: 196, sheen: 50, rim_top: 245, rim_bottom: 120, refract: true, blur: s.glass },
                fill: rgb(20, 24, 40),
                fill_a: 16,
            }
        }
    }
    fn solid(&self, g: &Glass) -> Glass {
        // senza effetto vetro: pannello quasi opaco, molto piu' economico
        let mut s = *g;
        s.tint_a = 236;
        s.refract = false;
        s
    }
}

pub fn ease(t: f32) -> f32 {
    let t = if t < 0.0 { 0.0 } else if t > 1.0 { 1.0 } else { t };
    let u = 1.0 - t;
    1.0 - u * u * u
}

/// Stato grafico condiviso: schermo, buffer, sfondo, input.
pub struct Gui {
    pub screen: Screen,
    pub canvas: Canvas,
    pub wall: Canvas,
    pub blur: Canvas,
    pub input: Input,
    pub theme: Theme,
    pub sys: SysInfo,
    last_frame: u64,
    wall_key: (bool, usize, bool),
}

impl Gui {
    pub fn new(screen: Screen, input: Input, sys: SysInfo, s: &Settings) -> Gui {
        let (w, h) = (screen.w, screen.h);
        let mut g = Gui {
            screen,
            canvas: Canvas::new(w, h),
            wall: Canvas::new(w, h),
            blur: Canvas::new(w, h),
            input,
            theme: Theme::new(s),
            sys,
            last_frame: 0,
            wall_key: (false, 99, false),
        };
        g.apply(s);
        g
    }

    /// Ricalcola tema e sfondo solo se e' cambiato qualcosa.
    pub fn apply(&mut self, s: &Settings) {
        self.theme = Theme::new(s);
        let key = (s.dark, s.accent, s.glass);
        if key != self.wall_key {
            let t0 = arch::now_ms();
            gfx::make_wallpaper(&mut self.wall, s.dark, s.accent());
            gfx::make_blur(&self.wall, &mut self.blur);
            self.wall_key = key;
            crate::log!("sfondo generato in {} ms", arch::now_ms() - t0);
        }
    }

    pub fn w(&self) -> i32 {
        self.screen.w
    }
    pub fn h(&self) -> i32 {
        self.screen.h
    }

    /// Attende il prossimo frame secondo il profilo scelto, poi legge l'input.
    pub fn next_events(&mut self, s: &Settings) -> Events {
        let target = self.last_frame + s.frame_ms();
        while arch::now_ms() < target {
            arch::pause();
        }
        self.input.poll()
    }

    pub fn present(&mut self) {
        let (mx, my) = (self.input.mx, self.input.my);
        draw_cursor(&mut self.canvas, mx, my);
        self.screen.present(&self.canvas);
        self.last_frame = arch::now_ms();
    }
}

pub fn draw_cursor(c: &mut Canvas, x: i32, y: i32) {
    const ART: [&str; 17] = [
        "X", "XX", "X.X", "X..X", "X...X", "X....X", "X.....X", "X......X", "X.......X", "X........X", "X.....XXXX",
        "X..X..X", "X.X X..X", "XX  X..X", "X    X..X", "     X..X", "      XX",
    ];
    for (dy, row) in ART.iter().enumerate() {
        for (dx, ch) in row.bytes().enumerate() {
            let (px, py) = (x + dx as i32, y + dy as i32);
            if px < 0 || py < 0 || px >= c.w || py >= c.h {
                continue;
            }
            let i = (py * c.w + px) as usize;
            match ch {
                b'X' => c.px[i] = rgb(20, 20, 28),
                b'.' => c.px[i] = WHITE,
                _ => {}
            }
        }
    }
}

/// Logo di MIN-OS: quadrato arrotondato in vetro colorato con una "M" geometrica.
pub fn draw_logo(c: &mut Canvas, x: i32, y: i32, size: i32, accent: u32) {
    let r = size * 28 / 100;
    if size >= 40 {
        c.shadow(x, y, size, size, r, size / 5, 90);
    }
    let c2 = mix(accent, rgb(255, 80, 160), 150);
    c.rrect_dgrad(x, y, size, size, r, accent, c2, 256);
    // riflesso del vetro nella meta' superiore
    let hh = size / 2;
    c.shape(x, y, size, hh, r, |_, py, p, cov| {
        let t = ((hh - (py - y)) * 256 / hh.max(1)) as u32;
        *p = blend(*p, WHITE, cov * (t * 70 >> 8) >> 8);
    });
    c.rrect_border(x, y, size, size, r, WHITE, 150);
    let s = size as f32;
    let (fx, fy) = (x as f32, y as f32);
    let pts = [(0.27, 0.70), (0.27, 0.31), (0.50, 0.56), (0.73, 0.31), (0.73, 0.70)];
    for i in 0..4 {
        let (a, b) = (pts[i], pts[i + 1]);
        c.line(fx + a.0 * s, fy + a.1 * s, fx + b.0 * s, fy + b.1 * s, s * 0.095, WHITE, 256);
    }
}

pub const SPIN: [(f32, f32); 12] = [
    (1.0, 0.0),
    (0.866, 0.5),
    (0.5, 0.866),
    (0.0, 1.0),
    (-0.5, 0.866),
    (-0.866, 0.5),
    (-1.0, 0.0),
    (-0.866, -0.5),
    (-0.5, -0.866),
    (0.0, -1.0),
    (0.5, -0.866),
    (0.866, -0.5),
];

pub fn spinner(c: &mut Canvas, cx: i32, cy: i32, r: i32, color: u32, now: u64) {
    let head = ((now / 80) % 12) as usize;
    for (i, &(dx, dy)) in SPIN.iter().enumerate() {
        let age = (head + 12 - i) % 12;
        let a = 256 - (age as u32 * 18).min(220);
        c.line(
            cx as f32 + dx * r as f32 * 0.55,
            cy as f32 + dy * r as f32 * 0.55,
            cx as f32 + dx * r as f32,
            cy as f32 + dy * r as f32,
            (r as f32 * 0.18).max(2.0),
            color,
            a,
        );
    }
}

pub fn check_mark(c: &mut Canvas, cx: i32, cy: i32, s: f32, color: u32) {
    let (x, y) = (cx as f32, cy as f32);
    c.line(x - s * 0.45, y + s * 0.02, x - s * 0.12, y + s * 0.34, s * 0.2, color, 256);
    c.line(x - s * 0.12, y + s * 0.34, x + s * 0.48, y - s * 0.32, s * 0.2, color, 256);
}

/// Salva una regione del canvas per poter fare dissolvenze dopo.
pub fn snapshot(c: &Canvas, x: i32, y: i32, w: i32, h: i32, buf: &mut Vec<u32>) {
    buf.clear();
    for py in y..y + h {
        for px in x..x + w {
            buf.push(if px >= 0 && py >= 0 && px < c.w && py < c.h { c.px[(py * c.w + px) as usize] } else { 0 });
        }
    }
}

/// Riporta la regione verso lo snapshot: `keep` = quanto del nuovo contenuto mostrare (0..=256).
pub fn fade_to_snapshot(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, buf: &[u32], keep: u32) {
    if keep >= 256 || buf.len() < (w * h) as usize {
        return;
    }
    for py in y.max(0)..(y + h).min(c.h) {
        for px in x.max(0)..(x + w).min(c.w) {
            let i = (py * c.w + px) as usize;
            let old = buf[((py - y) * w + (px - x)) as usize];
            c.px[i] = blend(old, c.px[i], keep);
        }
    }
}

/// Contesto di disegno per un frame.
pub struct Ui<'a> {
    pub c: &'a mut Canvas,
    pub blur: &'a Canvas,
    pub ev: &'a Events,
    pub th: &'a Theme,
    pub mx: i32,
    pub my: i32,
    pub now: u64,
    /// true quando il mouse e' sopra qualcos'altro (es. una finestra in primo piano)
    pub block: bool,
}

impl<'a> Ui<'a> {
    pub fn text_w(&self, s: &str, f: &Font) -> i32 {
        font::text_width(s, f)
    }

    pub fn hover(&self, x: i32, y: i32, w: i32, h: i32) -> bool {
        self.mx >= x && self.my >= y && self.mx < x + w && self.my < y + h
    }

    pub fn click(&self, x: i32, y: i32, w: i32, h: i32) -> bool {
        self.ev.pressed && !self.block && self.hover(x, y, w, h)
    }

    pub fn panel(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32) {
        self.c.shadow(x, y, w, h, r, 34, if self.th.dark { 120 } else { 60 });
        let g = if self.th.glass.blur { self.th.glass } else { self.th.solid(&self.th.glass) };
        self.c.glass(self.blur, x, y, w, h, r, &g);
    }

    pub fn panel_strong(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32) {
        self.c.shadow(x, y, w, h, r, 30, if self.th.dark { 130 } else { 70 });
        let g = if self.th.strong.blur { self.th.strong } else { self.th.solid(&self.th.strong) };
        self.c.glass(self.blur, x, y, w, h, r, &g);
    }

    /// Superficie interna leggera (righe, campi, schede).
    pub fn tile(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, hot: bool) {
        let a = self.th.fill_a + if hot { 14 } else { 0 };
        self.c.rrect(x, y, w, h, r, self.th.fill, a);
        self.c.rrect_border(x, y, w, h, r, WHITE, if self.th.dark { 34 } else { 150 });
    }

    pub fn text(&mut self, x: i32, y: i32, s: &str, f: &Font) -> i32 {
        self.c.text(x, y, s, f, self.th.text, 256)
    }

    pub fn sub(&mut self, x: i32, y: i32, s: &str, f: &Font) -> i32 {
        self.c.text(x, y, s, f, self.th.sub, 256)
    }

    pub fn button(&mut self, x: i32, y: i32, w: i32, h: i32, label: &str, primary: bool, enabled: bool) -> bool {
        let hot = enabled && self.hover(x, y, w, h);
        let r = h / 2;
        if primary {
            self.primary(x, y, w, h, label, false, enabled);
        } else {
            self.tile(x, y, w, h, r, hot);
            let tw = font::text_width(label, &font::BODY_B);
            let col = self.th.text;
            self.c.text(x + (w - tw) / 2, y + (h - 16) / 2 - 1, label, &font::BODY_B, col, if enabled { 256 } else { 110 });
        }
        enabled && self.click(x, y, w, h)
    }

    /// Pulsante principale in vetro colorato (stile iOS), con freccia opzionale.
    pub fn primary(&mut self, x: i32, y: i32, w: i32, h: i32, label: &str, chevron: bool, enabled: bool) -> bool {
        let hot = enabled && self.hover(x, y, w, h);
        let pressed = hot && self.ev.down;
        let r = h / 2;
        let acc = self.th.accent;
        if enabled {
            self.c.glow(x, y, w, h, r, 22, acc, if hot { 120 } else { 80 });
        }
        let tint = if pressed { mix(acc, BLACK, 40) } else if hot { mix(acc, WHITE, 36) } else { acc };
        let g = Glass {
            tint,
            tint_a: if enabled { 212 } else { 110 },
            sheen: 64,
            rim_top: 235,
            rim_bottom: 70,
            refract: true,
            blur: self.th.glass.blur,
        };
        self.c.glass(self.blur, x, y, w, h, r, &g);
        let a = if enabled { 256 } else { 150 };
        let f = &font::BODY_B;
        let tw = font::text_width(label, f);
        let cw = if chevron { 14 } else { 0 };
        let tx = x + (w - tw - cw) / 2;
        let ty = y + (h - 16) / 2 - 1;
        self.c.text(tx, ty, label, f, WHITE, a);
        if chevron {
            let (cx, cy) = ((tx + tw + 9) as f32, (y + h / 2) as f32);
            let off = if hot { 2.0 } else { 0.0 };
            self.c.line(cx + off, cy - 5.0, cx + off + 5.0, cy, 2.4, WHITE, a);
            self.c.line(cx + off + 5.0, cy, cx + off, cy + 5.0, 2.4, WHITE, a);
        }
        enabled && self.click(x, y, w, h)
    }

    /// Riga selezionabile (lista lingue, ecc.).
    pub fn row(&mut self, x: i32, y: i32, w: i32, h: i32, title: &str, detail: &str, selected: bool) -> bool {
        let hot = self.hover(x, y, w, h);
        let r = 14;
        if selected {
            self.c.rrect(x, y, w, h, r, self.th.accent, if self.th.dark { 90 } else { 60 });
            self.c.rrect_border(x, y, w, h, r, mix(self.th.accent, WHITE, 120), 220);
        } else {
            self.tile(x, y, w, h, r, hot);
        }
        let ty = y + (h - 20) / 2 - 1;
        let tw = self.text(x + 18, ty, title, &font::MED_B);
        if !detail.is_empty() {
            let col = self.th.sub;
            self.c.text(x + 18 + tw + 12, ty + 3, detail, &font::BODY, col, 256);
        }
        let (cx, cy) = (x + w - 26, y + h / 2);
        if selected {
            self.c.circle(cx, cy, 11, self.th.accent, 256);
            self.c.circle_border(cx, cy, 11, WHITE, 120);
            check_mark(self.c, cx, cy, 12.0, WHITE);
        } else {
            self.c.circle_border(cx, cy, 11, self.th.text, 70);
        }
        self.click(x, y, w, h)
    }

    /// Scheda selezionabile con titolo e descrizione centrati.
    pub fn card(&mut self, x: i32, y: i32, w: i32, h: i32, title: &str, detail: &str, selected: bool) -> bool {
        let hot = self.hover(x, y, w, h);
        let r = 16;
        if selected {
            self.c.rrect(x, y, w, h, r, self.th.accent, if self.th.dark { 90 } else { 60 });
            self.c.rrect_border(x, y, w, h, r, mix(self.th.accent, WHITE, 120), 230);
        } else {
            self.tile(x, y, w, h, r, hot);
        }
        let col = self.th.text;
        let sub = self.th.sub;
        let total = if detail.is_empty() { 16 } else { 16 + 8 + 16 };
        let ty = y + (h - total) / 2;
        self.c.text_center(x + w / 2, ty, title, &font::BODY_B, col, 256);
        if !detail.is_empty() {
            self.c.text_center(x + w / 2, ty + 24, detail, &font::BODY, sub, 256);
        }
        self.click(x, y, w, h)
    }

    pub fn toggle(&mut self, x: i32, y: i32, on: bool, enabled: bool) -> bool {
        let (w, h) = (52, 30);
        let track = if on { self.th.accent } else if self.th.dark { rgb(80, 82, 100) } else { rgb(200, 202, 212) };
        self.c.rrect(x, y, w, h, h / 2, track, if enabled { 256 } else { 120 });
        self.c.rrect_border(x, y, w, h, h / 2, WHITE, 70);
        let kx = if on { x + w - h / 2 } else { x + h / 2 };
        self.c.shadow(kx - 12, y + 3, 24, 24, 12, 6, 60);
        self.c.circle(kx, y + h / 2, 12, WHITE, 256);
        enabled && self.click(x, y, w, h)
    }

    /// Campo di testo con cursore lampeggiante.
    pub fn field(&mut self, x: i32, y: i32, w: i32, h: i32, value: &str, placeholder: &str, focused: bool) -> bool {
        let hot = self.hover(x, y, w, h);
        let r = 12;
        self.c.rrect(x, y, w, h, r, if self.th.dark { BLACK } else { WHITE }, if self.th.dark { 70 } else { 150 });
        if focused {
            self.c.rrect_border(x - 2, y - 2, w + 4, h + 4, r + 2, self.th.accent, 140);
            self.c.rrect_border(x, y, w, h, r, self.th.accent, 256);
        } else {
            self.c.rrect_border(x, y, w, h, r, WHITE, if hot { 90 } else { 50 });
        }
        let ty = y + (h - 20) / 2;
        let tw = if value.is_empty() {
            let col = self.th.sub;
            self.c.text(x + 16, ty, placeholder, &font::MED, col, if focused { 150 } else { 200 });
            0
        } else {
            self.text(x + 16, ty, value, &font::MED)
        };
        if focused && (self.now / 530) % 2 == 0 {
            self.c.rect(x + 17 + tw, ty, 2, 22, self.th.accent, 256);
        }
        self.click(x, y, w, h)
    }

    /// Cerchio colorato per la scelta dell'accento.
    pub fn swatch(&mut self, cx: i32, cy: i32, r: i32, color: u32, selected: bool) -> bool {
        if selected {
            self.c.circle_border(cx, cy, r + 5, self.th.text, 200);
            self.c.circle_border(cx, cy, r + 4, self.th.text, 120);
        }
        self.c.shadow(cx - r, cy - r, r * 2, r * 2, r, 8, 60);
        self.c.rrect_dgrad(cx - r, cy - r, r * 2, r * 2, r, mix(color, WHITE, 60), color, 256);
        self.c.circle_border(cx, cy, r, WHITE, 110);
        if selected {
            check_mark(self.c, cx, cy, r as f32 * 0.9, WHITE);
        }
        self.click(cx - r - 4, cy - r - 4, r * 2 + 8, r * 2 + 8)
    }
}

pub fn buffer() -> Vec<u32> {
    vec![]
}

/// Accorcia una stringa aggiungendo "..." se supera la larghezza.
pub fn ellipsize(s: &str, max_w: i32, f: &Font) -> String {
    if font::text_width(s, f) <= max_w {
        return String::from(s);
    }
    let mut out = String::new();
    for ch in s.chars() {
        out.push(ch);
        let mut t = out.clone();
        t.push_str("...");
        if font::text_width(&t, f) > max_w {
            out.pop();
            break;
        }
    }
    out.push_str("...");
    out
}
