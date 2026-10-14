//! Accesso diretto all'hardware x86_64: porte I/O, CPUID, timer, orologio, IDT.

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

#[inline(always)]
pub fn outb(port: u16, v: u8) {
    unsafe { asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack, preserves_flags)) }
}
#[inline(always)]
pub fn inb(port: u16) -> u8 {
    let v: u8;
    unsafe { asm!("in al, dx", out("al") v, in("dx") port, options(nomem, nostack, preserves_flags)) }
    v
}
#[inline(always)]
pub fn outw(port: u16, v: u16) {
    unsafe { asm!("out dx, ax", in("dx") port, in("ax") v, options(nomem, nostack, preserves_flags)) }
}

#[inline(always)]
pub fn inw(port: u16) -> u16 {
    let v: u16;
    unsafe { asm!("in ax, dx", out("ax") v, in("dx") port, options(nomem, nostack, preserves_flags)) }
    v
}
#[inline(always)]
pub fn outl(port: u16, v: u32) {
    unsafe { asm!("out dx, eax", in("dx") port, in("eax") v, options(nomem, nostack, preserves_flags)) }
}
#[inline(always)]
pub fn inl(port: u16) -> u32 {
    let v: u32;
    unsafe { asm!("in eax, dx", out("eax") v, in("dx") port, options(nomem, nostack, preserves_flags)) }
    v
}

#[inline(always)]
pub fn pause() {
    unsafe { asm!("pause", options(nomem, nostack, preserves_flags)) }
}

#[inline(always)]
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe { asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags)) }
    ((hi as u64) << 32) | lo as u64
}

pub fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let r = unsafe { core::arch::x86_64::__cpuid(leaf) };
    (r.eax, r.ebx, r.ecx, r.edx)
}

/// Nome commerciale del processore (es. "AMD Ryzen 7 5800X").
pub fn cpu_brand(out: &mut [u8; 48]) -> usize {
    let (max, _, _, _) = cpuid(0x8000_0000);
    if max < 0x8000_0004 {
        let (_, b, c, d) = cpuid(0);
        out[..4].copy_from_slice(&b.to_le_bytes());
        out[4..8].copy_from_slice(&d.to_le_bytes());
        out[8..12].copy_from_slice(&c.to_le_bytes());
        return 12;
    }
    for (i, leaf) in (0x8000_0002u32..=0x8000_0004).enumerate() {
        let (a, b, c, d) = cpuid(leaf);
        for (j, r) in [a, b, c, d].iter().enumerate() {
            out[i * 16 + j * 4..i * 16 + j * 4 + 4].copy_from_slice(&r.to_le_bytes());
        }
    }
    48
}

pub fn cpu_cores() -> u32 {
    let (_, b, _, _) = cpuid(1);
    ((b >> 16) & 0xFF).max(1)
}

// ---------------------------------------------------------------- timer

static TSC_PER_MS: AtomicU64 = AtomicU64::new(1_000_000);
static TSC_BOOT: AtomicU64 = AtomicU64::new(0);

/// Calibra il TSC usando il canale 2 del PIT (10 ms).
pub fn calibrate_timer() {
    let gate = inb(0x61);
    outb(0x61, (gate & !0x02) & !0x01);
    outb(0x43, 0b1011_0000); // canale 2, lo/hi, modo 0
    let count: u16 = 11932; // 1193182 Hz * 10 ms
    outb(0x42, (count & 0xFF) as u8);
    outb(0x42, (count >> 8) as u8);
    let t0 = rdtsc();
    outb(0x61, (gate & !0x02) | 0x01);
    let mut guard = 0u64;
    while inb(0x61) & 0x20 == 0 {
        guard += 1;
        if guard > 50_000_000 {
            break;
        }
    }
    let t1 = rdtsc();
    outb(0x61, gate);
    let per_ms = ((t1 - t0) / 10).max(10_000);
    TSC_PER_MS.store(per_ms, Ordering::Relaxed);
    TSC_BOOT.store(t0, Ordering::Relaxed);
}

/// Millisecondi dall'avvio.
pub fn now_ms() -> u64 {
    (rdtsc() - TSC_BOOT.load(Ordering::Relaxed)) / TSC_PER_MS.load(Ordering::Relaxed)
}

pub fn sleep_ms(ms: u64) {
    let end = now_ms() + ms;
    while now_ms() < end {
        pause();
    }
}

// ---------------------------------------------------------------- orologio (CMOS RTC)

fn cmos(reg: u8) -> u8 {
    outb(0x70, reg);
    inb(0x71)
}

#[derive(Clone, Copy, Default, PartialEq)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub min: u8,
    pub sec: u8,
}

pub fn rtc_now() -> DateTime {
    let mut guard = 0;
    while cmos(0x0A) & 0x80 != 0 && guard < 100_000 {
        guard += 1;
    }
    let b = cmos(0x0B);
    let bcd = b & 0x04 == 0;
    let conv = |v: u8| if bcd { (v & 0x0F) + (v >> 4) * 10 } else { v };
    let mut hour = cmos(0x04);
    let pm = hour & 0x80 != 0;
    hour = conv(hour & 0x7F);
    if b & 0x02 == 0 && pm {
        hour = (hour % 12) + 12;
    }
    DateTime {
        sec: conv(cmos(0x00)),
        min: conv(cmos(0x02)),
        hour,
        day: conv(cmos(0x07)),
        month: conv(cmos(0x08)),
        year: 2000 + conv(cmos(0x09)) as u16,
    }
}

