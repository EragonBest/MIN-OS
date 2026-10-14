//! Strumento di build di MIN-OS.
//!
//!   cargo run -- build        compila tutto e crea target/min-os.img
//!   cargo run -- run          compila e avvia MIN-OS in QEMU (avvio BIOS)
//!   cargo run -- run --uefi   compila e avvia MIN-OS in QEMU (avvio UEFI)
//!   cargo run -- vdi          crea anche target/min-os.vdi per VirtualBox
//!   cargo run -- keygen       crea la chiave che firma gli aggiornamenti (una volta sola)
//!   cargo run -- release      compila e prepara in target/dist l'aggiornamento firmato
//!                             (update.bin + latest.txt) e l'immagine: lo usa GitHub Actions
//!
//! Variabili: MINOS_BUILD (numero di build, lo mette GitHub), MINOS_REPO (utente/repo GitHub),
//! MINOS_SIGN_KEY (chiave privata PEM; in alternativa il file chiave-aggiornamenti.pem).
//!
//! Opzioni: --debug (build non ottimizzata), --no-accel (disattiva KVM/WHPX), --no-net.
//!
//! L'immagine e' "ibrida": lo stesso file si avvia sia da BIOS sia da UEFI.
//!   LBA 0          boot sector BIOS + tabella delle partizioni
//!   LBA 1..16      stage 2 del bootloader BIOS
//!   LBA 17..       kernel
//!   LBA 4000       configurazione salvata da MIN-OS
//!   LBA 4096..     partizione EFI (FAT16) con \EFI\BOOT\BOOTX64.EFI

use std::env;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

const TARGET: &str = "x86_64-unknown-linux-gnu";
const BOOT_ADDR: u64 = 0x7C00;
const BOOT_SECTORS: usize = 17;
const KERNEL_VMA: u64 = 0x100_0000;
const CONFIG_LBA: usize = 4000;
const ESP_LBA: usize = 4096;
const ESP_SIZE: usize = 16 * 1024 * 1024;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("run");
    let release = !args.iter().any(|a| a == "--debug");
    let root = project_root();

    match cmd {
        "build" => {
            build(&root, release);
        }
        "run" => {
            let img = build(&root, release);
            run_qemu(&root, &img, &args);
        }
        "vdi" => {
            let img = build(&root, release);
            make_vdi(&img);
        }
        "keygen" => keygen(&root),
        "release" => {
            build(&root, true);
            package(&root);
        }
        _ => {
            eprintln!("Uso: cargo run -- [build|run|vdi|keygen|release] [--uefi] [--debug] [--no-accel] [--no-net]");
            exit(1);
        }
    }
}

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn cargo_build(root: &Path, package: &str, release: bool, flags: &[String], envs: &[(&str, String)]) {
    let mut cmd = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(root)
        .args(["build", "--package", package, "--target", TARGET])
        // flag separati da 0x1F: funziona anche con percorsi che contengono spazi
        .env("CARGO_ENCODED_RUSTFLAGS", flags.join("\x1f"))
        .env_remove("RUSTFLAGS");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    if release {
        cmd.arg("--release");
    }
    if !cmd.status().map(|s| s.success()).unwrap_or(false) {
        eprintln!("Compilazione di `{package}` fallita.");
        exit(1);
    }
}

