//! USB 3 / 2 / 1.1 tramite controller xHCI, scritto da zero.
//!
//! Gestisce i dispositivi collegati direttamente alle porte del PC:
//!  - tastiere e mouse USB (protocollo HID "boot", supportato da quasi tutti)
//!  - chiavette e dischi USB (mass storage, comandi SCSI "bulk-only")
//! Funziona in polling, senza interrupt. Gli hub USB esterni non sono ancora supportati.

use crate::arch::{now_ms, sleep_ms};
use crate::input::{Events, Input};
use crate::mem::{dma_alloc, map_mmio, phys};
use crate::pci;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

const RING: usize = 128;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Trb {
    p: u64,
    s: u32,
    c: u32,
}

const T_NORMAL: u32 = 1;
const T_SETUP: u32 = 2;
const T_DATA: u32 = 3;
const T_STATUS: u32 = 4;
const T_LINK: u32 = 6;
const T_ENABLE_SLOT: u32 = 9;
const T_ADDRESS: u32 = 11;
const T_CONFIG_EP: u32 = 12;
const T_EVALUATE: u32 = 13;
const T_TRANSFER_EV: u32 = 32;
const T_CMD_DONE: u32 = 33;

struct Ring {
    trbs: *mut Trb,
    idx: usize,
    cycle: bool,
}

impl Ring {
    fn new() -> Ring {
        let trbs = dma_alloc(RING * 16, 4096) as *mut Trb;
        Ring { trbs, idx: 0, cycle: true }
    }
    fn base(&self) -> u64 {
        phys(self.trbs as u64)
    }
    /// Accoda un TRB e ritorna il suo indirizzo fisico.
    fn push(&mut self, mut t: Trb) -> u64 {
        t.c = (t.c & !1) | self.cycle as u32;
        let addr = phys(unsafe { self.trbs.add(self.idx) } as u64);
        unsafe { write_volatile(self.trbs.add(self.idx), t) };
        self.idx += 1;
        if self.idx == RING - 1 {
            let link = Trb { p: self.base(), s: 0, c: (T_LINK << 10) | 2 | self.cycle as u32 };
            unsafe { write_volatile(self.trbs.add(self.idx), link) };
            self.idx = 0;
            self.cycle = !self.cycle;
        }
        addr
    }
}

enum Kind {
    Keyboard { dci: u8, ring: Ring, buf: *mut u8, prev: [u8; 8], held: u8, next_repeat: u64, polls: u32 },
    Mouse { dci: u8, ring: Ring, buf: *mut u8 },
    Storage { in_dci: u8, out_dci: u8, inr: Ring, outr: Ring, buf: *mut u8, tag: u32, blocks: u64, bsize: u32 },
}

struct Device {
    slot: u8,
    ep0: Ring,
    kind: Kind,
}

struct Xhci {
    op: *mut u8,
    db: *mut u32,
    ir: *mut u8,
    csz: usize,
    ports: u8,
    dcbaa: *mut u64,
    cmd: Ring,
    ev: *mut Trb,
    ev_idx: usize,
    ev_cycle: bool,
    devices: Vec<Device>,
    scratch: *mut u8,
    /// eventi di tastiera/mouse arrivati mentre si aspettava altro (es. il disco)
    pending: Vec<Trb>,
}

static mut CTRLS: Vec<Xhci> = Vec::new();
/// chiavette: (controller, dispositivo)
static mut DISKS: Vec<(usize, usize)> = Vec::new();

fn ctrls() -> &'static mut Vec<Xhci> {
    unsafe { &mut *core::ptr::addr_of_mut!(CTRLS) }
}
fn disks() -> &'static mut Vec<(usize, usize)> {
    unsafe { &mut *core::ptr::addr_of_mut!(DISKS) }
}

fn r32(b: *mut u8, o: usize) -> u32 {
    unsafe { read_volatile(b.add(o) as *const u32) }
}
fn w32(b: *mut u8, o: usize, v: u32) {
    unsafe { write_volatile(b.add(o) as *mut u32, v) }
}
fn w64(b: *mut u8, o: usize, v: u64) {
    w32(b, o, v as u32);
    w32(b, o + 4, (v >> 32) as u32);
}

