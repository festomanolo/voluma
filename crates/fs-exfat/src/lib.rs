//! exFAT driver (read + write). Writes require a read-write device and a volume whose
//! VolumeDirty / MediaFailure flags are clear. All on-disk values are validated.
//!
//! Policy notes: new data always uses FAT chains (never NoFatChain); existing NoFatChain
//! files/directories are read natively and converted to chains only when a directory must grow.
//! Order of operations: data -> allocation bitmap/FAT -> directory entry set last.

use block_device::BlockDevice;
use fs_core::{normalize_path, validate_fat_name, Capabilities, DirEntry, EntryKind, Filesystem, FsError, Result};
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_DIR_BYTES: usize = 32 << 20;
const EOC: u32 = 0xFFFF_FFFF;

pub struct ExFatFs {
    dev: Arc<dyn BlockDevice>,
    cs: u64,
    fat_start: u64,
    heap_start: u64,
    cluster_count: u32,
    root_cluster: u32,
    flags_off: u64,
    bitmap_off: u64,
    bitmap_len: u64,
    upcase: Vec<u16>,
    label: Option<String>,
    wlock: Mutex<u32>,
}

#[derive(Clone, Copy, Debug)]
struct Chain { first: u32, nofat: bool, len: u64 }

#[derive(Clone, Debug)]
struct DirH { chain: Chain, set: Option<Vec<u64>> }

struct Located {
    entry: DirEntry,
    chain: Chain,
    offs: Vec<u64>,
}

fn le16(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o + 1]]) }
fn le32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) }
fn le64(b: &[u8], o: usize) -> u64 { u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) }

fn set_checksum(entries: &[[u8; 32]]) -> u16 {
    let mut sum: u16 = 0;
    for (i, e) in entries.iter().enumerate() {
        for (j, &b) in e.iter().enumerate() {
            if i == 0 && (j == 2 || j == 3) { continue; }
            sum = ((sum << 15) | (sum >> 1)).wrapping_add(b as u16);
        }
    }
    sum
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
}

fn ex_time(ts: u32, tenms: u8, utc: u8) -> Option<i64> {
    if ts == 0 { return None; }
    let (y, mo, d) = (1980 + (ts >> 25) as i64, ((ts >> 21) & 0xF) as i64, ((ts >> 16) & 0x1F) as i64);
    let (h, mi, s) = (((ts >> 11) & 0x1F) as i64, ((ts >> 5) & 0x3F) as i64, ((ts & 0x1F) * 2) as i64);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 59 { return None; }
    let mut t = days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + s + (tenms as i64) / 100;
    if utc & 0x80 != 0 {
        let off = ((utc & 0x7F) as i8) << 1 >> 1; // sign-extend 7 bits
        t -= off as i64 * 15 * 60;
    }
    Some(t)
}

/// Current time as (timestamp, 10ms-increment, utc-offset byte = valid, +00:00).
fn now_ts() -> (u32, u8, u8) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    let ts = (((y - 1980).clamp(0, 127) as u32) << 25) | ((m as u32) << 21) | ((d as u32) << 16)
        | (((rem / 3600) as u32) << 11) | ((((rem % 3600) / 60) as u32) << 5) | ((rem % 60 / 2) as u32);
    (ts, 0, 0x80)
}

