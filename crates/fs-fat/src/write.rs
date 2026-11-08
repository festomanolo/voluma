//! FAT mutation. Crash-ordering policy (documented in docs/): data first, then
//! FAT links, then the directory entry last, so an interruption leaks clusters
//! (recoverable by fsck) but never exposes a half-written file. FAT32 FSInfo is
//! marked "unknown" after changes so stale free-counts can't mislead anyone.

use super::*;
use fs_core::validate_fat_name;
use std::collections::HashSet;
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

fn eoc(kind: FatKind) -> u32 { match kind { FatKind::Fat12 => 0xFFF, FatKind::Fat16 => 0xFFFF, FatKind::Fat32 => 0x0FFF_FFFF } }

impl FatFs {
    // ---------- guards ----------
    fn require_writable(&self) -> Result<()> {
        if self.dev.is_read_only() { return Err(FsError::Refused("volume opened read-only")); }
        if self.is_dirty()? { return Err(FsError::Refused("volume was not cleanly unmounted; check it with the OS repair tool first")); }
        Ok(())
    }

    /// FAT16/32 keep a "clean shutdown" bit in FAT[1]. FAT12 has none.
    fn is_dirty(&self) -> Result<bool> {
        Ok(match self.kind {
            FatKind::Fat12 => false,
            FatKind::Fat16 => { let mut b = [0u8; 2]; self.dev.read_at(self.fat_start + 2, &mut b)?; u16::from_le_bytes(b) & 0x8000 == 0 }
            FatKind::Fat32 => { let mut b = [0u8; 4]; self.dev.read_at(self.fat_start + 4, &mut b)?; u32::from_le_bytes(b) & 0x0800_0000 == 0 }
        })
    }

    // ---------- FAT table ----------
    fn fat_raw(&self, n: u32) -> Result<u32> {
        let n = n as u64;
        Ok(match self.kind {
            FatKind::Fat12 => { let mut b = [0u8; 2]; self.dev.read_at(self.fat_start + n + n / 2, &mut b)?; let w = u16::from_le_bytes(b) as u32; if n & 1 == 1 { w >> 4 } else { w & 0xFFF } }
            FatKind::Fat16 => { let mut b = [0u8; 2]; self.dev.read_at(self.fat_start + n * 2, &mut b)?; u16::from_le_bytes(b) as u32 }
            FatKind::Fat32 => { let mut b = [0u8; 4]; self.dev.read_at(self.fat_start + n * 4, &mut b)?; u32::from_le_bytes(b) & 0x0FFF_FFFF }
        })
    }

    fn set_fat(&self, n: u32, v: u32) -> Result<()> {
        if n < 2 || n >= self.cluster_count + 2 { return Err(FsError::Corrupt("cluster out of range")); }
        for i in 0..self.nfats {
            let base = self.fat_start + i * self.fat_bytes;
            let nn = n as u64;
            match self.kind {
                FatKind::Fat12 => {
                    let at = base + nn + nn / 2;
                    let mut b = [0u8; 2];
                    self.dev.read_at(at, &mut b)?;
                    let w = u16::from_le_bytes(b);
                    let w = if nn & 1 == 1 { (w & 0x000F) | ((v as u16 & 0xFFF) << 4) } else { (w & 0xF000) | (v as u16 & 0xFFF) };
                    self.dev.write_at(at, &w.to_le_bytes())?;
                }
                FatKind::Fat16 => self.dev.write_at(base + nn * 2, &(v as u16).to_le_bytes())?,
                FatKind::Fat32 => {
                    let mut b = [0u8; 4];
                    self.dev.read_at(base + nn * 4, &mut b)?;
                    let nv = (u32::from_le_bytes(b) & 0xF000_0000) | (v & 0x0FFF_FFFF);
                    self.dev.write_at(base + nn * 4, &nv.to_le_bytes())?;
                }
            }
        }
        Ok(())
    }

    /// Allocates one cluster, marks it end-of-chain, optionally links `prev -> new`.
    fn alloc_cluster(&self, hint: &mut u32, prev: Option<u32>) -> Result<u32> {
        let total = self.cluster_count;
        let start = if *hint >= 2 && *hint < total + 2 { *hint } else { 2 };
        let mut c = start;
        for _ in 0..total {
            if self.fat_raw(c)? == 0 {
                self.set_fat(c, eoc(self.kind))?;
                if let Some(p) = prev { self.set_fat(p, c)?; }
                *hint = if c + 1 < total + 2 { c + 1 } else { 2 };
                return Ok(c);
            }
            c = if c + 1 < total + 2 { c + 1 } else { 2 };
        }
        Err(FsError::NoSpace)
    }

