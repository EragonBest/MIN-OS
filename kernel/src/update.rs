//! Aggiornamenti di MIN-OS da GitHub (come Windows Update).
//!
//! A ogni push, GitHub Actions compila MIN-OS e pubblica una release con:
//!   latest.txt   "build=N / version=0.5.N / size=..."
//!   update.bin   area di avvio BIOS + BOOTX64.EFI, firmati con ECDSA P-256
//! MIN-OS controlla latest.txt, scarica update.bin, verifica la firma con la chiave
//! pubblica incorporata (update_key.bin) e riscrive il proprio disco.
//! Prima salva la versione attuale in /System per poterla ripristinare.

use crate::fat::{self, Dir};
use crate::i18n_desk::Dk;
use crate::{block, net};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

const OWNER: u8 = 2;
const PUBKEY: &[u8] = include_bytes!("update_key.bin");
const CONFIG_LBA: u64 = 4000;
const SYS_DIR: &str = "System";
const BK_BOOT: &str = "prev-boot.bin";
const BK_EFI: &str = "prev-efi.bin";
const BK_VER: &str = "prev-version.txt";

#[derive(Clone, PartialEq)]
pub enum St {
    Idle,
    Checking,
    UpToDate,
    Available { build: u32, version: String, size: u32, notes: String },
    Downloading { version: String, size: u32 },
    Installing,
    /// installato: serve un riavvio
    Done(String),
    /// versione precedente rimessa: serve un riavvio
    Restored(String),
    Error(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Want {
    None,
    Latest,
    Package,
}

pub struct Updater {
    pub st: St,
    want: Want,
    redirects: u8,
    auto: bool,
    /// true se il controllo l'ha fatto MIN-OS da solo (per la notifica)
    silent: bool,
    /// notifica da mostrare sul desktop (versione disponibile)
    pub notify: Option<String>,
    pub last_check: Option<crate::arch::DateTime>,
    /// installazione richiesta: si esegue al frame dopo (cosi' lo schermo mostra "Installazione...")
    pending: Option<Vec<u8>>,
    /// versione salvata per il ripristino (letta dal disco una volta sola)
    backup: Option<Option<String>>,
}

pub fn base_url() -> String {
    // MINOS_UPDATE_URL serve solo per le prove con un server locale
    match option_env!("MINOS_UPDATE_URL") {
        Some(u) if !u.is_empty() => String::from(u),
        _ => format!("https://github.com/{}/releases/latest/download/", crate::REPO),
    }
}

impl Updater {
    pub fn new() -> Updater {
        Updater { st: St::Idle, want: Want::None, redirects: 0, auto: false, silent: false, notify: None, last_check: None, pending: None, backup: None }
    }

    pub fn busy(&self) -> bool {
        self.want != Want::None || self.pending.is_some()
    }

    /// Versione precedente salvata (se c'e'), con cache.
    pub fn backup_version(&mut self) -> Option<String> {
        if self.backup.is_none() {
            self.backup = Some(self.has_backup());
        }
        self.backup.clone().flatten()
    }

    /// C'e' una versione precedente da ripristinare?
    pub fn has_backup(&self) -> Option<String> {
        let d = fat::find(0, SYS_DIR).filter(|e| e.dir)?;
        fat::find(d.cluster, BK_EFI)?;
        fat::find(d.cluster, BK_BOOT)?;
        let v = fat::find(d.cluster, BK_VER).map(|e| String::from_utf8_lossy(&fat::read(&e)).trim().into()).unwrap_or_default();
        Some(v)
    }

    pub fn check(&mut self, dk: &'static Dk, silent: bool) {
        if self.busy() {
            return;
        }
        self.silent = silent;
        self.last_check = Some(crate::arch::rtc_now());
        self.get(format!("{}latest.txt", base_url()), Want::Latest, dk);
        if self.want == Want::Latest {
            self.st = St::Checking;
        }
    }

    pub fn install(&mut self, dk: &'static Dk) {
        if let St::Available { version, size, .. } = self.st.clone() {
            self.silent = false;
            self.get(format!("{}update.bin", base_url()), Want::Package, dk);
            if self.want == Want::Package {
                self.st = St::Downloading { version, size };
            }
        }
    }

    fn get(&mut self, url: String, want: Want, dk: &'static Dk) {
        self.redirects = 0;
        match net::fetch(&url, dk, OWNER) {
            Ok(()) => self.want = want,
            Err(e) => self.fail(e),
        }
    }

    fn fail(&mut self, e: String) {
        crate::log!("aggiornamenti: {}", e);
        self.want = Want::None;
        // un controllo automatico fallito (es. niente rete) non disturba
        self.st = if self.silent { St::Idle } else { St::Error(e) };
    }

    /// Byte scaricati del pacchetto (per la barra di avanzamento).
    pub fn progress(&self) -> usize {
        if self.want == Want::Package {
            net::fetch_progress(OWNER).unwrap_or(0)
        } else {
            0
        }
    }

    /// Da chiamare a ogni giro del desktop. Ritorna true se lo stato e' cambiato.
    pub fn tick(&mut self, dk: &'static Dk, now: u64, boot_t: u64) -> bool {
        // controllo automatico una volta, poco dopo l'avvio
        if !self.auto && now > boot_t + 6000 && net::info().ip.is_some() {
            self.auto = true;
            self.check(dk, true);
            return true;
        }
        if let Some(data) = self.pending.take() {
            self.backup = None;
            self.st = match apply(&data, dk) {
                Ok(v) => St::Done(v),
                Err(e) => {
                    crate::log!("aggiornamento rifiutato: {}", e);
                    St::Error(e)
                }
            };
            return true;
        }
        if self.want == Want::None {
            return false;
        }
        if net::fetch_owner() != OWNER {
            self.fail(String::from(dk.upd_interrupted));
            return true;
        }
        let Some(res) = net::fetch_take(OWNER) else { return self.want == Want::Package };
        let want = self.want;
        self.want = Want::None;
        let r = match res {
            Ok(r) => r,
            Err((e, _)) => {
                self.fail(e);
                return true;
            }
        };
        // GitHub risponde con un rimando (302) al file vero
        if (300..400).contains(&r.status) && !r.location.is_empty() && self.redirects < 8 {
            let n = self.redirects + 1;
            match net::fetch(&r.location, dk, OWNER) {
                Ok(()) => {
                    self.want = want;
                    self.redirects = n;
                }
                Err(e) => self.fail(e),
            }
            return true;
        }
        if r.status == 404 {
            self.fail(String::from(dk.upd_none_published));
            return true;
        }
        if r.status != 200 {
            self.fail(format!("{} (HTTP {})", dk.upd_failed, r.status));
            return true;
        }
        match want {
            Want::Latest => {
                let text = String::from_utf8_lossy(&r.body);
                let field = |k: &str| text.lines().find_map(|l| l.trim().strip_prefix(k).map(|v| String::from(v.trim())));
                let build = field("build=").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
                let version = field("version=").unwrap_or_default();
                let size = field("size=").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
                let notes = field("notes=").unwrap_or_default();
                if build == 0 {
                    self.fail(String::from(dk.upd_failed));
                } else if build > crate::BUILD {
                    if self.silent {
                        self.notify = Some(version.clone());
                    }
                    self.st = St::Available { build, version, size, notes };
                } else {
                    self.st = St::UpToDate;
                }
            }
            Want::Package => {
                self.st = St::Installing;
                self.pending = Some(r.body);
            }
            Want::None => {}
        }
        true
    }

    /// Rimette la versione salvata prima dell'ultimo aggiornamento.
    pub fn restore(&mut self, dk: &'static Dk) {
        self.backup = None;
        self.st = match restore_backup(dk) {
            Ok(v) => St::Restored(v),
            Err(e) => St::Error(e),
        };
    }
}

// ------------------------------------------------------------------ verifica e installazione

struct Package<'a> {
    build: u32,
    version: String,
    boot: &'a [u8],
    efi: &'a [u8],
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Controlla firma e integrita' del pacchetto.
fn verify<'a>(d: &'a [u8], dk: &Dk) -> Result<Package<'a>, String> {
    use p256::ecdsa::signature::Verifier;
    use p256::ecdsa::{DerSignature, VerifyingKey};
    use sha2::{Digest, Sha256};
    let bad = || String::from(dk.upd_bad_sig);
    if d.len() < 256 || &d[..8] != b"MNUP0001" {
        return Err(bad());
    }
    let (boot_len, efi_len) = (le32(d, 12) as usize, le32(d, 16) as usize);
    if d.len() != 256 + boot_len + efi_len || boot_len < 1024 || boot_len > CONFIG_LBA as usize * 512 || efi_len < 1024 {
        return Err(bad());
    }
    let sig_len = u16::from_le_bytes([d[176], d[177]]) as usize;
    if sig_len == 0 || sig_len > 78 {
        return Err(bad());
    }
    let key = VerifyingKey::from_sec1_bytes(PUBKEY).map_err(|_| bad())?;
    let sig = DerSignature::try_from(&d[178..178 + sig_len]).map_err(|_| bad())?;
    key.verify(&d[..176], &sig).map_err(|_| bad())?;
    let hash: [u8; 32] = Sha256::digest(&d[256..]).into();
    if hash[..] != d[64..96] {
        return Err(bad());
    }
    let vend = d[32..64].iter().position(|&c| c == 0).unwrap_or(32);
    Ok(Package {
        build: le32(d, 8),
        version: String::from_utf8_lossy(&d[32..32 + vend]).into_owned(),
        boot: &d[256..256 + boot_len],
        efi: &d[256 + boot_len..],
    })
}

