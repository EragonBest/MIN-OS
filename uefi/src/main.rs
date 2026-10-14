//! Bootloader UEFI di MIN-OS, scritto da zero (nessuna libreria UEFI esterna).
//!
//! 1. sceglie una modalita' grafica con il GOP (Graphics Output Protocol)
//! 2. trova le tabelle ACPI
//! 3. prenota la memoria a 16 MiB per il kernel e prepara le page table
//! 4. esce dai "boot services" del firmware
//! 5. copia il kernel (incorporato in questo file) e salta a `kentry64`

#![no_std]
#![no_main]

use core::arch::asm;
use core::ffi::c_void;

static KERNEL: &[u8] = include_bytes!(env!("MINOS_KERNEL_BLOB"));
const KERNEL_BASE: u64 = 0x100_0000;
const KERNEL_SPAN: u64 = parse(env!("MINOS_KERNEL_SPAN"));
/// GiB mappati 1:1 dalle page table (framebuffer e tabelle ACPI possono stare in alto)
const MAP_GIB: usize = 64;

const fn parse(s: &str) -> u64 {
    let b = s.as_bytes();
    let mut i = 0;
    let mut v = 0u64;
    while i < b.len() {
        v = v * 10 + (b[i] - b'0') as u64;
        i += 1;
    }
    v
}

type Status = usize;

#[repr(C)]
#[derive(Clone, Copy, PartialEq)]
struct Guid(u32, u16, u16, [u8; 8]);

const GOP_GUID: Guid = Guid(0x9042a9de, 0x23dc, 0x4a38, [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a]);
const ACPI2_GUID: Guid = Guid(0x8868e871, 0xe4f1, 0x11d3, [0xbc, 0x22, 0x00, 0x80, 0xc7, 0x3c, 0x88, 0x81]);
const ACPI1_GUID: Guid = Guid(0xeb9d2d30, 0x2d88, 0x11d3, [0x9a, 0x16, 0x00, 0x90, 0x27, 0x3f, 0xc1, 0x4d]);

#[repr(C)]
struct Header {
    signature: u64,
    revision: u32,
    size: u32,
    crc32: u32,
    reserved: u32,
}

#[repr(C)]
pub struct SystemTable {
    hdr: Header,
    vendor: *const u16,
    revision: u32,
    con_in_handle: *mut c_void,
    con_in: *mut c_void,
    con_out_handle: *mut c_void,
    con_out: *mut TextOut,
    err_handle: *mut c_void,
    err: *mut c_void,
    runtime: *mut c_void,
    boot: *mut BootServices,
    n_cfg: usize,
    cfg: *const ConfigTable,
}

#[repr(C)]
struct TextOut {
    reset: usize,
    output: extern "efiapi" fn(*mut TextOut, *const u16) -> Status,
}

#[repr(C)]
struct BootServices {
    hdr: Header,
    f: [usize; 44],
}

#[repr(C)]
struct ConfigTable {
    guid: Guid,
    table: *mut c_void,
}

#[repr(C)]
struct Gop {
    query_mode: extern "efiapi" fn(*mut Gop, u32, *mut usize, *mut *mut ModeInfo) -> Status,
    set_mode: extern "efiapi" fn(*mut Gop, u32) -> Status,
    blt: usize,
    mode: *mut GopMode,
}

#[repr(C)]
struct GopMode {
    max_mode: u32,
    mode: u32,
    info: *mut ModeInfo,
    size_of_info: usize,
    fb_base: u64,
    fb_size: usize,
}

#[repr(C)]
struct ModeInfo {
    version: u32,
    w: u32,
    h: u32,
    format: u32,
    mask: [u32; 4],
    stride: u32,
}

/// Identica a `BootInfo` del kernel.
#[repr(C)]
struct BootInfo {
    magic: u32,
    boot_drive: u32,
    fb_addr: u64,
    width: u32,
    height: u32,
    pitch: u32,
    bpp: u32,
    red_pos: u32,
    mmap_count: u32,
    rsdp: u64,
    mmap: [E820; 128],
    /// indirizzo fisico reale del kernel (che il paging mostra a 16 MiB)
    kphys: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct E820 {
    base: u64,
    len: u64,
    kind: u32,
    acpi: u32,
}

static mut INFO: BootInfo = BootInfo {
    magic: 0,
    boot_drive: 0,
    fb_addr: 0,
    width: 0,
    height: 0,
    pitch: 0,
    bpp: 0,
    red_pos: 0,
    mmap_count: 0,
    rsdp: 0,
    mmap: [E820 { base: 0, len: 0, kind: 0, acpi: 0 }; 128],
    kphys: 0,
};
static mut MMAP: [u8; 64 * 1024] = [0; 64 * 1024];

// indici delle funzioni nella tabella dei Boot Services
const ALLOCATE_PAGES: usize = 2;
const GET_MEMORY_MAP: usize = 4;
const EXIT_BOOT_SERVICES: usize = 26;
const SET_WATCHDOG: usize = 29;
const LOCATE_PROTOCOL: usize = 37;

unsafe fn bs(st: *mut SystemTable) -> &'static BootServices {
    unsafe { &*(*st).boot }
}