const PORT_KEEP: u32 = 0x0E00_C200; // bit RW da preservare scrivendo PORTSC

impl Xhci {
    fn portsc(&self, p: u8) -> usize {
        0x400 + 0x10 * (p as usize - 1)
    }

    fn ring_db(&self, slot: u8, target: u32) {
        unsafe { write_volatile(self.db.add(slot as usize), target) };
    }

    /// Prossimo evento, se c'e'.
    fn next_event(&mut self) -> Option<Trb> {
        let t = unsafe { read_volatile(self.ev.add(self.ev_idx)) };
        if (t.c & 1 != 0) != self.ev_cycle {
            return None;
        }
        self.ev_idx += 1;
        if self.ev_idx == RING {
            self.ev_idx = 0;
            self.ev_cycle = !self.ev_cycle;
        }
        let deq = phys(unsafe { self.ev.add(self.ev_idx) } as u64);
        w64(self.ir, 0x18, deq | 8);
        Some(t)
    }

    /// Attende un evento che soddisfa `f`; gli eventi di trasferimento degli altri
    /// dispositivi vengono conservati per `poll` (altrimenti tastiera e mouse si bloccano).
    fn wait(&mut self, ms: u64, f: impl Fn(&Trb) -> bool) -> Option<Trb> {
        let end = now_ms() + ms;
        while now_ms() < end {
            if let Some(t) = self.next_event() {
                if f(&t) {
                    return Some(t);
                }
                if (t.c >> 10) & 63 == T_TRANSFER_EV && self.pending.len() < 64 {
                    self.pending.push(t);
                }
            } else {
                crate::arch::pause();
            }
        }
        None
    }

    fn command(&mut self, t: Trb) -> Option<Trb> {
        let addr = self.cmd.push(t);
        self.ring_db(0, 0);
        let ev = self.wait(500, |e| (e.c >> 10) & 63 == T_CMD_DONE && e.p == addr)?;
        if ev.s >> 24 == 1 {
            Some(ev)
        } else {
            None
        }
    }

    fn wait_transfer(&mut self, slot: u8, dci: u8, ms: u64) -> Option<u32> {
        let ev = self.wait(ms, |e| (e.c >> 10) & 63 == T_TRANSFER_EV && (e.c >> 24) as u8 == slot && ((e.c >> 16) & 31) as u8 == dci)?;
        let code = ev.s >> 24;
        if code == 1 || code == 13 {
            Some(ev.s & 0xFF_FFFF)
        } else {
            None
        }
    }

    fn control(&mut self, slot: u8, ep0: &mut Ring, rt: u8, req: u8, value: u16, index: u16, len: u16, buf: u64) -> bool {
        let input = rt & 0x80 != 0;
        let trt = if len == 0 { 0 } else if input { 3 } else { 2 };
        let setup = (rt as u64) | (req as u64) << 8 | (value as u64) << 16 | (index as u64) << 32 | (len as u64) << 48;
        ep0.push(Trb { p: setup, s: 8, c: (T_SETUP << 10) | (1 << 6) | (trt << 16) });
        if len > 0 {
            ep0.push(Trb { p: buf, s: len as u32, c: (T_DATA << 10) | if input { 1 << 16 } else { 0 } });
        }
        let dir = if len > 0 && input { 0 } else { 1 << 16 };
        ep0.push(Trb { p: 0, s: 0, c: (T_STATUS << 10) | (1 << 5) | dir });
        self.ring_db(slot, 1);
        self.wait_transfer(slot, 1, 1000).is_some()
    }

