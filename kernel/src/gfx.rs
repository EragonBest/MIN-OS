//! Motore grafico di MIN-OS: canvas in RAM, forme antialiasing e materiale "Liquid Glass".
//!
//! Tutto viene disegnato in un back buffer e poi copiato sullo schermo in un colpo solo.
//! Il vetro e' economico: lo sfondo sfocato viene calcolato UNA volta e poi solo campionato.

use crate::mem::fill_u32;
use alloc::vec;
use alloc::vec::Vec;

pub const WHITE: u32 = 0xFFFFFF;
pub const BLACK: u32 = 0x000000;

#[inline(always)]
pub const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    (r << 16) | (g << 8) | b
}

/// Fonde `src` sopra `dst` con opacita' `a` (0..=256).
#[inline(always)]
pub fn blend(dst: u32, src: u32, a: u32) -> u32 {
    if a >= 256 {
        return src;
    }
    let ia = 256 - a;
    let rb = (((src & 0xFF00FF) * a + (dst & 0xFF00FF) * ia) >> 8) & 0xFF00FF;
    let g = (((src & 0x00FF00) * a + (dst & 0x00FF00) * ia) >> 8) & 0x00FF00;
    rb | g
}

/// Interpolazione tra due colori (t 0..=256).
#[inline(always)]
pub fn mix(a: u32, b: u32, t: u32) -> u32 {
    blend(a, b, t.min(256))
}

#[inline(always)]
pub fn sqrt(x: f32) -> f32 {
    use core::arch::x86_64::*;
    unsafe { _mm_cvtss_f32(_mm_sqrt_ss(_mm_set_ss(x))) }
}

