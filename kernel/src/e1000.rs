//! Driver per le schede di rete Intel PRO/1000 (famiglia e1000).
//! E' la scheda che QEMU, VirtualBox e VMware emulano di default.
//! Funziona in polling: nessun interrupt, i pacchetti vengono letti a ogni ciclo.

use crate::arch::sleep_ms;
use crate::mem::phys;
use crate::pci::Dev;
use alloc::alloc::{alloc_zeroed, Layout};
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

const CTRL: usize = 0x0000;
const STATUS: usize = 0x0008;
const EERD: usize = 0x0014;
const ICR: usize = 0x00C0;
const IMC: usize = 0x00D8;
const RCTL: usize = 0x0100;
const TCTL: usize = 0x0400;
const TIPG: usize = 0x0410;
const RDBAL: usize = 0x2800;
const RDBAH: usize = 0x2804;
const RDLEN: usize = 0x2808;
const RDH: usize = 0x2810;
const RDT: usize = 0x2818;
const TDBAL: usize = 0x3800;
const TDBAH: usize = 0x3804;
const TDLEN: usize = 0x3808;
const TDH: usize = 0x3810;
const TDT: usize = 0x3818;
const MTA: usize = 0x5200;
const RAL: usize = 0x5400;
const RAH: usize = 0x5404;

const N: usize = 256;
const BUF: usize = 2048;

#[repr(C)]
#[derive(Clone, Copy)]
struct RxDesc {
    addr: u64,
    len: u16,
    csum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TxDesc {
    addr: u64,
    len: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

pub struct E1000 {
    mmio: *mut u32,
    rx: *mut RxDesc,
    tx: *mut TxDesc,
    rx_buf: *mut u8,
    tx_buf: *mut u8,
    rx_cur: usize,
    tx_cur: usize,
    pub mac: [u8; 6],
    pub name: &'static str,
    pub rx_count: u64,
    pub tx_count: u64,
}

/// Schede supportate (vendor Intel).
pub fn model(device: u16) -> Option<&'static str> {
    Some(match device {
        0x100E => "Intel PRO/1000 MT (82540EM)",
        0x100F => "Intel PRO/1000 MT (82545EM)",
        0x1004 => "Intel PRO/1000 MT (82543GC)",
        0x1015 => "Intel PRO/1000 MT (82540EM LOM)",
        0x1026 | 0x1027 | 0x1028 => "Intel PRO/1000 MT (82545GM)",
        0x1076 | 0x1077 | 0x1078 => "Intel PRO/1000 (82541)",
        0x107C => "Intel PRO/1000 GT (82541PI)",
        0x10D3 => "Intel 82574L (e1000e)",
        _ => return None,
    })
}

fn dma_alloc(size: usize) -> *mut u8 {
    unsafe { alloc_zeroed(Layout::from_size_align(size, 4096).unwrap()) }
}

impl E1000 {
    fn rd(&self, r: usize) -> u32 {
        unsafe { read_volatile(self.mmio.add(r / 4)) }
    }
    fn wr(&self, r: usize, v: u32) {
        unsafe { write_volatile(self.mmio.add(r / 4), v) }
    }