    fn init(dev: &pci::Dev) -> Option<Xhci> {
        let bar = dev.bar_mem(0)?;
        map_mmio(bar, 0x10000);
        dev.enable();
        let cap = bar as *mut u8;
        let caplen = unsafe { read_volatile(cap) } as usize;
        let hcs1 = r32(cap, 0x04);
        let hcs2 = r32(cap, 0x08);
        let hcc1 = r32(cap, 0x10);
        let op = unsafe { cap.add(caplen) };
        let db = unsafe { cap.add(r32(cap, 0x14) as usize & !3) } as *mut u32;
        let rt = unsafe { cap.add(r32(cap, 0x18) as usize & !0x1F) };
        let slots = (hcs1 & 0xFF).min(32) as u8;
        let ports = (hcs1 >> 24) as u8;

        // il BIOS ci lascia il controller (fine dell'emulazione PS/2 via SMI)
        let mut x = (hcc1 >> 16) as usize * 4;
        while x != 0 {
            let v = r32(cap, x);
            if v & 0xFF == 1 {
                w32(cap, x, v | (1 << 24));
                for _ in 0..1000 {
                    if r32(cap, x) & (1 << 16) == 0 {
                        break;
                    }
                    sleep_ms(1);
                }
                w32(cap, x + 4, r32(cap, x + 4) & 0xE000_0000);
            }
            let next = ((v >> 8) & 0xFF) as usize * 4;
            x = if next == 0 { 0 } else { x + next };
        }

        // stop e reset
        w32(op, 0x00, r32(op, 0x00) & !1);
        for _ in 0..200 {
            if r32(op, 0x04) & 1 != 0 {
                break;
            }
            sleep_ms(1);
        }
        w32(op, 0x00, 2);
        for _ in 0..1000 {
            if r32(op, 0x00) & 2 == 0 && r32(op, 0x04) & (1 << 11) == 0 {
                break;
            }
            sleep_ms(1);
        }
        w32(op, 0x38, slots as u32);

        let dcbaa = dma_alloc(2048, 4096) as *mut u64;
        let nscratch = (((hcs2 >> 21) & 0x1F) << 5 | (hcs2 >> 27)) as usize;
        let mut scratch = core::ptr::null_mut();
        if nscratch > 0 {
            let arr = dma_alloc(nscratch * 8, 4096) as *mut u64;
            for i in 0..nscratch {
                unsafe { *arr.add(i) = phys(dma_alloc(4096, 4096) as u64) };
            }
            unsafe { *dcbaa = phys(arr as u64) };
            scratch = arr as *mut u8;
        }
        w64(op, 0x30, phys(dcbaa as u64));
        let cmd = Ring::new();
        w64(op, 0x18, cmd.base() | 1);

        // anello degli eventi (interrupter 0)
        let ev = dma_alloc(RING * 16, 4096) as *mut Trb;
        let erst = dma_alloc(64, 64) as *mut u64;
        unsafe {
            *erst = phys(ev as u64);
            *(erst.add(1) as *mut u32) = RING as u32;
        }
        let ir = unsafe { rt.add(0x20) };
        w32(ir, 0x08, 1);
        w64(ir, 0x18, phys(ev as u64));
        w64(ir, 0x10, phys(erst as u64));

        w32(op, 0x00, 1); // avvio
        for _ in 0..200 {
            if r32(op, 0x04) & 1 == 0 {
                break;
            }
            sleep_ms(1);
        }
        let mut c = Xhci {
            op,
            db,
            ir,
            csz: if hcc1 & 4 != 0 { 64 } else { 32 },
            ports,
            dcbaa,
            cmd,
            ev,
            ev_idx: 0,
            ev_cycle: true,
            devices: Vec::new(),
            scratch,
            pending: Vec::new(),
        };
        sleep_ms(50);
        for p in 1..=ports {
            c.port(p);
        }
        Some(c)
    }

    fn port(&mut self, p: u8) {
        let reg = self.portsc(p);
        let sc = r32(self.op, reg);
        if sc & 1 == 0 {
            return; // niente collegato
        }
        if sc & 2 == 0 {
            // porta USB 2: reset per abilitarla
            w32(self.op, reg, (sc & PORT_KEEP) | (1 << 4));
            for _ in 0..500 {
                if r32(self.op, reg) & (1 << 21) != 0 {
                    break;
                }
                sleep_ms(1);
            }
        }
        let sc = r32(self.op, reg);
        w32(self.op, reg, (sc & PORT_KEEP) | 0x00FE_0000); // pulisci i bit di cambiamento
        if sc & 2 == 0 {
            return;
        }
        sleep_ms(20);
        let speed = (sc >> 10) & 0xF;
        if let Some(d) = self.enumerate(p, speed) {
            self.devices.push(d);
        }
    }

