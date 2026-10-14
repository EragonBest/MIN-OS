# ============================================================================
#  MIN-OS bootloader - scritto da zero
#
#  Stage 1 (settore 0, 512 byte): caricato dal BIOS a 0x7C00, carica lo stage 2.
#  Stage 2 (16 settori a 0x7E00):
#     1. carica il kernel sopra 1 MiB (unreal mode + int 13h)
#     2. legge la mappa della memoria (E820)
#     3. sceglie e imposta la modalita' grafica VBE a 32 bit
#     4. costruisce le page table (identita' sui primi 4 GiB)
#     5. passa direttamente dalla real mode alla long mode (64 bit)
#     6. salta a kmain() in Rust
#
#  Struttura BootInfo a 0x5000 (vedi main.rs):
#     0x00 magic   0x04 boot_drive  0x08 fb_addr(64)  0x10 width  0x14 height
#     0x18 pitch   0x1C bpp         0x20 red_pos      0x24 mmap_count
#     0x30 voci E820 (24 byte ciascuna, max 128)
# ============================================================================

.set BOOTINFO,      0x5000
.set VBE_CTRL,      0x6000
.set VBE_MODE,      0x6200
.set LOAD_BUF_SEG,  0x1000
.set STAGE2_SECTS,  16
.set MAX_W,         1280
.set MAX_H,         800

.section .boot, "awx"
.code16
.global _start16
_start16:
    cli
    ljmp $0, $.Lnorm
.Lnorm:
    xor %ax, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    mov $0x7C00, %sp
    cld
    sti
    mov %dl, boot_drive

    # Linea A20 (metodo "fast A20")
    in $0x92, %al
    or $2, %al
    and $0xFE, %al
    out %al, $0x92

    mov $msg_boot, %si
    call print

    # Carica lo stage 2 (LBA 1..16) a 0x7E00
    movw $STAGE2_SECTS, dap_count
    movw $0x7E00, dap_off
    movw $0, dap_seg
    movl $1, dap_lba
    movl $0, dap_lba + 4
    mov $dap, %si
    mov boot_drive, %dl
    mov $0x42, %ah
    int $0x13
    jc disk_error
    jmp stage2

disk_error:
    mov $msg_disk, %si
    call print
halt16:
    cli
    hlt
    jmp halt16

# stampa la stringa terminata da zero in DS:SI
print:
    lodsb
    test %al, %al
    jz 1f
    mov $0x0E, %ah
    mov $7, %bx
    int $0x10
    jmp print
1:  ret

msg_boot: .asciz "MIN-OS "
msg_disk: .asciz "\r\nErrore di lettura del disco."

.balign 4
dap:        .byte 16, 0
dap_count:  .word 0
dap_off:    .word 0
dap_seg:    .word 0
dap_lba:    .long 0, 0
boot_drive: .byte 0

.org 446
    .fill 64, 1, 0          # tabella delle partizioni (scritta da xtask)
    .word 0xAA55

# ============================================================================
#  STAGE 2  (0x7E00)
# ============================================================================
stage2:
    # azzera la BootInfo
    xor %eax, %eax
    mov $BOOTINFO, %di
    mov $(0x1000 / 4), %cx
    rep stosl
    movl $0x534F4E4D, BOOTINFO          # 'MNOS'
    movzbl boot_drive, %eax
    movl %eax, BOOTINFO + 0x04

    # ---------------- caricamento del kernel ----------------
    movl $__kernel_sectors, %eax
    movl %eax, remaining
    movl $(1 + STAGE2_SECTS), %eax
    movl %eax, cur_lba
    movl $0x1000000, %eax              # il kernel va a 16 MiB
    movl %eax, dest

.Lload:
    movl remaining, %eax
    test %eax, %eax
    jz .Lloaded
    cmp $64, %eax
    jbe 1f
    mov $64, %eax
1:  movl %eax, chunk
    movw %ax, dap_count
    movw $0, dap_off
    movw $LOAD_BUF_SEG, dap_seg
    movl cur_lba, %eax
    movl %eax, dap_lba
    movl $0, dap_lba + 4
    mov $dap, %si
    mov boot_drive, %dl
    mov $0x42, %ah
    int $0x13
    jc disk_error

    call enter_unreal
    movl chunk, %ecx
    shl $7, %ecx                        # settori * 128 = dword
    movl $(LOAD_BUF_SEG * 16), %esi
    movl dest, %edi
