//! File system FAT16/FAT32 (lettura e scrittura) sulla partizione di MIN-OS.
//!
//! Supporta cartelle, nomi lunghi (VFAT), creazione, sovrascrittura, spostamento ed eliminazione.
//! E' la stessa partizione che contiene il bootloader UEFI: i file sono leggibili anche da
//! Windows, macOS e Linux collegando la chiavetta o il disco.

use crate::arch;
use crate::block;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Cartella: 0 = radice, altrimenti il primo cluster.
pub type Dir = u32;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u32,
    pub cluster: u32,
    pub date: u16,
    pub time: u16,
    /// posizioni (lba, offset) delle voci: prima quelle del nome lungo, ultima quella corta
    slots: Vec<(u64, usize)>,
}

impl Entry {
    /// "gg/mm/aaaa hh:mm"
    pub fn modified(&self) -> String {
        let (d, t) = (self.date, self.time);
        alloc::format!("{:02}/{:02}/{} {:02}:{:02}", d & 31, (d >> 5) & 15, 1980 + (d >> 9) as u32, t >> 11, (t >> 5) & 63)
    }
}

struct Fs {
    part: u64,
    spc: u32,
    fat_start: u64,
    fat_sz: u32,
    nfats: u32,
    root_start: u64,
    root_secs: u32,
    data_start: u64,
    root_cluster: u32,
    fat32: bool,
    clusters: u32,
    fat: Vec<u32>,
    dirty: Vec<bool>,
}

static mut FS: Option<Fs> = None;

fn fs() -> Option<&'static mut Fs> {
    unsafe { (*core::ptr::addr_of_mut!(FS)).as_mut() }
}

pub fn mounted() -> bool {
    fs().is_some()
}

