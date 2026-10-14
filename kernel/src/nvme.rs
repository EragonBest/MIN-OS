//! Driver NVMe: SSD M.2 / PCIe dei PC moderni. Una coda di I/O, in polling.

use crate::arch::sleep_ms;
use crate::mem::{dma_alloc, phys};
use crate::pci::Dev;
use core::ptr::{read_volatile, write_volatile};

const Q: usize = 16;

pub struct Nvme {
    bar: *mut u8,
    dstrd: usize,
    asq: *mut u32,
    acq: *mut u32,
    iosq: *mut u32,
    iocq: *mut u32,
    a_tail: usize,
    a_head: usize,
    a_phase: bool,
    io_tail: usize,
    io_head: usize,
    io_phase: bool,
    buf: *mut u8,
    cid: u16,
    pub block: usize,
    pub blocks: u64,
}

impl Nvme {
    fn r32(&self, o: usize) -> u32 {
        unsafe { read_volatile(self.bar.add(o) as *const u32) }
    }
    fn w32(&self, o: usize, v: u32) {
        unsafe { write_volatile(self.bar.add(o) as *mut u32, v) }
    }
    fn w64(&self, o: usize, v: u64) {
        self.w32(o, v as u32);
        self.w32(o + 4, (v >> 32) as u32);
    }
    fn doorbell(&self, idx: usize, v: u32) {
        self.w32(0x1000 + idx * (4 << self.dstrd), v);
    }

    /// Invia un comando e attende la risposta. `admin` sceglie la coda.
    fn submit(&mut self, admin: bool, cmd: [u32; 16]) -> bool {
        self.cid = self.cid.wrapping_add(1);
        let (sq, cq, tail, head, phase, qid) = if admin {
            (self.asq, self.acq, &mut self.a_tail, &mut self.a_head, &mut self.a_phase, 0)
        } else {
            (self.iosq, self.iocq, &mut self.io_tail, &mut self.io_head, &mut self.io_phase, 1)
        };
        unsafe {
            let e = sq.add(*tail * 16);
            for (i, v) in cmd.iter().enumerate() {
                write_volatile(e.add(i), *v);
            }
            write_volatile(e, cmd[0] | (self.cid as u32) << 16);
        }
        *tail = (*tail + 1) % Q;
        let t = *tail as u32;
        let db = self.bar;
        let stride = 4usize << self.dstrd;
        unsafe { write_volatile(db.add(0x1000 + (2 * qid) * stride) as *mut u32, t) };
        for _ in 0..50_000_000 {
            let st = unsafe { read_volatile(cq.add(*head * 4 + 3)) };
            if ((st >> 16) & 1 != 0) == *phase {
                let ok = (st >> 17) & 0x7FF == 0;
                *head = (*head + 1) % Q;
                if *head == 0 {
                    *phase = !*phase;
                }
                let h = *head as u32;
                unsafe { write_volatile(db.add(0x1000 + (2 * qid + 1) * stride) as *mut u32, h) };
                return ok;
            }
        }
        false
    }

    pub fn new(dev: &Dev) -> Option<Nvme> {
        let bar_addr = dev.bar_mem(0)?;
        crate::mem::map_mmio(bar_addr, 0x4000);
        let bar = bar_addr as *mut u8;
        dev.enable();
        let cap = unsafe { read_volatile(bar as *const u64) };
        let mut n = Nvme {
            bar,
            dstrd: ((cap >> 32) & 0xF) as usize,
            asq: dma_alloc(4096, 4096) as *mut u32,
            acq: dma_alloc(4096, 4096) as *mut u32,
            iosq: dma_alloc(4096, 4096) as *mut u32,
            iocq: dma_alloc(4096, 4096) as *mut u32,
            a_tail: 0,
            a_head: 0,
            a_phase: true,
            io_tail: 0,
            io_head: 0,
            io_phase: true,
            buf: dma_alloc(4096, 4096),
            cid: 0,
            block: 512,
            blocks: 0,
        };
        // disattiva, configura le code di amministrazione, riattiva
        n.w32(0x14, 0);
        for _ in 0..500 {
            if n.r32(0x1C) & 1 == 0 {
                break;
            }
            sleep_ms(1);
        }
        n.w32(0x24, ((Q - 1) as u32) << 16 | (Q - 1) as u32);
        n.w64(0x28, phys(n.asq as u64));
        n.w64(0x30, phys(n.acq as u64));
        n.w32(0x14, 1 | (6 << 16) | (4 << 20));
        let mut ready = false;
        for _ in 0..2000 {
            if n.r32(0x1C) & 1 != 0 {
                ready = true;
                break;
            }
            sleep_ms(1);
        }
        if !ready {
            return None;
        }
        let b = phys(n.buf as u64);
        // IDENTIFY del namespace 1: dimensione e formato dei blocchi
        if !n.submit(true, [0x06, 1, 0, 0, 0, 0, b as u32, (b >> 32) as u32, 0, 0, 0, 0, 0, 0, 0, 0]) {
            return None;
        }
        unsafe {
            n.blocks = read_volatile(n.buf as *const u64);
            let flbas = *n.buf.add(26) & 0xF;
            let lbaf = read_volatile(n.buf.add(128 + flbas as usize * 4) as *const u32);
            n.block = 1 << ((lbaf >> 16) & 0xFF);
        }
        if n.block < 512 || n.block > 4096 {
            return None;
        }
        // code di I/O: prima quella di completamento, poi quella di invio
        let cq = phys(n.iocq as u64);
        let sq = phys(n.iosq as u64);
        if !n.submit(true, [0x05, 0, 0, 0, 0, 0, cq as u32, (cq >> 32) as u32, 0, 0, ((Q - 1) as u32) << 16 | 1, 1, 0, 0, 0, 0]) {
            return None;
        }
        if !n.submit(true, [0x01, 0, 0, 0, 0, 0, sq as u32, (sq >> 32) as u32, 0, 0, ((Q - 1) as u32) << 16 | 1, (1 << 16) | 1, 0, 0, 0, 0]) {
            return None;
        }
        Some(n)
    }

    fn io(&mut self, write: bool, lba: u64) -> bool {
        let b = phys(self.buf as u64);
        self.submit(false, [if write { 0x01 } else { 0x02 }, 1, 0, 0, 0, 0, b as u32, (b >> 32) as u32, 0, 0, lba as u32, (lba >> 32) as u32, 0, 0, 0, 0])
    }

    /// Lettura di un settore da 512 byte (anche su dischi con blocchi da 4 KiB).
    pub fn read(&mut self, sector: u64, out: &mut [u8; 512]) -> bool {
        let per = (self.block / 512) as u64;
        if !self.io(false, sector / per) {
            return false;
        }
        let off = ((sector % per) * 512) as usize;
        unsafe { core::ptr::copy_nonoverlapping(self.buf.add(off), out.as_mut_ptr(), 512) };
        true
    }

    pub fn write(&mut self, sector: u64, data: &[u8; 512]) -> bool {
        let per = (self.block / 512) as u64;
        if per > 1 && !self.io(false, sector / per) {
            return false;
        }
        let off = ((sector % per) * 512) as usize;
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), self.buf.add(off), 512) };
        self.io(true, sector / per)
    }

    pub fn sectors(&self) -> u64 {
        self.blocks * (self.block as u64 / 512)
    }
}
