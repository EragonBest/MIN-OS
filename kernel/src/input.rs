//! Tastiera e mouse PS/2 letti in polling (niente interrupt: semplice e leggero).

use crate::arch::{inb, outb};
use crate::keymap::Layout;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Delete,
    Tab,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Super,
    PageUp,
    PageDown,
    F2,
    F5,
    /// Ctrl + lettera (es. Ctrl+S)
    Ctrl(char),
}

#[derive(Default)]
pub struct Events {
    pub keys: Vec<Key>,
    pub shift_tab: bool,
    /// rotellina: positivo = verso il basso
    pub wheel: i32,
    /// Ctrl tenuto premuto (per la selezione multipla)
    pub ctrl: bool,
    pub shift: bool,
    pub moved: bool,
    pub pressed: bool,
    pub released: bool,
    pub down: bool,
    pub rpressed: bool,
    /// numero di pressioni del tasto sinistro in questo frame (2 = doppio clic veloce)
    pub clicks: u32,
    pub any: bool,
}

impl Events {
    pub fn has(&self, k: Key) -> bool {
        self.keys.contains(&k)
    }
}

pub struct Input {
    pub layout: Layout,
    shift_l: bool,
    shift_r: bool,
    ctrl: bool,
    altgr: bool,
    caps: bool,
    e0: bool,
    pkt: [u8; 4],
    /// 4 = mouse con rotellina (IntelliMouse)
    pkt_len: usize,
    pkt_i: usize,
    pub mx: i32,
    pub my: i32,
    pub mouse_ok: bool,
    btn: bool,
    rbtn: bool,
    w: i32,
    h: i32,
}

fn wait_write() {
    for _ in 0..100_000 {
        if inb(0x64) & 0x02 == 0 {
            return;
        }
    }
}

fn wait_read() -> bool {
    for _ in 0..100_000 {
        if inb(0x64) & 0x01 != 0 {
            return true;
        }
    }
    false
}

fn cmd(c: u8) {
    wait_write();
    outb(0x64, c);
}

fn data(d: u8) {
    wait_write();
    outb(0x60, d);
}

fn mouse_cmd(c: u8) -> bool {
    cmd(0xD4);
    data(c);
    wait_read() && inb(0x60) == 0xFA
}

impl Input {
    pub fn new(w: i32, h: i32) -> Input {
        // inizializza il controller 8042
        cmd(0xAD); // disabilita tastiera
        cmd(0xA7); // disabilita mouse
        while inb(0x64) & 1 != 0 {
            inb(0x60);
        }
        cmd(0x20);
        let mut cfg = if wait_read() { inb(0x60) } else { 0x45 };
        cfg &= !0b0000_0011; // niente IRQ: usiamo il polling
        cfg |= 0b0100_0000; // traduzione in scancode set 1
        cfg &= !0b0011_0000; // clock attivi
        cmd(0x60);
        data(cfg);
        cmd(0xAE); // abilita tastiera
        cmd(0xA8); // abilita mouse
        let mut mouse_ok = mouse_cmd(0xF6);
        // "bussata" IntelliMouse: frequenza 200, 100, 80 -> il mouse attiva la rotellina
        let mut pkt_len = 3;
        if mouse_ok {
            for rate in [200u8, 100, 80] {
                mouse_cmd(0xF3);
                mouse_cmd(rate);
            }
            if mouse_cmd(0xF2) && wait_read() && inb(0x60) == 3 {
                pkt_len = 4;
            }
            mouse_ok = mouse_cmd(0xF4);
        }
        while inb(0x64) & 1 != 0 {
            inb(0x60);
        }
        crate::log!("PS/2 pronto, mouse: {}", mouse_ok);
        Input {
            layout: Layout::It,
            shift_l: false,
            shift_r: false,
            ctrl: false,
            altgr: false,
            caps: false,
            e0: false,
            pkt: [0; 4],
            pkt_len,
            pkt_i: 0,
            mx: w / 2,
            my: h / 2,
            mouse_ok,
            btn: false,
            rbtn: false,
            w,
            h,
        }
    }

    pub fn poll(&mut self) -> Events {
        let mut ev = Events::default();

        let mut guard = 0;
        loop {
            let st = inb(0x64);
            if st & 1 == 0 || guard > 256 {
                break;
            }
            guard += 1;
            let b = inb(0x60);
            if st & 0x20 != 0 {
                self.mouse_byte(b, &mut ev);
            } else {
                self.key_byte(b, &mut ev);
            }
        }
        crate::usb::poll(self, &mut ev);
        ev.down = self.btn;
        ev.pressed = ev.clicks > 0;
        ev.ctrl = self.ctrl;
        ev.shift = self.shift_l || self.shift_r;
        ev.any = ev.wheel != 0 || ev.moved || ev.pressed || ev.released || ev.rpressed || !ev.keys.is_empty() || ev.shift_tab;
        ev
    }