fn rd(lba: u64) -> Option<[u8; 512]> {
    let mut b = [0u8; 512];
    if block::read(lba, &mut b) {
        Some(b)
    } else {
        None
    }
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Monta la prima partizione FAT del disco di MIN-OS.
pub fn mount() -> bool {
    let Some(mbr) = rd(0) else { return false };
    let mut part = None;
    for i in 0..4 {
        let e = &mbr[446 + i * 16..462 + i * 16];
        if matches!(e[4], 0xEF | 0x06 | 0x0E | 0x0B | 0x0C | 0x04) {
            part = Some(u32_at(e, 8) as u64);
            break;
        }
    }
    let Some(part) = part else { return false };
    let Some(bs) = rd(part) else { return false };
    if u16_at(&bs, 11) != 512 || bs[510] != 0x55 || bs[511] != 0xAA {
        return false;
    }
    let spc = bs[13] as u32;
    let reserved = u16_at(&bs, 14) as u32;
    let nfats = bs[16] as u32;
    let root_entries = u16_at(&bs, 17) as u32;
    let total = if u16_at(&bs, 19) != 0 { u16_at(&bs, 19) as u32 } else { u32_at(&bs, 32) };
    let fat16_sz = u16_at(&bs, 22) as u32;
    let fat32 = fat16_sz == 0;
    let fat_sz = if fat32 { u32_at(&bs, 36) } else { fat16_sz };
    let root_secs = (root_entries * 32).div_ceil(512);
    let data_start = reserved + nfats * fat_sz + root_secs;
    if spc == 0 || total <= data_start {
        return false;
    }
    let clusters = (total - data_start) / spc;
    let mut f = Fs {
        part,
        spc,
        fat_start: part + reserved as u64,
        fat_sz,
        nfats,
        root_start: part + (reserved + nfats * fat_sz) as u64,
        root_secs,
        data_start: part + data_start as u64,
        root_cluster: if fat32 { u32_at(&bs, 44) } else { 0 },
        fat32,
        clusters,
        fat: Vec::new(),
        dirty: vec![false; fat_sz as usize],
    };
    // tutta la FAT in memoria (piccola: pochi KB)
    let per = if fat32 { 128 } else { 256 };
    let needed = ((clusters + 2) as usize).div_ceil(per).min(fat_sz as usize);
    for s in 0..needed {
        let Some(b) = rd(f.fat_start + s as u64) else { return false };
        for i in 0..per {
            f.fat.push(if fat32 { u32_at(&b, i * 4) & 0x0FFF_FFFF } else { u16_at(&b, i * 2) as u32 });
        }
    }
    f.fat.truncate((clusters + 2) as usize);
    crate::log!("FAT{}: {} cluster da {} KB, {} KB liberi", if fat32 { 32 } else { 16 }, clusters, spc / 2, f.free() / 1024);
    unsafe { *core::ptr::addr_of_mut!(FS) = Some(f) };
    true
}

impl Fs {
    fn eoc(&self) -> u32 {
        if self.fat32 {
            0x0FFF_FFF8
        } else {
            0xFFF8
        }
    }
    fn lba(&self, cluster: u32) -> u64 {
        self.data_start + (cluster as u64 - 2) * self.spc as u64
    }
    fn chain(&self, mut c: u32) -> Vec<u32> {
        let mut v = Vec::new();
        while c >= 2 && c < self.eoc() && (c as usize) < self.fat.len() && v.len() < 1_000_000 {
            v.push(c);
            c = self.fat[c as usize];
        }
        v
    }
    fn free(&self) -> u64 {
        self.fat.iter().skip(2).filter(|&&v| v == 0).count() as u64 * self.spc as u64 * 512
    }
    fn set(&mut self, c: u32, v: u32) {
        self.fat[c as usize] = v;
        let per = if self.fat32 { 128 } else { 256 };
        self.dirty[c as usize / per] = true;
    }
    fn alloc(&mut self, n: usize) -> Option<Vec<u32>> {
        let free: Vec<u32> = (2..self.fat.len() as u32).filter(|&c| self.fat[c as usize] == 0).take(n).collect();
        if free.len() < n {
            return None;
        }
        for (i, &c) in free.iter().enumerate() {
            let next = if i + 1 < free.len() { free[i + 1] } else { self.eoc() | 7 };
            self.set(c, next);
        }
        Some(free)
    }
    fn free_chain(&mut self, c: u32) {
        for x in self.chain(c) {
            self.set(x, 0);
        }
    }
    fn flush(&mut self) -> bool {
        let per = if self.fat32 { 128 } else { 256 };
        let mut ok = true;
        for s in 0..self.dirty.len() {
            if !self.dirty[s] {
                continue;
            }
            let mut b = [0u8; 512];
            for i in 0..per {
                let c = s * per + i;
                let v = self.fat.get(c).copied().unwrap_or(0);
                if self.fat32 {
                    b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
                } else {
                    b[i * 2..i * 2 + 2].copy_from_slice(&(v as u16).to_le_bytes());
                }
            }
            for f in 0..self.nfats {
                ok &= block::write(self.fat_start + (f * self.fat_sz) as u64 + s as u64, &b);
            }
            self.dirty[s] = false;
        }
        ok
    }
    fn dir_sectors(&self, dir: Dir) -> Vec<u64> {
        if dir == 0 && !self.fat32 {
            return (0..self.root_secs as u64).map(|i| self.root_start + i).collect();
        }
        let first = if dir == 0 { self.root_cluster } else { dir };
        let mut v = Vec::new();
        for c in self.chain(first) {
            for s in 0..self.spc as u64 {
                v.push(self.lba(c) + s);
            }
        }
        v
    }
    fn zero_cluster(&self, c: u32) -> bool {
        let z = [0u8; 512];
        (0..self.spc as u64).all(|s| block::write(self.lba(c) + s, &z))
    }
}

// ------------------------------------------------------------------ nomi

fn short_name(raw: &[u8], nt: u8) -> String {
    let base = core::str::from_utf8(&raw[..8]).unwrap_or("").trim_end();
    let ext = core::str::from_utf8(&raw[8..11]).unwrap_or("").trim_end();
    let low = |s: &str, on: bool| if on { s.to_ascii_lowercase() } else { String::from(s) };
    let mut n = low(base, nt & 0x08 != 0);
    if !ext.is_empty() {
        n.push('.');
        n.push_str(&low(ext, nt & 0x10 != 0));
    }
    n
}

fn lfn_checksum(short: &[u8; 11]) -> u8 {
    short.iter().fold(0u8, |s, &c| (((s & 1) << 7) | (s >> 1)).wrapping_add(c))
}

const LFN_POS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];