    fn ctx(&self, base: *mut u8, i: usize) -> *mut u32 {
        unsafe { base.add(i * self.csz) as *mut u32 }
    }

    fn enumerate(&mut self, port: u8, speed: u32) -> Option<Device> {
        let ev = self.command(Trb { p: 0, s: 0, c: T_ENABLE_SLOT << 10 })?;
        let slot = (ev.c >> 24) as u8;
        let out = dma_alloc(self.csz * 32, 4096);
        unsafe { *self.dcbaa.add(slot as usize) = phys(out as u64) };
        let input = dma_alloc(self.csz * 33, 4096);
        let mut ep0 = Ring::new();
        let mut mps: u32 = match speed {
            2 | 1 => 8,
            3 => 64,
            _ => 512,
        };
        unsafe {
            write_volatile(self.ctx(input, 0).add(1), 0b11);
            let sl = self.ctx(input, 1);
            write_volatile(sl, (1 << 27) | (speed << 20));
            write_volatile(sl.add(1), (port as u32) << 16);
            let e0 = self.ctx(input, 2);
            write_volatile(e0.add(1), (3 << 1) | (4 << 3) | (mps << 16));
            let deq = ep0.base() | 1;
            write_volatile(e0.add(2), deq as u32);
            write_volatile(e0.add(3), (deq >> 32) as u32);
            write_volatile(e0.add(4), 8);
        }
        self.command(Trb { p: phys(input as u64), s: 0, c: (T_ADDRESS << 10) | (slot as u32) << 24 })?;
        sleep_ms(5);
        let buf = dma_alloc(512, 4096);
        let bp = phys(buf as u64);
        // primi 8 byte del descrittore: dimensione reale dei pacchetti di controllo
        if !self.control(slot, &mut ep0, 0x80, 6, 0x0100, 0, 8, bp) {
            return None;
        }
        let real = unsafe { *buf.add(7) } as u32;
        if speed <= 2 && real != mps && real >= 8 {
            mps = real;
            unsafe {
                core::ptr::write_bytes(input, 0, self.csz * 33);
                write_volatile(self.ctx(input, 0).add(1), 0b10);
                write_volatile(self.ctx(input, 2).add(1), (3 << 1) | (4 << 3) | (mps << 16));
            }
            self.command(Trb { p: phys(input as u64), s: 0, c: (T_EVALUATE << 10) | (slot as u32) << 24 });
        }
        // descrittore di configurazione
        if !self.control(slot, &mut ep0, 0x80, 6, 0x0200, 0, 9, bp) {
            return None;
        }
        let total = (unsafe { *buf.add(2) } as u16 | (unsafe { *buf.add(3) } as u16) << 8).min(512);
        if !self.control(slot, &mut ep0, 0x80, 6, 0x0200, 0, total, bp) {
            return None;
        }
        let cfg: Vec<u8> = unsafe { core::slice::from_raw_parts(buf, total as usize) }.to_vec();
        let cfg_value = cfg[5];

        // cerca un'interfaccia che sappiamo gestire
        let mut i = 0;
        let mut found: Option<(u8, u8, u8, Vec<(u8, u8, u16, u8)>)> = None; // (iface, class, proto, endpoints)
        let mut cur: Option<(u8, u8, u8, u8)> = None;
        let mut eps: Vec<(u8, u8, u16, u8)> = Vec::new();
        while i + 2 <= cfg.len() {
            let len = cfg[i] as usize;
            if len < 2 || i + len > cfg.len() {
                break;
            }
            match cfg[i + 1] {
                4 if len >= 9 => {
                    if let Some((n, c, s, p)) = cur.take() {
                        if wanted(c, s, p) && found.is_none() {
                            found = Some((n, c, p, core::mem::take(&mut eps)));
                        }
                    }
                    eps.clear();
                    cur = Some((cfg[i + 2], cfg[i + 5], cfg[i + 6], cfg[i + 7]));
                }
                5 if len >= 7 => eps.push((cfg[i + 2], cfg[i + 3] & 3, u16::from_le_bytes([cfg[i + 4], cfg[i + 5]]) & 0x7FF, cfg[i + 6])),
                _ => {}
            }
            i += len;
        }
        if let Some((n, c, s, p)) = cur {
            if wanted(c, s, p) && found.is_none() {
                found = Some((n, c, p, eps));
            }
        }
        let (iface, class, proto, eps) = found?;
        if !self.control(slot, &mut ep0, 0x00, 9, cfg_value as u16, 0, 0, 0) {
            return None;
        }

        // configura gli endpoint
        let mut rings: Vec<(u8, Ring)> = Vec::new();
        unsafe { core::ptr::write_bytes(input, 0, self.csz * 33) };
        let mut add = 1u32;
        let mut max_dci = 1u8;
        for &(addr, typ, emps, interval) in eps.iter() {
            let inb = addr & 0x80 != 0;
            let dci = (addr & 0xF) * 2 + inb as u8;
            let ep_type = match (typ, inb) {
                (3, true) => 7,
                (2, true) => 6,
                (2, false) => 2,
                (3, false) => 3,
                _ => continue,
            };
            let ring = Ring::new();
            let ivl: u32 = if typ == 3 {
                if speed <= 2 {
                    let frames = (interval.max(1) as u32) * 8;
                    31 - frames.leading_zeros()
                } else {
                    (interval.max(1) - 1) as u32
                }
            } else {
                0
            };
            unsafe {
                let e = self.ctx(input, dci as usize + 1);
                write_volatile(e, ivl << 16);
                write_volatile(e.add(1), (3 << 1) | (ep_type << 3) | ((emps as u32) << 16));
                let deq = ring.base() | 1;
                write_volatile(e.add(2), deq as u32);
                write_volatile(e.add(3), (deq >> 32) as u32);
                write_volatile(e.add(4), if typ == 3 { (emps as u32) << 16 | emps as u32 } else { 512 });
            }
            add |= 1 << dci;
            max_dci = max_dci.max(dci);
            rings.push((dci, ring));
        }
        unsafe {
            write_volatile(self.ctx(input, 0).add(1), add);
            write_volatile(self.ctx(input, 1), ((max_dci as u32) << 27) | (speed << 20));
            write_volatile(self.ctx(input, 1).add(1), (port as u32) << 16);
        }
        self.command(Trb { p: phys(input as u64), s: 0, c: (T_CONFIG_EP << 10) | (slot as u32) << 24 })?;

        let kind = if class == 3 {
            // HID: protocollo "boot", nessuna ripetizione automatica dal dispositivo
            self.control(slot, &mut ep0, 0x21, 0x0B, 0, iface as u16, 0, 0);
            self.control(slot, &mut ep0, 0x21, 0x0A, 0, iface as u16, 0, 0);
            let (dci, mut ring) = rings.into_iter().find(|(d, _)| d % 2 == 1)?;
            let b = dma_alloc(64, 64);
            ring.push(Trb { p: phys(b as u64), s: 8, c: (T_NORMAL << 10) | (1 << 5) | (1 << 2) });
            self.ring_db(slot, dci as u32);
            if proto == 1 {
                crate::log!("USB: tastiera sulla porta {}", port);
                Kind::Keyboard { dci, ring, buf: b, prev: [0; 8], held: 0, next_repeat: 0, polls: 0 }
            } else {
                crate::log!("USB: mouse sulla porta {}", port);
                Kind::Mouse { dci, ring, buf: b }
            }
        } else {
            let mut in_r = None;
            let mut out_r = None;
            for (dci, r) in rings {
                if dci % 2 == 1 {
                    in_r = Some((dci, r));
                } else {
                    out_r = Some((dci, r));
                }
            }
            let ((in_dci, inr), (out_dci, outr)) = (in_r?, out_r?);
            crate::log!("USB: memoria di massa sulla porta {}", port);
            Kind::Storage { in_dci, out_dci, inr, outr, buf: dma_alloc(4096, 4096), tag: 1, blocks: 0, bsize: 512 }
        };
        let mut d = Device { slot, ep0, kind };
        if let Kind::Storage { .. } = d.kind {
            if !self.storage_init(&mut d) {
                return None;
            }
        }
        Some(d)
    }