#[inline(always)]
fn clampf(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

#[inline(always)]
fn absf(v: f32) -> f32 {
    if v < 0.0 {
        -v
    } else {
        v
    }
}

/// Copertura antialiasing (0..=256) data la distanza con segno dal bordo.
#[inline(always)]
fn cov_from(d: f32) -> u32 {
    (clampf(0.5 - d, 0.0, 1.0) * 256.0) as u32
}

pub fn lum(c: u32) -> u32 {
    (((c >> 16) & 0xFF) * 77 + ((c >> 8) & 0xFF) * 150 + (c & 0xFF) * 29) >> 8
}

/// Stile del materiale vetro.
#[derive(Clone, Copy)]
pub struct Glass {
    pub tint: u32,
    pub tint_a: u32,
    pub sheen: u32,
    pub rim_top: u32,
    pub rim_bottom: u32,
    pub refract: bool,
    pub blur: bool,
}

pub struct Canvas {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
}

impl Canvas {
    pub fn new(w: i32, h: i32) -> Canvas {
        Canvas { w, h, px: vec![0; (w * h) as usize] }
    }

    #[inline(always)]
    pub fn get(&self, x: i32, y: i32) -> u32 {
        let x = x.clamp(0, self.w - 1);
        let y = y.clamp(0, self.h - 1);
        self.px[(y * self.w + x) as usize]
    }

    pub fn copy_from(&mut self, o: &Canvas) {
        self.px.copy_from_slice(&o.px);
    }

    pub fn copy_rect_from(&mut self, o: &Canvas, x: i32, y: i32, w: i32, h: i32) {
        let x0 = x.max(0);
        let x1 = (x + w).min(self.w);
        if x1 <= x0 {
            return;
        }
        for py in y.max(0)..(y + h).min(self.h) {
            let a = (py * self.w + x0) as usize;
            let b = (py * self.w + x1) as usize;
            self.px[a..b].copy_from_slice(&o.px[a..b]);
        }
    }

    pub fn fill(&mut self, c: u32) {
        fill_u32(&mut self.px, c);
    }

    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: u32, a: u32) {
        let x0 = x.max(0);
        let x1 = (x + w).min(self.w);
        if x1 <= x0 {
            return;
        }
        for py in y.max(0)..(y + h).min(self.h) {
            let row = &mut self.px[(py * self.w + x0) as usize..(py * self.w + x1) as usize];
            if a >= 256 {
                fill_u32(row, c);
            } else {
                for p in row.iter_mut() {
                    *p = blend(*p, c, a);
                }
            }
        }
    }

    /// Itera i pixel di un rettangolo arrotondato passando la copertura antialiasing (0..=256).
    #[inline(always)]
    pub fn shape<F: FnMut(i32, i32, &mut u32, u32)>(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, mut f: F) {
        if w <= 0 || h <= 0 {
            return;
        }
        let r = r.min(w / 2).min(h / 2).max(0);
        let (x0, x1) = (x.max(0), (x + w).min(self.w));
        let (y0, y1) = (y.max(0), (y + h).min(self.h));
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let rf = r as f32;
        let sw = self.w;
        for py in y0..y1 {
            let row = (py * sw) as usize;
            let band = py < y + r || py >= y + h - r;
            if !band {
                for px in x0..x1 {
                    f(px, py, &mut self.px[row + px as usize], 256);
                }
                continue;
            }
            let cy = if py < y + r { (y + r) as f32 } else { (y + h - r) as f32 };
            let dy = py as f32 + 0.5 - cy;
            for px in x0..x1 {
                let cov = if px < x + r || px >= x + w - r {
                    let cx = if px < x + r { (x + r) as f32 } else { (x + w - r) as f32 };
                    let dx = px as f32 + 0.5 - cx;
                    cov_from(sqrt(dx * dx + dy * dy) - rf)
                } else {
                    256
                };
                if cov > 0 {
                    f(px, py, &mut self.px[row + px as usize], cov);
                }
            }
        }
    }

    /// Come `shape`, ma solo il bordo spesso 1 pixel.
    #[inline(always)]
    pub fn ring<F: FnMut(i32, i32, &mut u32, u32)>(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, mut f: F) {
        if w <= 1 || h <= 1 {
            return;
        }
        let r = r.min(w / 2).min(h / 2).max(0);
        let rf = r as f32;
        let sw = self.w;
        let (cw, ch) = (self.w, self.h);
        let mut put = |px: i32, py: i32, cov: u32, pxs: &mut Vec<u32>| {
            if px >= 0 && py >= 0 && px < cw && py < ch && cov > 0 {
                f(px, py, &mut pxs[(py * sw + px) as usize], cov);
            }
        };
        for py in y.max(0)..(y + h).min(self.h) {
            let band = py < y + r || py >= y + h - r;
            let edge_row = py == y || py == y + h - 1;
            if !band {
                put(x, py, 256, &mut self.px);
                put(x + w - 1, py, 256, &mut self.px);
                continue;
            }
            let cy = if py < y + r { (y + r) as f32 } else { (y + h - r) as f32 };
            let dy = py as f32 + 0.5 - cy;
            for px in x.max(0)..(x + w).min(self.w) {
                if px < x + r || px >= x + w - r {
                    let cx = if px < x + r { (x + r) as f32 } else { (x + w - r) as f32 };
                    let dx = px as f32 + 0.5 - cx;
                    let d = sqrt(dx * dx + dy * dy) - rf;
                    let cov = (clampf(1.0 - absf(d + 0.5), 0.0, 1.0) * 256.0) as u32;
                    put(px, py, cov, &mut self.px);
                } else if edge_row {
                    put(px, py, 256, &mut self.px);
                }
            }
        }
    }

    pub fn rrect(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, c: u32, a: u32) {
        self.shape(x, y, w, h, r, |_, _, p, cov| *p = blend(*p, c, cov * a >> 8));
    }

    pub fn rrect_border(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, c: u32, a: u32) {
        self.ring(x, y, w, h, r, |_, _, p, cov| *p = blend(*p, c, cov * a >> 8));
    }

    /// Rettangolo arrotondato con sfumatura verticale tra due colori.
    pub fn rrect_vgrad(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, top: u32, bottom: u32, a: u32) {
        let hh = h.max(1);
        self.shape(x, y, w, h, r, |_, py, p, cov| {
            let t = ((py - y) * 256 / hh) as u32;
            *p = blend(*p, mix(top, bottom, t), cov * a >> 8);
        });
    }

    /// Sfumatura diagonale (usata per le icone).
    pub fn rrect_dgrad(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, c1: u32, c2: u32, a: u32) {
        let span = (w + h).max(1);
        self.shape(x, y, w, h, r, |px, py, p, cov| {
            let t = ((px - x + py - y) * 256 / span) as u32;
            *p = blend(*p, mix(c1, c2, t), cov * a >> 8);
        });
    }

    pub fn circle(&mut self, cx: i32, cy: i32, r: i32, c: u32, a: u32) {
        self.rrect(cx - r, cy - r, r * 2, r * 2, r, c, a);
    }

    pub fn circle_border(&mut self, cx: i32, cy: i32, r: i32, c: u32, a: u32) {
        self.rrect_border(cx - r, cy - r, r * 2, r * 2, r, c, a);
    }

    /// Segmento spesso con estremi arrotondati (check, frecce, icone).
    pub fn line(&mut self, ax: f32, ay: f32, bx: f32, by: f32, thick: f32, c: u32, a: u32) {
        let hw = thick / 2.0;
        let minx = (if ax < bx { ax } else { bx } - hw - 1.0) as i32;
        let maxx = (if ax > bx { ax } else { bx } + hw + 2.0) as i32;
        let miny = (if ay < by { ay } else { by } - hw - 1.0) as i32;
        let maxy = (if ay > by { ay } else { by } + hw + 2.0) as i32;
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = (dx * dx + dy * dy).max(0.0001);
        for py in miny.max(0)..maxy.min(self.h) {
            for px in minx.max(0)..maxx.min(self.w) {
                let (qx, qy) = (px as f32 + 0.5 - ax, py as f32 + 0.5 - ay);
                let t = clampf((qx * dx + qy * dy) / len2, 0.0, 1.0);
                let (ex, ey) = (qx - dx * t, qy - dy * t);
                let cov = cov_from(sqrt(ex * ex + ey * ey) - hw);
                if cov > 0 {
                    let i = (py * self.w + px) as usize;
                    self.px[i] = blend(self.px[i], c, cov * a >> 8);
                }
            }
        }
    }

    /// Ombra morbida attorno a un rettangolo arrotondato.
    pub fn shadow(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, spread: i32, a: u32) {
        self.glow(x, y, w, h, r, spread, BLACK, a);
    }

    /// Alone morbido colorato attorno a un rettangolo arrotondato.
    /// Calcolo intero in mezzi pixel: la radice quadrata serve solo negli angoli.
    pub fn glow(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, spread: i32, color: u32, a: u32) {
        let panel_y = y;
        let y = y + spread / 3;
        let spread = spread.clamp(1, 127);
        let r = r.min(w / 2).min(h / 2).max(0);
        let mut lut = [0u32; 128];
        for (i, v) in lut.iter_mut().enumerate().take(spread as usize + 1) {
            let t = (spread - i as i32) as u32;
            *v = t * t * a / (spread * spread) as u32;
        }
        let (x2c, y2c) = (2 * x + w, 2 * y + h);
        let (ex, ey) = (w - 2 * r, h - 2 * r);
        let lim = 2 * spread;
        for py in (y - spread).max(0)..(y + h + spread).min(self.h) {
            let inner_row = py >= panel_y + r && py < panel_y + h - r;
            let qy = (2 * py + 1 - y2c).abs() - ey;
            let row = (py * self.w) as usize;
            let mut px = (x - spread).max(0);
            let end = (x + w + spread).min(self.w);
            while px < end {
                if inner_row && px >= x && px < x + w {
                    px = x + w;
                    continue;
                }
                let qx = (2 * px + 1 - x2c).abs() - ex;
                let d = if qx > 0 && qy > 0 { sqrt((qx * qx + qy * qy) as f32) as i32 } else { qx.max(qy) } - 2 * r;
                if d < lim {
                    let al = lut[(d.max(0) / 2) as usize];
                    if al > 0 {
                        let i = row + px as usize;
                        self.px[i] = blend(self.px[i], color, al);
                    }
                }
                px += 1;
            }
        }
    }

    /// Pannello di vetro liquido: sfondo sfocato + rifrazione sui bordi + riflesso + bordo luminoso.
    pub fn glass(&mut self, bg: &Canvas, x: i32, y: i32, w: i32, h: i32, r: i32, g: &Glass) {
        let zone = 14i32;
        let sheen_h = (h * 45 / 100).max(1);
        let bw = bg.w;
        let (bwm, bhm) = (bg.w - 1, bg.h - 1);
        self.shape(x, y, w, h, r, |px, py, p, cov| {
            let mut c = if g.blur {
                let (mut sx, mut sy) = (px, py);
                if g.refract {
                    let ex = (px - x).min(x + w - 1 - px);
                    let ey = (py - y).min(y + h - 1 - py);
                    if ex < zone {
                        let s = (zone - ex) * (zone - ex) / zone;
                        sx += if px - x < x + w - 1 - px { s } else { -s };
                    }
                    if ey < zone {
                        let s = (zone - ey) * (zone - ey) / zone;
                        sy += if py - y < y + h - 1 - py { s } else { -s };
                    }
                }
                bg.px[(sy.clamp(0, bhm) * bw + sx.clamp(0, bwm)) as usize]
            } else {
                *p
            };
            c = blend(c, g.tint, g.tint_a);
            let ty = py - y;
            if ty < sheen_h && g.sheen > 0 {
                let t = (sheen_h - ty) as u32 * 256 / sheen_h as u32;
                c = blend(c, WHITE, g.sheen * t * t >> 16);
            }
            *p = blend(*p, c, cov);
        });
        let hh = h.max(1) as u32;
        self.ring(x, y, w, h, r, |_, py, p, cov| {
            let t = ((py - y).max(0) as u32 * 256 / hh).min(256);
            let a = (g.rim_top * (256 - t) + g.rim_bottom * t) >> 8;
            *p = blend(*p, WHITE, cov * a >> 8);
        });
    }
}

