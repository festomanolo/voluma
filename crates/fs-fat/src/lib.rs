//! FAT12/16/32 driver. Writes only on a device opened read-write AND a cleanly
//! unmounted volume. Every on-disk value is validated; malformed
//! images yield `FsError::Corrupt`, never out-of-range access or endless loops.

use block_device::BlockDevice;
use fs_core::{normalize_path, Capabilities, DirEntry, EntryKind, Filesystem, FsError, Result};
use std::sync::{Arc, Mutex};

mod write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatKind { Fat12, Fat16, Fat32 }

const MAX_DIR_BYTES: usize = 4 << 20;

pub struct FatFs {
    dev: Arc<dyn BlockDevice>,
    kind: FatKind,
    bps: u64,
    spc: u64,
    fat_start: u64,       // byte offset of first FAT
    root_start: u64,      // FAT12/16: byte offset of fixed root dir
    root_size: u64,       // FAT12/16: bytes in fixed root dir
    data_start: u64,      // byte offset of cluster 2
    cluster_count: u32,
    root_cluster: u32,    // FAT32
    label: Option<String>,
    nfats: u64,
    fat_bytes: u64,       // size of one FAT copy
    fsinfo_sector: u64,   // FAT32 only, 0 if absent
    /// Serialises all mutation (and the allocation hint).
    wlock: Mutex<u32>,
    /// Read cursor `(first cluster of the file, chain index, cluster)`: lets sequential reads resume instead of
    /// re-walking the chain from the start of the file on every call. Cleared by every mutation.
    cursor: Mutex<(u32, u64, u32)>,
}

fn le16(b: &[u8], o: usize) -> u64 { u16::from_le_bytes([b[o], b[o + 1]]) as u64 }
fn le32(b: &[u8], o: usize) -> u64 { u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as u64 }

impl FatFs {
    pub fn open(dev: Arc<dyn BlockDevice>) -> Result<Self> {
        let mut s = [0u8; 512];
        dev.read_at(0, &mut s)?;
        if s[510] != 0x55 || s[511] != 0xAA { return Err(FsError::Corrupt("missing boot signature")); }
        let bps = le16(&s, 11);
        let spc = s[13] as u64;
        let reserved = le16(&s, 14);
        let nfats = s[16] as u64;
        let root_entries = le16(&s, 17);
        let total = if le16(&s, 19) != 0 { le16(&s, 19) } else { le32(&s, 32) };
        let fat_sz = if le16(&s, 22) != 0 { le16(&s, 22) } else { le32(&s, 36) };
        if !matches!(bps, 512 | 1024 | 2048 | 4096) || spc == 0 || !spc.is_power_of_two()
            || reserved == 0 || nfats == 0 || fat_sz == 0 || total == 0 {
            return Err(FsError::Corrupt("invalid BPB"));
        }
        let root_secs = (root_entries * 32).div_ceil(bps);
        let meta = reserved + nfats * fat_sz + root_secs;
        if total <= meta || total * bps > dev.size() { return Err(FsError::Corrupt("BPB exceeds device")); }
        let clusters = (total - meta) / spc;
        let kind = if clusters < 4085 { FatKind::Fat12 } else if clusters < 65525 { FatKind::Fat16 } else { FatKind::Fat32 };
        let root_cluster = if kind == FatKind::Fat32 { le32(&s, 44) as u32 } else { 0 };
        if kind == FatKind::Fat32 && (root_cluster < 2 || root_cluster as u64 >= clusters + 2) {
            return Err(FsError::Corrupt("bad root cluster"));
        }
        let lbl = if kind == FatKind::Fat32 { &s[71..82] } else { &s[43..54] };
        let label = Some(String::from_utf8_lossy(lbl).trim_matches(|c: char| c == ' ' || c == '\0').to_string())
            .filter(|l| !l.is_empty() && l != "NO NAME");
        let mut fs = Self {
            dev, kind, bps, spc,
            fat_start: reserved * bps,
            root_start: (reserved + nfats * fat_sz) * bps,
            root_size: root_secs * bps,
            data_start: meta * bps,
            cluster_count: clusters as u32,
            root_cluster, label,
            nfats, fat_bytes: fat_sz * bps,
            fsinfo_sector: if kind == FatKind::Fat32 { le16(&s, 48) } else { 0 },
            wlock: Mutex::new(2),
            cursor: Mutex::new((0, 0, 0)),
        };
        // A volume-label entry in the root directory overrides the BPB label.
        if let Ok(root) = fs.dir_bytes(DirLoc::Root) {
            for e in root.chunks_exact(32) {
                if e[0] == 0 { break; }
                if e[0] != 0xE5 && e[11] & 0x3F == 0x08 {
                    let l = String::from_utf8_lossy(&e[0..11]).trim_end().to_string();
                    if !l.is_empty() { fs.label = Some(l); }
                    break;
                }
            }
        }
        Ok(fs)
    }