    fn free_chain(&self, first: u32) -> Result<()> {
        let mut c = first;
        for _ in 0..=self.cluster_count {
            let next = self.next_cluster(c)?;
            self.set_fat(c, 0)?;
            match next { Some(n) => c = n, None => return Ok(()) }
        }
        Err(FsError::Corrupt("cluster chain loop"))
    }

    fn zero_cluster(&self, c: u32) -> Result<()> {
        self.dev.write_at(self.cluster_offset(c), &vec![0u8; self.cluster_size() as usize])?;
        Ok(())
    }

    fn touch_fsinfo(&self) -> Result<()> {
        if self.fsinfo_sector == 0 { return Ok(()); }
        let base = self.fsinfo_sector * self.bps;
        let mut sig = [0u8; 4];
        let mut sig2 = [0u8; 4];
        self.dev.read_at(base, &mut sig)?;
        self.dev.read_at(base + 484, &mut sig2)?;
        if sig == 0x4161_5252u32.to_le_bytes() && sig2 == 0x6141_7272u32.to_le_bytes() {
            self.dev.write_at(base + 488, &[0xFF; 8])?; // free count + next free: unknown
        }
        Ok(())
    }

    // ---------- directory plumbing ----------
    /// (device offset, length) for each contiguous piece of the directory.
    fn dir_extents(&self, loc: DirLoc) -> Result<Vec<(u64, usize)>> {
        match loc {
            DirLoc::Root if self.kind != FatKind::Fat32 => Ok(vec![(self.root_start, self.root_size as usize)]),
            _ => {
                let first = match loc { DirLoc::Root => self.root_cluster, DirLoc::Cluster(c) => c };
                let mut v = Vec::new();
                let mut c = first;
                for _ in 0..=self.cluster_count {
                    v.push((self.cluster_offset(c), self.cluster_size() as usize));
                    match self.next_cluster(c)? { Some(n) => c = n, None => return Ok(v) }
                }
                Err(FsError::Corrupt("cluster chain loop"))
            }
        }
    }

    fn slot_offset(ext: &[(u64, usize)], slot: usize) -> Option<u64> {
        let mut rem = slot * 32;
        for &(off, len) in ext { if rem < len { return Some(off + rem as u64); } rem -= len; }
        None
    }

    fn last_cluster(&self, first: u32) -> Result<u32> {
        let mut c = first;
        for _ in 0..=self.cluster_count { match self.next_cluster(c)? { Some(n) => c = n, None => return Ok(c) } }
        Err(FsError::Corrupt("cluster chain loop"))
    }

    fn resolve_dir(&self, comps: &[&str]) -> Result<DirLoc> {
        let mut loc = DirLoc::Root;
        for c in comps {
            let ent = self.parse_dir(&self.dir_bytes(loc)?).into_iter()
                .find(|e| e.name.to_lowercase() == c.to_lowercase())
                .ok_or_else(|| FsError::NotFound((*c).into()))?;
            if ent.kind != EntryKind::Directory { return Err(FsError::NotADirectory((*c).into())); }
            loc = self.dir_loc(&ent);
        }
        Ok(loc)
    }

    /// Splits "/a/b/c" into (dir loc of /a/b, "c").
    fn split_parent<'a>(&self, comps: &'a [&'a str]) -> Result<(DirLoc, &'a str)> {
        let (name, parents) = comps.split_last().ok_or_else(|| FsError::InvalidName("root has no name".into()))?;
        Ok((self.resolve_dir(parents)?, name))
    }

    fn find_located(&self, loc: DirLoc, name: &str) -> Result<Option<Located>> {
        Ok(self.parse_dir_located(&self.dir_bytes(loc)?).into_iter()
            .find(|l| l.entry.name.to_lowercase() == name.to_lowercase()))
    }