// ------------------------------------------------------------------ sfondo e sfocatura

fn bilinear_upscale(small: &[u32], sw: i32, sh: i32, out: &mut Canvas, factor: i32, dither: bool) {
    let f = factor;
    for y in 0..out.h {
        // coordinate in virgola fissa 8.8
        let fy = (((y * 256 + 128) / f) - 128).max(0);
        let y0 = (fy >> 8).min(sh - 1);
        let y1 = (y0 + 1).min(sh - 1);
        let ty = (fy & 0xFF) as u32;
        for x in 0..out.w {
            let fx = (((x * 256 + 128) / f) - 128).max(0);
            let x0 = (fx >> 8).min(sw - 1);
            let x1 = (x0 + 1).min(sw - 1);
            let tx = (fx & 0xFF) as u32;
            let a = small[(y0 * sw + x0) as usize];
            let b = small[(y0 * sw + x1) as usize];
            let c = small[(y1 * sw + x0) as usize];
            let d = small[(y1 * sw + x1) as usize];
            let mut v = mix(mix(a, b, tx), mix(c, d, tx), ty);
            if dither {
                // rumore leggerissimo contro le bande di colore
                let mut n = (x as u32).wrapping_mul(374_761_393).wrapping_add((y as u32).wrapping_mul(668_265_263));
                n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
                n ^= n >> 16;
                match n & 7 {
                    0 => v = blend(v, WHITE, 2),
                    1 => v = blend(v, BLACK, 2),
                    _ => {}
                }
            }
            out.px[(y * out.w + x) as usize] = v;
        }
    }
}

