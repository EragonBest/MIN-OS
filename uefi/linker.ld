/* Loader UEFI: eseguibile indipendente dalla posizione, convertito in PE da xtask. */
ENTRY(efi_main)
SECTIONS
{
    . = 0x1000;
    .text : { *(.text .text.*) }
    . = ALIGN(16);
    .rodata : { *(.rodata .rodata.*) }
    .data.rel.ro : { *(.data.rel.ro .data.rel.ro.*) }
    .got : { *(.got .got.*) }
    .data : { *(.data .data.*) }
    .dynamic : { *(.dynamic) }
    .rela.dyn : { *(.rela.dyn .rela.*) }
    .dynsym : { *(.dynsym) }
    .dynstr : { *(.dynstr) }
    .hash : { *(.hash) }
    .gnu.hash : { *(.gnu.hash) }
    .bss : { *(.bss .bss.*) *(COMMON) }
    /DISCARD/ : { *(.eh_frame*) *(.comment) *(.note*) *(.interp) }
}