    // ---------- names ----------
    fn short_name_for(&self, name: &str, taken: &HashSet<[u8; 11]>) -> Result<([u8; 11], bool)> {
        const OK: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!#$%&'()-@^_`{}~";
        let (base, ext) = match name.rfind('.') { Some(i) if i > 0 => (&name[..i], &name[i + 1..]), _ => (name, "") };
        let exact = !base.is_empty() && base.len() <= 8 && ext.len() <= 3 && !base.contains('.')
            && base.chars().chain(ext.chars()).all(|c| OK.contains(c));
        let pack = |b: &str, e: &str| { let mut o = [b' '; 11]; o[..b.len()].copy_from_slice(b.as_bytes()); o[8..8 + e.len()].copy_from_slice(e.as_bytes()); o };
        if exact { return Ok((pack(base, ext), false)); }
        let clean = |s: &str| -> String { s.to_uppercase().chars().filter(|c| *c != ' ' && *c != '.')
            .map(|c| if OK.contains(c) { c } else { '_' }).collect() };
        let mut b = clean(base);
        if b.is_empty() { b = "_".into(); }
        let e: String = clean(ext).chars().take(3).collect();
        for n in 1..1_000_000u32 {
            let tail = format!("~{n}");
            let keep = 8 - tail.len();
            let cand: String = b.chars().take(keep).collect::<String>() + &tail;
            let packed = pack(&cand, &e);
            if !taken.contains(&packed) { return Ok((packed, true)); }
        }
        Err(FsError::NoSpace)
    }

    fn build_entries(name: &str, short: [u8; 11], lfn: bool, mut base: [u8; 32]) -> Vec<[u8; 32]> {
        base[0..11].copy_from_slice(&short);
        base[12] = 0; // no NT lowercase flags; case lives in the LFN
        let mut out = Vec::new();
        if lfn {
            let units: Vec<u16> = name.encode_utf16().collect();
            let n = units.len().div_ceil(13);
            let sum = short_checksum(&short);
            for i in (1..=n).rev() {
                let mut e = [0u8; 32];
                e[0] = i as u8 | if i == n { 0x40 } else { 0 };
                e[11] = 0x0F; e[13] = sum;
                for (k, o) in [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30].iter().enumerate() {
                    let idx = (i - 1) * 13 + k;
                    let u = if idx < units.len() { units[idx] } else if idx == units.len() { 0 } else { 0xFFFF };
                    e[*o..*o + 2].copy_from_slice(&u.to_le_bytes());
                }
                out.push(e);
            }
        }
        out.push(base);
        out
    }

    fn now() -> (u16, u16, u8) {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let days = secs.div_euclid(86400);
        let rem = secs.rem_euclid(86400);
        // civil from days (Hinnant)
        let z = days + 719468;
        let era = z.div_euclid(146097);
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        let year = (y - 1980).clamp(0, 127) as u16;
        let date = (year << 9) | ((m as u16) << 5) | d as u16;
        let time = (((rem / 3600) as u16) << 11) | ((((rem % 3600) / 60) as u16) << 5) | ((rem % 60 / 2) as u16);
        (date, time, 0)
    }

    fn base_entry(&self, attr: u8, first: u32, size: u32) -> [u8; 32] {
        let (d, t, tenths) = Self::now();
        let mut e = [0u8; 32];
        e[11] = attr; e[13] = tenths;
        e[14..16].copy_from_slice(&t.to_le_bytes()); e[16..18].copy_from_slice(&d.to_le_bytes());
        e[18..20].copy_from_slice(&d.to_le_bytes());
        e[20..22].copy_from_slice(&((first >> 16) as u16).to_le_bytes());
        e[22..24].copy_from_slice(&t.to_le_bytes()); e[24..26].copy_from_slice(&d.to_le_bytes());
        e[26..28].copy_from_slice(&(first as u16).to_le_bytes());
        e[28..32].copy_from_slice(&size.to_le_bytes());
        e
    }

    /// Inserts a directory-entry set into `loc`; grows the directory if allowed.
    fn insert_entry(&self, loc: DirLoc, name: &str, base: [u8; 32], hint: &mut u32) -> Result<()> {
        validate_fat_name(name)?;
        let raw = self.dir_bytes(loc)?;
        let located = self.parse_dir_located(&raw);
        if located.iter().any(|l| l.entry.name.to_lowercase() == name.to_lowercase()) {
            return Err(FsError::AlreadyExists(name.into()));
        }
        let taken: HashSet<[u8; 11]> = located.iter().map(|l| l.short).collect();
        let (short, lfn) = self.short_name_for(name, &taken)?;
        let entries = Self::build_entries(name, short, lfn, base);
        let need = entries.len();

        loop {
            let raw = self.dir_bytes(loc)?;
            let slots = raw.len() / 32;
            let free = |i: usize| raw[i * 32] == 0xE5 || raw[i * 32] == 0;
            let mut run = 0usize;
            let mut start = None;
            for i in 0..slots {
                if free(i) { run += 1; if run == need { start = Some(i + 1 - need); break; } } else { run = 0; }
                // A 0x00 entry means everything after it is free too.
                if raw[i * 32] == 0 { start = Some(i + 1 - run); if slots - start.unwrap() >= need { break; } else { start = None; break; } }
            }
            if let Some(s) = start {
                let ext = self.dir_extents(loc)?;
                for (k, e) in entries.iter().enumerate() {
                    let off = Self::slot_offset(&ext, s + k).ok_or(FsError::Corrupt("slot out of range"))?;
                    self.dev.write_at(off, e)?;
                }
                return Ok(());
            }
            // Need more room: only chained directories can grow.
            let first = match loc {
                DirLoc::Root if self.kind != FatKind::Fat32 => return Err(FsError::NoSpace),
                DirLoc::Root => self.root_cluster,
                DirLoc::Cluster(c) => c,
            };
            if raw.len() + self.cluster_size() as usize > MAX_DIR_BYTES { return Err(FsError::NoSpace); }
            let last = self.last_cluster(first)?;
            let new = self.alloc_cluster(hint, Some(last))?;
            self.zero_cluster(new)?;
        }
    }