    pub fn new(dev: &Dev) -> Option<E1000> {
        let name = model(dev.device)?;
        let bar = dev.bar_mem(0)?;
        crate::mem::map_mmio(bar, 0x20000);
        dev.enable();
        let mut nic = E1000 {
            mmio: bar as *mut u32,
            rx: dma_alloc(N * 16) as *mut RxDesc,
            tx: dma_alloc(N * 16) as *mut TxDesc,
            rx_buf: dma_alloc(N * BUF),
            tx_buf: dma_alloc(N * BUF),
            rx_cur: 0,
            tx_cur: 0,
            mac: [0; 6],
            name,
            rx_count: 0,
            tx_count: 0,
        };
        // reset della scheda
        nic.wr(IMC, 0xFFFF_FFFF);
        nic.wr(CTRL, nic.rd(CTRL) | (1 << 26));
        sleep_ms(20);
        nic.wr(IMC, 0xFFFF_FFFF);
        let _ = nic.rd(ICR);
        // link su, rilevamento automatico della velocita'
        let ctrl = (nic.rd(CTRL) | (1 << 6) | (1 << 5)) & !((1 << 3) | (1 << 31) | (1 << 7) | (1 << 30));
        nic.wr(CTRL, ctrl);

        // indirizzo MAC: dai registri, oppure dalla EEPROM
        let (ral, rah) = (nic.rd(RAL), nic.rd(RAH));
        if rah & (1 << 31) != 0 && ral != 0 {
            nic.mac = [ral as u8, (ral >> 8) as u8, (ral >> 16) as u8, (ral >> 24) as u8, rah as u8, (rah >> 8) as u8];
        } else {
            for i in 0..3 {
                nic.wr(EERD, ((i as u32) << 8) | 1);
                let mut v = 0;
                for _ in 0..100_000 {
                    v = nic.rd(EERD);
                    if v & (1 << 4) != 0 {
                        break;
                    }
                }
                let w = (v >> 16) as u16;
                nic.mac[i * 2] = w as u8;
                nic.mac[i * 2 + 1] = (w >> 8) as u8;
            }
            let m = nic.mac;
            nic.wr(RAL, u32::from_le_bytes([m[0], m[1], m[2], m[3]]));
            nic.wr(RAH, u32::from_le_bytes([m[4], m[5], 0, 0]) | (1 << 31));
        }
        for i in 0..128 {
            nic.wr(MTA + i * 4, 0);
        }

        unsafe {
            // ricezione
            for i in 0..N {
                let d = RxDesc { addr: phys(nic.rx_buf.add(i * BUF) as u64), len: 0, csum: 0, status: 0, errors: 0, special: 0 };
                write_volatile(nic.rx.add(i), d);
            }
            let rx = phys(nic.rx as u64);
            nic.wr(RDBAL, rx as u32);
            nic.wr(RDBAH, (rx >> 32) as u32);
            nic.wr(RDLEN, (N * 16) as u32);
            nic.wr(RDH, 0);
            nic.wr(RDT, (N - 1) as u32);
            // EN | BAM (broadcast) | SECRC (togli il CRC) | buffer da 2048 byte
            nic.wr(RCTL, (1 << 1) | (1 << 15) | (1 << 26));

            // trasmissione
            for i in 0..N {
                let d = TxDesc { addr: phys(nic.tx_buf.add(i * BUF) as u64), len: 0, cso: 0, cmd: 0, status: 1, css: 0, special: 0 };
                write_volatile(nic.tx.add(i), d);
            }
            let tx = phys(nic.tx as u64);
            nic.wr(TDBAL, tx as u32);
            nic.wr(TDBAH, (tx >> 32) as u32);
            nic.wr(TDLEN, (N * 16) as u32);
            nic.wr(TDH, 0);
            nic.wr(TDT, 0);
            nic.wr(TCTL, (1 << 1) | (1 << 3) | (0x0F << 4) | (0x40 << 12));
            nic.wr(TIPG, 0x0060_200A);
        }
        crate::log!(
            "rete: {} MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} link={}",
            name,
            nic.mac[0],
            nic.mac[1],
            nic.mac[2],
            nic.mac[3],
            nic.mac[4],
            nic.mac[5],
            nic.link()
        );
        Some(nic)
    }

    pub fn link(&self) -> bool {
        self.rd(STATUS) & 2 != 0
    }

    pub fn recv(&mut self) -> Option<Vec<u8>> {
        unsafe {
            let d = self.rx.add(self.rx_cur);
            let desc = read_volatile(d);
            if desc.status & 1 == 0 {
                return None;
            }
            let len = (desc.len as usize).min(BUF);
            let data = core::slice::from_raw_parts(self.rx_buf.add(self.rx_cur * BUF), len).to_vec();
            write_volatile(d, RxDesc { status: 0, ..desc });
            self.wr(RDT, self.rx_cur as u32);
            self.rx_cur = (self.rx_cur + 1) % N;
            self.rx_count += 1;
            Some(data)
        }
    }

    pub fn can_send(&self) -> bool {
        unsafe { read_volatile(self.tx.add(self.tx_cur)).status & 1 != 0 }
    }

    pub fn send(&mut self, len: usize, fill: impl FnOnce(&mut [u8])) {
        let len = len.min(BUF);
        unsafe {
            let buf = core::slice::from_raw_parts_mut(self.tx_buf.add(self.tx_cur * BUF), len);
            fill(buf);
            let d = self.tx.add(self.tx_cur);
            let mut desc = read_volatile(d);
            desc.len = len as u16;
            desc.cmd = 1 | 2 | 8; // EOP | IFCS | RS
            desc.status = 0;
            write_volatile(d, desc);
            self.tx_cur = (self.tx_cur + 1) % N;
            self.wr(TDT, self.tx_cur as u32);
            self.tx_count += 1;
        }
    }
}