impl ExFatFs {
    pub fn open(dev: Arc<dyn BlockDevice>) -> Result<Self> {
        let mut s = [0u8; 512];
        dev.read_at(0, &mut s)?;
        if &s[3..11] != b"EXFAT   " || s[510] != 0x55 || s[511] != 0xAA || s[11..64].iter().any(|&b| b != 0) {
            return Err(FsError::Corrupt("not an exFAT boot sector"));
        }
        let (bsh, ssh) = (s[108] as u32, s[109] as u32);
        if !(9..=12).contains(&bsh) || ssh > 25 - bsh { return Err(FsError::Corrupt("invalid sector/cluster shift")); }
        let bps = 1u64 << bsh;
        let cs = bps << ssh;
        let vol_len = le64(&s, 72) * bps;
        let (fat_off, fat_len) = (le32(&s, 80) as u64, le32(&s, 84) as u64);
        let (heap_off, ccount, root) = (le32(&s, 88) as u64, le32(&s, 92), le32(&s, 96));
        if s[110] != 1 { return Err(FsError::Unsupported("TexFAT / multiple FATs")); }
        if vol_len == 0 || vol_len > dev.size() || ccount == 0 || ccount > 0xFFFF_FFF5
            || fat_off < 24 || fat_len == 0 || heap_off < fat_off + fat_len
            || (heap_off + ccount as u64 * (1 << ssh)) * bps > vol_len
            || fat_len * bps < (ccount as u64 + 2) * 4 || root < 2 || root >= ccount + 2 {
            return Err(FsError::Corrupt("inconsistent exFAT geometry"));
        }
        let mut fs = Self {
            dev, cs, fat_start: fat_off * bps, heap_start: heap_off * bps, cluster_count: ccount,
            root_cluster: root, flags_off: 106, bitmap_off: 0, bitmap_len: 0, upcase: Vec::new(), label: None,
            wlock: Mutex::new(2),
        };
        let root_h = DirH { chain: Chain { first: root, nofat: false, len: 0 }, set: None };
        let raw = fs.dir_raw(&root_h.chain)?.0;
        let (mut upc, mut have_bitmap) = (None, false);
        for e in raw.chunks_exact(32) {
            match e[0] {
                0x00 => break,
                0x81 if !have_bitmap && e[1] & 1 == 0 => {
                    let (c, len) = (le32(e, 20), le64(e, 24));
                    if c < 2 || c >= ccount + 2 || len < (ccount as u64).div_ceil(8)
                        || (c as u64 - 2) * cs + len > ccount as u64 * cs { return Err(FsError::Corrupt("bad allocation bitmap")); }
                    fs.bitmap_off = fs.cluster_offset(c); fs.bitmap_len = len; have_bitmap = true;
                }
                0x82 => upc = Some((le32(e, 20), le64(e, 24))),
                0x83 => {
                    let n = (e[1] as usize).min(11);
                    let u: Vec<u16> = (0..n).map(|i| le16(e, 2 + i * 2)).collect();
                    let l = String::from_utf16_lossy(&u);
                    if !l.is_empty() { fs.label = Some(l); }
                }
                _ => {}
            }
        }
        if !have_bitmap { return Err(FsError::Corrupt("allocation bitmap missing")); }
        fs.upcase = fs.load_upcase(upc);
        Ok(fs)
    }

    pub fn cluster_size(&self) -> u64 { self.cs }

    fn load_upcase(&self, loc: Option<(u32, u64)>) -> Vec<u16> {
        let mut t: Vec<u16> = (0..=0xFFFFu32).map(|c| c as u16).collect();
        // Fallback (no/invalid table): ASCII only.
        for c in b'a'..=b'z' { t[c as usize] = c as u16 - 32; }
        let Some((c, len)) = loc else { return t };
        if c < 2 || c >= self.cluster_count + 2 || len == 0 || len > 131_072 || len % 2 != 0
            || (c as u64 - 2) * self.cs + len > self.cluster_count as u64 * self.cs { return t; }
        let mut raw = vec![0u8; len as usize];
        if self.dev.read_at(self.cluster_offset(c), &mut raw).is_err() { return t; }
        for (i, x) in t.iter_mut().enumerate() { *x = i as u16; }
        let (mut ch, mut i) = (0usize, 0usize);
        while i + 1 < raw.len() && ch <= 0xFFFF {
            let v = le16(&raw, i); i += 2;
            if v == 0xFFFF && i + 2 <= raw.len() { ch += le16(&raw, i) as usize; i += 2; }
            else { t[ch] = v; ch += 1; }
        }
        t
    }

    fn up(&self, c: u16) -> u16 { self.upcase[c as usize] }
    fn name_eq(&self, a: &str, b: &str) -> bool {
        let (x, y): (Vec<u16>, Vec<u16>) = (a.encode_utf16().collect(), b.encode_utf16().collect());
        x.len() == y.len() && x.iter().zip(&y).all(|(&p, &q)| self.up(p) == self.up(q))
    }
    fn name_hash(&self, name: &str) -> u16 {
        let mut h: u16 = 0;
        for u in name.encode_utf16() {
            for b in self.up(u).to_le_bytes() { h = ((h << 15) | (h >> 1)).wrapping_add(b as u16); }
        }
        h
    }

    fn cluster_offset(&self, c: u32) -> u64 { self.heap_start + (c as u64 - 2) * self.cs }
    fn in_range(&self, c: u32) -> bool { c >= 2 && c < self.cluster_count + 2 }