    // ---------- public operations (called from the trait impl) ----------
    pub(crate) fn w_create_dir(&self, path: &str) -> Result<DirEntry> {
        self.require_writable()?;
        let comps = normalize_path(path)?;
        let mut hint = self.wlock.lock().unwrap();
        let (parent, name) = self.split_parent(&comps)?;
        validate_fat_name(name)?;
        if self.find_located(parent, name)?.is_some() { return Err(FsError::AlreadyExists(path.into())); }
        let c = self.alloc_cluster(&mut hint, None)?;
        let res = (|| {
            self.zero_cluster(c)?;
            let parent_c = match parent { DirLoc::Root => 0, DirLoc::Cluster(p) => p };
            let dot = Self::build_dot(b".          ", c, &self.base_entry(0x10, c, 0));
            let dotdot = Self::build_dot(b"..         ", parent_c, &self.base_entry(0x10, parent_c, 0));
            self.dev.write_at(self.cluster_offset(c), &dot)?;
            self.dev.write_at(self.cluster_offset(c) + 32, &dotdot)?;
            self.insert_entry(parent, name, self.base_entry(0x10, c, 0), &mut hint)
        })();
        if let Err(e) = res { let _ = self.free_chain(c); return Err(e); }
        self.touch_fsinfo()?;
        drop(hint);
        self.stat(path)
    }

