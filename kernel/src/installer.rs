//! Installer di MIN-OS: lingua, tastiera, account, aspetto, ottimizzazione, riepilogo, configurazione.

use crate::arch;
use crate::font;
use crate::gfx::{mix, BLACK, WHITE};
use crate::i18n;
use crate::input::Key;
use crate::keymap;
use crate::ui::{self, check_mark, draw_logo, ease, spinner, Gui, Settings, Ui, ACCENTS};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

const STEPS: usize = 6;
const INSTALL: usize = 6;
const TRANS_MS: u64 = 300;
const MEM_WORDS: usize = 2 * 1024 * 1024; // 16 MiB di test

#[derive(PartialEq)]
enum Act {
    None,
    Next,
    Back,
}

struct Installer {
    s: Settings,
    page: usize,
    trans_t: u64,
    dir: i32,
    off: i32,
    snap: Vec<u32>,
    focus: usize,
    test: String,
    err: bool,
    inst_step: usize,
    step_t: u64,
    mem: Vec<u64>,
    mem_done: usize,
    mem_ok: bool,
    done_t: u64,
    detail: [String; 5],
}

pub fn splash(gui: &mut Gui, s: &Settings) {
    let t0 = arch::now_ms();
    let dur = if s.anim { 1600 } else { 500 };
    loop {
        let now = arch::now_ms();
        let t = now - t0;
        if t > dur {
            break;
        }
        let (w, h) = (gui.w(), gui.h());
        gui.canvas.copy_from(&gui.wall);
        let size = 112;
        let e = ease(t as f32 / 700.0);
        let lift = ((1.0 - e) * 24.0) as i32;
        let (lx, ly) = ((w - size) / 2, h / 2 - size + lift - 10);
        draw_logo(&mut gui.canvas, lx, ly, size, gui.theme.accent);
        gui.canvas.text_center(w / 2, ly + size + 26, "MIN-OS", &font::TITLE, gui.theme.text, 256);
        spinner(&mut gui.canvas, w / 2, ly + size + 100, 13, gui.theme.text, now);
        // dissolvenza dal nero
        let fade = 256 - (ease(t as f32 / 600.0) * 256.0) as u32;
        if fade > 0 {
            gui.canvas.rect(0, 0, w, h, BLACK, fade);
        }
        gui.screen.present(&gui.canvas);
        arch::sleep_ms(8);
    }
}

pub fn run(gui: &mut Gui, s: Settings) -> Settings {
    let mut ins = Installer {
        s,
        page: 0,
        trans_t: 0,
        dir: 1,
        off: 0,
        snap: ui::buffer(),
        focus: 0,
        test: String::new(),
        err: false,
        inst_step: 0,
        step_t: 0,
        mem: Vec::new(),
        mem_done: 0,
        mem_ok: true,
        done_t: 0,
        detail: Default::default(),
    };
    gui.input.layout = ins.s.layout;
    ins.detail[0] = format!("{} core  ·  {} MB", gui.sys.cores, gui.sys.ram_mb);
    let mut last_draw = 0u64;
    let mut first = true;
    loop {
        let ev = gui.next_events(&ins.s);
        let now = arch::now_ms();
        let animating = ins.s.anim && now < ins.trans_t + TRANS_MS + 20;
        let tick = match ins.page {
            0 => 400,
            1 | 2 => 265,
            INSTALL => 0,
            _ => 1000,
        };
        if !first && !ev.any && !animating && now < last_draw + tick {
            arch::pause();
            continue;
        }
        first = false;
        last_draw = now;

        let before = (ins.s.dark, ins.s.accent, ins.s.glass, ins.s.layout, ins.s.lang);
        let finished = ins.frame(gui, &ev, now);
        gui.present();
        if before != (ins.s.dark, ins.s.accent, ins.s.glass, ins.s.layout, ins.s.lang) {
            gui.input.layout = ins.s.layout;
            gui.apply(&ins.s);
        }
        if finished {
            return ins.s;
        }
    }
}

impl Installer {
    fn go(&mut self, page: usize, now: u64) {
        self.dir = if page > self.page { 1 } else { -1 };
        self.page = page;
        self.trans_t = now;
        self.err = false;
        if page == INSTALL {
            self.inst_step = 0;
            self.step_t = now;
        }
    }

    fn valid_account(&self) -> bool {
        let ok = |s: &str| !s.trim().is_empty() && s.chars().count() <= 20;
        ok(&self.s.user) && ok(&self.s.pc)
    }

