//! ACPI minimale: trova le tabelle del firmware per
//!  - spegnere davvero il computer (stato S5, dal DSDT)
//!  - contare i core della CPU (tabella MADT)
//!
//! Idea ispirata a Redox OS (src/acpi), riscritta da zero in forma molto ridotta.

use crate::arch::{inb, inw, outb, outw, sleep_ms};
use core::ptr::read_unaligned;
use core::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Default)]
pub struct Acpi {
    pub found: bool,
    pub revision: u8,
    pub pm1a_cnt: u16,
    pub pm1b_cnt: u16,
    pub slp_typa: u16,
    pub slp_typb: u16,
    pub smi_cmd: u32,
    pub acpi_enable: u8,
    pub s5: bool,
    pub cpus: u32,
    pub oem: [u8; 6],
}

static mut ACPI: Acpi = Acpi {
    found: false,
    revision: 0,
    pm1a_cnt: 0,
    pm1b_cnt: 0,
    slp_typa: 0,
    slp_typb: 0,
    smi_cmd: 0,
    acpi_enable: 0,
    s5: false,
    cpus: 0,
    oem: [0; 6],
};

/// Limite della memoria mappata (le tabelle oltre non vengono lette).
static MAPPED: AtomicU64 = AtomicU64::new(4 << 30);

pub fn get() -> Acpi {
    unsafe { *core::ptr::addr_of!(ACPI) }
}

fn ok(addr: u64, len: u64) -> bool {
    addr != 0 && addr.saturating_add(len) <= MAPPED.load(Ordering::Relaxed)
}

unsafe fn r8(a: u64) -> u8 {
    unsafe { read_unaligned(a as *const u8) }
}
unsafe fn r16(a: u64) -> u16 {
    unsafe { read_unaligned(a as *const u16) }
}
unsafe fn r32(a: u64) -> u32 {
    unsafe { read_unaligned(a as *const u32) }
}
unsafe fn r64(a: u64) -> u64 {
    unsafe { read_unaligned(a as *const u64) }
}

fn checksum(a: u64, len: u64) -> bool {
    let mut s: u8 = 0;
    for i in 0..len {
        s = s.wrapping_add(unsafe { r8(a + i) });
    }
    s == 0
}

fn find_rsdp_bios() -> Option<u64> {
    let scan = |start: u64, end: u64| -> Option<u64> {
        let mut a = start;
        while a + 20 <= end {
            if unsafe { r64(a) } == u64::from_le_bytes(*b"RSD PTR ") && checksum(a, 20) {
                return Some(a);
            }
            a += 16;
        }
        None
    };
    let ebda = (unsafe { r16(0x40E) } as u64) << 4;
    if ebda >= 0x80000 && ebda < 0xA0000 {
        if let Some(a) = scan(ebda, ebda + 1024) {
            return Some(a);
        }
    }
    scan(0xE0000, 0x100000)
}

/// Inizializza ACPI. `rsdp_hint` arriva dal loader UEFI (0 con il BIOS).
pub fn init(rsdp_hint: u64, mapped_limit: u64) -> Acpi {
    MAPPED.store(mapped_limit, Ordering::Relaxed);
    let mut info = Acpi::default();
    let rsdp = if rsdp_hint != 0 { Some(rsdp_hint) } else { find_rsdp_bios() };
    let Some(rsdp) = rsdp else {
        crate::log!("ACPI: RSDP non trovato");
        return info;
    };
    unsafe {
        info.found = true;
        info.revision = r8(rsdp + 15);
        for i in 0..6 {
            info.oem[i] = r8(rsdp + 9 + i as u64);
        }
        let (root, entry) = if info.revision >= 2 && r64(rsdp + 24) != 0 { (r64(rsdp + 24), 8u64) } else { (r32(rsdp + 16) as u64, 4u64) };
        if !ok(root, 36) {
            return info;
        }
        let len = r32(root + 4) as u64;
        let n = len.saturating_sub(36) / entry;
        for i in 0..n.min(64) {
            let p = root + 36 + i * entry;
            let t = if entry == 8 { r64(p) } else { r32(p) as u64 };
            if !ok(t, 36) {
                continue;
            }
            let sig = r32(t);
            if sig == u32::from_le_bytes(*b"FACP") {
                parse_fadt(t, &mut info);
            } else if sig == u32::from_le_bytes(*b"APIC") {
                info.cpus = count_cpus(t);
            }
        }
    }
    crate::log!(
        "ACPI rev {}: S5={} PM1a={:#x} SLP_TYP={} core={}",
        info.revision,
        info.s5,
        info.pm1a_cnt,
        info.slp_typa,
        info.cpus
    );
    unsafe { *core::ptr::addr_of_mut!(ACPI) = info };
    info
}

