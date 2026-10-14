//! Bus PCI: elenco dei dispositivi tramite le porte di configurazione 0xCF8 / 0xCFC.

use crate::arch::{inl, outl};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug)]
pub struct Dev {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
}

fn addr(b: u8, d: u8, f: u8, off: u8) -> u32 {
    0x8000_0000 | (b as u32) << 16 | (d as u32) << 11 | (f as u32) << 8 | (off as u32 & 0xFC)
}

pub fn read(b: u8, d: u8, f: u8, off: u8) -> u32 {
    outl(0xCF8, addr(b, d, f, off));
    inl(0xCFC)
}

pub fn write(b: u8, d: u8, f: u8, off: u8, v: u32) {
    outl(0xCF8, addr(b, d, f, off));
    outl(0xCFC, v);
}

impl Dev {
    pub fn read(&self, off: u8) -> u32 {
        read(self.bus, self.dev, self.func, off)
    }
    pub fn write(&self, off: u8, v: u32) {
        write(self.bus, self.dev, self.func, off, v)
    }

    /// Indirizzo di memoria di un BAR (gestisce i BAR a 64 bit). None se e' un BAR di I/O.
    pub fn bar_mem(&self, i: u8) -> Option<u64> {
        let off = 0x10 + i * 4;
        let lo = self.read(off);
        if lo & 1 != 0 {
            return None;
        }
        let mut a = (lo & !0xF) as u64;
        if (lo >> 1) & 3 == 2 {
            a |= (self.read(off + 4) as u64) << 32;
        }
        Some(a)
    }

    /// Abilita l'accesso alla memoria e il "bus mastering" (necessario per il DMA).
    pub fn enable(&self) {
        let cmd = self.read(0x04);
        self.write(0x04, (cmd & 0xFFFF_0000) | (cmd & 0xFFFF) | 0b110);
    }
}

pub fn scan() -> Vec<Dev> {
    let mut out = Vec::new();
    for bus in 0..=255u8 {
        for dev in 0..32u8 {
            let id = read(bus, dev, 0, 0);
            if id & 0xFFFF == 0xFFFF {
                continue;
            }
            let multi = (read(bus, dev, 0, 0x0C) >> 16) & 0x80 != 0;
            for func in 0..if multi { 8 } else { 1 } {
                let id = read(bus, dev, func, 0);
                if id & 0xFFFF == 0xFFFF {
                    continue;
                }
                let cls = read(bus, dev, func, 0x08);
                out.push(Dev {
                    bus,
                    dev,
                    func,
                    vendor: id as u16,
                    device: (id >> 16) as u16,
                    class: (cls >> 24) as u8,
                    subclass: (cls >> 16) as u8,
                });
            }
        }
    }
    out
}