    fn frame(&mut self, gui: &mut Gui, ev: &crate::input::Events, now: u64) -> bool {
        let (w, h) = (gui.w(), gui.h());
        gui.canvas.copy_from(&gui.wall);
        let cw = (w - 60).min(960);
        let ch = (h - 90).min(600);
        let cx = (w - cw) / 2;
        let cy = (h - ch) / 2 - 10;
        let side = (cw / 4).clamp(190, 240);

        let mut ui = Ui { c: &mut gui.canvas, blur: &gui.blur, ev, th: &gui.theme, mx: gui.input.mx, my: gui.input.my, now, block: false };
        let t = self.s.tr();

        ui.panel(cx, cy, cw, ch, 30);

        // ---- barra laterale con i passaggi
        draw_logo(ui.c, cx + 28, cy + 30, 40, ui.th.accent);
        ui.text(cx + 80, cy + 34, "MIN-OS", &font::LARGE);
        let tcol = ui.th.sub;
        ui.c.text(cx + 81, cy + 58, "Setup", &font::BODY, tcol, 220);
        let sep = if ui.th.dark { WHITE } else { BLACK };
        ui.c.rect(cx + side, cy + 28, 1, ch - 56, sep, 30);
        for i in 0..STEPS {
            let sy = cy + 110 + i as i32 * 52;
            let (dx, dy) = (cx + 44, sy + 14);
            let done = i < self.page;
            let cur = i == self.page;
            if cur {
                ui.c.rrect(cx + 18, sy - 6, side - 36, 40, 14, ui.th.fill, ui.th.fill_a + 10);
            }
            if done {
                ui.c.circle(dx, dy, 12, ui.th.accent, 256);
                check_mark(ui.c, dx, dy, 12.0, WHITE);
            } else if cur {
                ui.c.circle(dx, dy, 12, ui.th.accent, 256);
                ui.c.circle_border(dx, dy, 12, WHITE, 140);
                ui.c.text_center(dx, dy - 8, &format!("{}", i + 1), &font::BODY_B, WHITE, 256);
            } else {
                let tc = ui.th.text;
                ui.c.circle_border(dx, dy, 12, tc, 90);
                ui.c.text_center(dx, dy - 8, &format!("{}", i + 1), &font::BODY_B, tc, 140);
            }
            let col = ui.th.text;
            let label = ui::ellipsize(t.steps[i], side - 90, &font::BODY_B);
            ui.c.text(dx + 24, dy - 8, &label, if cur { &font::BODY_B } else { &font::BODY }, col, if cur || done { 256 } else { 150 });
        }

        // ---- contenuto della pagina (con transizione)
        let (px, py) = (cx + side + 36, cy + 40);
        let pw = cw - side - 36 - 36;
        let ph = ch - 40 - 96;
        let tr = if self.s.anim { ((now.saturating_sub(self.trans_t)) as f32 / TRANS_MS as f32).min(1.0) } else { 1.0 };
        let e = ease(tr);
        self.off = ((1.0 - e) * 40.0 * self.dir as f32) as i32;
        let fading = tr < 1.0;
        if fading {
            ui::snapshot(ui.c, px - 20, cy + 10, pw + 40, ch - 20, &mut self.snap);
        }
        let mut act = match self.page {
            0 => self.page_lang(&mut ui, px, py, pw, ph),
            1 => self.page_kbd(&mut ui, px, py, pw),
            2 => self.page_account(&mut ui, px, py, pw),
            3 => self.page_style(&mut ui, px, py, pw),
            4 => self.page_perf(&mut ui, px, py, pw),
            5 => self.page_summary(&mut ui, px, py, pw),
            _ => {
                if self.page_install(&mut ui, px, py, pw, now) {
                    return true;
                }
                Act::None
            }
        };

        // ---- pulsanti in basso
        if self.page < INSTALL {
            let (bw, bh) = (164, 48);
            let by = cy + ch - 30 - bh;
            let nx = cx + cw - 36 - bw;
            let label = if self.page == STEPS - 1 { t.install } else { t.next };
            let can_next = self.page != 2 || self.valid_account();
            if ui.primary(nx, by, bw, bh, label, self.page < STEPS - 1, true) {
                act = Act::Next;
            }
            if self.page > 0 && ui.button(nx - bw - 14, by, bw, bh, t.back, false, true) {
                act = Act::Back;
            }
            if ev.has(Key::Esc) && self.page > 0 {
                act = Act::Back;
            }
            if act == Act::Next && !can_next {
                self.err = true;
                act = Act::None;
            }
        }
        if fading {
            ui::fade_to_snapshot(ui.c, px - 20, cy + 10, pw + 40, ch - 20, &self.snap, (e * 256.0) as u32);
        }
        // suggerimento sotto la scheda
        let hint = t.hint;
        let sc = ui.th.text;
        ui.c.text_center(w / 2, cy + ch + 20, hint, &font::BODY, sc, 170);

        match act {
            Act::Next => {
                let p = self.page + 1;
                self.go(p, now);
            }
            Act::Back => {
                let p = self.page - 1;
                self.go(p, now);
            }
            Act::None => {}
        }
        false
    }