fn box_blur(img: &mut [u32], w: i32, h: i32, r: i32) {
    let mut tmp = vec![0u32; img.len()];
    let n = (2 * r + 1) as u32;
    // orizzontale
    for y in 0..h {
        for x in 0..w {
            let (mut sr, mut sg, mut sb) = (0u32, 0u32, 0u32);
            for k in -r..=r {
                let c = img[(y * w + (x + k).clamp(0, w - 1)) as usize];
                sr += (c >> 16) & 0xFF;
                sg += (c >> 8) & 0xFF;
                sb += c & 0xFF;
            }
            tmp[(y * w + x) as usize] = rgb(sr / n, sg / n, sb / n);
        }
    }
    // verticale
    for y in 0..h {
        for x in 0..w {
            let (mut sr, mut sg, mut sb) = (0u32, 0u32, 0u32);
            for k in -r..=r {
                let c = tmp[((y + k).clamp(0, h - 1) * w + x) as usize];
                sr += (c >> 16) & 0xFF;
                sg += (c >> 8) & 0xFF;
                sb += c & 0xFF;
            }
            img[(y * w + x) as usize] = rgb(sr / n, sg / n, sb / n);
        }
    }
}

/// Versione molto sfocata (e un po' piu' satura) di un'immagine: e' la "materia" del vetro.
pub fn make_blur(src: &Canvas, dst: &mut Canvas) {
    let f = 8;
    let sw = (src.w + f - 1) / f;
    let sh = (src.h + f - 1) / f;
    let mut small = vec![0u32; (sw * sh) as usize];
    for sy in 0..sh {
        for sx in 0..sw {
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for y in (sy * f)..((sy + 1) * f).min(src.h) {
                for x in (sx * f)..((sx + 1) * f).min(src.w) {
                    let c = src.px[(y * src.w + x) as usize];
                    r += (c >> 16) & 0xFF;
                    g += (c >> 8) & 0xFF;
                    b += c & 0xFF;
                    n += 1;
                }
            }
            let n = n.max(1);
            let (r, g, b) = (r / n, g / n, b / n);
            // saturazione +25%
            let l = (r * 77 + g * 150 + b * 29) >> 8;
            let sat = |v: u32| ((l as i32 + (v as i32 - l as i32) * 5 / 4).clamp(0, 255)) as u32;
            small[(sy * sw + sx) as usize] = rgb(sat(r), sat(g), sat(b));
        }
    }
    for _ in 0..3 {
        box_blur(&mut small, sw, sh, 2);
    }
    bilinear_upscale(&small, sw, sh, dst, f, false);
}

