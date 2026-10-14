//! Driver ATA/IDE in modalita' PIO (dischi IDE, QEMU "pc", VirtualBox, PC datati).

use crate::arch::{inb, inw, outb, outw};

#[derive(Clone, Copy)]
pub struct Ata {
    io: u16,
    ctrl: u16,
    slave: bool,
    lba48: bool,
    pub sectors: u64,
}

fn delay(ctrl: u16) {
    for _ in 0..4 {
        inb(ctrl);
    }
}

impl Ata {
    fn wait_ready(&self) -> bool {
        for _ in 0..1_000_000 {
            let s = inb(self.io + 7);
            if s & 0x80 == 0 {
                return s & 0x21 == 0; // niente ERR / DF
            }
        }
        false
    }

    fn wait_drq(&self) -> bool {
        for _ in 0..1_000_000 {
            let s = inb(self.io + 7);
            if s & 0x21 != 0 {
                return false;
            }
            if s & 0x80 == 0 && s & 0x08 != 0 {
                return true;
            }
        }
        false
    }

    pub fn identify(io: u16, ctrl: u16, slave: bool) -> Option<Ata> {
        if inb(io + 7) == 0xFF {
            return None; // canale assente
        }
        outb(ctrl, 0x02); // niente interrupt
        outb(io + 6, 0xA0 | ((slave as u8) << 4));
        delay(ctrl);
        for r in 2..6 {
            outb(io + r, 0);
        }
        outb(io + 7, 0xEC); // IDENTIFY
        delay(ctrl);
        if inb(io + 7) == 0 {
            return None;
        }
        let mut busy = true;
        for _ in 0..1_000_000 {
            if inb(io + 7) & 0x80 == 0 {
                busy = false;
                break;
            }
        }
        if busy || inb(io + 4) != 0 || inb(io + 5) != 0 {
            return None; // non e' un disco ATA (es. lettore CD ATAPI)
        }
        let probe = Ata { io, ctrl, slave, lba48: false, sectors: 0 };
        if !probe.wait_drq() {
            return None;
        }
        let mut id = [0u16; 256];
        for w in id.iter_mut() {
            *w = inw(io);
        }
        let lba48 = id[83] & (1 << 10) != 0;
        let sectors = if lba48 {
            id[100] as u64 | (id[101] as u64) << 16 | (id[102] as u64) << 32 | (id[103] as u64) << 48
        } else {
            id[60] as u64 | (id[61] as u64) << 16
        };
        Some(Ata { io, ctrl, slave, lba48, sectors })
    }

    fn select(&self, lba: u64, cmd48: u8, cmd28: u8) {
        let s = (self.slave as u8) << 4;
        if self.lba48 {
            outb(self.io + 6, 0x40 | s);
            delay(self.ctrl);
            outb(self.io + 2, 0);
            outb(self.io + 3, (lba >> 24) as u8);
            outb(self.io + 4, (lba >> 32) as u8);
            outb(self.io + 5, (lba >> 40) as u8);
            outb(self.io + 2, 1);
            outb(self.io + 3, lba as u8);
            outb(self.io + 4, (lba >> 8) as u8);
            outb(self.io + 5, (lba >> 16) as u8);
            outb(self.io + 7, cmd48);
        } else {
            outb(self.io + 6, 0xE0 | s | ((lba >> 24) as u8 & 0x0F));
            delay(self.ctrl);
            outb(self.io + 2, 1);
            outb(self.io + 3, lba as u8);
            outb(self.io + 4, (lba >> 8) as u8);
            outb(self.io + 5, (lba >> 16) as u8);
            outb(self.io + 7, cmd28);
        }
    }

    pub fn read(&self, lba: u64, buf: &mut [u8; 512]) -> bool {
        if !self.wait_ready() {
            return false;
        }
        self.select(lba, 0x24, 0x20);
        if !self.wait_drq() {
            return false;
        }
        for i in 0..256 {
            let w = inw(self.io);
            buf[i * 2] = w as u8;
            buf[i * 2 + 1] = (w >> 8) as u8;
        }
        true
    }

    pub fn write(&self, lba: u64, buf: &[u8; 512]) -> bool {
        if !self.wait_ready() {
            return false;
        }
        self.select(lba, 0x34, 0x30);
        if !self.wait_drq() {
            return false;
        }
        for i in 0..256 {
            outw(self.io, buf[i * 2] as u16 | (buf[i * 2 + 1] as u16) << 8);
        }
        outb(self.io + 7, if self.lba48 { 0xEA } else { 0xE7 }); // svuota la cache del disco
        self.wait_ready()
    }
}


/// Tutti i dischi ATA sui due canali standard.
pub fn probe() -> alloc::vec::Vec<Ata> {
    let mut v = alloc::vec::Vec::new();
    for (io, ctrl) in [(0x1F0u16, 0x3F6u16), (0x170, 0x376)] {
        for slave in [false, true] {
            if let Some(d) = Ata::identify(io, ctrl, slave) {
                v.push(d);
            }
        }
    }
    v
}