    fn title(&self, ui: &mut Ui, x: i32, y: i32, title: &str, sub: &str) -> i32 {
        ui.text(x + self.off, y, title, &font::TITLE);
        ui.sub(x + self.off, y + 42, sub, &font::BODY);
        y + 84
    }

    fn page_lang(&mut self, ui: &mut Ui, x: i32, y: i32, w: i32, _h: i32) -> Act {
        let x = x + self.off;
        // saluto che cambia lingua ogni 2 secondi, con dissolvenza
        let period = 2200u64;
        let idx = ((ui.now / period) % 5) as usize;
        let phase = ui.now % period;
        let a = if !self.s.anim {
            256
        } else if phase < 350 {
            (phase * 256 / 350) as u32
        } else if phase > period - 350 {
            ((period - phase) * 256 / 350) as u32
        } else {
            256
        };
        let hello = if self.s.anim { i18n::ALL[idx].tr().hello } else { self.s.tr().hello };
        let col = ui.th.text;
        ui.c.text(x - 4, y - 8, hello, &font::HERO, col, a);
        let t = self.s.tr();
        ui.text(x, y + 66, t.lang_title, &font::LARGE);
        ui.sub(x, y + 98, t.lang_sub, &font::BODY);

        let mut act = Act::None;
        let rh = 50;
        for (i, l) in i18n::ALL.iter().enumerate() {
            let ry = y + 134 + i as i32 * (rh + 8);
            let lt = l.tr();
            if ui.row(x, ry, w, rh, lt.name, lt.region, self.s.lang == *l) {
                self.s.lang = *l;
                self.s.layout = default_layout(*l);
            }
        }
        let cur = i18n::ALL.iter().position(|l| *l == self.s.lang).unwrap_or(0);
        if ui.ev.has(Key::Down) && cur + 1 < 5 {
            self.s.lang = i18n::ALL[cur + 1];
            self.s.layout = default_layout(self.s.lang);
        }
        if ui.ev.has(Key::Up) && cur > 0 {
            self.s.lang = i18n::ALL[cur - 1];
            self.s.layout = default_layout(self.s.lang);
        }
        if ui.ev.has(Key::Enter) {
            act = Act::Next;
        }
        act
    }

    fn page_kbd(&mut self, ui: &mut Ui, x: i32, y: i32, w: i32) -> Act {
        let t = self.s.tr();
        let y = self.title(ui, x, y, t.kbd_title, t.kbd_sub);
        let x = x + self.off;
        let gap = 12;
        let bw = (w - gap * 2) / 3;
        let bh = 76;
        for (i, l) in keymap::ALL.iter().enumerate() {
            let bx = x + (i as i32 % 3) * (bw + gap);
            let by = y + (i as i32 / 3) * (bh + gap);
            if ui.card(bx, by, bw, bh, l.name(), l.sample(), self.s.layout == *l) {
                self.s.layout = *l;
            }
        }
        let cur = keymap::ALL.iter().position(|l| *l == self.s.layout).unwrap_or(0);
        if ui.ev.has(Key::Right) && cur + 1 < 6 {
            self.s.layout = keymap::ALL[cur + 1];
        }
        if ui.ev.has(Key::Left) && cur > 0 {
            self.s.layout = keymap::ALL[cur - 1];
        }
        if ui.ev.has(Key::Down) && cur + 3 < 6 {
            self.s.layout = keymap::ALL[cur + 3];
        }
        if ui.ev.has(Key::Up) && cur >= 3 {
            self.s.layout = keymap::ALL[cur - 3];
        }
        for k in ui.ev.keys.iter() {
            match k {
                Key::Char(c) if self.test.chars().count() < 40 => self.test.push(*c),
                Key::Backspace => {
                    self.test.pop();
                }
                _ => {}
            }
        }
        let fy = y + 2 * (bh + gap) + 18;
        ui.field(x, fy, w, 52, &self.test, t.kbd_test, true);
        if ui.ev.has(Key::Enter) {
            return Act::Next;
        }
        Act::None
    }

