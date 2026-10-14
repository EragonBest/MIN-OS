//! Salvataggio della configurazione di MIN-OS.
//!
//! Le impostazioni stanno nel settore 4000 del disco di MIN-OS, una zona riservata
//! dell'immagine prima della partizione EFI.

use crate::block;
use crate::i18n;
use crate::keymap;
use crate::ui::Settings;
use alloc::string::String;

pub const CONFIG_LBA: u64 = 4000;
const MAGIC: &[u8; 8] = b"MINOSCF1";

// ------------------------------------------------------------------ configurazione

fn put_str(b: &mut [u8], at: usize, s: &str) {
    let bytes = s.as_bytes();
    let n = bytes.len().min(63);
    b[at] = n as u8;
    b[at + 1..at + 1 + n].copy_from_slice(&bytes[..n]);
}

fn get_str(b: &[u8], at: usize) -> String {
    let n = (b[at] as usize).min(63);
    String::from(core::str::from_utf8(&b[at + 1..at + 1 + n]).unwrap_or(""))
}

fn checksum(b: &[u8]) -> u32 {
    b.iter().fold(0x811C_9DC5u32, |h, &x| (h ^ x as u32).wrapping_mul(0x0100_0193))
}

pub fn encode(s: &Settings) -> [u8; 512] {
    let mut b = [0u8; 512];
    b[..8].copy_from_slice(MAGIC);
    b[8] = i18n::ALL.iter().position(|l| *l == s.lang).unwrap_or(0) as u8;
    b[9] = keymap::ALL.iter().position(|l| *l == s.layout).unwrap_or(0) as u8;
    b[10] = s.dark as u8;
    b[11] = s.accent as u8;
    b[12] = s.preset as u8;
    b[13] = s.anim as u8;
    b[14] = s.glass as u8;
    b[15] = s.icon_size as u8;
    b[16] = s.clock24 as u8;
    b[17] = s.show_icons as u8;
    b[18] = s.sort_name as u8;
    put_str(&mut b, 32, &s.user);
    put_str(&mut b, 96, &s.pc);
    let c = checksum(&b[..508]);
    b[508..].copy_from_slice(&c.to_le_bytes());
    b
}

fn decode(b: &[u8; 512]) -> Option<Settings> {
    if &b[..8] != MAGIC || checksum(&b[..508]) != u32::from_le_bytes(b[508..].try_into().ok()?) {
        return None;
    }
    let user = get_str(b, 32);
    if user.is_empty() {
        return None;
    }
    Some(Settings {
        lang: *i18n::ALL.get(b[8] as usize)?,
        layout: *keymap::ALL.get(b[9] as usize)?,
        dark: b[10] != 0,
        accent: (b[11] as usize).min(5),
        preset: (b[12] as usize).min(2),
        anim: b[13] != 0,
        glass: b[14] != 0,
        icon_size: (b[15] as usize).min(2),
        clock24: b[16] != 0,
        show_icons: b[17] != 0,
        sort_name: b[18] != 0,
        user,
        pc: get_str(b, 96),
    })
}

pub fn load() -> Option<Settings> {
    let mut b = [0u8; 512];
    if !block::read(CONFIG_LBA, &mut b) {
        return None;
    }
    let s = decode(&b);
    crate::log!("configurazione su disco: {}", if s.is_some() { "trovata" } else { "assente" });
    s
}

pub fn save_raw(b: &[u8; 512]) -> bool {
    let ok = block::write(CONFIG_LBA, b);
    crate::log!("configurazione salvata: {}", ok);
    ok
}

pub fn save(s: &Settings) -> bool {
    save_raw(&encode(s))
}

/// Cancella la configurazione: al prossimo avvio riparte l'installer.
pub fn erase() -> bool {
    save_raw(&[0u8; 512])
}
