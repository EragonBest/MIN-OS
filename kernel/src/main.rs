//! MIN-OS: un piccolo sistema operativo scritto da zero in Rust.

#![no_std]
#![no_main]
#![allow(dead_code, unused_unsafe)]

extern crate alloc;

mod acpi;
mod arch;
mod desktop;
mod font;
mod gfx;
mod i18n;
mod i18n_desk;
mod input;
mod installer;
mod keymap;
mod mem;
mod net;
mod pci;
mod e1000;
mod storage;
mod ata;
mod ahci;
mod nvme;
mod block;
mod usb;
mod fat;
mod apps;
mod clip;
mod update;
mod web;
mod ui;

use core::sync::atomic::{AtomicU64, Ordering};

/// Versione e numero di build (li imposta il programma di build; su GitHub il numero cresce a ogni push).
pub const VERSION: &str = match option_env!("MINOS_VERSION") {
    Some(v) => v,
    None => "0.5.0",
};
pub const BUILD: u32 = parse_u32(option_env!("MINOS_BUILD"));
/// Repository GitHub da cui arrivano gli aggiornamenti.
pub const REPO: &str = match option_env!("MINOS_REPO") {
    Some(r) => r,
    None => "EragonBest/MIN-OS",
};

const fn parse_u32(s: Option<&str>) -> u32 {
    let Some(s) = s else { return 0 };
    let b = s.as_bytes();
    let mut i = 0;
    let mut n: u32 = 0;
    while i < b.len() {
        if b[i] >= b'0' && b[i] <= b'9' {
            n = n * 10 + (b[i] - b'0') as u32;
        }
        i += 1;
    }
    n
}
use linked_list_allocator::LockedHeap;

core::arch::global_asm!(include_str!("boot.S"), options(att_syntax, raw));

/// Informazioni lasciate dal bootloader all'indirizzo 0x5000.
#[repr(C)]
pub struct BootInfo {
    pub magic: u32,
    pub boot_drive: u32,
    pub fb_addr: u64,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub bpp: u32,
    pub red_pos: u32,
    pub mmap_count: u32,
    /// indirizzo della tabella ACPI RSDP (0 = cercala nella memoria del BIOS)
    pub rsdp: u64,
    pub mmap: [E820; 128],
    /// UEFI: indirizzo fisico reale del kernel (0 = coincide con quello virtuale)
    pub kphys: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct E820 {
    pub base: u64,
    pub len: u64,
    pub kind: u32,
    pub acpi: u32,
}

// ------------------------------------------------------------------ memoria dinamica

const HEAP_SIZE: usize = 72 * 1024 * 1024;

#[repr(C, align(4096))]
struct Heap([u8; HEAP_SIZE]);
static mut HEAP: Heap = Heap([0; HEAP_SIZE]);

#[global_allocator]
static ALLOC: LockedHeap = LockedHeap::empty();

// framebuffer salvato per la schermata di errore
static FB: AtomicU64 = AtomicU64::new(0);
static FB_GEOM: AtomicU64 = AtomicU64::new(0);

#[no_mangle]
pub extern "C" fn kmain(info: &'static BootInfo) -> ! {
    arch::serial_init();
    log!("");
    log!("MIN-OS {} (build {}) - kernel avviato", VERSION, BUILD);
    log!("schermo {}x{} ({} bpp) @ {:#x}", info.width, info.height, info.bpp, info.fb_addr);

    FB.store(info.fb_addr, Ordering::Relaxed);
    FB_GEOM.store((info.width as u64) | (info.height as u64) << 16 | (info.pitch as u64) << 32 | (info.red_pos as u64) << 56, Ordering::Relaxed);

    mem::set_kernel_phys(info.kphys);
    arch::init_idt();
    unsafe {
        let heap = core::ptr::addr_of_mut!(HEAP) as *mut u8;
        ALLOC.lock().init(heap, HEAP_SIZE);
    }
    arch::calibrate_timer();

    // memoria totale dalla mappa E820
    let mut ram: u64 = 0;
    for i in 0..(info.mmap_count as usize).min(128) {
        let e = info.mmap[i];
        if e.kind == 1 {
            ram += e.len;
        }
    }
    let ram_mb = ram / (1024 * 1024);
    let mut brand = [0u8; 48];
    let n = arch::cpu_brand(&mut brand);
    let cpu: alloc::string::String = core::str::from_utf8(&brand[..n]).unwrap_or("x86_64").trim_matches(char::from(0)).trim().into();
    let uefi = info.magic == u32::from_le_bytes(*b"MNOU");
    log!("avvio: {}", if uefi { "UEFI" } else { "BIOS" });
    let acpi = acpi::init(info.rsdp, if uefi { 64 << 30 } else { 4 << 30 });
    let cores = if acpi.cpus > 0 { acpi.cpus } else { arch::cpu_cores() };
    log!("CPU: {} ({} core), RAM: {} MB", cpu, cores, ram_mb);
    desktop::SYS.set((cpu.clone(), cores, ram_mb, info.width as i32, info.height as i32));

    let screen = gfx::Screen::new(info.fb_addr, info.width, info.height, info.pitch, info.red_pos);
    let settings = ui::Settings {
        lang: i18n::Lang::It,
        layout: keymap::Layout::It,
        user: alloc::string::String::new(),
        pc: alloc::string::String::from("MIN-PC"),
        dark: true,
        accent: 0,
        preset: 1,
        anim: true,
        glass: true,
        icon_size: 1,
        clock24: true,
        show_icons: true,
        sort_name: false,
    };
    let input = input::Input::new(info.width as i32, info.height as i32);
    let sys = ui::SysInfo { cpu, cores, ram_mb, w: info.width as i32, h: info.height as i32 };
    let mut gui = ui::Gui::new(screen, input, sys, &settings);

    let devs = pci::scan();
    log!("PCI: {} dispositivi", devs.len());
    usb::init(&devs);
    block::init(&devs);
    fat::mount();
    net::init(&devs);
    if let Some(saved) = storage::load() {
        // gia' installato: dritti al desktop con le impostazioni salvate
        gui.input.layout = saved.layout;
        gui.apply(&saved);
        installer::splash(&mut gui, &saved);
        log!("bentornato, {}", saved.user);
        desktop::run(&mut gui, saved)
    }
    installer::splash(&mut gui, &settings);
    let settings = installer::run(&mut gui, settings);
    log!("installazione completata: utente {}", settings.user);
    desktop::run(&mut gui, settings)
}

// ------------------------------------------------------------------ errori fatali

struct Buf {
    data: [u8; 256],
    len: usize,
}
impl core::fmt::Write for Buf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.len < self.data.len() {
                self.data[self.len] = b;
                self.len += 1;
            }
        }
        Ok(())
    }
}