    pub fn kind(&self) -> FatKind { self.kind }
    pub fn cluster_size(&self) -> u64 { self.bps * self.spc }

    fn fat_entry(&self, n: u32) -> Result<u32> {
        if n < 2 || n >= self.cluster_count + 2 { return Err(FsError::Corrupt("cluster out of range")); }
        let n = n as u64;
        let v = match self.kind {
            FatKind::Fat12 => {
                let mut b = [0u8; 2];
                self.dev.read_at(self.fat_start + n + n / 2, &mut b)?;
                let w = u16::from_le_bytes(b) as u32;
                if n & 1 == 1 { w >> 4 } else { w & 0xFFF }
            }
            FatKind::Fat16 => { let mut b = [0u8; 2]; self.dev.read_at(self.fat_start + n * 2, &mut b)?; u16::from_le_bytes(b) as u32 }
            FatKind::Fat32 => { let mut b = [0u8; 4]; self.dev.read_at(self.fat_start + n * 4, &mut b)?; u32::from_le_bytes(b) & 0x0FFF_FFFF }
        };
        Ok(v)
    }

    fn is_eoc(&self, v: u32) -> bool {
        match self.kind { FatKind::Fat12 => v >= 0xFF8, FatKind::Fat16 => v >= 0xFFF8, FatKind::Fat32 => v >= 0x0FFF_FFF8 }
    }

    /// Next cluster in chain, or None at end. Bad/free/out-of-range => Corrupt.
    fn next_cluster(&self, c: u32) -> Result<Option<u32>> {
        let v = self.fat_entry(c)?;
        if self.is_eoc(v) { Ok(None) }
        else if v < 2 || v >= self.cluster_count + 2 { Err(FsError::Corrupt("broken cluster chain")) }
        else { Ok(Some(v)) }
    }

    fn cluster_offset(&self, c: u32) -> u64 { self.data_start + (c as u64 - 2) * self.cluster_size() }

    /// Walk `skip` clusters from `first`; loop-protected.
    fn nth_cluster(&self, first: u32, skip: u64) -> Result<Option<u32>> {
        let mut c = first;
        for _ in 0..skip {
            match self.next_cluster(c)? { Some(n) => c = n, None => return Ok(None) }
        }
        Ok(Some(c))
    }

    fn dir_bytes(&self, loc: DirLoc) -> Result<Vec<u8>> {
        match loc {
            DirLoc::Root if self.kind != FatKind::Fat32 => {
                let mut v = vec![0u8; self.root_size as usize];
                self.dev.read_at(self.root_start, &mut v)?;
                Ok(v)
            }
            DirLoc::Root => self.chain_bytes(self.root_cluster),
            DirLoc::Cluster(c) => self.chain_bytes(c),
        }
    }

    fn chain_bytes(&self, first: u32) -> Result<Vec<u8>> {
        let cs = self.cluster_size() as usize;
        let mut out = Vec::new();
        let mut c = first;
        for _ in 0..=self.cluster_count {
            let at = out.len();
            out.resize(at + cs, 0);
            self.dev.read_at(self.cluster_offset(c), &mut out[at..])?;
            if out.len() > MAX_DIR_BYTES { return Err(FsError::Corrupt("directory too large")); }
            match self.next_cluster(c)? { Some(n) => c = n, None => return Ok(out) }
        }
        Err(FsError::Corrupt("cluster chain loop"))
    }

    fn parse_dir(&self, raw: &[u8]) -> Vec<DirEntry> {
        self.parse_dir_located(raw).into_iter().map(|l| l.entry).collect()
    }