    fn mouse_byte(&mut self, b: u8, ev: &mut Events) {
        if self.pkt_i == 0 && b & 0x08 == 0 {
            return; // fuori sincronia
        }
        self.pkt[self.pkt_i] = b;
        self.pkt_i += 1;
        if self.pkt_i < self.pkt_len {
            return;
        }
        self.pkt_i = 0;
        let f = self.pkt[0];
        if f & 0xC0 != 0 {
            return; // overflow
        }
        let mut dx = self.pkt[1] as i32;
        let mut dy = self.pkt[2] as i32;
        if f & 0x10 != 0 {
            dx -= 256;
        }
        if f & 0x20 != 0 {
            dy -= 256;
        }
        if self.pkt_len == 4 {
            let z = (self.pkt[3] & 0x0F) as i32;
            ev.wheel += if z >= 8 { z - 16 } else { z };
        }
        // PS/2: l'asse Y e' verso l'alto
        self.feed_mouse(dx, -dy, f & 1 != 0, f & 2 != 0, ev);
    }

    /// Movimento e tasti del mouse (usato da PS/2 e USB). `dy` positivo = verso il basso.
    pub fn feed_mouse(&mut self, dx: i32, dy: i32, l: bool, r: bool, ev: &mut Events) {
        // leggera accelerazione
        let acc = |d: i32| if d.abs() > 6 { d * 3 / 2 } else { d };
        let (dx, dy) = (acc(dx), acc(dy));
        if dx != 0 || dy != 0 {
            self.mx = (self.mx + dx).clamp(0, self.w - 1);
            self.my = (self.my + dy).clamp(0, self.h - 1);
            ev.moved = true;
        }
        if l && !self.btn {
            ev.clicks += 1;
        }
        if !l && self.btn {
            ev.released = true;
        }
        if r && !self.rbtn {
            ev.rpressed = true;
        }
        self.btn = l;
        self.rbtn = r;
    }

    /// Tasto in formato scancode set 1 (usato dalle tastiere USB).
    pub fn feed_scancode(&mut self, sc: u8, ext: bool, release: bool, _mods: u8, ev: &mut Events) {
        if ext {
            self.key_byte(0xE0, ev);
        }
        self.key_byte(sc | if release { 0x80 } else { 0 }, ev);
    }

    fn key_byte(&mut self, b: u8, ev: &mut Events) {
        if b == 0xE0 {
            self.e0 = true;
            return;
        }
        let ext = self.e0;
        self.e0 = false;
        let release = b & 0x80 != 0;
        let sc = b & 0x7F;
        match (ext, sc) {
            (false, 0x2A) => self.shift_l = !release,
            (false, 0x36) => self.shift_r = !release,
            (_, 0x1D) => self.ctrl = !release,
            (true, 0x38) => self.altgr = !release,
            (false, 0x3A) if !release => self.caps = !self.caps,
            _ if release => {}
            (true, 0x48) => ev.keys.push(Key::Up),
            (true, 0x50) => ev.keys.push(Key::Down),
            (true, 0x4B) => ev.keys.push(Key::Left),
            (true, 0x4D) => ev.keys.push(Key::Right),
            (true, 0x47) => ev.keys.push(Key::Home),
            (true, 0x49) => ev.keys.push(Key::PageUp),
            (true, 0x51) => ev.keys.push(Key::PageDown),
            (false, 0x3C) => ev.keys.push(Key::F2),
            (false, 0x3F) => ev.keys.push(Key::F5),
            (true, 0x4F) => ev.keys.push(Key::End),
            (true, 0x53) => ev.keys.push(Key::Delete),
            (true, 0x5B) | (true, 0x5C) => ev.keys.push(Key::Super),
            (true, 0x1C) | (false, 0x1C) => ev.keys.push(Key::Enter),
            (false, 0x0E) => ev.keys.push(Key::Backspace),
            (false, 0x01) => ev.keys.push(Key::Esc),
            (false, 0x0F) => {
                if self.shift_l || self.shift_r {
                    ev.shift_tab = true;
                } else {
                    ev.keys.push(Key::Tab)
                }
            }
            (false, _) => {
                let shift = self.shift_l || self.shift_r;
                // AltGr arriva anche come Ctrl+Alt su alcune macchine
                if let Some(c) = self.layout.map(sc, shift, self.altgr, self.caps) {
                    if self.ctrl && !self.altgr && c.is_ascii_alphabetic() {
                        ev.keys.push(Key::Ctrl(c.to_ascii_lowercase()));
                    } else {
                        ev.keys.push(Key::Char(c));
                    }
                }
            }
            _ => {}
        }
    }
}
