//! Funzioni di memoria di base richieste dal compilatore (niente libc in un kernel).

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

const KERNEL_VIRT: u64 = 0x100_0000;
static KERNEL_PHYS: AtomicU64 = AtomicU64::new(0);

pub fn set_kernel_phys(p: u64) {
    KERNEL_PHYS.store(p, Ordering::Relaxed);
}

extern "C" {
    static __bss_end: u8;
}

/// Byte occupati dal kernel in memoria (codice, dati, heap e stack).
fn kernel_span() -> u64 {
    (unsafe { core::ptr::addr_of!(__bss_end) } as u64) - KERNEL_VIRT
}

/// Indirizzo fisico di un indirizzo del kernel (serve per il DMA dei dispositivi).
/// Con il BIOS coincidono; con UEFI il kernel puo' stare altrove in memoria fisica.
pub fn phys(virt: u64) -> u64 {
    let k = KERNEL_PHYS.load(Ordering::Relaxed);
    if k != 0 && virt >= KERNEL_VIRT && virt < KERNEL_VIRT + kernel_span() {
        virt - KERNEL_VIRT + k
    } else {
        virt
    }
}

#[no_mangle]
pub unsafe extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    asm!("rep movsb", inout("rcx") n => _, inout("rdi") dst => _, inout("rsi") src => _, options(nostack, preserves_flags));
    dst
}

#[no_mangle]
pub unsafe extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if (dst as usize) <= (src as usize) || (dst as usize) >= (src as usize + n) {
        return memcpy(dst, src, n);
    }
    asm!(
        "std",
        "rep movsb",
        "cld",
        inout("rcx") n => _,
        inout("rdi") dst.add(n - 1) => _,
        inout("rsi") src.add(n - 1) => _,
        options(nostack)
    );
    dst
}

#[no_mangle]
pub unsafe extern "C" fn memset(dst: *mut u8, c: i32, n: usize) -> *mut u8 {
    asm!("rep stosb", inout("rcx") n => _, inout("rdi") dst => _, in("al") c as u8, options(nostack, preserves_flags));
    dst
}

#[no_mangle]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let mut i = 0;
    while i < n {
        let x = core::ptr::read_volatile(a.add(i));
        let y = core::ptr::read_volatile(b.add(i));
        if x != y {
            return x as i32 - y as i32;
        }
        i += 1;
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn bcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    memcmp(a, b, n)
}

/// Riempie una regione di u32 (usato per riempire i pixel velocemente).
#[inline(always)]
pub fn fill_u32(dst: &mut [u32], v: u32) {
    unsafe {
        asm!("rep stosd", inout("rcx") dst.len() => _, inout("rdi") dst.as_mut_ptr() => _, in("eax") v, options(nostack, preserves_flags));
    }
}

/// La libreria `alloc` precompilata contiene riferimenti allo "stack unwinding".
/// MIN-OS usa panic = abort, quindi non viene mai chiamata davvero.
#[no_mangle]
pub extern "C" fn _Unwind_Resume() -> ! {
    loop {
        crate::arch::halt();
    }
}

#[no_mangle]
pub extern "C" fn rust_eh_personality() {}

/// Memoria azzerata e allineata per il DMA dei dispositivi (non viene mai liberata).
pub fn dma_alloc(size: usize, align: usize) -> *mut u8 {
    unsafe { alloc::alloc::alloc_zeroed(alloc::alloc::Layout::from_size_align(size.max(1), align).unwrap()) }
}

// ------------------------------------------------------------------ mappatura dei dispositivi

/// Indirizzo virtuale di una pagina fisica delle page table.
fn table_virt(p: u64) -> u64 {
    let k = KERNEL_PHYS.load(Ordering::Relaxed);
    let span = kernel_span();
    if k != 0 && p >= k && p < k + span {
        p - k + KERNEL_VIRT
    } else {
        p
    }
}

/// Mappa 1:1 (non cacheable) la memoria di un dispositivo, ovunque si trovi.
/// I registri dei dispositivi PCI possono stare oltre i 4 o 64 GiB gia' mappati.
pub fn map_mmio(addr: u64, size: u64) {
    let cr3: u64;
    unsafe { asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack)) };
    let pml4 = table_virt(cr3 & 0x000F_FFFF_FFFF_F000) as *mut u64;
    let mut a = addr & !0x1F_FFFF;
    let end = addr + size.max(1);
    let mut changed = false;
    unsafe {
        while a < end {
            let next = |t: *mut u64, i: usize| -> *mut u64 {
                let e = core::ptr::read_volatile(t.add(i));
                if e & 1 != 0 {
                    return table_virt(e & 0x000F_FFFF_FFFF_F000) as *mut u64;
                }
                let n = dma_alloc(4096, 4096) as *mut u64;
                core::ptr::write_volatile(t.add(i), phys(n as u64) | 3);
                n
            };
            let pdpt = next(pml4, ((a >> 39) & 511) as usize);
            let pdpte = core::ptr::read_volatile(pdpt.add(((a >> 30) & 511) as usize));
            if pdpte & 0x81 == 0x81 {
                a += 0x4000_0000; // gia' mappato con una pagina da 1 GiB
                continue;
            }
            let pd = next(pdpt, ((a >> 30) & 511) as usize);
            let i = ((a >> 21) & 511) as usize;
            if core::ptr::read_volatile(pd.add(i)) & 1 == 0 {
                // 2 MiB, presente + scrivibile + pagina grande + cache disattivata
                core::ptr::write_volatile(pd.add(i), a | 0x83 | 0x18);
                changed = true;
            }
            a += 0x20_0000;
        }
        if changed {
            asm!("mov cr3, {}", in(reg) cr3, options(nostack));
        }
    }
}