    // ------------------------------------------------------------ memoria di massa (SCSI)

    fn scsi(&mut self, d: &mut Device, cdb: &[u8], len: u32, data_in: bool) -> bool {
        let slot = d.slot;
        let Kind::Storage { in_dci, out_dci, inr, outr, buf, tag, .. } = &mut d.kind else { return false };
        let (in_dci, out_dci, buf) = (*in_dci, *out_dci, *buf);
        *tag = tag.wrapping_add(1);
        let cbw = unsafe { buf.add(3584) };
        unsafe {
            core::ptr::write_bytes(cbw, 0, 31);
            core::ptr::copy_nonoverlapping(0x4342_5355u32.to_le_bytes().as_ptr(), cbw, 4);
            core::ptr::copy_nonoverlapping(tag.to_le_bytes().as_ptr(), cbw.add(4), 4);
            core::ptr::copy_nonoverlapping(len.to_le_bytes().as_ptr(), cbw.add(8), 4);
            *cbw.add(12) = if data_in { 0x80 } else { 0 };
            *cbw.add(14) = cdb.len() as u8;
            core::ptr::copy_nonoverlapping(cdb.as_ptr(), cbw.add(15), cdb.len());
        }
        outr.push(Trb { p: phys(cbw as u64), s: 31, c: (T_NORMAL << 10) | (1 << 5) });
        self.ring_db(slot, out_dci as u32);
        if self.wait_transfer(slot, out_dci, 2000).is_none() {
            return false;
        }
        if len > 0 {
            let r = if data_in { &mut *inr } else { &mut *outr };
            r.push(Trb { p: phys(buf as u64), s: len, c: (T_NORMAL << 10) | (1 << 5) });
            let dci = if data_in { in_dci } else { out_dci };
            self.ring_db(slot, dci as u32);
            if self.wait_transfer(slot, dci, 3000).is_none() {
                return false;
            }
        }
        let csw = unsafe { buf.add(3840) };
        inr.push(Trb { p: phys(csw as u64), s: 13, c: (T_NORMAL << 10) | (1 << 5) });
        self.ring_db(slot, in_dci as u32);
        if self.wait_transfer(slot, in_dci, 2000).is_none() {
            return false;
        }
        unsafe { read_volatile(csw as *const u32) == 0x5342_5355 && *csw.add(12) == 0 }
    }