fn common_flags() -> Vec<String> {
    ["-Clinker=rust-lld", "-Clinker-flavor=ld.lld", "-Cno-redzone=yes", "-Ctarget-cpu=x86-64", "-Clink-arg=-nostdlib", "-Clink-arg=--build-id=none"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn build(root: &Path, release: bool) -> PathBuf {
    let profile = if release { "release" } else { "debug" };
    let out_dir = root.join("target").join(TARGET).join(profile);

    // ---- 1. kernel
    println!("==> Compilo il kernel di MIN-OS...");
    let mut kflags = common_flags();
    kflags.extend([
        "-Crelocation-model=static".to_string(),
        "-Ccode-model=small".to_string(),
        "-Clink-arg=-static".to_string(),
        "-Clink-arg=--no-pie".to_string(),
        format!("-Clink-arg=-T{}", root.join("kernel").join("linker.ld").display()),
    ]);
    let (build_no, version, repo) = version_info();
    println!("    versione {version} (build {build_no}), aggiornamenti da github.com/{repo}");
    cargo_build(
        root,
        "kernel",
        release,
        &kflags,
        &[
            ("MINOS_BUILD", build_no.to_string()),
            ("MINOS_VERSION", version),
            ("MINOS_REPO", repo),
            ("MINOS_UPDATE_URL", env::var("MINOS_UPDATE_URL").unwrap_or_default()),
        ],
    );
    let elf = fs::read(out_dir.join("min-os")).expect("kernel ELF non trovato");
    let (flat, span) = flatten_elf(&elf);
    let kernel_blob = &flat[BOOT_SECTORS * 512..];
    let kernel_path = root.join("target").join("kernel.bin");
    fs::write(&kernel_path, kernel_blob).unwrap();
    assert!(flat.len() <= CONFIG_LBA * 512, "il kernel e' troppo grande per lo spazio riservato");
    // area di avvio BIOS (MBR + stage 2 + kernel): e' quella che gli aggiornamenti riscrivono
    fs::write(root.join("target").join("boot-area.bin"), &flat).unwrap();

    // ---- 2. bootloader UEFI (con il kernel incorporato)
    println!("==> Compilo il bootloader UEFI...");
    let mut uflags = common_flags();
    uflags.extend([
        "-Crelocation-model=pic".to_string(),
        "-Clink-arg=-pie".to_string(),
        "-Clink-arg=--no-dynamic-linker".to_string(),
        "-Clink-arg=-znotext".to_string(),
        format!("-Clink-arg=-T{}", root.join("uefi").join("linker.ld").display()),
    ]);
    cargo_build(
        root,
        "uefi-loader",
        release,
        &uflags,
        &[("MINOS_KERNEL_BLOB", kernel_path.display().to_string()), ("MINOS_KERNEL_SPAN", span.to_string())],
    );
    let uelf = fs::read(out_dir.join("bootx64")).expect("loader UEFI non trovato");
    let efi = elf_to_efi(&uelf);
    fs::write(root.join("target").join("BOOTX64.EFI"), &efi).unwrap();

    // ---- 3. immagine del disco
    let mut image = vec![0u8; ESP_LBA * 512 + ESP_SIZE];
    image[..flat.len()].copy_from_slice(&flat);
    image[440..444].copy_from_slice(b"MNOS"); // firma del disco: MIN-OS la usa per ritrovarsi
    let esp = make_esp(&efi, root);
    image[ESP_LBA * 512..].copy_from_slice(&esp);
    let e = &mut image[446..462];
    e[0] = 0x80; // avviabile
    e[1..4].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    e[4] = 0xEF; // partizione di sistema EFI
    e[5..8].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    e[8..12].copy_from_slice(&(ESP_LBA as u32).to_le_bytes());
    e[12..16].copy_from_slice(&((ESP_SIZE / 512) as u32).to_le_bytes());

    let out = root.join("target").join("min-os.img");
    fs::write(&out, &image).expect("impossibile scrivere l'immagine");
    println!(
        "==> Immagine pronta: {} (kernel {} KiB, loader UEFI {} KiB, avviabile da BIOS e UEFI)",
        out.display(),
        kernel_blob.len() / 1024,
        efi.len() / 1024
    );
    out
}

// ------------------------------------------------------------------ ELF

fn u16_at(b: &[u8], o: usize) -> usize {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap()) as usize
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Converte l'ELF del kernel in un'immagine piatta usando gli indirizzi fisici (LMA).
/// Ritorna anche lo spazio totale occupato in memoria dal kernel (bss compreso).
fn flatten_elf(elf: &[u8]) -> (Vec<u8>, u64) {
    assert!(&elf[0..4] == b"\x7FELF" && elf[4] == 2, "il kernel non e' un ELF64");
    let (phoff, phentsize, phnum) = (u64_at(elf, 0x20) as usize, u16_at(elf, 0x36), u16_at(elf, 0x38));
    let mut image = Vec::new();
    let mut span = 0u64;
    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        if u32_at(elf, ph) != 1 {
            continue;
        }
        let offset = u64_at(elf, ph + 0x08) as usize;
        let vaddr = u64_at(elf, ph + 0x10);
        let paddr = u64_at(elf, ph + 0x18);
        let filesz = u64_at(elf, ph + 0x20) as usize;
        let memsz = u64_at(elf, ph + 0x28);
        if vaddr >= KERNEL_VMA {
            span = span.max(vaddr + memsz - KERNEL_VMA);
        }
        if filesz == 0 || paddr < BOOT_ADDR {
            continue;
        }
        let at = (paddr - BOOT_ADDR) as usize;
        if image.len() < at + filesz {
            image.resize(at + filesz, 0);
        }
        image[at..at + filesz].copy_from_slice(&elf[offset..offset + filesz]);
    }
    assert!(image.len() > 512 && image[510] == 0x55 && image[511] == 0xAA, "boot sector non valido");
    image.resize(image.len().div_ceil(512) * 512, 0);
    (image, span)
}

/// Converte il loader (ELF "static-pie") in un eseguibile UEFI PE32+ con rilocazioni.
fn elf_to_efi(elf: &[u8]) -> Vec<u8> {
    const IMAGE_BASE: u64 = 0x1_4000_0000;
    let (phoff, phentsize, phnum) = (u64_at(elf, 0x20) as usize, u16_at(elf, 0x36), u16_at(elf, 0x38));
    let entry = u64_at(elf, 0x18);

    // immagine in memoria (indirizzi = RVA, il loader e' collegato a partire da 0)
    let mut mem: Vec<u8> = Vec::new();
    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        if u32_at(elf, ph) != 1 {
            continue;
        }
        let offset = u64_at(elf, ph + 0x08) as usize;
        let vaddr = u64_at(elf, ph + 0x10) as usize;
        let filesz = u64_at(elf, ph + 0x20) as usize;
        let memsz = u64_at(elf, ph + 0x28) as usize;
        if mem.len() < vaddr + memsz {
            mem.resize(vaddr + memsz, 0);
        }
        mem[vaddr..vaddr + filesz].copy_from_slice(&elf[offset..offset + filesz]);
    }
    let start = 0x1000usize;
    let end = mem.len().div_ceil(0x1000) * 0x1000;
    mem.resize(end, 0);

    // rilocazioni R_X86_64_RELATIVE -> rilocazioni PE DIR64
    let (shoff, shentsize, shnum) = (u64_at(elf, 0x28) as usize, u16_at(elf, 0x3A), u16_at(elf, 0x3C));
    let mut relocs: Vec<u32> = Vec::new();
    for i in 0..shnum {
        let sh = shoff + i * shentsize;
        if u32_at(elf, sh + 4) != 4 {
            continue; // solo SHT_RELA
        }
        let off = u64_at(elf, sh + 0x18) as usize;
        let size = u64_at(elf, sh + 0x20) as usize;
        for r in (off..off + size).step_by(24) {
            let at = u64_at(elf, r) as usize;
            let typ = u64_at(elf, r + 8) & 0xFFFF_FFFF;
            let addend = u64_at(elf, r + 16);
            match typ {
                0 => {}
                8 => {
                    mem[at..at + 8].copy_from_slice(&(IMAGE_BASE + addend).to_le_bytes());
                    relocs.push(at as u32);
                }
                t => panic!("rilocazione non supportata nel loader UEFI: tipo {t}"),
            }
        }
    }
    relocs.sort();
    let mut reloc_data: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < relocs.len() {
        let page = relocs[i] & !0xFFF;
        let mut entries: Vec<u16> = Vec::new();
        while i < relocs.len() && relocs[i] & !0xFFF == page {
            entries.push((10 << 12) | (relocs[i] & 0xFFF) as u16);
            i += 1;
        }
        if entries.len() % 2 == 1 {
            entries.push(0);
        }
        reloc_data.extend_from_slice(&page.to_le_bytes());
        reloc_data.extend_from_slice(&((8 + entries.len() * 2) as u32).to_le_bytes());
        for e in entries {
            reloc_data.extend_from_slice(&e.to_le_bytes());
        }
    }
    if reloc_data.is_empty() {
        // un blocco vuoto, cosi' il firmware sa che l'immagine e' rilocabile
        reloc_data.extend_from_slice(&0u32.to_le_bytes());
        reloc_data.extend_from_slice(&8u32.to_le_bytes());
    }

    let text = &mem[start..end];
    let reloc_rva = end as u32;
    let reloc_vsize = reloc_data.len() as u32;
    let file_align = 0x200usize;
    let headers = 0x400usize;
    let text_raw = text.len().div_ceil(file_align) * file_align;
    let reloc_raw = reloc_data.len().div_ceil(file_align) * file_align;
    let image_size = (end + reloc_data.len()).div_ceil(0x1000) * 0x1000;

    let mut pe = vec![0u8; headers];
    pe[0..2].copy_from_slice(b"MZ");
    pe[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
    let mut h: Vec<u8> = Vec::new();
    h.extend_from_slice(b"PE\0\0");
    // intestazione COFF
    h.extend_from_slice(&0x8664u16.to_le_bytes()); // x86_64
    h.extend_from_slice(&2u16.to_le_bytes()); // 2 sezioni
    h.extend_from_slice(&[0; 12]);
    h.extend_from_slice(&240u16.to_le_bytes()); // dimensione optional header
    h.extend_from_slice(&0x0022u16.to_le_bytes()); // eseguibile, indirizzi grandi
    // optional header PE32+
    h.extend_from_slice(&0x20Bu16.to_le_bytes());
    h.extend_from_slice(&[14, 0]);
    h.extend_from_slice(&(text_raw as u32).to_le_bytes());
    h.extend_from_slice(&(reloc_raw as u32).to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes());
    h.extend_from_slice(&(entry as u32).to_le_bytes());
    h.extend_from_slice(&(start as u32).to_le_bytes());
    h.extend_from_slice(&IMAGE_BASE.to_le_bytes());
    h.extend_from_slice(&0x1000u32.to_le_bytes());
    h.extend_from_slice(&(file_align as u32).to_le_bytes());
    h.extend_from_slice(&[0; 16]); // versioni + Win32VersionValue
    h.extend_from_slice(&(image_size as u32).to_le_bytes());
    h.extend_from_slice(&(headers as u32).to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes()); // checksum
    h.extend_from_slice(&10u16.to_le_bytes()); // subsystem: applicazione EFI
    h.extend_from_slice(&0u16.to_le_bytes());
    for v in [0x10_0000u64, 0x1000, 0x10_0000, 0x1000] {
        h.extend_from_slice(&v.to_le_bytes());
    }
    h.extend_from_slice(&0u32.to_le_bytes());
    h.extend_from_slice(&16u32.to_le_bytes());
    for d in 0..16 {
        if d == 5 {
            h.extend_from_slice(&reloc_rva.to_le_bytes());
            h.extend_from_slice(&reloc_vsize.to_le_bytes());
        } else {
            h.extend_from_slice(&[0; 8]);
        }
    }
    let mut section = |name: &[u8], vsize: u32, rva: u32, raw: u32, ptr: u32, flags: u32| {
        let mut n = [0u8; 8];
        n[..name.len()].copy_from_slice(name);
        h.extend_from_slice(&n);
        for v in [vsize, rva, raw, ptr, 0, 0] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        h.extend_from_slice(&[0; 4]);
        h.extend_from_slice(&flags.to_le_bytes());
    };
    section(b".text", text.len() as u32, start as u32, text_raw as u32, headers as u32, 0xE000_0060);
    section(b".reloc", reloc_vsize, reloc_rva, reloc_raw as u32, (headers + text_raw) as u32, 0x4200_0040);
    pe[0x40..0x40 + h.len()].copy_from_slice(&h);
    pe.extend_from_slice(text);
    pe.resize(headers + text_raw, 0);
    pe.extend_from_slice(&reloc_data);
    pe.resize(headers + text_raw + reloc_raw, 0);
    pe
}

/// Partizione di sistema EFI (FAT16) con il loader.
fn make_esp(efi: &[u8], root: &Path) -> Vec<u8> {
    let mut disk = Cursor::new(vec![0u8; ESP_SIZE]);
    fatfs::format_volume(&mut disk, fatfs::FormatVolumeOptions::new().fat_type(fatfs::FatType::Fat16).volume_label(*b"MIN-OS     "))
        .expect("formattazione FAT fallita");
    {
        let fs = fatfs::FileSystem::new(&mut disk, fatfs::FsOptions::new()).expect("FAT non valida");
        let rootdir = fs.root_dir();
        let boot = rootdir.create_dir("EFI").unwrap().create_dir("BOOT").unwrap();
        boot.create_file("BOOTX64.EFI").unwrap().write_all(efi).unwrap();
        if let Ok(readme) = fs::read(root.join("README.md")) {
            rootdir.create_file("LEGGIMI.md").unwrap().write_all(&readme).unwrap();
        }
    }
    let mut esp = disk.into_inner();
    fix_dot_entries(&mut esp);
    esp
}

/// La libreria `fatfs` scrive dei "nomi lunghi" davanti alle voci "." e ".." delle cartelle:
/// alcuni firmware e `fsck` li considerano errori. Qui le cartelle vengono compattate senza.
fn fix_dot_entries(img: &mut [u8]) {
    let u16le = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as usize;
    let spc = img[13] as usize;
    let reserved = u16le(img, 14);
    let nfats = img[16] as usize;
    let root_entries = u16le(img, 17);
    let fat_sz = u16le(img, 22);
    let fat = reserved * 512;
    let root = (reserved + nfats * fat_sz) * 512;
    let data = root + root_entries * 32;
    let csize = spc * 512;
    let mut stack: Vec<usize> = Vec::new();
    // cartelle nella radice
    for i in 0..root_entries {
        let e = &img[root + i * 32..root + i * 32 + 32];
        if e[0] == 0 {
            break;
        }
        if e[0] != 0xE5 && e[11] & 0x10 != 0 && e[11] != 0x0F && e[0] != b'.' {
            stack.push(u16le(e, 26));
        }
    }
    while let Some(first) = stack.pop() {
        let mut chain = Vec::new();
        let mut c = first;
        while (2..0xFFF8).contains(&c) && chain.len() < 4096 {
            chain.push(c);
            c = u16le(img, fat + c * 2);
        }
        let slots: Vec<usize> = chain.iter().flat_map(|&c| (0..csize / 32).map(move |i| data + (c - 2) * csize + i * 32)).collect();
        let mut entries: Vec<[u8; 32]> = Vec::new();
        let mut lfn: Vec<[u8; 32]> = Vec::new();
        for &o in slots.iter() {
            let e: [u8; 32] = img[o..o + 32].try_into().unwrap();
            if e[0] == 0 {
                break;
            }
            if e[0] == 0xE5 {
                lfn.clear();
                continue;
            }
            if e[11] == 0x0F {
                lfn.push(e);
                continue;
            }
            let dot = &e[..11] == b".          " || &e[..11] == b"..         ";
            if !dot {
                entries.append(&mut lfn);
                if e[11] & 0x10 != 0 {
                    stack.push(u16le(&e, 26));
                }
            }
            lfn.clear();
            entries.push(e);
        }
        for (i, &o) in slots.iter().enumerate() {
            let e = entries.get(i).copied().unwrap_or([0; 32]);
            img[o..o + 32].copy_from_slice(&e);
        }
    }
}

// ------------------------------------------------------------------ QEMU

fn find_qemu() -> String {
    let name = if cfg!(windows) { "qemu-system-x86_64.exe" } else { "qemu-system-x86_64" };
    if let Ok(dir) = env::var("QEMU_DIR") {
        return Path::new(&dir).join(name).display().to_string();
    }
    if cfg!(windows) {
        for p in [r"C:\Program Files\qemu", r"C:\Program Files (x86)\qemu"] {
            let full = Path::new(p).join(name);
            if full.exists() {
                return full.display().to_string();
            }
        }
    }
    name.to_string()
}

/// Firmware UEFI (OVMF) per QEMU: ritorna (codice, modello delle variabili).
/// QEMU per Windows lo include gia' nella cartella `share`.
fn find_ovmf(qemu: &str) -> Option<(PathBuf, Option<PathBuf>)> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = Path::new(qemu).parent() {
        dirs.push(dir.join("share"));
    }
    for d in [r"C:\Program Files\qemu\share", "/usr/share/qemu", "/usr/share/OVMF", "/usr/share/ovmf", "/opt/homebrew/share/qemu", "/usr/local/share/qemu"] {
        dirs.push(PathBuf::from(d));
    }
    let pairs = [
        ("edk2-x86_64-code.fd", "edk2-i386-vars.fd"),
        ("OVMF_CODE_4M.fd", "OVMF_VARS_4M.fd"),
        ("OVMF_CODE.fd", "OVMF_VARS.fd"),
        ("OVMF.fd", ""),
    ];
    for d in dirs {
        for (code, vars) in pairs {
            let c = d.join(code);
            if c.exists() {
                let v = d.join(vars);
                return Some((c, if !vars.is_empty() && v.exists() { Some(v) } else { None }));
            }
        }
    }
    None
}

