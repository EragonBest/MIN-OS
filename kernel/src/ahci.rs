//! Driver AHCI: dischi SATA dei PC moderni (e QEMU con macchina "q35").
//! Un solo comando alla volta, in polling.

use crate::mem::{dma_alloc, phys};
use crate::pci::Dev;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

pub struct Port {
    regs: *mut u32,
    clb: *mut u32,
    table: *mut u8,
    buf: *mut u8,
    pub sectors: u64,
}

const P_CLB: usize = 0x00;
const P_CLBU: usize = 0x04;
const P_FB: usize = 0x08;
const P_FBU: usize = 0x0C;
const P_IS: usize = 0x10;
const P_CMD: usize = 0x18;
const P_TFD: usize = 0x20;
const P_SIG: usize = 0x24;
const P_SSTS: usize = 0x28;
const P_SERR: usize = 0x30;
const P_CI: usize = 0x38;

impl Port {
    fn rd(&self, r: usize) -> u32 {
        unsafe { read_volatile(self.regs.add(r / 4)) }
    }
    fn wr(&self, r: usize, v: u32) {
        unsafe { write_volatile(self.regs.add(r / 4), v) }
    }

    fn init(regs: *mut u32) -> Option<Port> {
        let mut p = Port { regs, clb: dma_alloc(1024, 1024) as *mut u32, table: dma_alloc(256, 128), buf: dma_alloc(4096, 4096), sectors: 0 };
        // ferma il motore dei comandi
        p.wr(P_CMD, p.rd(P_CMD) & !(1 | (1 << 4)));
        for _ in 0..1_000_000 {
            if p.rd(P_CMD) & ((1 << 15) | (1 << 14)) == 0 {
                break;
            }
        }
        let fis = dma_alloc(256, 256);
        let clb = phys(p.clb as u64);
        let fb = phys(fis as u64);
        p.wr(P_CLB, clb as u32);
        p.wr(P_CLBU, (clb >> 32) as u32);
        p.wr(P_FB, fb as u32);
        p.wr(P_FBU, (fb >> 32) as u32);
        p.wr(P_SERR, 0xFFFF_FFFF);
        p.wr(P_IS, 0xFFFF_FFFF);
        p.wr(P_CMD, p.rd(P_CMD) | (1 << 4));
        p.wr(P_CMD, p.rd(P_CMD) | 1);
        // IDENTIFY per conoscere la dimensione
        if !p.command(0xEC, 0, false) {
            return None;
        }
        let id = unsafe { core::slice::from_raw_parts(p.buf as *const u16, 256) };
        p.sectors = if id[83] & (1 << 10) != 0 {
            id[100] as u64 | (id[101] as u64) << 16 | (id[102] as u64) << 32 | (id[103] as u64) << 48
        } else {
            id[60] as u64 | (id[61] as u64) << 16
        };
        Some(p)
    }

    fn command(&mut self, cmd: u8, lba: u64, write: bool) -> bool {
        for _ in 0..1_000_000 {
            if self.rd(P_TFD) & 0x88 == 0 {
                break;
            }
        }
        unsafe {
            // intestazione del comando 0: FIS di 5 dword, 1 voce PRDT
            let h = self.clb;
            write_volatile(h, 5 | if write { 1 << 6 } else { 0 } | (1 << 16));
            write_volatile(h.add(1), 0);
            let t = phys(self.table as u64);
            write_volatile(h.add(2), t as u32);
            write_volatile(h.add(3), (t >> 32) as u32);
            core::ptr::write_bytes(self.table, 0, 256);
            let f = self.table;
            *f = 0x27; // FIS registro host -> dispositivo
            *f.add(1) = 0x80;
            *f.add(2) = cmd;
            *f.add(4) = lba as u8;
            *f.add(5) = (lba >> 8) as u8;
            *f.add(6) = (lba >> 16) as u8;
            *f.add(7) = 1 << 6;
            *f.add(8) = (lba >> 24) as u8;
            *f.add(9) = (lba >> 32) as u8;
            *f.add(10) = (lba >> 40) as u8;
            *f.add(12) = 1;
            let prdt = self.table.add(0x80) as *mut u32;
            let b = phys(self.buf as u64);
            write_volatile(prdt, b as u32);
            write_volatile(prdt.add(1), (b >> 32) as u32);
            write_volatile(prdt.add(3), 511 | (1 << 31));
        }
        self.wr(P_IS, 0xFFFF_FFFF);
        self.wr(P_CI, 1);
        for _ in 0..10_000_000 {
            if self.rd(P_IS) & (1 << 30) != 0 {
                return false; // errore del disco
            }
            if self.rd(P_CI) & 1 == 0 {
                return self.rd(P_TFD) & 1 == 0;
            }
        }
        false
    }

    pub fn read(&mut self, lba: u64, out: &mut [u8; 512]) -> bool {
        if !self.command(0x25, lba, false) {
            return false;
        }
        unsafe { core::ptr::copy_nonoverlapping(self.buf, out.as_mut_ptr(), 512) };
        true
    }

    pub fn write(&mut self, lba: u64, data: &[u8; 512]) -> bool {
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), self.buf, 512) };
        self.command(0x35, lba, true) && self.command(0xEA, 0, false)
    }
}

/// Tutti i dischi SATA collegati a un controller AHCI.
pub fn probe(dev: &Dev) -> Vec<Port> {
    let mut out = Vec::new();
    let Some(abar) = dev.bar_mem(5) else { return out };
    crate::mem::map_mmio(abar, 0x2000);
    dev.enable();
    let hba = abar as *mut u32;
    unsafe {
        // passaggio di proprieta' BIOS -> sistema operativo, se previsto
        let cap2 = read_volatile(hba.add(0x24 / 4));
        if cap2 & 1 != 0 {
            write_volatile(hba.add(0x28 / 4), read_volatile(hba.add(0x28 / 4)) | 2);
            for _ in 0..1_000_000 {
                if read_volatile(hba.add(0x28 / 4)) & 1 == 0 {
                    break;
                }
            }
        }
        write_volatile(hba.add(1), read_volatile(hba.add(1)) | (1 << 31)); // GHC.AE
        let pi = read_volatile(hba.add(3));
        for i in 0..32 {
            if pi & (1 << i) == 0 {
                continue;
            }
            let regs = (abar + 0x100 + i as u64 * 0x80) as *mut u32;
            let ssts = read_volatile(regs.add(P_SSTS / 4));
            let sig = read_volatile(regs.add(P_SIG / 4));
            if ssts & 0xF != 3 || sig != 0x0000_0101 {
                continue; // nessun disco SATA su questa porta
            }
            if let Some(p) = Port::init(regs) {
                out.push(p);
            }
        }
    }
    out
}