fn efi_dir() -> Option<Dir> {
    fat::resolve(0, "/EFI/BOOT")
}

/// Scrive l'area di avvio BIOS lasciando com'e' la tabella delle partizioni (byte 440-511).
fn write_boot(boot: &[u8]) -> bool {
    let mut s0 = [0u8; 512];
    if !block::read(0, &mut s0) {
        return false;
    }
    let mut ok = true;
    for (i, chunk) in boot.chunks(512).enumerate() {
        let mut b = [0u8; 512];
        b[..chunk.len()].copy_from_slice(chunk);
        if i == 0 {
            b[440..].copy_from_slice(&s0[440..]);
        }
        ok &= block::write(i as u64, &b);
    }
    ok
}

/// Sostituisce \EFI\BOOT\BOOTX64.EFI in modo sicuro: prima scrive il nuovo file, poi scambia.
fn write_efi(efi: &[u8]) -> bool {
    let Some(d) = efi_dir() else { return false };
    if let Some(old) = fat::find(d, "BOOTX64.NEW") {
        fat::delete(&old);
    }
    if !fat::write(d, "BOOTX64.NEW", efi) {
        return false;
    }
    if let Some(old) = fat::find(d, "BOOTX64.EFI") {
        fat::delete(&old);
    }
    match fat::find(d, "BOOTX64.NEW") {
        Some(n) => fat::move_to(&n, d, "BOOTX64.EFI"),
        None => false,
    }
}