    fn storage_init(&mut self, d: &mut Device) -> bool {
        for _ in 0..5 {
            if self.scsi(d, &[0, 0, 0, 0, 0, 0], 0, false) {
                break;
            }
            self.scsi(d, &[3, 0, 0, 0, 18, 0], 18, true); // REQUEST SENSE
            sleep_ms(50);
        }
        if !self.scsi(d, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], 8, true) {
            return false;
        }
        if let Kind::Storage { buf, blocks, bsize, .. } = &mut d.kind {
            let b = unsafe { core::slice::from_raw_parts(*buf, 8) };
            *blocks = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64 + 1;
            *bsize = u32::from_be_bytes([b[4], b[5], b[6], b[7]]);
            return *bsize == 512;
        }
        false
    }

    fn rw(&mut self, dev: usize, lba: u64, data: *mut u8, write: bool) -> bool {
        let mut d = self.devices.remove(dev);
        let cdb = [if write { 0x2A } else { 0x28 }, 0, (lba >> 24) as u8, (lba >> 16) as u8, (lba >> 8) as u8, lba as u8, 0, 0, 1, 0];
        let buf = if let Kind::Storage { buf, .. } = d.kind { buf } else { core::ptr::null_mut() };
        let mut ok = false;
        if !buf.is_null() {
            if write {
                unsafe { core::ptr::copy_nonoverlapping(data, buf, 512) };
            }
            for _ in 0..3 {
                if self.scsi(&mut d, &cdb, 512, !write) {
                    ok = true;
                    break;
                }
            }
            if ok && !write {
                unsafe { core::ptr::copy_nonoverlapping(buf, data, 512) };
            }
        }
        self.devices.insert(dev, d);
        ok
    }

    // ------------------------------------------------------------ tastiera e mouse

    fn poll(&mut self, input: &mut Input, ev: &mut Events) {
        let now = now_ms();
        let mut queue = core::mem::take(&mut self.pending);
        while let Some(e) = self.next_event() {
            queue.push(e);
        }
        for t in queue {
            if (t.c >> 10) & 63 != T_TRANSFER_EV {
                continue;
            }
            let (slot, dci) = ((t.c >> 24) as u8, ((t.c >> 16) & 31) as u8);
            let ok = matches!(t.s >> 24, 1 | 13);
            let db = self.db;
            for d in self.devices.iter_mut().filter(|d| d.slot == slot) {
                match &mut d.kind {
                    Kind::Keyboard { dci: kd, ring, buf, prev, held, next_repeat, polls } if *kd == dci => {
                        if ok {
                            let r: [u8; 8] = unsafe { core::ptr::read_volatile(*buf as *const [u8; 8]) };
                            keyboard_report(input, ev, prev, &r, held);
                            *next_repeat = now + 500;
                            *polls = 0;
                            *prev = r;
                        }
                        ring.push(Trb { p: phys(*buf as u64), s: 8, c: (T_NORMAL << 10) | (1 << 5) | (1 << 2) });
                        unsafe { write_volatile(db.add(slot as usize), dci as u32) };
                    }
                    Kind::Mouse { dci: md, ring, buf } if *md == dci => {
                        if ok {
                            let r: [u8; 4] = unsafe { core::ptr::read_volatile(*buf as *const [u8; 4]) };
                            if t.s & 0xFF_FFFF <= 4 {
                                // 4 byte ricevuti: il quarto e' la rotellina
                                ev.wheel -= r[3] as i8 as i32;
                            }
                            input.feed_mouse(r[1] as i8 as i32, r[2] as i8 as i32, r[0] & 1 != 0, r[0] & 2 != 0, ev);
                        }
                        ring.push(Trb { p: phys(*buf as u64), s: 8, c: (T_NORMAL << 10) | (1 << 5) | (1 << 2) });
                        unsafe { write_volatile(db.add(slot as usize), dci as u32) };
                    }
                    _ => {}
                }
            }
        }
        // ripetizione automatica del tasto tenuto premuto
        for d in self.devices.iter_mut() {
            if let Kind::Keyboard { held, next_repeat, prev, polls, .. } = &mut d.kind {
                *polls += 1;
                // ripete solo se il rilascio non e' "in viaggio" (piu' letture senza novita')
                if *held != 0 && now >= *next_repeat && *polls >= 3 {
                    if let Some((sc, ext)) = usage_to_set1(*held) {
                        input.feed_scancode(sc, ext, false, prev[0], ev);
                    }
                    *next_repeat = now + 35;
                }
            }
        }
    }
}