fn run_qemu(root: &Path, img: &Path, args: &[String]) {
    let qemu = find_qemu();
    let mut cmd = Command::new(&qemu);
    cmd.arg("-drive")
        .arg(format!("format=raw,file={}", img.display()))
        .args(["-m", "512M", "-smp", "4", "-rtc", "base=localtime", "-vga", "std", "-serial", "stdio"])
        .args(["-name", "MIN-OS"]);
    if !args.iter().any(|a| a == "--no-net") {
        cmd.args(["-nic", "user,model=e1000"]);
    }
    if args.iter().any(|a| a == "--uefi") {
        match find_ovmf(&qemu) {
            Some((code, vars)) => {
                cmd.arg("-drive").arg(format!("if=pflash,format=raw,readonly=on,file={}", code.display()));
                if let Some(v) = vars {
                    let copy = root.join("target").join("uefi-vars.fd");
                    if !copy.exists() {
                        let _ = fs::copy(&v, &copy);
                    }
                    cmd.arg("-drive").arg(format!("if=pflash,format=raw,file={}", copy.display()));
                }
            }
            None => eprintln!("Firmware UEFI per QEMU non trovato: avvio in modalita' BIOS."),
        }
    }
    if !args.iter().any(|a| a == "--no-accel") {
        if cfg!(windows) {
            cmd.args(["-accel", "whpx,kernel-irqchip=off", "-accel", "tcg"]);
        } else if cfg!(target_os = "macos") {
            cmd.args(["-accel", "hvf", "-accel", "tcg"]);
        } else {
            cmd.args(["-accel", "kvm", "-accel", "tcg"]);
        }
    }
    if let Some(pos) = args.iter().position(|a| a == "--") {
        cmd.args(&args[pos + 1..]);
    }
    println!("==> Avvio QEMU (clicca nella finestra per catturare il mouse, Ctrl+Alt+G per rilasciarlo)");
    if let Err(e) = cmd.status() {
        eprintln!("Impossibile avviare QEMU ({qemu}): {e}");
        eprintln!("Installa QEMU da https://www.qemu.org/download/ oppure imposta QEMU_DIR.");
        exit(1);
    }
}