/// Schermata di errore disegnata direttamente sul framebuffer (senza heap).
fn crash(title: &str, detail: &str) -> ! {
    log!("ERRORE: {} - {}", title, detail);
    let fb = FB.load(Ordering::Relaxed);
    let g = FB_GEOM.load(Ordering::Relaxed);
    if fb != 0 {
        let (w, h, pitch, red) = ((g & 0xFFFF) as u32, ((g >> 16) & 0xFFFF) as u32, ((g >> 32) & 0xFFFFFF) as u32, (g >> 56) as u32);
        let scr = gfx::Screen::new(fb, w, h, pitch, red);
        let bg = gfx::rgb(28, 14, 30);
        scr.raw_fill(bg);
        let lines = [":(", "MIN-OS ha riscontrato un problema e deve fermarsi.", title, detail, "Riavvia il computer per continuare."];
        let mut y = h as i32 / 2 - 110;
        for (i, l) in lines.iter().enumerate() {
            let f = if i == 0 { font::HERO } else if i == 2 { font::LARGE } else { font::MED };
            raw_text(&scr, 80, y, l, &f, bg);
            y += if i == 0 { 90 } else { 40 };
        }
    }
    loop {
        arch::halt();
    }
}

fn raw_text(scr: &gfx::Screen, x: i32, y: i32, s: &str, f: &font::Font, bg: u32) {
    use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight};
    let rh = match f.size {
        font::Size::S16 => RasterHeight::Size16,
        font::Size::S20 => RasterHeight::Size20,
        font::Size::S24 => RasterHeight::Size24,
        font::Size::S32 => RasterHeight::Size32,
    };
    let wt = if f.bold { FontWeight::Bold } else { FontWeight::Regular };
    let cw = get_raster_width(wt, rh) as i32;
    for (i, c) in s.chars().enumerate() {
        if let Some(r) = get_raster(c, wt, rh) {
            for (ry, row) in r.raster().iter().enumerate() {
                for (rx, &v) in row.iter().enumerate() {
                    if v > 0 {
                        for sy in 0..f.scale {
                            for sx in 0..f.scale {
                                scr.raw_put(
                                    x + (i as i32 * cw + rx as i32) * f.scale + sx,
                                    y + ry as i32 * f.scale + sy,
                                    gfx::blend(bg, gfx::WHITE, v as u32 + 1),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[no_mangle]
extern "C" fn exception_handler(f: &arch::ExceptionFrame) -> ! {
    use core::fmt::Write;
    let mut b = Buf { data: [0; 256], len: 0 };
    let _ = write!(b, "RIP {:#x}  codice {:#x}", f.rip, f.error);
    if f.vector == 14 {
        let _ = write!(b, "  indirizzo {:#x}", arch::cr2());
    }
    let detail = core::str::from_utf8(&b.data[..b.len]).unwrap_or("");
    crash(arch::exception_name(f.vector), detail)
}

#[panic_handler]
fn panic(p: &core::panic::PanicInfo) -> ! {
    use core::fmt::Write;
    let mut b = Buf { data: [0; 256], len: 0 };
    if let Some(l) = p.location() {
        let _ = write!(b, "{}:{}  ", l.file(), l.line());
    }
    let _ = write!(b, "{}", p.message());
    let detail = core::str::from_utf8(&b.data[..b.len]).unwrap_or("panic");
    crash("Kernel panic", detail)
}