fn valid_short_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "$%'-_@~`!(){}^#&".contains(c)
}

/// Ritorna (nome corto 8.3, flag minuscole, serve il nome lungo?)
fn make_short(name: &str, taken: &dyn Fn(&[u8; 11]) -> bool) -> ([u8; 11], u8, bool) {
    let (base, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    };
    let fits = base.len() <= 8 && ext.len() <= 3 && base.chars().all(valid_short_char) && ext.chars().all(valid_short_char) && !base.is_empty();
    let case_ok = |s: &str| s.chars().all(|c| !c.is_ascii_alphabetic()) || s == s.to_ascii_lowercase() || s == s.to_ascii_uppercase();
    let mut raw = [b' '; 11];
    if fits && case_ok(base) && case_ok(ext) {
        for (i, c) in base.bytes().enumerate() {
            raw[i] = c.to_ascii_uppercase();
        }
        for (i, c) in ext.bytes().enumerate() {
            raw[8 + i] = c.to_ascii_uppercase();
        }
        let mut nt = 0;
        if base.chars().any(|c| c.is_ascii_lowercase()) {
            nt |= 0x08;
        }
        if ext.chars().any(|c| c.is_ascii_lowercase()) {
            nt |= 0x10;
        }
        if !taken(&raw) {
            return (raw, nt, false);
        }
    }
    // nome lungo: genera un nome corto unico tipo "LISTAD~1.TXT"
    let clean: Vec<u8> = base.chars().filter(|c| valid_short_char(*c)).map(|c| c.to_ascii_uppercase() as u8).take(6).collect();
    let e: Vec<u8> = ext.chars().filter(|c| valid_short_char(*c)).map(|c| c.to_ascii_uppercase() as u8).take(3).collect();
    for n in 1..1000u32 {
        let tail = alloc::format!("~{}", n);
        let keep = (8 - tail.len()).min(clean.len()).max(if clean.is_empty() { 0 } else { 1 });
        let mut r = [b' '; 11];
        let mut i = 0;
        for &c in clean.iter().take(keep) {
            r[i] = c;
            i += 1;
        }
        if i == 0 {
            r[0] = b'F';
            i = 1;
        }
        for c in tail.bytes() {
            r[i] = c;
            i += 1;
        }
        for (j, &c) in e.iter().enumerate() {
            r[8 + j] = c;
        }
        if !taken(&r) {
            return (r, 0, true);
        }
    }
    (raw, 0, true)
}

fn now_stamp() -> (u16, u16) {
    let d = arch::rtc_now();
    let date = ((d.year.saturating_sub(1980)) << 9) | ((d.month as u16) << 5) | d.day as u16;
    let time = ((d.hour as u16) << 11) | ((d.min as u16) << 5) | (d.sec as u16 / 2);
    (date, time)
}

// ------------------------------------------------------------------ cartelle