fn wanted(class: u8, sub: u8, proto: u8) -> bool {
    (class == 3 && sub == 1 && (proto == 1 || proto == 2)) || (class == 8 && sub == 6 && proto == 0x50)
}

/// Codici HID -> scancode set 1 (cosi' i layout di tastiera restano gli stessi del PS/2).
fn usage_to_set1(u: u8) -> Option<(u8, bool)> {
    const LETTERS: [u8; 26] = [
        0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18, 0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
    ];
    Some(match u {
        4..=29 => (LETTERS[(u - 4) as usize], false),
        30..=39 => (0x02 + (u - 30), false),
        40 => (0x1C, false),
        41 => (0x01, false),
        42 => (0x0E, false),
        43 => (0x0F, false),
        44 => (0x39, false),
        45 => (0x0C, false),
        46 => (0x0D, false),
        47 => (0x1A, false),
        48 => (0x1B, false),
        49 | 50 => (0x2B, false),
        51 => (0x27, false),
        52 => (0x28, false),
        53 => (0x29, false),
        54 => (0x33, false),
        55 => (0x34, false),
        56 => (0x35, false),
        57 => (0x3A, false),
        59 => (0x3C, false),
        62 => (0x3F, false),
        74 => (0x47, true),
        75 => (0x49, true),
        78 => (0x51, true),
        76 => (0x53, true),
        77 => (0x4F, true),
        79 => (0x4D, true),
        80 => (0x4B, true),
        81 => (0x50, true),
        82 => (0x48, true),
        88 => (0x1C, true),
        100 => (0x56, false),
        _ => return None,
    })
}