fn print(st: *mut SystemTable, s: &str) {
    let mut buf = [0u16; 160];
    let mut i = 0;
    for c in s.chars() {
        if i + 3 >= buf.len() {
            break;
        }
        if c == '\n' {
            buf[i] = '\r' as u16;
            i += 1;
        }
        buf[i] = if (c as u32) < 0x10000 { c as u16 } else { '?' as u16 };
        i += 1;
    }
    buf[i] = 0;
    unsafe {
        let out = (*st).con_out;
        if !out.is_null() {
            ((*out).output)(out, buf.as_ptr());
        }
    }
}

fn fail(st: *mut SystemTable, msg: &str) -> ! {
    print(st, "\nMIN-OS: ");
    print(st, msg);
    print(st, "\n");
    loop {
        unsafe { asm!("hlt") };
    }
}

unsafe fn allocate(st: *mut SystemTable, kind: u32, pages: usize, addr: &mut u64) -> Status {
    unsafe {
        let f: extern "efiapi" fn(u32, u32, usize, *mut u64) -> Status = core::mem::transmute(bs(st).f[ALLOCATE_PAGES]);
        f(kind, 2 /* LoaderData */, pages, addr)
    }
}

#[no_mangle]
pub extern "efiapi" fn efi_main(image: *mut c_void, st: *mut SystemTable) -> Status {
    unsafe {
        let wd: extern "efiapi" fn(usize, u64, usize, *const u16) -> Status = core::mem::transmute(bs(st).f[SET_WATCHDOG]);
        wd(0, 0, 0, core::ptr::null());
        print(st, "MIN-OS: avvio UEFI...\n");

        // ---- grafica
        let locate: extern "efiapi" fn(*const Guid, *mut c_void, *mut *mut c_void) -> Status = core::mem::transmute(bs(st).f[LOCATE_PROTOCOL]);
        let mut gop: *mut Gop = core::ptr::null_mut();
        if locate(&GOP_GUID, core::ptr::null_mut(), &mut gop as *mut *mut Gop as *mut *mut c_void) != 0 || gop.is_null() {
            fail(st, "scheda grafica UEFI (GOP) non trovata.");
        }
        let max = (*(*gop).mode).max_mode;
        let mut best: Option<(u32, u32)> = None; // (modo, area)
        for m in 0..max {
            let mut size = 0usize;
            let mut info: *mut ModeInfo = core::ptr::null_mut();
            if ((*gop).query_mode)(gop, m, &mut size, &mut info) != 0 || info.is_null() {
                continue;
            }
            let i = &*info;
            if i.format > 1 || i.w < 800 || i.h < 600 || i.w > 1280 || i.h > 800 {
                continue;
            }
            let area = i.w * i.h;
            if best.map(|b| area > b.1).unwrap_or(true) {
                best = Some((m, area));
            }
        }
        if let Some((m, _)) = best {
            if (*(*gop).mode).mode != m {
                ((*gop).set_mode)(gop, m);
            }
        }
        let gm = &*(*gop).mode;
        let mi = &*gm.info;
        if mi.format > 1 {
            fail(st, "formato dei pixel non supportato.");
        }
        let info = &mut *core::ptr::addr_of_mut!(INFO);
        info.magic = u32::from_le_bytes(*b"MNOU");
        info.fb_addr = gm.fb_base;
        info.width = mi.w;
        info.height = mi.h;
        info.pitch = mi.stride * 4;
        info.bpp = 32;
        info.red_pos = if mi.format == 0 { 0 } else { 16 };

        // ---- tabelle ACPI
        for i in 0..(*st).n_cfg {
            let c = &*(*st).cfg.add(i);
            if c.guid == ACPI2_GUID {
                info.rsdp = c.table as u64;
            } else if c.guid == ACPI1_GUID && info.rsdp == 0 {
                info.rsdp = c.table as u64;
            }
        }

        // ---- memoria per il kernel: ovunque ci sia spazio (allineata a 2 MiB);
        //      il paging la fara' comunque apparire all'indirizzo 16 MiB.
        let big = (KERNEL_SPAN as usize).div_ceil(0x20_0000);
        let pages = big * 512 + 512;
        let mut kaddr = 0u64;
        if allocate(st, 0 /* AnyPages */, pages, &mut kaddr) != 0 {
            fail(st, "memoria insufficiente per il kernel (servono almeno 128 MB).");
        }
        let kphys = (kaddr + 0x1F_FFFF) & !0x1F_FFFF;
        let mut pt = 0u64;
        if allocate(st, 0 /* AnyPages */, 2 + MAP_GIB, &mut pt) != 0 {
            fail(st, "memoria insufficiente per le page table.");
        }
        let tables = core::slice::from_raw_parts_mut(pt as *mut u64, (2 + MAP_GIB) * 512);
        tables.fill(0);
        tables[0] = (pt + 4096) | 3; // PML4[0] -> PDPT
        for g in 0..MAP_GIB {
            let pd = pt + 8192 + g as u64 * 4096;
            tables[512 + g] = pd | 3;
            for e in 0..512 {
                tables[1024 + g * 512 + e] = ((g as u64 * 512 + e as u64) << 21) | 0x83;
            }
        }
        let first = (KERNEL_BASE >> 21) as usize;
        for k in 0..big {
            tables[1024 + first + k] = (kphys + (k as u64) * 0x20_0000) | 0x83;
        }
        info.kphys = kphys;

        // ---- uscita dai boot services
        let get_map: extern "efiapi" fn(*mut usize, *mut u8, *mut usize, *mut usize, *mut u32) -> Status = core::mem::transmute(bs(st).f[GET_MEMORY_MAP]);
        let exit: extern "efiapi" fn(*mut c_void, usize) -> Status = core::mem::transmute(bs(st).f[EXIT_BOOT_SERVICES]);
        let map = &mut *core::ptr::addr_of_mut!(MMAP);
        let (mut size, mut key, mut dsize, mut dver) = (0usize, 0usize, 0usize, 0u32);
        print(st, "MIN-OS: avvio del kernel\n");
        let mut exited = false;
        for _ in 0..3 {
            size = map.len();
            if get_map(&mut size, map.as_mut_ptr(), &mut key, &mut dsize, &mut dver) != 0 {
                continue;
            }
            if exit(image, key) == 0 {
                exited = true;
                break;
            }
        }
        if !exited {
            fail(st, "il firmware non ha permesso di uscire dai boot services.");
        }

        // ---- da qui in poi niente firmware: siamo soli
        asm!("cli");
        let mut n = 0usize;
        let mut off = 0;
        while off + dsize <= size && dsize >= 40 {
            let d = map.as_ptr().add(off);
            let typ = (d as *const u32).read_unaligned();
            let base = (d.add(8) as *const u64).read_unaligned();
            let len = (d.add(24) as *const u64).read_unaligned() * 4096;
            let kind = match typ {
                1..=4 | 7 => 1,
                9 => 3,
                10 => 4,
                _ => 2,
            };
            if n > 0 {
                let last = &mut info.mmap[n - 1];
                let (lb, ll, lk) = (last.base, last.len, last.kind);
                if lk == kind && lb + ll == base {
                    last.len = ll + len;
                    off += dsize;
                    continue;
                }
            }
            if n < 128 {
                info.mmap[n] = E820 { base, len, kind, acpi: 1 };
                n += 1;
            }
            off += dsize;
        }
        info.mmap_count = n as u32;

        core::ptr::copy_nonoverlapping(KERNEL.as_ptr(), kphys as *mut u8, KERNEL.len());
        asm!(
            "mov cr3, {pt}",
            "jmp {entry}",
            pt = in(reg) pt,
            entry = in(reg) KERNEL_BASE,
            in("rdi") info as *mut BootInfo as u64,
            options(noreturn)
        );
    }
}

