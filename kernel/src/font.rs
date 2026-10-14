//! Testo: glifi Noto Sans Mono pre-rasterizzati con antialiasing,
//! resi "proporzionali" tagliando lo spazio vuoto ai lati di ogni lettera.

use crate::gfx::{blend, Canvas};
use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight, RasterizedChar};

#[derive(Clone, Copy, PartialEq)]
pub enum Size {
    S16,
    S20,
    S24,
    S32,
}

#[derive(Clone, Copy)]
pub struct Font {
    pub size: Size,
    pub bold: bool,
    /// fattore di ingrandimento (1 = normale, 2 = titoli grandi)
    pub scale: i32,
}

pub const BODY: Font = Font { size: Size::S16, bold: false, scale: 1 };
pub const BODY_B: Font = Font { size: Size::S16, bold: true, scale: 1 };
pub const MED: Font = Font { size: Size::S20, bold: false, scale: 1 };
pub const MED_B: Font = Font { size: Size::S20, bold: true, scale: 1 };
pub const LARGE: Font = Font { size: Size::S24, bold: true, scale: 1 };
pub const TITLE: Font = Font { size: Size::S32, bold: true, scale: 1 };
pub const HERO: Font = Font { size: Size::S32, bold: true, scale: 2 };

impl Font {
    fn rh(&self) -> RasterHeight {
        match self.size {
            Size::S16 => RasterHeight::Size16,
            Size::S20 => RasterHeight::Size20,
            Size::S24 => RasterHeight::Size24,
            Size::S32 => RasterHeight::Size32,
        }
    }
    fn weight(&self) -> FontWeight {
        if self.bold {
            FontWeight::Bold
        } else {
            FontWeight::Regular
        }
    }
    pub fn height(&self) -> i32 {
        self.rh().val() as i32 * self.scale
    }
    fn slot(&self) -> usize {
        (self.size as usize) * 2 + self.bold as usize
    }
}

fn raster(c: char, f: &Font) -> RasterizedChar {
    get_raster(c, f.weight(), f.rh())
        .or_else(|| get_raster(fallback(c), f.weight(), f.rh()))
        .unwrap_or_else(|| get_raster('?', f.weight(), f.rh()).unwrap())
}

fn fallback(c: char) -> char {
    match c {
        '€' => 'E',
        '‘' | '’' => '\'',
        '“' | '”' => '"',
        '–' | '—' => '-',
        '…' => '.',
        _ => '?',
    }
}

/// Limiti orizzontali dell'"inchiostro" di un glifo (cache per Latin-1).
static mut INK: [[(u8, u8); 256]; 8] = [[(0, 0); 256]; 8];

fn ink(c: char, f: &Font) -> (i32, i32) {
    let code = c as u32;
    let slot = f.slot();
    if code < 256 {
        let e = unsafe { (*core::ptr::addr_of!(INK))[slot][code as usize] };
        if e != (0, 0) {
            return (e.0 as i32, e.1 as i32);
        }
    }
    let r = raster(c, f);
    let w = r.width();
    let (mut lo, mut hi) = (w, 0usize);
    for row in r.raster() {
        for (x, &v) in row.iter().enumerate() {
            if v > 40 {
                lo = lo.min(x);
                hi = hi.max(x + 1);
            }
        }
    }
    let res = if hi <= lo {
        (0, (w * 2 / 5) as i32) // spazio
    } else {
        (lo as i32, hi as i32)
    };
    if code < 256 {
        unsafe { (*core::ptr::addr_of_mut!(INK))[slot][code as usize] = (res.0 as u8, res.1 as u8) };
    }
    res
}

fn tracking(f: &Font) -> i32 {
    match f.size {
        Size::S16 => 1,
        Size::S20 => 2,
        Size::S24 => 2,
        Size::S32 => 2,
    }
}

pub fn char_width(c: char, f: &Font) -> i32 {
    let (lo, hi) = ink(c, f);
    (hi - lo + tracking(f)) * f.scale
}