2:  movl (%esi), %eax
    movl %eax, (%edi)
    add $4, %esi
    add $4, %edi
    dec %ecx
    jnz 2b
    movl %edi, dest

    movl chunk, %eax
    addl %eax, cur_lba
    subl %eax, remaining
    mov $0x0E2E, %ax                    # stampa '.'
    mov $7, %bx
    int $0x10
    jmp .Lload
.Lloaded:

    # ---------------- mappa della memoria E820 ----------------
    xor %ebx, %ebx
    mov $(BOOTINFO + 0x30), %di
.Le820:
    movl $1, %es:20(%di)
    mov $0xE820, %eax
    mov $24, %ecx
    mov $0x534D4150, %edx
    int $0x15
    jc .Le820_done
    cmp $0x534D4150, %eax
    jne .Le820_done
    incl BOOTINFO + 0x24
    add $24, %di
    test %ebx, %ebx
    jz .Le820_done
    cmpl $128, BOOTINFO + 0x24
    jae .Le820_done
    jmp .Le820
.Le820_done:

    # ---------------- grafica VBE ----------------
    movl $0x32454256, VBE_CTRL          # 'VBE2'
    mov $0x4F00, %ax
    mov $VBE_CTRL, %di
    int $0x10
    cmp $0x004F, %ax
    jne no_vbe

    movw VBE_CTRL + 14, %si             # puntatore far alla lista dei modi
    movw VBE_CTRL + 16, %ax
    mov %ax, %fs
    movw $0xFFFF, best_mode
    movl $0, best_area

.Lmode:
    movw %fs:(%si), %cx
    cmp $0xFFFF, %cx
    je .Lmode_done
    add $2, %si
    mov %cx, cur_mode
    push %si
    push %fs
    mov $0x4F01, %ax
    mov $VBE_MODE, %di
    int $0x10
    pop %fs
    pop %si
    cmp $0x004F, %ax
    jne .Lmode
    movw VBE_MODE, %ax
    and $0x99, %ax                      # supportato, colore, grafico, LFB
    cmp $0x99, %ax
    jne .Lmode
    cmpb $32, VBE_MODE + 25             # 32 bit per pixel
    jne .Lmode
    cmpb $6, VBE_MODE + 27              # direct color
    jne .Lmode
    movzwl VBE_MODE + 18, %eax
    cmp $MAX_W, %eax
    ja .Lmode
    cmp $800, %eax
    jb .Lmode
    movzwl VBE_MODE + 20, %ebx
    cmp $MAX_H, %ebx
    ja .Lmode
    imul %ebx, %eax
    cmpl best_area, %eax
    jbe .Lmode
    movl %eax, best_area
    movw cur_mode, %ax
    movw %ax, best_mode
    jmp .Lmode
.Lmode_done:
    cmpw $0xFFFF, best_mode
    je no_vbe

    mov best_mode, %cx
    mov $0x4F01, %ax
    mov $VBE_MODE, %di
    int $0x10
    mov best_mode, %bx
    or $0x4000, %bx                     # framebuffer lineare
    mov $0x4F02, %ax
    int $0x10
    cmp $0x004F, %ax
    jne no_vbe

    movl VBE_MODE + 40, %eax
    movl %eax, BOOTINFO + 0x08
    movzwl VBE_MODE + 18, %eax
    movl %eax, BOOTINFO + 0x10
    movzwl VBE_MODE + 20, %eax
    movl %eax, BOOTINFO + 0x14
    movzwl VBE_MODE + 16, %eax
    movl %eax, BOOTINFO + 0x18
    movzbl VBE_MODE + 25, %eax
    movl %eax, BOOTINFO + 0x1C
    movzbl VBE_MODE + 32, %eax
    movl %eax, BOOTINFO + 0x20

    # ---------------- verso la long mode ----------------
    cli
    mov $0xFF, %al                      # maschera i PIC
    out %al, $0xA1
    out %al, $0x21

    # page table a 0x70000: PML4, PDPT, 4 PD (pagine da 2 MiB, 0..4 GiB)
    mov $0x7000, %ax
    mov %ax, %es
    xor %di, %di
    xor %eax, %eax
    mov $(0x6000 / 4), %ecx
    rep stosl
    movl $0x71003, %es:0x0000
    movl $0x72003, %es:0x1000
    movl $0x73003, %es:0x1008
    movl $0x74003, %es:0x1010
    movl $0x75003, %es:0x1018
    mov $0x2000, %di
    mov $0x83, %eax
    mov $2048, %ecx