    fn page_account(&mut self, ui: &mut Ui, x: i32, y: i32, w: i32) -> Act {
        let t = self.s.tr();
        let y = self.title(ui, x, y, t.acc_title, t.acc_sub);
        let x = x + self.off;
        let fw = w.min(460);
        ui.text(x, y + 4, t.user, &font::BODY_B);
        if ui.field(x, y + 30, fw, 52, &self.s.user, "Mario", self.focus == 0) {
            self.focus = 0;
        }
        ui.text(x, y + 104, t.pc, &font::BODY_B);
        if ui.field(x, y + 130, fw, 52, &self.s.pc, "MIN-PC", self.focus == 1) {
            self.focus = 1;
        }
        let mut act = Act::None;
        for k in ui.ev.keys.iter() {
            let target = if self.focus == 0 { &mut self.s.user } else { &mut self.s.pc };
            match k {
                Key::Char(c) if target.chars().count() < 20 && !c.is_control() => {
                    // il nome del computer non accetta spazi
                    if !(self.focus == 1 && *c == ' ') {
                        target.push(*c);
                    }
                }
                Key::Backspace => {
                    target.pop();
                }
                Key::Tab | Key::Down => self.focus = 1 - self.focus,
                Key::Up => self.focus = 0,
                Key::Enter => {
                    if self.focus == 0 {
                        self.focus = 1;
                    } else {
                        act = Act::Next;
                    }
                }
                _ => {}
            }
        }
        if ui.ev.shift_tab {
            self.focus = 1 - self.focus;
        }
        if self.err && !self.valid_account() {
            let red = 0xFF453A;
            ui.c.circle(x + 8, y + 214, 8, red, 256);
            ui.c.text_center(x + 8, y + 206, "!", &font::BODY_B, WHITE, 256);
            ui.c.text(x + 24, y + 206, t.err_name, &font::BODY, red, 256);
        }
        act
    }

    fn page_style(&mut self, ui: &mut Ui, x: i32, y: i32, w: i32) -> Act {
        let t = self.s.tr();
        let y = self.title(ui, x, y, t.style_title, t.style_sub);
        let x = x + self.off;
        let gap = 16;
        let cw = (w - gap) / 2;
        let chh = 150;
        for i in 0..2 {
            let dark = i == 0;
            let bx = x + i as i32 * (cw + gap);
            let sel = self.s.dark == dark;
            let hot = ui.hover(bx, y, cw, chh);
            if sel {
                ui.c.rrect(bx, y, cw, chh, 18, ui.th.accent, if ui.th.dark { 90 } else { 60 });
                ui.c.rrect_border(bx, y, cw, chh, 18, mix(ui.th.accent, WHITE, 120), 230);
            } else {
                ui.tile(bx, y, cw, chh, 18, hot);
            }
            // anteprima in miniatura
            let (mx, my, mw, mh) = (bx + 14, y + 14, cw - 28, chh - 56);
            let (b1, b2) = if dark { (0x14162A, ACCENTS[self.s.accent]) } else { (0xEEF1F8, mix(ACCENTS[self.s.accent], WHITE, 120)) };
            ui.c.rrect_dgrad(mx, my, mw, mh, 12, b2, b1, 256);
            let pc = if dark { 0x24263A } else { 0xFFFFFF };
            ui.c.rrect(mx + 14, my + 14, mw / 2, mh - 28, 8, pc, 190);
            ui.c.rrect_border(mx + 14, my + 14, mw / 2, mh - 28, 8, WHITE, 120);
            ui.c.rrect(mx + mw / 2 + 24, my + mh - 24, mw / 2 - 38, 12, 6, ACCENTS[self.s.accent], 256);
            let label = if dark { t.dark } else { t.light };
            let col = ui.th.text;
            ui.c.text_center(bx + cw / 2, y + chh - 32, label, &font::BODY_B, col, 256);
            if ui.click(bx, y, cw, chh) {
                self.s.dark = dark;
            }
        }
        if ui.ev.has(Key::Up) || ui.ev.has(Key::Down) {
            self.s.dark = !self.s.dark;
        }
        let ay = y + chh + 26;
        ui.text(x, ay, t.accent, &font::BODY_B);
        for (i, &c) in ACCENTS.iter().enumerate() {
            if ui.swatch(x + 22 + i as i32 * 56, ay + 52, 18, c, self.s.accent == i) {
                self.s.accent = i;
            }
        }
        if ui.ev.has(Key::Right) && self.s.accent < 5 {
            self.s.accent += 1;
        }
        if ui.ev.has(Key::Left) && self.s.accent > 0 {
            self.s.accent -= 1;
        }
        if ui.ev.has(Key::Enter) {
            return Act::Next;
        }
        Act::None
    }