/// Legge tutte le voci (anche "." e "..") di una cartella.
fn scan(f: &Fs, dir: Dir) -> (Vec<Entry>, Vec<(u64, usize, u8)>) {
    let mut out = Vec::new();
    let mut shorts = Vec::new();
    let mut lfn: Vec<(u8, [u16; 13])> = Vec::new();
    let mut lfn_slots: Vec<(u64, usize)> = Vec::new();
    'outer: for lba in f.dir_sectors(dir) {
        let Some(b) = rd(lba) else { break };
        for i in 0..16 {
            let e = &b[i * 32..i * 32 + 32];
            if e[0] == 0 {
                break 'outer;
            }
            if e[0] == 0xE5 {
                lfn.clear();
                lfn_slots.clear();
                continue;
            }
            if e[11] == 0x0F {
                let mut chars = [0u16; 13];
                for (k, &p) in LFN_POS.iter().enumerate() {
                    chars[k] = u16_at(e, p);
                }
                lfn.push((e[0] & 0x1F, chars));
                lfn_slots.push((lba, i * 32));
                continue;
            }
            if e[11] & 0x08 != 0 {
                lfn.clear();
                lfn_slots.clear();
                continue; // etichetta del volume
            }
            let raw: [u8; 11] = e[..11].try_into().unwrap();
            shorts.push((lba, i * 32, e[0]));
            let mut name = short_name(&raw, e[12]);
            if !lfn.is_empty() && lfn.iter().all(|_| true) {
                let mut parts = lfn.clone();
                parts.sort_by_key(|p| p.0);
                let mut u: Vec<u16> = Vec::new();
                for (_, c) in parts.iter() {
                    for &ch in c.iter() {
                        if ch == 0 || ch == 0xFFFF {
                            break;
                        }
                        u.push(ch);
                    }
                }
                name = String::from_utf16_lossy(&u);
            }
            let mut slots = core::mem::take(&mut lfn_slots);
            slots.push((lba, i * 32));
            lfn.clear();
            let hi = if f.fat32 { (u16_at(e, 20) as u32) << 16 } else { 0 };
            out.push(Entry {
                name,
                dir: e[11] & 0x10 != 0,
                size: u32_at(e, 28),
                cluster: hi | u16_at(e, 26) as u32,
                date: u16_at(e, 24),
                time: u16_at(e, 22),
                slots,
            });
        }
    }
    (out, shorts)
}

/// Contenuto di una cartella (senza "." e "..", cartelle prima, in ordine alfabetico).
pub fn list(dir: Dir) -> Vec<Entry> {
    let Some(f) = fs() else { return Vec::new() };
    let (mut v, _) = scan(f, dir);
    v.retain(|e| e.name != "." && e.name != "..");
    v.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    v
}

pub fn find(dir: Dir, name: &str) -> Option<Entry> {
    list(dir).into_iter().find(|e| e.name.eq_ignore_ascii_case(name))
}

pub fn read(e: &Entry) -> Vec<u8> {
    let Some(f) = fs() else { return Vec::new() };
    let mut out = Vec::with_capacity(e.size as usize);
    'c: for c in f.chain(e.cluster) {
        for s in 0..f.spc as u64 {
            if out.len() >= e.size as usize {
                break 'c;
            }
            let Some(b) = rd(f.lba(c) + s) else { break 'c };
            let take = (e.size as usize - out.len()).min(512);
            out.extend_from_slice(&b[..take]);
        }
    }
    out
}

/// Trova `n` voci libere consecutive (allarga la cartella se serve).
fn free_slots(f: &mut Fs, dir: Dir, n: usize) -> Option<Vec<(u64, usize)>> {
    for _ in 0..2 {
        let mut run: Vec<(u64, usize)> = Vec::new();
        for lba in f.dir_sectors(dir) {
            let b = rd(lba)?;
            for i in 0..16 {
                let first = b[i * 32];
                if first == 0 || first == 0xE5 {
                    run.push((lba, i * 32));
                    if run.len() == n {
                        return Some(run);
                    }
                } else {
                    run.clear();
                }
            }
        }
        if dir == 0 && !f.fat32 {
            return None; // la radice FAT16 ha dimensione fissa
        }
        // aggiungi un cluster vuoto alla cartella
        let first = if dir == 0 { f.root_cluster } else { dir };
        let last = *f.chain(first).last()?;
        let new = f.alloc(1)?[0];
        f.set(last, new);
        if !f.zero_cluster(new) || !f.flush() {
            return None;
        }
    }
    None
}