/// Seno approssimato (serie di Taylor dopo riduzione a [-pi/2, pi/2]).
pub fn sin(x: f32) -> f32 {
    const PI: f32 = 3.141_592_7;
    let mut x = x - ((x / (2.0 * PI)) as i32) as f32 * (2.0 * PI);
    if x > PI {
        x -= 2.0 * PI;
    } else if x < -PI {
        x += 2.0 * PI;
    }
    if x > PI / 2.0 {
        x = PI - x;
    } else if x < -PI / 2.0 {
        x = -PI - x;
    }
    let x2 = x * x;
    x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0)))
}

pub fn cos(x: f32) -> f32 {
    sin(x + 1.570_796_3)
}

fn lerp_rgb(a: u32, b: u32, t: f32) -> u32 {
    mix(a, b, (clampf(t, 0.0, 1.0) * 256.0) as u32)
}

/// Sfondo di MIN-OS: "fogli" curvi sovrapposti che salgono in diagonale,
/// con i bordi illuminati. Tutto calcolato matematicamente, una sola volta.
pub fn make_wallpaper(out: &mut Canvas, dark: bool, accent: u32) {
    const PI: f32 = 3.141_592_7;
    let (w, h) = (out.w as f32, out.h as f32);
    // tavolozza: indaco notte (scuro) o lavanda perla (chiaro), sfumata verso l'accento
    let (bg_top, bg_bot, sheet, lit, rim, shade) = if dark {
        (
            mix(rgb(4, 4, 12), accent, 8),
            mix(rgb(10, 10, 28), accent, 20),
            mix(rgb(9, 9, 24), accent, 16),
            mix(rgb(58, 54, 140), accent, 70),
            mix(rgb(178, 172, 255), accent, 50),
            rgb(1, 1, 5),
        )
    } else {
        (
            mix(rgb(226, 228, 244), accent, 14),
            mix(rgb(206, 206, 236), accent, 26),
            mix(rgb(222, 222, 242), accent, 22),
            mix(rgb(252, 252, 255), accent, 16),
            WHITE,
            mix(rgb(120, 118, 170), accent, 40),
        )
    };
    // (inclinazione, posizione, ampiezza, frequenza, fase, luminosita')
    let sheets: [(f32, f32, f32, f32, f32, f32); 5] = [
        (0.50, 0.18, 0.21, 1.00, 0.3, 0.55),
        (0.58, 0.47, 0.23, 0.92, 1.3, 0.75),
        (0.48, 0.76, 0.19, 1.08, 2.3, 0.92),
        (0.60, 1.05, 0.21, 0.88, 3.2, 1.00),
        (0.55, 1.36, 0.17, 1.15, 4.3, 0.85),
    ];
    let n = sheets.len();
    let mut g = [0f32; 5];
    let mut grad = [0f32; 5];
    let glow_w = w * 0.11;
    for y in 0..out.h {
        let v = (y as f32 + 0.5) / h;
        for i in 0..n {
            let (k, c, amp, fr, ph, _) = sheets[i];
            g[i] = k * v + amp * sin(fr * PI * v + ph) - c;
            let dgdv = k + amp * fr * PI * cos(fr * PI * v + ph);
            grad[i] = sqrt(1.0 / (w * w) + (dgdv / h) * (dgdv / h));
        }
        for x in 0..out.w {
            let u = (x as f32 + 0.5) / w;
            // fondo: sfumatura verticale + alone di luce a destra
            let mut col = lerp_rgb(bg_top, bg_bot, v);
            let (lx, ly) = (u - 0.92, (v - 0.62) * 0.8);
            let l = 1.0 - (lx * lx + ly * ly) * 2.2;
            if l > 0.0 {
                col = lerp_rgb(col, lit, l * l * 0.45);
            }
            for i in 0..n {
                let d = (u + g[i]) / grad[i]; // distanza in pixel dal bordo del foglio
                let bright = sheets[i].5;
                if d > -60.0 && d < 0.0 {
                    // ombra morbida proiettata sul foglio sottostante
                    let t = 1.0 + d / 60.0;
                    col = lerp_rgb(col, shade, t * t * if dark { 0.55 } else { 0.30 });
                }
                if d > -1.0 {
                    let cov = clampf(d + 0.5, 0.0, 1.0);
                    // il foglio: luce vicino al bordo che si spegne verso l'interno
                    let q = d / glow_w;
                    let glow = bright / (1.0 + q * q * 4.0);
                    let mut sc = lerp_rgb(lerp_rgb(sheet, bg_bot, v * 0.5), lit, glow * if dark { 0.62 } else { 0.85 });
                    // luce ambientale da destra
                    if l > 0.0 {
                        sc = lerp_rgb(sc, lit, l * l * 0.35 * bright);
                    }
                    col = lerp_rgb(col, sc, cov);
                    // bordo luminoso sottile con un piccolo alone
                    let ad = absf(d - 0.8);
                    let rim_a = if ad < 1.2 { 1.0 - ad / 1.2 } else { 0.0 } * 0.9 + 0.30 / (1.0 + (d - 0.8) * (d - 0.8) * 0.12);
                    col = lerp_rgb(col, rim, rim_a * bright * if dark { 0.85 } else { 1.0 });
                }
            }
            // dithering leggero contro le bande di colore
            let mut hsh = (x as u32).wrapping_mul(374_761_393).wrapping_add((y as u32).wrapping_mul(668_265_263));
            hsh = (hsh ^ (hsh >> 13)).wrapping_mul(1_274_126_177);
            hsh ^= hsh >> 16;
            match hsh & 7 {
                0 => col = blend(col, WHITE, 3),
                1 => col = blend(col, BLACK, 3),
                _ => {}
            }
            out.px[(y * out.w + x) as usize] = col;
        }
    }
}