fn keyboard_report(input: &mut Input, ev: &mut Events, prev: &[u8; 8], r: &[u8; 8], held: &mut u8) {
    // modificatori: Ctrl, Shift, Alt, Win (sinistri e destri)
    const MODS: [(u8, bool); 8] = [(0x1D, false), (0x2A, false), (0x38, false), (0x5B, true), (0x1D, true), (0x36, false), (0x38, true), (0x5C, true)];
    for (bit, &(sc, ext)) in MODS.iter().enumerate() {
        let was = prev[0] & (1 << bit) != 0;
        let now = r[0] & (1 << bit) != 0;
        if was != now {
            input.feed_scancode(sc, ext, !now, r[0], ev);
        }
    }
    for &u in prev[2..].iter() {
        if u > 3 && !r[2..].contains(&u) {
            if let Some((sc, ext)) = usage_to_set1(u) {
                input.feed_scancode(sc, ext, true, r[0], ev);
            }
            if *held == u {
                *held = 0;
            }
        }
    }
    for &u in r[2..].iter() {
        if u > 3 && !prev[2..].contains(&u) {
            if let Some((sc, ext)) = usage_to_set1(u) {
                input.feed_scancode(sc, ext, false, r[0], ev);
                *held = u;
            }
        }
    }
}

// ------------------------------------------------------------------ interfaccia pubblica

pub fn init(devs: &[pci::Dev]) {
    for d in devs.iter().filter(|d| d.class == 0x0C && d.subclass == 0x03) {
        let prog = (d.read(0x08) >> 8) & 0xFF;
        if prog != 0x30 {
            continue; // solo xHCI (USB 3); UHCI/OHCI/EHCI non sono gestiti
        }
        if let Some(c) = Xhci::init(d) {
            crate::log!("USB: controller xHCI con {} porte, {} dispositivi", c.ports, c.devices.len());
            let ci = ctrls().len();
            for (di, dev) in c.devices.iter().enumerate() {
                if let Kind::Storage { .. } = dev.kind {
                    disks().push((ci, di));
                }
            }
            ctrls().push(c);
        }
    }
}

/// Chiamata dal ciclo dell'input: legge tastiere e mouse USB.
pub fn poll(input: &mut Input, ev: &mut Events) {
    for c in ctrls().iter_mut() {
        c.poll(input, ev);
    }
}

pub fn keyboards() -> usize {
    ctrls().iter().map(|c| c.devices.iter().filter(|d| matches!(d.kind, Kind::Keyboard { .. })).count()).sum()
}

pub fn disk_count() -> usize {
    disks().len()
}

pub fn disk_read(i: usize, lba: u64, buf: &mut [u8; 512]) -> bool {
    let Some(&(c, d)) = disks().get(i) else { return false };
    ctrls()[c].rw(d, lba, buf.as_mut_ptr(), false)
}

pub fn disk_write(i: usize, lba: u64, buf: &[u8; 512]) -> bool {
    let Some(&(c, d)) = disks().get(i) else { return false };
    let mut tmp = *buf;
    ctrls()[c].rw(d, lba, tmp.as_mut_ptr(), true)
}

pub fn disk_sectors(i: usize) -> u64 {
    let Some(&(c, d)) = disks().get(i) else { return 0 };
    match ctrls()[c].devices[d].kind {
        Kind::Storage { blocks, .. } => blocks,
        _ => 0,
    }
}