    fn parse_dir_located(&self, raw: &[u8]) -> Vec<Located> {
        let mut out = Vec::new();
        let mut first_slot: Option<usize> = None;
        let mut lfn: Vec<(u8, [u16; 13])> = Vec::new();
        let mut lfn_sum: Option<u8> = None;
        for (slot, e) in raw.chunks_exact(32).enumerate() {
            if e[0] == 0 { break; }
            if e[0] == 0xE5 { lfn.clear(); lfn_sum = None; first_slot = None; continue; }
            let attr = e[11];
            if attr & 0x3F == 0x0F {
                let seq = e[0];
                if seq & 0x40 != 0 { lfn.clear(); lfn_sum = Some(e[13]); first_slot = Some(slot); }
                let mut ch = [0u16; 13];
                for (i, o) in [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30].iter().enumerate() {
                    ch[i] = u16::from_le_bytes([e[*o], e[*o + 1]]);
                }
                lfn.push((seq & 0x3F, ch));
                continue;
            }
            if attr & 0x08 != 0 { lfn.clear(); first_slot = None; continue; } // volume label
            let mut short = [0u8; 11];
            short.copy_from_slice(&e[0..11]);
            if short[0] == 0x05 { short[0] = 0xE5; }
            let name_bytes = &short[0..8];
            let ext_bytes = &short[8..11];
            if name_bytes.starts_with(b". ") || name_bytes.starts_with(b".. ") { lfn.clear(); first_slot = None; continue; }
            let used_lfn = !lfn.is_empty() && lfn_sum == Some(short_checksum(&e[0..11]));

            let name = if !lfn.is_empty() && lfn_sum == Some(short_checksum(&e[0..11])) {
                lfn.sort_by_key(|(s, _)| *s);
                let units: Vec<u16> = lfn.iter().flat_map(|(_, c)| c.iter().copied())
                    .take_while(|&u| u != 0 && u != 0xFFFF).collect();
                String::from_utf16_lossy(&units)
            } else {
                let lower_base = e[12] & 0x08 != 0;
                let lower_ext = e[12] & 0x10 != 0;
                let b = ascii_trim(name_bytes, lower_base);
                let x = ascii_trim(ext_bytes, lower_ext);
                if x.is_empty() { b } else { format!("{b}.{x}") }
            };
            lfn.clear(); lfn_sum = None;
            let start = if used_lfn { first_slot.unwrap_or(slot) } else { slot };
            first_slot = None;

            let hi = if self.kind == FatKind::Fat32 { le16(e, 20) } else { 0 };
            let first = ((hi << 16) | le16(e, 26)) as u32;
            let is_dir = attr & 0x10 != 0;
            out.push(Located { first_slot: start, short_slot: slot, short: e[0..11].try_into().unwrap(), entry: DirEntry {
                name,
                kind: if is_dir { EntryKind::Directory } else { EntryKind::File },
                size: if is_dir { 0 } else { le32(e, 28) },
                modified: dos_time(le16(e, 24), le16(e, 22), 0),
                created: dos_time(le16(e, 16), le16(e, 14), e[13]),
                hidden: attr & 0x02 != 0,
                read_only: attr & 0x01 != 0,
                inode: first as u64,
            }});
        }
        out
    }

    fn dir_loc(&self, e: &DirEntry) -> DirLoc {
        if e.inode < 2 { DirLoc::Root } else { DirLoc::Cluster(e.inode as u32) }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DirLoc { Root, Cluster(u32) }

/// A parsed entry plus where its directory-entry set lives.
struct Located { entry: DirEntry, first_slot: usize, short_slot: usize, short: [u8; 11] }

fn ascii_trim(b: &[u8], lower: bool) -> String {
    let s: String = b.iter().map(|&c| c as char).collect::<String>().trim_end().to_string();
    if lower { s.to_lowercase() } else { s }
}

fn short_checksum(n: &[u8]) -> u8 {
    n.iter().fold(0u8, |s, &c| ((s & 1) << 7).wrapping_add(s >> 1).wrapping_add(c))
}

/// DOS date/time -> Unix seconds (local time treated as UTC; FAT stores no zone).
fn dos_time(date: u64, time: u64, tenths: u8) -> Option<i64> {
    if date == 0 { return None; }
    let (y, m, d) = (1980 + (date >> 9) as i64, ((date >> 5) & 0xF) as i64, (date & 0x1F) as i64);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) { return None; }
    let (h, mi, s) = ((time >> 11) as i64, ((time >> 5) & 0x3F) as i64, ((time & 0x1F) * 2) as i64);
    // days from civil (Howard Hinnant)
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + mi * 60 + s + (tenths as i64) / 100)
}

