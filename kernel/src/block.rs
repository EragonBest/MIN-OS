//! Dischi: un'interfaccia unica per ATA, AHCI (SATA), NVMe e chiavette USB.
//! All'avvio MIN-OS prova tutti i dischi e sceglie quello con la propria firma ("MNOS").

use crate::ahci;
use crate::ata::{self, Ata};
use crate::nvme::Nvme;
use crate::pci;
use alloc::vec::Vec;

pub enum Disk {
    Ata(Ata),
    Ahci(ahci::Port),
    Nvme(Nvme),
    Usb(usize),
}

impl Disk {
    fn read(&mut self, lba: u64, buf: &mut [u8; 512]) -> bool {
        match self {
            Disk::Ata(d) => d.read(lba, buf),
            Disk::Ahci(d) => d.read(lba, buf),
            Disk::Nvme(d) => d.read(lba, buf),
            Disk::Usb(i) => crate::usb::disk_read(*i, lba, buf),
        }
    }
    fn write(&mut self, lba: u64, buf: &[u8; 512]) -> bool {
        match self {
            Disk::Ata(d) => d.write(lba, buf),
            Disk::Ahci(d) => d.write(lba, buf),
            Disk::Nvme(d) => d.write(lba, buf),
            Disk::Usb(i) => crate::usb::disk_write(*i, lba, buf),
        }
    }
    fn sectors(&self) -> u64 {
        match self {
            Disk::Ata(d) => d.sectors,
            Disk::Ahci(d) => d.sectors,
            Disk::Nvme(d) => d.sectors(),
            Disk::Usb(i) => crate::usb::disk_sectors(*i),
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Disk::Ata(_) => "IDE",
            Disk::Ahci(_) => "SATA",
            Disk::Nvme(_) => "NVMe",
            Disk::Usb(_) => "USB",
        }
    }
}

static mut DISK: Option<Disk> = None;

fn disk() -> Option<&'static mut Disk> {
    unsafe { (*core::ptr::addr_of_mut!(DISK)).as_mut() }
}

/// Cerca il disco di MIN-OS fra tutti quelli collegati.
pub fn init(pci_devs: &[pci::Dev]) -> bool {
    let mut all: Vec<Disk> = Vec::new();
    for d in ata::probe() {
        all.push(Disk::Ata(d));
    }
    for dev in pci_devs.iter().filter(|d| d.class == 0x01) {
        match dev.subclass {
            0x06 => all.extend(ahci::probe(dev).into_iter().map(Disk::Ahci)),
            0x08 => {
                if let Some(n) = Nvme::new(dev) {
                    all.push(Disk::Nvme(n));
                }
            }
            _ => {}
        }
    }
    for i in 0..crate::usb::disk_count() {
        all.push(Disk::Usb(i));
    }
    for mut d in all {
        let mut s0 = [0u8; 512];
        let ours = d.read(0, &mut s0) && &s0[440..444] == b"MNOS";
        crate::log!("disco {}: {} MB{}", d.kind(), d.sectors() / 2048, if ours { " (MIN-OS)" } else { "" });
        if ours {
            unsafe { *core::ptr::addr_of_mut!(DISK) = Some(d) };
            return true;
        }
    }
    crate::log!("nessun disco MIN-OS trovato: le modifiche resteranno in memoria");
    false
}

pub fn read(lba: u64, buf: &mut [u8; 512]) -> bool {
    disk().map(|d| d.read(lba, buf)).unwrap_or(false)
}

pub fn write(lba: u64, buf: &[u8; 512]) -> bool {
    disk().map(|d| d.write(lba, buf)).unwrap_or(false)
}

/// (tipo di disco, dimensione in MB) se il disco di MIN-OS e' stato trovato.
pub fn info() -> Option<(&'static str, u64)> {
    disk().map(|d| (d.kind(), d.sectors() / 2048))
}