fn make_vdi(img: &Path) {
    let vdi = img.with_extension("vdi");
    let _ = fs::remove_file(&vdi);
    let vbox = if cfg!(windows) {
        let p = Path::new(r"C:\Program Files\Oracle\VirtualBox\VBoxManage.exe");
        if p.exists() {
            p.display().to_string()
        } else {
            "VBoxManage".into()
        }
    } else {
        "VBoxManage".into()
    };
    let ok = Command::new(&vbox)
        .args(["convertfromraw", &img.display().to_string(), &vdi.display().to_string(), "--format", "VDI"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        println!("==> Disco VirtualBox pronto: {}", vdi.display());
    } else {
        eprintln!("VBoxManage non trovato: installa VirtualBox oppure usa direttamente min-os.img con QEMU.");
        exit(1);
    }
}

// ------------------------------------------------------------------ aggiornamenti

/// Numero di build, versione e repository GitHub da cui MIN-OS scarica gli aggiornamenti.
fn version_info() -> (u32, String, String) {
    let build_no: u32 = env::var("MINOS_BUILD").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    let version = format!("0.5.{build_no}");
    let repo = env::var("MINOS_REPO")
        .or_else(|_| env::var("GITHUB_REPOSITORY"))
        .unwrap_or_else(|_| "EragonBest/MIN-OS".to_string());
    (build_no, version, repo)
}

const KEY_FILE: &str = "chiave-aggiornamenti.pem";
const PUBKEY_FILE: &str = "kernel/src/update_key.bin";

/// Crea la coppia di chiavi: la privata resta a te (e nei Secrets di GitHub),
/// la pubblica finisce dentro il kernel.
fn keygen(root: &Path) {
    use p256::ecdsa::SigningKey;
    use p256::pkcs8::{EncodePrivateKey, LineEnding};
    let priv_path = root.join(KEY_FILE);
    if priv_path.exists() {
        eprintln!("{} esiste gia': non la sovrascrivo (perderesti la possibilita' di aggiornare).", priv_path.display());
        exit(1);
    }
    let key = SigningKey::random(&mut rand_core::OsRng);
    let pem = key.to_pkcs8_pem(LineEnding::LF).unwrap();
    fs::write(&priv_path, pem.as_bytes()).unwrap();
    let public = key.verifying_key().to_encoded_point(false);
    fs::write(root.join(PUBKEY_FILE), public.as_bytes()).unwrap();
    println!("==> Chiave privata: {} (NON caricarla su GitHub: va nei Secrets come MINOS_SIGN_KEY)", priv_path.display());
    println!("==> Chiave pubblica: {PUBKEY_FILE} (questa si', va nel repository)");
}

fn load_key(root: &Path) -> p256::ecdsa::SigningKey {
    use p256::pkcs8::DecodePrivateKey;
    let pem = match env::var("MINOS_SIGN_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => fs::read_to_string(root.join(KEY_FILE)).unwrap_or_else(|_| {
            eprintln!("Manca la chiave per firmare: imposta MINOS_SIGN_KEY o crea {KEY_FILE} con `cargo run -- keygen`.");
            exit(1)
        }),
    };
    let key = p256::ecdsa::SigningKey::from_pkcs8_pem(pem.trim()).unwrap_or_else(|e| {
        eprintln!("Chiave di firma non valida: {e}");
        exit(1)
    });
    // la chiave deve corrispondere a quella pubblica dentro il kernel
    let public = key.verifying_key().to_encoded_point(false);
    let inside = fs::read(root.join(PUBKEY_FILE)).unwrap_or_default();
    if public.as_bytes() != &inside[..] {
        eprintln!("La chiave privata non corrisponde a {PUBKEY_FILE}: MIN-OS rifiuterebbe l'aggiornamento.");
        exit(1);
    }
    key
}