pub fn text_width(s: &str, f: &Font) -> i32 {
    let w: i32 = s.chars().map(|c| char_width(c, f)).sum();
    if w > 0 {
        w - tracking(f) * f.scale
    } else {
        0
    }
}

/// Larghezza monospazio (per il terminale).
pub fn mono_width(f: &Font) -> i32 {
    get_raster_width(f.weight(), f.rh()) as i32
}

impl Canvas {
    /// Disegna testo; ritorna la larghezza occupata.
    pub fn text(&mut self, x: i32, y: i32, s: &str, f: &Font, color: u32, alpha: u32) -> i32 {
        let mut cx = x;
        for c in s.chars() {
            let (lo, hi) = ink(c, f);
            if c != ' ' {
                self.glyph(cx - lo * f.scale, y, c, f, color, alpha, lo, hi);
            }
            cx += (hi - lo + tracking(f)) * f.scale;
        }
        cx - x
    }

    pub fn text_center(&mut self, cx: i32, y: i32, s: &str, f: &Font, color: u32, alpha: u32) {
        let w = text_width(s, f);
        self.text(cx - w / 2, y, s, f, color, alpha);
    }

    pub fn text_right(&mut self, rx: i32, y: i32, s: &str, f: &Font, color: u32, alpha: u32) {
        let w = text_width(s, f);
        self.text(rx - w, y, s, f, color, alpha);
    }

    /// Testo monospazio (terminale).
    pub fn text_mono(&mut self, x: i32, y: i32, s: &str, f: &Font, color: u32, alpha: u32) {
        let cw = mono_width(f);
        for (i, c) in s.chars().enumerate() {
            if c != ' ' {
                let r = raster(c, f);
                self.glyph(x + i as i32 * cw, y, c, f, color, alpha, 0, r.width() as i32);
            }
        }
    }

    fn glyph(&mut self, x: i32, y: i32, c: char, f: &Font, color: u32, alpha: u32, lo: i32, hi: i32) {
        let r = raster(c, f);
        let rows = r.raster();
        if f.scale == 1 {
            for (ry, row) in rows.iter().enumerate() {
                let py = y + ry as i32;
                if py < 0 || py >= self.h {
                    continue;
                }
                for gx in lo..hi {
                    let v = row[gx as usize] as u32;
                    let px = x + gx;
                    if v == 0 || px < 0 || px >= self.w {
                        continue;
                    }
                    let i = (py * self.w + px) as usize;
                    self.px[i] = blend(self.px[i], color, (v + 1) * alpha >> 8);
                }
            }
            return;
        }
        // ingrandimento bilineare: bordi morbidi anche nei titoli grandi
        let s = f.scale;
        let gw = r.width() as i32;
        let gh = rows.len() as i32;
        let sample = |gx: i32, gy: i32| -> u32 {
            if gx < 0 || gy < 0 || gx >= gw || gy >= gh {
                0
            } else {
                rows[gy as usize][gx as usize] as u32
            }
        };
        for oy in 0..gh * s {
            let py = y + oy;
            if py < 0 || py >= self.h {
                continue;
            }
            let fy = (oy * 256 + 128) / s - 128;
            let (gy, ty) = (fy >> 8, (fy & 0xFF) as u32);
            for ox in lo * s..hi * s {
                let px = x + ox;
                if px < 0 || px >= self.w {
                    continue;
                }
                let fx = (ox * 256 + 128) / s - 128;
                let (gx, tx) = (fx >> 8, (fx & 0xFF) as u32);
                let a = sample(gx, gy) * (256 - tx) + sample(gx + 1, gy) * tx;
                let b = sample(gx, gy + 1) * (256 - tx) + sample(gx + 1, gy + 1) * tx;
                let v = ((a >> 8) * (256 - ty) + (b >> 8) * ty) >> 8;
                // leggero contrasto per non sembrare sfocato
                let v = ((v as i32 - 128) * 5 / 4 + 128).clamp(0, 255) as u32;
                if v > 0 {
                    let i = (py * self.w + px) as usize;
                    self.px[i] = blend(self.px[i], color, (v + 1) * alpha >> 8);
                }
            }
        }
    }
}