fn write_slot(lba: u64, off: usize, data: &[u8; 32]) -> bool {
    let Some(mut b) = rd(lba) else { return false };
    b[off..off + 32].copy_from_slice(data);
    block::write(lba, &b)
}

/// Crea le voci (nome lungo + nome corto) per un file o una cartella.
fn add_entry(f: &mut Fs, dir: Dir, name: &str, attr: u8, cluster: u32, size: u32) -> bool {
    let (_, shorts) = scan(f, dir);
    let mut existing: Vec<[u8; 11]> = Vec::new();
    for (lba, off, _) in shorts {
        if let Some(b) = rd(lba) {
            existing.push(b[off..off + 11].try_into().unwrap());
        }
    }
    let taken = |r: &[u8; 11]| existing.iter().any(|e| e == r);
    let (short, nt, need_lfn) = make_short(name, &taken);
    let units: Vec<u16> = name.encode_utf16().collect();
    let nlfn = if need_lfn { units.len().div_ceil(13) } else { 0 };
    let Some(slots) = free_slots(f, dir, nlfn + 1) else { return false };
    let sum = lfn_checksum(&short);
    let mut ok = true;
    for k in 0..nlfn {
        let ord = (nlfn - k) as u8;
        let mut e = [0u8; 32];
        e[0] = ord | if k == 0 { 0x40 } else { 0 };
        e[11] = 0x0F;
        e[13] = sum;
        for (j, &p) in LFN_POS.iter().enumerate() {
            let idx = (ord as usize - 1) * 13 + j;
            let ch: u16 = if idx < units.len() {
                units[idx]
            } else if idx == units.len() {
                0
            } else {
                0xFFFF
            };
            e[p..p + 2].copy_from_slice(&ch.to_le_bytes());
        }
        ok &= write_slot(slots[k].0, slots[k].1, &e);
    }
    let (date, time) = now_stamp();
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(&short);
    e[11] = attr;
    e[12] = nt;
    e[14..16].copy_from_slice(&time.to_le_bytes());
    e[16..18].copy_from_slice(&date.to_le_bytes());
    e[18..20].copy_from_slice(&date.to_le_bytes());
    e[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
    e[22..24].copy_from_slice(&time.to_le_bytes());
    e[24..26].copy_from_slice(&date.to_le_bytes());
    e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    let (lba, off) = slots[nlfn];
    ok && write_slot(lba, off, &e)
}

fn remove_slots(e: &Entry) -> bool {
    let mut ok = true;
    for &(lba, off) in e.slots.iter() {
        if let Some(mut b) = rd(lba) {
            b[off] = 0xE5;
            ok &= block::write(lba, &b);
        }
    }
    ok
}

fn bad_name(name: &str) -> bool {
    name.trim().is_empty() || name == "." || name == ".." || name.chars().any(|c| "\\/:*?\"<>|".contains(c) || (c as u32) < 32)
}

/// Crea o sovrascrive un file.
pub fn write(dir: Dir, name: &str, data: &[u8]) -> bool {
    if bad_name(name) {
        return false;
    }
    let old = find(dir, name);
    let Some(f) = fs() else { return false };
    if let Some(old) = old {
        if old.dir {
            return false;
        }
        f.free_chain(old.cluster);
        remove_slots(&old);
    }
    let csize = f.spc as usize * 512;
    let n = data.len().div_ceil(csize);
    let clusters = if n == 0 { Vec::new() } else { match f.alloc(n) { Some(c) => c, None => return false } };
    let mut ok = true;
    for (i, &c) in clusters.iter().enumerate() {
        for s in 0..f.spc as usize {
            let start = i * csize + s * 512;
            let mut b = [0u8; 512];
            if start < data.len() {
                let end = (start + 512).min(data.len());
                b[..end - start].copy_from_slice(&data[start..end]);
            }
            ok &= block::write(f.lba(c) + s as u64, &b);
        }
    }
    ok &= f.flush();
    ok && add_entry(f, dir, name.trim(), 0x20, clusters.first().copied().unwrap_or(0), data.len() as u32)
}

/// Crea una cartella e ritorna il suo identificativo.
pub fn mkdir(dir: Dir, name: &str) -> Option<Dir> {
    if bad_name(name) || find(dir, name).is_some() {
        return None;
    }
    let f = fs()?;
    let c = f.alloc(1)?[0];
    if !f.zero_cluster(c) || !f.flush() {
        return None;
    }
    // voci "." e ".."
    let (date, time) = now_stamp();
    let mut b = [0u8; 512];
    for (i, (n, cl)) in [(b".          ", c), (b"..         ", if dir == f.root_cluster { 0 } else { dir })].iter().enumerate() {
        let e = &mut b[i * 32..i * 32 + 32];
        e[..11].copy_from_slice(*n);
        e[11] = 0x10;
        e[20..22].copy_from_slice(&((*cl >> 16) as u16).to_le_bytes());
        e[22..24].copy_from_slice(&time.to_le_bytes());
        e[24..26].copy_from_slice(&date.to_le_bytes());
        e[26..28].copy_from_slice(&(*cl as u16).to_le_bytes());
    }
    if !block::write(f.lba(c), &b) {
        return None;
    }
    if add_entry(f, dir, name.trim(), 0x10, c, 0) {
        Some(c)
    } else {
        None
    }
}

/// Elimina definitivamente un file o una cartella (con tutto il contenuto).
pub fn delete(e: &Entry) -> bool {
    if e.dir && e.cluster >= 2 {
        for child in list(e.cluster) {
            delete(&child);
        }
    }
    let Some(f) = fs() else { return false };
    if e.cluster >= 2 {
        f.free_chain(e.cluster);
    }
    remove_slots(e) && f.flush()
}

/// Sposta (o rinomina) una voce in un'altra cartella senza copiare i dati.
pub fn move_to(e: &Entry, dest: Dir, new_name: &str) -> bool {
    if bad_name(new_name) || find(dest, new_name).is_some() {
        return false;
    }
    let Some(f) = fs() else { return false };
    let attr = if e.dir { 0x10 } else { 0x20 };
    if !add_entry(f, dest, new_name, attr, e.cluster, e.size) {
        return false;
    }
    if e.dir && e.cluster >= 2 {
        // aggiorna ".." della cartella spostata
        let lba = f.lba(e.cluster);
        if let Some(mut b) = rd(lba) {
            let parent = if dest == f.root_cluster { 0 } else { dest };
            b[32 + 20..32 + 22].copy_from_slice(&((parent >> 16) as u16).to_le_bytes());
            b[32 + 26..32 + 28].copy_from_slice(&(parent as u16).to_le_bytes());
            block::write(lba, &b);
        }
    }
    remove_slots(e)
}

/// Byte liberi sulla partizione.
pub fn free_bytes() -> u64 {
    fs().map(|f| f.free()).unwrap_or(0)
}

/// Cerca (o crea) una cartella nella radice: usata per "Documenti" e "Cestino".
pub fn ensure_dir(name: &str) -> Option<Dir> {
    match find(0, name) {
        Some(e) if e.dir => Some(e.cluster),
        Some(_) => None,
        None => mkdir(0, name),
    }
}

/// Percorso -> cartella (es. "Documenti/Progetti"). Cartelle separate da '/'.
pub fn resolve(base: Dir, path: &str) -> Option<Dir> {
    let mut cur = if path.starts_with('/') { 0 } else { base };
    for part in path.split('/').filter(|p| !p.is_empty()) {
        if part == "." {
            continue;
        }
        if part == ".." {
            if cur == 0 {
                continue;
            }
            let f = fs()?;
            let (all, _) = scan(f, cur);
            cur = all.iter().find(|e| e.name == "..").map(|e| e.cluster).unwrap_or(0);
            continue;
        }
        let e = find(cur, part)?;
        if !e.dir {
            return None;
        }
        cur = e.cluster;
    }
    Some(cur)
}