impl Filesystem for FatFs {
    fn name(&self) -> &'static str {
        match self.kind { FatKind::Fat12 => "FAT12", FatKind::Fat16 => "FAT16", FatKind::Fat32 => "FAT32" }
    }
    fn label(&self) -> Option<String> { self.label.clone() }
    fn capabilities(&self) -> Capabilities {
        if self.dev.is_read_only() { return Capabilities::READ_ONLY; }
        Capabilities { read: true, write: true, create: true, delete: true, rename: true,
            permissions: false, symbolic_links: false, extended_attributes: false }
    }
    fn create_dir(&self, path: &str) -> Result<DirEntry> { self.w_create_dir(path) }
    fn write_file(&self, path: &str, src: &mut dyn std::io::Read, overwrite: bool) -> Result<DirEntry> { self.w_write_file(path, src, overwrite) }
    fn remove(&self, path: &str) -> Result<()> { self.w_remove(path) }
    fn rename(&self, from: &str, to: &str) -> Result<()> { self.w_rename(from, to) }

    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        let comps = normalize_path(path)?;
        let mut loc = DirLoc::Root;
        for c in comps {
            let ent = self.parse_dir(&self.dir_bytes(loc)?).into_iter()
                .find(|e| e.name.eq_ignore_ascii_case(c) || e.name.to_lowercase() == c.to_lowercase())
                .ok_or_else(|| FsError::NotFound(c.into()))?;
            if ent.kind != EntryKind::Directory { return Err(FsError::NotADirectory(c.into())); }
            loc = self.dir_loc(&ent);
        }
        Ok(self.parse_dir(&self.dir_bytes(loc)?))
    }

    fn stat(&self, path: &str) -> Result<DirEntry> {
        let comps = normalize_path(path)?;
        let Some((last, parents)) = comps.split_last() else {
            return Ok(DirEntry { name: "/".into(), kind: EntryKind::Directory, size: 0, modified: None, created: None, hidden: false, read_only: false, inode: 0 });
        };
        let parent = parents.join("/");
        self.read_dir(&parent)?.into_iter()
            .find(|e| e.name.to_lowercase() == last.to_lowercase())
            .ok_or_else(|| FsError::NotFound(path.into()))
    }

    fn read_at(&self, entry: &DirEntry, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if entry.kind == EntryKind::Directory { return Err(FsError::IsADirectory(entry.name.clone())); }
        if offset >= entry.size || buf.is_empty() || entry.size == 0 { return Ok(0); }
        let want = (buf.len() as u64).min(entry.size - offset) as usize;
        let cs = self.cluster_size();
        let first = entry.inode as u32;
        // Resume from the cursor when this continues a sequential read of the same file.
        let target = offset / cs;
        let (mut idx, mut c) = {
            let cur = self.cursor.lock().unwrap();
            if cur.0 == first && cur.0 != 0 && cur.1 <= target { (cur.1, cur.2) } else { (0, first) }
        };
        while idx < target {
            c = self.next_cluster(c)?.ok_or(FsError::Corrupt("file shorter than its size"))?;
            idx += 1;
        }
        let mut within = offset % cs;
        let mut done = 0usize;
        let (mut last, mut last_idx) = (c, idx);
        while done < want {
            // Extend over physically contiguous clusters so one read covers the whole run.
            last = c; last_idx = idx;
            let (mut avail, mut next) = (cs - within, None);
            while (avail as usize) < want - done {
                match self.next_cluster(last)? {
                    Some(n) if n == last + 1 => { last = n; last_idx += 1; avail += cs; }
                    Some(n) => { next = Some(n); break; }
                    None => return Err(FsError::Corrupt("file shorter than its size")),
                }
            }
            let n = (avail as usize).min(want - done);
            self.dev.read_at(self.cluster_offset(c) + within, &mut buf[done..done + n])?;
            done += n; within = 0;
            if done < want {
                c = next.ok_or(FsError::Corrupt("file shorter than its size"))?;
                idx = last_idx + 1;
            }
        }
        // `last` holds the final byte just read: the next sequential read resumes here.
        *self.cursor.lock().unwrap() = (first, last_idx, last);
        Ok(done)
    }
}