    fn page_perf(&mut self, ui: &mut Ui, x: i32, y: i32, w: i32) -> Act {
        let t = self.s.tr();
        let y = self.title(ui, x, y, t.perf_title, t.perf_sub);
        let x = x + self.off;
        let gap = 12;
        let bw = (w - gap * 2) / 3;
        for i in 0..3 {
            let bx = x + i as i32 * (bw + gap);
            let d = ui::ellipsize(t.presets_d[i], bw - 16, &font::BODY);
            if ui.card(bx, y, bw, 84, t.presets[i], &d, self.s.preset == i) {
                self.s.preset = i;
                if i == 2 {
                    self.s.anim = false;
                } else {
                    self.s.anim = true;
                }
            }
        }
        if ui.ev.has(Key::Right) && self.s.preset < 2 {
            self.s.preset += 1;
        }
        if ui.ev.has(Key::Left) && self.s.preset > 0 {
            self.s.preset -= 1;
        }
        let ry = y + 104;
        let rows = [t.anim, t.glass, t.telemetry];
        for (i, label) in rows.iter().enumerate() {
            let yy = ry + i as i32 * 58;
            ui.tile(x, yy, w, 50, 14, false);
            ui.text(x + 18, yy + 15, label, &font::BODY_B);
            match i {
                0 => {
                    if ui.toggle(x + w - 70, yy + 10, self.s.anim, true) {
                        self.s.anim = !self.s.anim;
                    }
                }
                1 => {
                    if ui.toggle(x + w - 70, yy + 10, self.s.glass, true) {
                        self.s.glass = !self.s.glass;
                    }
                }
                _ => {
                    let col = ui.th.sub;
                    ui.c.text_right(x + w - 20, yy + 16, t.telemetry_v, &font::BODY, col, 256);
                }
            }
        }
        let col = ui.th.sub;
        let st = ui::ellipsize(t.stats, w, &font::BODY);
        ui.c.text(x, ry + 3 * 58 + 8, &st, &font::BODY, col, 220);
        if ui.ev.has(Key::Enter) {
            return Act::Next;
        }
        Act::None
    }

    fn page_summary(&mut self, ui: &mut Ui, x: i32, y: i32, w: i32) -> Act {
        let t = self.s.tr();
        let y = self.title(ui, x, y, t.sum_title, t.sum_sub);
        let x = x + self.off;
        let theme = String::from(if self.s.dark { t.dark } else { t.light });
        let rows: [(&str, String); 6] = [
            (t.language, String::from(t.name)),
            (t.keyboard, String::from(self.s.layout.name())),
            (t.user, self.s.user.clone()),
            (t.pc, self.s.pc.clone()),
            (t.theme, theme),
            (t.profile, String::from(t.presets[self.s.preset])),
        ];
        let rh = 44;
        ui.tile(x, y, w, rh * 6 + 12, 16, false);
        for (i, (k, v)) in rows.iter().enumerate() {
            let ry = y + 6 + i as i32 * rh;
            if i > 0 {
                let sep = if ui.th.dark { WHITE } else { BLACK };
                ui.c.rect(x + 18, ry, w - 36, 1, sep, 26);
            }
            let col = ui.th.sub;
            ui.c.text(x + 18, ry + 13, k, &font::BODY, col, 256);
            let v = ui::ellipsize(v, w / 2, &font::BODY_B);
            let tc = ui.th.text;
            ui.c.text_right(x + w - 18, ry + 13, &v, &font::BODY_B, tc, 256);
            if i == 4 {
                let vw = font::text_width(&v, &font::BODY_B);
                ui.c.circle(x + w - 18 - vw - 14, ry + 21, 6, ACCENTS[self.s.accent], 256);
            }
        }
        if ui.ev.has(Key::Enter) {
            return Act::Next;
        }
        Act::None
    }