// ------------------------------------------------------------------ supporto al compilatore

#[no_mangle]
pub unsafe extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    unsafe { asm!("rep movsb", inout("rcx") n => _, inout("rdi") dst => _, inout("rsi") src => _, options(nostack, preserves_flags)) };
    dst
}

#[no_mangle]
pub unsafe extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    unsafe {
        if (dst as usize) <= (src as usize) || (dst as usize) >= (src as usize + n) {
            return memcpy(dst, src, n);
        }
        asm!("std", "rep movsb", "cld", inout("rcx") n => _, inout("rdi") dst.add(n - 1) => _, inout("rsi") src.add(n - 1) => _, options(nostack));
    }
    dst
}

#[no_mangle]
pub unsafe extern "C" fn memset(dst: *mut u8, c: i32, n: usize) -> *mut u8 {
    unsafe { asm!("rep stosb", inout("rcx") n => _, inout("rdi") dst => _, in("al") c as u8, options(nostack, preserves_flags)) };
    dst
}

#[no_mangle]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let mut i = 0;
    while i < n {
        let (x, y) = unsafe { (core::ptr::read_volatile(a.add(i)), core::ptr::read_volatile(b.add(i))) };
        if x != y {
            return x as i32 - y as i32;
        }
        i += 1;
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn bcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    unsafe { memcmp(a, b, n) }
}

#[no_mangle]
pub extern "C" fn _Unwind_Resume() -> ! {
    loop {}
}

#[no_mangle]
pub extern "C" fn rust_eh_personality() {}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        unsafe { asm!("hlt") };
    }
}