3:  movl %eax, %es:(%di)
    add $0x200000, %eax
    add $8, %di
    dec %ecx
    jnz 3b
    xor %ax, %ax
    mov %ax, %es

    mov $0x70000, %eax
    mov %eax, %cr3
    mov %cr4, %eax
    or $0x620, %eax                     # PAE | OSFXSR | OSXMMEXCPT
    mov %eax, %cr4
    mov $0xC0000080, %ecx               # EFER.LME
    rdmsr
    or $0x100, %eax
    wrmsr
    lgdtl gdt_desc
    mov %cr0, %eax
    and $0xFFFFFFFB, %eax               # niente emulazione FPU
    or $0x80000003, %eax                # PG | MP | PE
    mov %eax, %cr0
    ljmp $0x08, $long_entry

no_vbe:
    mov $msg_vbe, %si
    call print
    jmp halt16

enter_unreal:
    cli
    push %ds
    push %es
    lgdtl gdt_desc
    mov %cr0, %eax
    or $1, %al
    mov %eax, %cr0
    jmp 1f
1:  mov $0x10, %bx
    mov %bx, %ds
    mov %bx, %es
    and $0xFE, %al
    mov %eax, %cr0
    pop %es
    pop %ds
    sti
    ret

msg_vbe: .asciz "\r\nMIN-OS richiede una scheda video VBE a 32 bit (almeno 800x600)."

.balign 4
remaining:  .long 0
cur_lba:    .long 0
dest:       .long 0
chunk:      .long 0
best_area:  .long 0
best_mode:  .word 0
cur_mode:   .word 0

.balign 8
gdt:
    .quad 0
    .quad 0x00AF9A000000FFFF            # 0x08: codice 64 bit
    .quad 0x00CF92000000FFFF            # 0x10: dati 4 GiB
gdt_desc:
    .word gdt_desc - gdt - 1
    .long gdt

.code64
long_entry:
    mov $BOOTINFO, %edi
    movabs $kentry64, %rax
    jmp *%rax

.org (512 * (1 + STAGE2_SECTS))

# ============================================================================
#  Ingresso a 64 bit comune (BIOS e UEFI). RDI = puntatore alla BootInfo.
#  E' la prima cosa del kernel: si trova esattamente a 16 MiB.
# ============================================================================
.section .text.kentry, "ax"
.code64
.global kentry64
kentry64:
    cli
    mov %rdi, %r12
    lgdt kgdt_desc(%rip)
    lea 1f(%rip), %rax
    pushq $0x08
    pushq %rax
    lretq
1:  mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    mov %ax, %fs
    mov %ax, %gs
    movabs $__stack_top, %rsp
    movabs $__bss_start, %rdi
    movabs $__bss_end, %rcx
    sub %rdi, %rcx
    shr $3, %rcx
    xor %eax, %eax
    rep stosq
    mov %cr0, %rax                      # FPU/SSE attivi anche se il firmware non l'ha fatto
    and $~0x04, %rax
    or $0x02, %rax
    mov %rax, %cr0
    mov %cr4, %rax
    or $0x600, %rax
    mov %rax, %cr4
    fninit
    mov %r12, %rdi
    movabs $kmain, %rax
    call *%rax
2:  cli
    hlt
    jmp 2b

.section .rodata.kgdt, "a"
.balign 16
kgdt:
    .quad 0
    .quad 0x00AF9A000000FFFF
    .quad 0x00CF92000000FFFF
kgdt_desc:
    .word kgdt_desc - kgdt - 1
    .quad kgdt

# ============================================================================
#  Stub delle eccezioni CPU (chiamano exception_handler in Rust)
# ============================================================================
.section .text.isr, "ax"
.code64
.irp n, 0,1,2,3,4,5,6,7,9,15,16,18,19,20,22,23,24,25,26,27,28,31
isr\n:
    push $0
    push $\n
    jmp isr_common
.endr
.irp n, 8,10,11,12,13,14,17,21,29,30
isr\n:
    push $\n
    jmp isr_common
.endr
isr_common:
    mov %rsp, %rdi
    and $-16, %rsp
    call exception_handler
2:  cli
    hlt
    jmp 2b

.section .rodata.isr, "a"
.balign 8
.global isr_table
isr_table:
.irp n, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    .quad isr\n
.endr