    /// Esegue davvero ogni passaggio, spalmato sui frame per restare fluido.
    fn page_install(&mut self, ui: &mut Ui, x: i32, y: i32, w: i32, now: u64) -> bool {
        let t = self.s.tr();
        let min_ms = if self.s.anim { 750 } else { 300 };
        let elapsed = now.saturating_sub(self.step_t);
        let mut frac = 0.0f32;

        if self.done_t == 0 {
            let step_done = match self.inst_step {
                0 => true,
                1 => {
                    if self.mem.is_empty() && self.mem_done == 0 {
                        self.mem = vec![0u64; MEM_WORDS];
                    }
                    let chunk = 256 * 1024;
                    let end = (self.mem_done + chunk).min(MEM_WORDS);
                    for i in self.mem_done..end {
                        self.mem[i] = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xA5A5_5A5A;
                    }
                    for i in self.mem_done..end {
                        if self.mem[i] != (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xA5A5_5A5A {
                            self.mem_ok = false;
                        }
                    }
                    self.mem_done = end;
                    frac = self.mem_done as f32 / MEM_WORDS as f32;
                    if self.mem_done >= MEM_WORDS {
                        self.mem = Vec::new();
                        self.detail[1] = format!("16 MB  ·  {}", if self.mem_ok { "OK" } else { "!" });
                        true
                    } else {
                        false
                    }
                }
                2 => {
                    self.detail[2] = format!("{}  ·  {}", t.name, self.s.layout.code());
                    true
                }
                3 => {
                    self.detail[3] = String::from(t.presets[self.s.preset]);
                    true
                }
                _ => {
                    if self.detail[4].is_empty() {
                        self.detail[4] = match crate::block::info() {
                            Some((kind, mb)) if crate::storage::save(&self.s) => format!("{} {} MB  ·  OK", kind, mb),
                            Some(_) => String::from("!"),
                            None => String::from("RAM"),
                        };
                    }
                    true
                }
            };
            if self.inst_step != 1 {
                frac = (elapsed as f32 / min_ms as f32).min(1.0);
            }
            if step_done && elapsed >= min_ms {
                self.inst_step += 1;
                self.step_t = now;
                if self.inst_step >= 5 {
                    self.done_t = now;
                }
            }
        }

        let x = x + self.off;
        let y = self.title(ui, x, y, t.inst_title, if self.done_t > 0 { t.ready } else { t.inst_sub });
        let total = if self.done_t > 0 { 1.0 } else { (self.inst_step as f32 + frac) / 5.0 };
        // barra di avanzamento in vetro
        let bh = 12;
        ui.c.rrect(x, y, w, bh, bh / 2, ui.th.fill, ui.th.fill_a + 10);
        let fw = ((w as f32 * total) as i32).max(bh);
        let a2 = mix(ui.th.accent, WHITE, 90);
        ui.c.rrect_vgrad(x, y, fw, bh, bh / 2, a2, ui.th.accent, 256);
        ui.c.rrect_border(x, y, w, bh, bh / 2, WHITE, 60);
        let pct = format!("{}%", (total * 100.0) as i32);
        let col = ui.th.sub;
        ui.c.text_right(x + w, y + 20, &pct, &font::BODY_B, col, 256);

        for i in 0..5 {
            let ry = y + 48 + i as i32 * 46;
            let (ix, iy) = (x + 14, ry + 14);
            let done = i < self.inst_step || self.done_t > 0;
            if done {
                ui.c.circle(ix, iy, 12, ui.th.accent, 256);
                check_mark(ui.c, ix, iy, 12.0, WHITE);
            } else if i == self.inst_step {
                let c = ui.th.accent;
                spinner(ui.c, ix, iy, 11, c, now);
            } else {
                let c = ui.th.text;
                ui.c.circle_border(ix, iy, 11, c, 70);
            }
            let tc = ui.th.text;
            ui.c.text(x + 40, ry + 4, t.inst_steps[i], if i == self.inst_step { &font::BODY_B } else { &font::BODY }, tc, if done || i == self.inst_step { 256 } else { 130 });
            if done && !self.detail[i].is_empty() {
                let d = self.detail[i].clone();
                let sc = ui.th.sub;
                ui.c.text_right(x + w, ry + 4, &d, &font::BODY, sc, 256);
            }
        }
        self.done_t > 0 && now > self.done_t + if self.s.anim { 900 } else { 200 }
    }
}

pub fn default_layout(l: i18n::Lang) -> keymap::Layout {
    use i18n::Lang::*;
    use keymap::Layout;
    match l {
        It => Layout::It,
        En => Layout::Us,
        Es => Layout::Es,
        Fr => Layout::Fr,
        De => Layout::De,
    }
}