/// Giorno della settimana (0 = lunedi').
pub fn weekday(d: &DateTime) -> usize {
    let t = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut y = d.year as i32;
    if d.month < 3 {
        y -= 1;
    }
    let w = (y + y / 4 - y / 100 + y / 400 + t[(d.month as usize).clamp(1, 12) - 1] + d.day as i32) % 7;
    ((w + 6) % 7) as usize
}

// ---------------------------------------------------------------- alimentazione

pub fn reboot() -> ! {
    // controller della tastiera: impulso di reset
    for _ in 0..1000 {
        if inb(0x64) & 0x02 == 0 {
            break;
        }
    }
    outb(0x64, 0xFE);
    sleep_ms(200);
    // ultima risorsa: triple fault con IDT vuota
    unsafe {
        let null: [u8; 10] = [0; 10];
        asm!("lidt [{}]", "int3", in(reg) &null);
    }
    loop {
        halt();
    }
}

/// Spegne il computer: prima con ACPI, poi con le porte degli emulatori.
pub fn shutdown() {
    // 1) ACPI vero (funziona sui PC reali)
    crate::acpi::shutdown();
    // 2) porte "magiche" degli emulatori
    outw(0x604, 0x2000); // QEMU (q35/pc recenti)
    outw(0xB004, 0x2000); // Bochs / QEMU vecchi
    outw(0x4004, 0x3400); // VirtualBox
    sleep_ms(300);
}

pub fn halt() {
    unsafe { asm!("cli", "hlt", options(nomem, nostack)) }
}

// ---------------------------------------------------------------- seriale (log di debug su COM1)

pub fn serial_init() {
    let p = 0x3F8;
    outb(p + 1, 0x00);
    outb(p + 3, 0x80);
    outb(p, 0x01); // 115200 baud
    outb(p + 1, 0x00);
    outb(p + 3, 0x03);
    outb(p + 2, 0xC7);
    outb(p + 4, 0x0B);
}

pub fn serial_write(s: &str) {
    for b in s.bytes() {
        let mut guard = 0;
        while inb(0x3F8 + 5) & 0x20 == 0 && guard < 10_000 {
            guard += 1;
        }
        outb(0x3F8, b);
    }
}

pub struct Serial;
impl core::fmt::Write for Serial {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        serial_write(s);
        Ok(())
    }
}

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => {{
        use core::fmt::Write;
        let _ = write!($crate::arch::Serial, $($t)*);
        $crate::arch::serial_write("\n");
    }};
}

// ---------------------------------------------------------------- IDT (solo eccezioni)

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    off_lo: u16,
    sel: u16,
    ist: u8,
    attr: u8,
    off_mid: u16,
    off_hi: u32,
    zero: u32,
}

static mut IDT: [IdtEntry; 32] = [IdtEntry { off_lo: 0, sel: 0, ist: 0, attr: 0, off_mid: 0, off_hi: 0, zero: 0 }; 32];

extern "C" {
    static isr_table: [u64; 32];
}

pub fn init_idt() {
    unsafe {
        let idt = &mut *core::ptr::addr_of_mut!(IDT);
        let table = &*core::ptr::addr_of!(isr_table);
        for (i, e) in idt.iter_mut().enumerate() {
            let a = table[i];
            *e = IdtEntry {
                off_lo: a as u16,
                sel: 0x08,
                ist: 0,
                attr: 0x8E,
                off_mid: (a >> 16) as u16,
                off_hi: (a >> 32) as u32,
                zero: 0,
            };
        }
        #[repr(C, packed)]
        struct Ptr {
            limit: u16,
            base: u64,
        }
        let p = Ptr { limit: (32 * 16 - 1) as u16, base: idt.as_ptr() as u64 };
        asm!("lidt [{}]", in(reg) &p, options(nostack));
    }
}

#[repr(C)]
pub struct ExceptionFrame {
    pub vector: u64,
    pub error: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
}

pub fn exception_name(v: u64) -> &'static str {
    match v {
        0 => "Divisione per zero",
        1 => "Debug",
        2 => "NMI",
        3 => "Breakpoint",
        4 => "Overflow",
        5 => "Bound range",
        6 => "Istruzione non valida",
        7 => "Device not available",
        8 => "Double fault",
        10 => "TSS non valido",
        11 => "Segmento non presente",
        12 => "Errore dello stack",
        13 => "General protection fault",
        14 => "Page fault",
        16 => "Errore x87",
        17 => "Alignment check",
        18 => "Machine check",
        19 => "Errore SIMD",
        _ => "Eccezione",
    }
}

pub fn cr2() -> u64 {
    let v: u64;
    unsafe { asm!("mov {}, cr2", out(reg) v, options(nomem, nostack)) }
    v
}
