/* MIN-OS: layout dell'immagine.
 * Su disco:   [boot sector][stage2 16 settori][kernel...]
 * In memoria: boot a 0x7C00, kernel a 16 MiB (indirizzo fisico = virtuale). */
ENTRY(_start16)

KERNEL_VMA = 0x1000000;
BOOT_SIZE  = 17 * 512;
KERNEL_LMA = 0x7C00 + BOOT_SIZE;

SECTIONS
{
    . = 0x7C00;
    .boot : AT(0x7C00) { KEEP(*(.boot)) }

    . = KERNEL_VMA;
    __kernel_start = .;
    .text : AT(KERNEL_LMA) { KEEP(*(.text.kentry)) *(.text .text.*) }
    .rodata : AT(KERNEL_LMA + ADDR(.rodata) - KERNEL_VMA) { *(.rodata .rodata.*) *(.data.rel.ro .data.rel.ro.*) }
    .data : AT(KERNEL_LMA + ADDR(.data) - KERNEL_VMA) { *(.data .data.*) *(.got .got.*) . = ALIGN(16); }
    __kernel_end = .;

    .bss (NOLOAD) : ALIGN(4096) {
        __bss_start = .;
        *(.bss .bss.*) *(COMMON)
        . = ALIGN(16);
        . += 0x100000;          /* stack del kernel: 1 MiB */
        __stack_top = .;
        . = ALIGN(8);
        __bss_end = .;
    }

    /DISCARD/ : { *(.eh_frame*) *(.comment) *(.note*) *(.gnu.hash) *(.hash) *(.dynsym) *(.dynstr) *(.dynamic) }
}

__kernel_sectors = (__kernel_end - __kernel_start + 511) / 512;