// ------------------------------------------------------------------ schermo fisico

pub struct Screen {
    fb: *mut u32,
    pub w: i32,
    pub h: i32,
    stride: usize,
    swap_rb: bool,
}

impl Screen {
    pub fn new(fb: u64, w: u32, h: u32, pitch: u32, red_pos: u32) -> Screen {
        Screen { fb: fb as *mut u32, w: w as i32, h: h as i32, stride: pitch as usize / 4, swap_rb: red_pos == 0 }
    }

    pub fn present(&self, c: &Canvas) {
        let w = self.w.min(c.w) as usize;
        for y in 0..self.h.min(c.h) as usize {
            let src = &c.px[y * c.w as usize..y * c.w as usize + w];
            unsafe {
                let dst = self.fb.add(y * self.stride);
                if self.swap_rb {
                    for (i, &v) in src.iter().enumerate() {
                        let s = (v & 0x00FF00) | ((v >> 16) & 0xFF) | ((v & 0xFF) << 16);
                        dst.add(i).write_volatile(s);
                    }
                } else {
                    core::ptr::copy_nonoverlapping(src.as_ptr(), dst, w);
                }
            }
        }
    }

    /// Scrittura diretta (usata per la schermata di errore, senza allocazioni).
    pub fn raw_fill(&self, c: u32) {
        for y in 0..self.h as usize {
            for x in 0..self.w as usize {
                unsafe { self.fb.add(y * self.stride + x).write_volatile(c) };
            }
        }
    }

    pub fn raw_put(&self, x: i32, y: i32, c: u32) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            unsafe { self.fb.add(y as usize * self.stride + x as usize).write_volatile(c) };
        }
    }
}