    fn build_dot(name: &[u8; 11], cluster: u32, base: &[u8; 32]) -> [u8; 32] {
        let mut e = *base;
        e[0..11].copy_from_slice(name);
        e[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
        e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
        e
    }

    pub(crate) fn w_write_file(&self, path: &str, src: &mut dyn Read, overwrite: bool) -> Result<DirEntry> {
        self.require_writable()?;
        let comps = normalize_path(path)?;
        let mut hint = self.wlock.lock().unwrap();
        let (parent, name) = self.split_parent(&comps)?;
        validate_fat_name(name)?;
        let existing = self.find_located(parent, name)?;
        if let Some(l) = &existing {
            if l.entry.kind == EntryKind::Directory { return Err(FsError::IsADirectory(path.into())); }
            if !overwrite { return Err(FsError::AlreadyExists(path.into())); }
        }
        // 1. Stream data into freshly allocated clusters.
        let cs = self.cluster_size() as usize;
        let mut buf = vec![0u8; cs];
        let (mut first, mut last, mut total) = (0u32, 0u32, 0u64);
        let stream: Result<()> = (|| loop {
            let mut n = 0;
            while n < cs {
                let k = src.read(&mut buf[n..]).map_err(|e| FsError::Source(e.to_string()))?;
                if k == 0 { break; }
                n += k;
            }
            if n == 0 { return Ok(()); }
            if total + n as u64 > u32::MAX as u64 { return Err(FsError::NoSpace); }
            buf[n..].fill(0);
            let c = self.alloc_cluster(&mut hint, if last != 0 { Some(last) } else { None })?;
            if first == 0 { first = c; }
            last = c;
            self.dev.write_at(self.cluster_offset(c), &buf)?;
            total += n as u64;
            if n < cs { return Ok(()); }
        })();
        if let Err(e) = stream { if first != 0 { let _ = self.free_chain(first); } return Err(e); }

        // 2. Publish: directory entry last.
        let res = match existing {
            None => self.insert_entry(parent, name, self.base_entry(0x20, first, total as u32), &mut hint),
            Some(l) => (|| {
                let ext = self.dir_extents(parent)?;
                let off = Self::slot_offset(&ext, l.short_slot).ok_or(FsError::Corrupt("slot out of range"))?;
                let mut e = [0u8; 32];
                self.dev.read_at(off, &mut e)?;
                let (d, t, _) = Self::now();
                e[20..22].copy_from_slice(&((first >> 16) as u16).to_le_bytes());
                e[26..28].copy_from_slice(&(first as u16).to_le_bytes());
                e[28..32].copy_from_slice(&(total as u32).to_le_bytes());
                e[22..24].copy_from_slice(&t.to_le_bytes()); e[24..26].copy_from_slice(&d.to_le_bytes());
                self.dev.write_at(off, &e)?;
                if l.entry.inode >= 2 { self.free_chain(l.entry.inode as u32)?; }
                Ok(())
            })(),
        };
        if let Err(e) = res { if first != 0 { let _ = self.free_chain(first); } return Err(e); }
        self.touch_fsinfo()?;
        drop(hint);
        self.stat(path)
    }

    pub(crate) fn w_remove(&self, path: &str) -> Result<()> {
        self.require_writable()?;
        let comps = normalize_path(path)?;
        let _g = self.wlock.lock().unwrap();
        let (parent, name) = self.split_parent(&comps)?;
        let l = self.find_located(parent, name)?.ok_or_else(|| FsError::NotFound(path.into()))?;
        if l.entry.kind == EntryKind::Directory && !self.parse_dir(&self.dir_bytes(self.dir_loc(&l.entry))?).is_empty() {
            return Err(FsError::NotEmpty(path.into()));
        }
        let ext = self.dir_extents(parent)?;
        // Delete the entry set first, then release the clusters.
        for slot in l.first_slot..=l.short_slot {
            let off = Self::slot_offset(&ext, slot).ok_or(FsError::Corrupt("slot out of range"))?;
            self.dev.write_at(off, &[0xE5])?;
        }
        if l.entry.inode >= 2 { self.free_chain(l.entry.inode as u32)?; }
        self.touch_fsinfo()
    }

    pub(crate) fn w_rename(&self, from: &str, to: &str) -> Result<()> {
        self.require_writable()?;
        let f = normalize_path(from)?;
        let t = normalize_path(to)?;
        let mut hint = self.wlock.lock().unwrap();
        let (src_parent, src_name) = self.split_parent(&f)?;
        let l = self.find_located(src_parent, src_name)?.ok_or_else(|| FsError::NotFound(from.into()))?;
        let lower = |v: &[&str]| v.iter().map(|s| s.to_lowercase()).collect::<Vec<_>>();
        if l.entry.kind == EntryKind::Directory && lower(&t).starts_with(&lower(&f)) {
            return Err(FsError::InvalidName("cannot move a directory into itself".into()));
        }
        let (dst_parent, dst_name) = self.split_parent(&t)?;
        validate_fat_name(dst_name)?;
        let same_entry = dst_parent == src_parent && dst_name.to_lowercase() == src_name.to_lowercase();
        if !same_entry && self.find_located(dst_parent, dst_name)?.is_some() { return Err(FsError::AlreadyExists(to.into())); }

        let src_ext = self.dir_extents(src_parent)?;
        let mut raw = [0u8; 32];
        self.dev.read_at(Self::slot_offset(&src_ext, l.short_slot).ok_or(FsError::Corrupt("slot"))?, &mut raw)?;
        let erase = |this: &Self| -> Result<()> {
            for slot in l.first_slot..=l.short_slot {
                this.dev.write_at(Self::slot_offset(&src_ext, slot).ok_or(FsError::Corrupt("slot"))?, &[0xE5])?;
            }
            Ok(())
        };
        if same_entry { erase(self)?; self.insert_entry(dst_parent, dst_name, raw, &mut hint)?; }
        else { self.insert_entry(dst_parent, dst_name, raw, &mut hint)?; erase(self)?; }

        if l.entry.kind == EntryKind::Directory && dst_parent != src_parent {
            let new_parent = match dst_parent { DirLoc::Root => 0, DirLoc::Cluster(p) => p };
            let at = self.cluster_offset(l.entry.inode as u32) + 32;
            self.dev.write_at(at + 20, &((new_parent >> 16) as u16).to_le_bytes())?;
            self.dev.write_at(at + 26, &(new_parent as u16).to_le_bytes())?;
        }
        self.touch_fsinfo()
    }
}