unsafe fn parse_fadt(t: u64, info: &mut Acpi) {
    unsafe {
        let len = r32(t + 4) as u64;
        let mut dsdt = r32(t + 40) as u64;
        if len >= 148 && r64(t + 140) != 0 {
            dsdt = r64(t + 140);
        }
        info.smi_cmd = r32(t + 48);
        info.acpi_enable = r8(t + 52);
        info.pm1a_cnt = r32(t + 64) as u16;
        info.pm1b_cnt = r32(t + 68) as u16;
        if !ok(dsdt, 36) {
            return;
        }
        // cerca l'oggetto \_S5_ nel bytecode AML del DSDT
        let dlen = r32(dsdt + 4) as u64;
        if !ok(dsdt, dlen) {
            return;
        }
        let mut p = dsdt + 36;
        let end = dsdt + dlen;
        while p + 4 < end {
            if r32(p) == u32::from_le_bytes(*b"_S5_") {
                let prev_ok = r8(p - 1) == 0x08 || (r8(p - 2) == 0x08 && r8(p - 1) == b'\\');
                if prev_ok && r8(p + 4) == 0x12 {
                    // PackageOp: salta PkgLength e NumElements
                    let mut q = p + 5;
                    q += ((r8(q) as u64 & 0xC0) >> 6) + 2;
                    let read_val = |q: &mut u64| -> u16 {
                        if r8(*q) == 0x0A {
                            *q += 1; // BytePrefix
                        }
                        let v = r8(*q) as u16;
                        *q += 1;
                        v
                    };
                    info.slp_typa = read_val(&mut q);
                    info.slp_typb = read_val(&mut q);
                    info.s5 = true;
                }
                break;
            }
            p += 1;
        }
    }
}

unsafe fn count_cpus(t: u64) -> u32 {
    unsafe {
        let len = r32(t + 4) as u64;
        let mut p = t + 44;
        let mut n = 0;
        while p + 2 <= t + len {
            let typ = r8(p);
            let l = r8(p + 1) as u64;
            if l < 2 {
                break;
            }
            match typ {
                0 if l >= 8 => {
                    if r32(p + 4) & 0b11 != 0 {
                        n += 1;
                    }
                }
                9 if l >= 16 => {
                    if r32(p + 8) & 0b11 != 0 {
                        n += 1;
                    }
                }
                _ => {}
            }
            p += l;
        }
        n
    }
}

/// Spegnimento ACPI (stato S5). Ritorna solo se non ha funzionato.
pub fn shutdown() {
    let a = get();
    if !a.s5 || a.pm1a_cnt == 0 {
        return;
    }
    // se l'ACPI non e' ancora attivo, chiedilo al firmware
    if a.smi_cmd != 0 && a.acpi_enable != 0 && inw(a.pm1a_cnt) & 1 == 0 {
        outb(a.smi_cmd as u16, a.acpi_enable);
        for _ in 0..300 {
            if inw(a.pm1a_cnt) & 1 != 0 {
                break;
            }
            sleep_ms(10);
        }
    }
    outw(a.pm1a_cnt, (a.slp_typa << 10) | (1 << 13));
    if a.pm1b_cnt != 0 {
        outw(a.pm1b_cnt, (a.slp_typb << 10) | (1 << 13));
    }
    sleep_ms(500);
    let _ = inb(0x80);
}