    fn fat_get(&self, c: u32) -> Result<u32> {
        if !self.in_range(c) { return Err(FsError::Corrupt("cluster out of range")); }
        let mut b = [0u8; 4];
        self.dev.read_at(self.fat_start + c as u64 * 4, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn next(&self, c: u32) -> Result<Option<u32>> {
        let v = self.fat_get(c)?;
        if v >= 0xFFFF_FFF8 { Ok(None) } else if self.in_range(v) { Ok(Some(v)) } else { Err(FsError::Corrupt("broken cluster chain")) }
    }

    fn clusters(&self, ch: &Chain) -> Result<Vec<u32>> {
        if ch.first == 0 { return Ok(vec![]); }
        if !self.in_range(ch.first) { return Err(FsError::Corrupt("first cluster out of range")); }
        if ch.nofat {
            let n = ch.len.div_ceil(self.cs);
            if ch.first as u64 + n > self.cluster_count as u64 + 2 { return Err(FsError::Corrupt("contiguous run exceeds volume")); }
            return Ok((0..n as u32).map(|i| ch.first + i).collect());
        }
        let mut v = Vec::new();
        let mut c = ch.first;
        for _ in 0..=self.cluster_count {
            v.push(c);
            match self.next(c)? { Some(n) => c = n, None => return Ok(v) }
        }
        Err(FsError::Corrupt("cluster chain loop"))
    }

    fn nth_cluster(&self, ch: &Chain, n: u64) -> Result<Option<u32>> {
        if ch.first == 0 { return Ok(None); }
        if ch.nofat {
            let c = ch.first as u64 + n;
            return Ok(if c < self.cluster_count as u64 + 2 { Some(c as u32) } else { None });
        }
        let mut c = ch.first;
        for _ in 0..n { match self.next(c)? { Some(x) => c = x, None => return Ok(None) } }
        Ok(Some(c))
    }

    /// Raw directory bytes plus the cluster list they came from.
    fn dir_raw(&self, ch: &Chain) -> Result<(Vec<u8>, Vec<u32>)> {
        let cl = self.clusters(ch)?;
        if cl.len() as u64 * self.cs > MAX_DIR_BYTES as u64 { return Err(FsError::Corrupt("directory too large")); }
        let mut raw = vec![0u8; cl.len() * self.cs as usize];
        for (i, &c) in cl.iter().enumerate() {
            self.dev.read_at(self.cluster_offset(c), &mut raw[i * self.cs as usize..(i + 1) * self.cs as usize])?;
        }
        Ok((raw, cl))
    }

    fn slot_off(&self, cl: &[u32], slot: usize) -> u64 {
        let byte = slot as u64 * 32;
        self.cluster_offset(cl[(byte / self.cs) as usize]) + byte % self.cs
    }

    fn list(&self, dh: &DirH) -> Result<Vec<Located>> {
        let (raw, cl) = self.dir_raw(&dh.chain)?;
        let slots = raw.len() / 32;
        let mut out = Vec::new();
        let mut i = 0;
        while i < slots {
            let e = &raw[i * 32..i * 32 + 32];
            if e[0] == 0 { break; }
            if e[0] != 0x85 { i += 1; continue; }
            let n = e[1] as usize;
            if !(2..=18).contains(&n) || i + 1 + n > slots { i += 1; continue; }
            let set: Vec<[u8; 32]> = (0..=n).map(|k| raw[(i + k) * 32..(i + k) * 32 + 32].try_into().unwrap()).collect();
            if set_checksum(&set) != le16(&set[0], 2) || set[1][0] != 0xC0 { i += 1 + n; continue; }
            let st = &set[1];
            let name_len = st[3] as usize;
            let mut units = Vec::new();
            let mut ok = name_len > 0 && set[2..].iter().all(|x| x[0] == 0xC1) && n - 1 >= name_len.div_ceil(15);
            if ok {
                for x in &set[2..] { for k in 0..15 { units.push(le16(x, 2 + k * 2)); } }
                units.truncate(name_len);
                ok = units.len() == name_len;
            }
            if !ok { i += 1 + n; continue; }
            let attr = le16(&set[0], 4);
            let (data_len, valid) = (le64(st, 24), le64(st, 8));
            let is_dir = attr & 0x10 != 0;
            let nofat = st[1] & 0x02 != 0;
            let first = le32(st, 20);
            let size = if is_dir { 0 } else { valid.min(data_len) };
            out.push(Located {
                entry: DirEntry {
                    name: String::from_utf16_lossy(&units),
                    kind: if is_dir { EntryKind::Directory } else { EntryKind::File },
                    size,
                    modified: ex_time(le32(&set[0], 12), set[0][21], set[0][23]),
                    created: ex_time(le32(&set[0], 8), set[0][20], set[0][22]),
                    hidden: attr & 0x02 != 0,
                    read_only: attr & 0x01 != 0,
                    inode: first as u64 | if nofat { 1 << 32 } else { 0 },
                },
                chain: Chain { first, nofat, len: data_len },
                offs: (0..=n).map(|k| self.slot_off(&cl, i + k)).collect(),
            });
            i += 1 + n;
        }
        Ok(out)
    }

    fn root_h(&self) -> DirH { DirH { chain: Chain { first: self.root_cluster, nofat: false, len: 0 }, set: None } }

    fn dir_of(&self, l: &Located) -> Result<DirH> {
        if l.entry.kind != EntryKind::Directory { return Err(FsError::NotADirectory(l.entry.name.clone())); }
        Ok(DirH { chain: l.chain, set: Some(l.offs.clone()) })
    }

    fn walk(&self, comps: &[&str]) -> Result<DirH> {
        let mut h = self.root_h();
        for c in comps {
            let l = self.list(&h)?.into_iter().find(|l| self.name_eq(&l.entry.name, c)).ok_or_else(|| FsError::NotFound((*c).into()))?;
            h = self.dir_of(&l)?;
        }
        Ok(h)
    }

    fn find(&self, dh: &DirH, name: &str) -> Result<Option<Located>> {
        Ok(self.list(dh)?.into_iter().find(|l| self.name_eq(&l.entry.name, name)))
    }
}

impl Filesystem for ExFatFs {
    fn name(&self) -> &'static str { "exFAT" }
    fn label(&self) -> Option<String> { self.label.clone() }
    fn capabilities(&self) -> Capabilities {
        if self.dev.is_read_only() { return Capabilities::READ_ONLY; }
        Capabilities { read: true, write: true, create: true, delete: true, rename: true,
            permissions: false, symbolic_links: false, extended_attributes: false }
    }

    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        let h = self.walk(&normalize_path(path)?)?;
        Ok(self.list(&h)?.into_iter().map(|l| l.entry).collect())
    }

    fn stat(&self, path: &str) -> Result<DirEntry> {
        let comps = normalize_path(path)?;
        let Some((last, parents)) = comps.split_last() else {
            return Ok(DirEntry { name: "/".into(), kind: EntryKind::Directory, size: 0, modified: None, created: None, hidden: false, read_only: false, inode: 0 });
        };
        let h = self.walk(parents)?;
        self.find(&h, last)?.map(|l| l.entry).ok_or_else(|| FsError::NotFound(path.into()))
    }

    fn read_at(&self, entry: &DirEntry, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if entry.kind == EntryKind::Directory { return Err(FsError::IsADirectory(entry.name.clone())); }
        if offset >= entry.size || buf.is_empty() { return Ok(0); }
        let want = (buf.len() as u64).min(entry.size - offset) as usize;
        let first = entry.inode as u32;
        let ch = Chain { first, nofat: entry.inode >> 32 & 1 == 1, len: entry.size };
        if !self.in_range(first) { return Err(FsError::Corrupt("file start out of range")); }
        let mut c = self.nth_cluster(&ch, offset / self.cs)?.ok_or(FsError::Corrupt("file shorter than its size"))?;
        let (mut within, mut done) = (offset % self.cs, 0usize);
        while done < want {
            let n = ((self.cs - within) as usize).min(want - done);
            if !self.in_range(c) { return Err(FsError::Corrupt("cluster out of range")); }
            self.dev.read_at(self.cluster_offset(c) + within, &mut buf[done..done + n])?;
            done += n; within = 0;
            if done < want {
                c = if ch.nofat { c + 1 } else { self.next(c)?.ok_or(FsError::Corrupt("file shorter than its size"))? };
            }
        }
        Ok(done)
    }

    fn create_dir(&self, path: &str) -> Result<DirEntry> { self.w_create_dir(path) }
    fn write_file(&self, path: &str, src: &mut dyn Read, overwrite: bool) -> Result<DirEntry> { self.w_write_file(path, src, overwrite) }
    fn remove(&self, path: &str) -> Result<()> { self.w_remove(path) }
    fn rename(&self, from: &str, to: &str) -> Result<()> { self.w_rename(from, to) }
}

mod write;