/// Pacchetto di aggiornamento (update.bin):
///   0   "MNUP0001"
///   8   numero di build (u32)      12  byte dell'area di avvio BIOS (u32)
///   16  byte di BOOTX64.EFI (u32)  32  versione (testo, 32 byte)
///   64  SHA-256 dei dati            176 lunghezza firma (u16) + firma ECDSA P-256 (DER)
///   256 dati: area di avvio BIOS, poi BOOTX64.EFI
/// La firma copre i primi 176 byte (e quindi, tramite lo SHA-256, anche i dati).
fn package(root: &Path) {
    use p256::ecdsa::{signature::Signer, DerSignature};
    use sha2::{Digest, Sha256};
    let key = load_key(root);
    let (build_no, version, repo) = version_info();
    let t = root.join("target");
    let boot = fs::read(t.join("boot-area.bin")).unwrap();
    let efi = fs::read(t.join("BOOTX64.EFI")).unwrap();
    let mut payload = boot.clone();
    payload.extend_from_slice(&efi);

    let mut head = vec![0u8; 256];
    head[0..8].copy_from_slice(b"MNUP0001");
    head[8..12].copy_from_slice(&build_no.to_le_bytes());
    head[12..16].copy_from_slice(&(boot.len() as u32).to_le_bytes());
    head[16..20].copy_from_slice(&(efi.len() as u32).to_le_bytes());
    let vb = version.as_bytes();
    head[32..32 + vb.len().min(32)].copy_from_slice(&vb[..vb.len().min(32)]);
    head[64..96].copy_from_slice(&Sha256::digest(&payload));
    let sig: DerSignature = key.sign(&head[..176]);
    let sb = sig.as_bytes();
    head[176..178].copy_from_slice(&(sb.len() as u16).to_le_bytes());
    head[178..178 + sb.len()].copy_from_slice(sb);

    let dist = t.join("dist");
    fs::create_dir_all(&dist).unwrap();
    let mut upd = head;
    upd.extend_from_slice(&payload);
    fs::write(dist.join("update.bin"), &upd).unwrap();
    // note: la prima riga del messaggio del commit (le mostra MIN-OS)
    let notes: String = env::var("MINOS_NOTES").unwrap_or_default().lines().next().unwrap_or("").chars().take(120).collect();
    fs::write(dist.join("latest.txt"), format!("build={build_no}\nversion={version}\nsize={}\nnotes={}\n", upd.len(), notes.trim())).unwrap();
    fs::copy(t.join("min-os.img"), dist.join("min-os.img")).unwrap();
    println!("==> Aggiornamento firmato: {} ({} KiB) per github.com/{repo}", dist.join("update.bin").display(), upd.len() / 1024);
}