/// Copia la versione attuale in /System (area di avvio e loader UEFI).
fn backup() -> bool {
    let Some(sys) = fat::ensure_dir(SYS_DIR) else { return false };
    let Some(d) = efi_dir() else { return false };
    let Some(cur) = fat::find(d, "BOOTX64.EFI") else { return false };
    let efi = fat::read(&cur);
    if efi.is_empty() {
        return false;
    }
    // l'area di avvio va dal settore 0 fino alla configurazione
    let mut boot = vec![0u8; CONFIG_LBA as usize * 512];
    for i in 0..CONFIG_LBA as usize {
        let mut b = [0u8; 512];
        if !block::read(i as u64, &mut b) {
            return false;
        }
        boot[i * 512..(i + 1) * 512].copy_from_slice(&b);
    }
    // gli ultimi settori vuoti non servono
    let used = boot.iter().rposition(|&c| c != 0).map(|p| (p / 512 + 1) * 512).unwrap_or(512);
    boot.truncate(used);
    fat::write(sys, BK_BOOT, &boot) && fat::write(sys, BK_EFI, &efi) && fat::write(sys, BK_VER, crate::VERSION.as_bytes())
}

fn apply(data: &[u8], dk: &Dk) -> Result<String, String> {
    let p = verify(data, dk)?;
    if p.build <= crate::BUILD {
        return Err(String::from(dk.upd_uptodate));
    }
    if !fat::mounted() || efi_dir().is_none() {
        return Err(String::from(dk.upd_no_disk));
    }
    crate::log!("aggiornamento: {} -> {}", crate::VERSION, p.version);
    if !backup() {
        return Err(String::from(dk.upd_no_disk));
    }
    // prima il loader UEFI (file nuovo + scambio), poi l'area BIOS
    if !write_efi(p.efi) || !write_boot(p.boot) {
        return Err(String::from(dk.upd_write_failed));
    }
    Ok(p.version)
}

fn restore_backup(dk: &Dk) -> Result<String, String> {
    let sys = fat::find(0, SYS_DIR).filter(|e| e.dir).ok_or_else(|| String::from(dk.upd_no_backup))?;
    let boot = fat::find(sys.cluster, BK_BOOT).map(|e| fat::read(&e)).unwrap_or_default();
    let efi = fat::find(sys.cluster, BK_EFI).map(|e| fat::read(&e)).unwrap_or_default();
    let ver = fat::find(sys.cluster, BK_VER).map(|e| String::from_utf8_lossy(&fat::read(&e)).trim().into()).unwrap_or_default();
    if boot.len() < 1024 || efi.len() < 1024 {
        return Err(String::from(dk.upd_no_backup));
    }
    if !write_efi(&efi) || !write_boot(&boot) {
        return Err(String::from(dk.upd_write_failed));
    }
    crate::log!("ripristinata la versione {}", ver);
    Ok(ver)
}
