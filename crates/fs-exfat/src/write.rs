use super::*;

impl ExFatFs {
    // ---------- guards ----------
    fn require_writable(&self) -> Result<()> {
        if self.dev.is_read_only() { return Err(FsError::Refused("volume opened read-only")); }
        let mut f = [0u8; 2];
        self.dev.read_at(self.flags_off, &mut f)?;
        let flags = u16::from_le_bytes(f);
        if flags & 0x02 != 0 { return Err(FsError::Refused("volume is marked dirty; check it with the OS repair tool first")); }
        if flags & 0x04 != 0 { return Err(FsError::Refused("volume reports media failure")); }
        Ok(())
    }

    // ---------- bitmap / FAT ----------
    fn bit_set(&self, c: u32, v: bool) -> Result<()> {
        let i = (c - 2) as u64;
        let at = self.bitmap_off + i / 8;
        let mut b = [0u8; 1];
        self.dev.read_at(at, &mut b)?;
        if v { b[0] |= 1 << (i % 8) } else { b[0] &= !(1 << (i % 8)) }
        self.dev.write_at(at, &b)?;
        Ok(())
    }
    fn fat_put(&self, c: u32, v: u32) -> Result<()> {
        if !self.in_range(c) { return Err(FsError::Corrupt("cluster out of range")); }
        self.dev.write_at(self.fat_start + c as u64 * 4, &v.to_le_bytes())?;
        Ok(())
    }

    fn alloc_cluster(&self, hint: &mut u32, prev: Option<u32>) -> Result<u32> {
        let total = self.cluster_count;
        let mut c = if self.in_range(*hint) { *hint } else { 2 };
        let mut cache: (u64, Vec<u8>) = (u64::MAX, Vec::new());
        for _ in 0..total {
            let i = (c - 2) as u64;
            let chunk = i / 8 / 4096;
            if cache.0 != chunk {
                let start = chunk * 4096;
                let n = (self.bitmap_len - start).min(4096) as usize;
                let mut v = vec![0u8; n];
                self.dev.read_at(self.bitmap_off + start, &mut v)?;
                cache = (chunk, v);
            }
            let byte = cache.1[(i / 8 - chunk * 4096) as usize];
            if byte >> (i % 8) & 1 == 0 {
                self.bit_set(c, true)?;
                self.fat_put(c, EOC)?;
                if let Some(p) = prev { self.fat_put(p, c)?; }
                *hint = if c + 1 < total + 2 { c + 1 } else { 2 };
                return Ok(c);
            }
            c = if c + 1 < total + 2 { c + 1 } else { 2 };
        }
        Err(FsError::NoSpace)
    }

    fn release(&self, ch: &Chain) -> Result<()> {
        for c in self.clusters(ch)? {
            self.bit_set(c, false)?;
            if !ch.nofat { self.fat_put(c, 0)?; }
        }
        Ok(())
    }

    fn touch_volume(&self) -> Result<()> {
        self.dev.write_at(112, &[0xFF])?; // PercentInUse: not available (excluded from boot checksum)
        Ok(())
    }

    // ---------- entry sets ----------
    fn build_set(&self, name: &str, proto: &[[u8; 32]; 2]) -> Vec<[u8; 32]> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let names = units.len().div_ceil(15);
        let mut set = vec![proto[0], proto[1]];
        set[0][0] = 0x85; set[0][1] = (1 + names) as u8;
        set[1][0] = 0xC0; set[1][3] = units.len() as u8;
        set[1][4..6].copy_from_slice(&self.name_hash(name).to_le_bytes());
        for k in 0..names {
            let mut e = [0u8; 32];
            e[0] = 0xC1;
            for j in 0..15 {
                let u = units.get(k * 15 + j).copied().unwrap_or(0);
                e[2 + j * 2..4 + j * 2].copy_from_slice(&u.to_le_bytes());
            }
            set.push(e);
        }
        let sum = set_checksum(&set);
        set[0][2..4].copy_from_slice(&sum.to_le_bytes());
        set
    }

    fn proto(attr: u16, first: u32, len: u64) -> [[u8; 32]; 2] {
        let (ts, tenms, utc) = now_ts();
        let mut f = [0u8; 32];
        f[0] = 0x85;
        f[4..6].copy_from_slice(&attr.to_le_bytes());
        for off in [8usize, 12, 16] { f[off..off + 4].copy_from_slice(&ts.to_le_bytes()); }
        f[20] = tenms; f[21] = tenms; f[22] = utc; f[23] = utc; f[24] = utc;
        let mut s = [0u8; 32];
        s[0] = 0xC0; s[1] = 0x01;
        s[8..16].copy_from_slice(&len.to_le_bytes());
        s[20..24].copy_from_slice(&first.to_le_bytes());
        s[24..32].copy_from_slice(&len.to_le_bytes());
        [f, s]
    }

    fn rewrite_set(&self, offs: &[u64], f: impl FnOnce(&mut Vec<[u8; 32]>)) -> Result<()> {
        let mut set = Vec::new();
        for &o in offs { let mut e = [0u8; 32]; self.dev.read_at(o, &mut e)?; set.push(e); }
        f(&mut set);
        let sum = set_checksum(&set);
        set[0][2..4].copy_from_slice(&sum.to_le_bytes());
        for (o, e) in offs.iter().zip(&set) { self.dev.write_at(*o, e)?; }
        Ok(())
    }

    fn mark_deleted(&self, offs: &[u64]) -> Result<()> {
        for &o in offs {
            let mut b = [0u8; 1];
            self.dev.read_at(o, &mut b)?;
            self.dev.write_at(o, &[b[0] & 0x7F])?;
        }
        Ok(())
    }

    /// Adds one cluster to a directory (converting NoFatChain to a FAT chain first).
    fn grow_dir(&self, dh: &mut DirH, hint: &mut u32) -> Result<()> {
        let cl = self.clusters(&dh.chain)?;
        let last = *cl.last().ok_or(FsError::Corrupt("directory has no clusters"))?;
        if (cl.len() as u64 + 1) * self.cs > MAX_DIR_BYTES as u64 { return Err(FsError::NoSpace); }
        if dh.chain.nofat {
            for w in cl.windows(2) { self.fat_put(w[0], w[1])?; }
            self.fat_put(last, EOC)?;
        }
        let new = self.alloc_cluster(hint, Some(last))?;
        self.dev.write_at(self.cluster_offset(new), &vec![0u8; self.cs as usize])?;
        let add = self.cs;
        if let Some(set) = &dh.set {
            self.rewrite_set(set, |s| {
                s[1][1] = 0x01; // AllocationPossible, FAT chain in use
                let (v, l) = (le64(&s[1], 8) + add, le64(&s[1], 24) + add);
                s[1][8..16].copy_from_slice(&v.to_le_bytes());
                s[1][24..32].copy_from_slice(&l.to_le_bytes());
            })?;
        }
        dh.chain.nofat = false;
        dh.chain.len += add;
        Ok(())
    }

    fn insert_set(&self, dh: &DirH, name: &str, proto: &[[u8; 32]; 2], hint: &mut u32) -> Result<()> {
        validate_fat_name(name)?;
        if self.find(dh, name)?.is_some() { return Err(FsError::AlreadyExists(name.into())); }
        let set = self.build_set(name, proto);
        let need = set.len();
        let mut dh = dh.clone();
        loop {
            let (raw, cl) = self.dir_raw(&dh.chain)?;
            let slots = raw.len() / 32;
            let mut run = 0usize;
            let mut start = None;
            for i in 0..slots {
                if raw[i * 32] & 0x80 == 0 { run += 1; if run == need { start = Some(i + 1 - need); break; } } else { run = 0; }
                if raw[i * 32] == 0 {
                    // End marker: everything after is free.
                    if slots - (i + 1 - run) >= need { start = Some(i + 1 - run); }
                    break;
                }
            }
            if let Some(s) = start {
                // Write the secondaries first and the primary last, so a torn write never
                // exposes a primary whose set is incomplete.
                for k in (0..need).rev() { self.dev.write_at(self.slot_off(&cl, s + k), &set[k])?; }
                return Ok(());
            }
            self.grow_dir(&mut dh, hint)?;
        }
    }

    // ---------- operations ----------
    pub(crate) fn w_create_dir(&self, path: &str) -> Result<DirEntry> {
        self.require_writable()?;
        let comps = normalize_path(path)?;
        let (name, parents) = comps.split_last().ok_or_else(|| FsError::InvalidName("root".into()))?;
        let mut hint = self.wlock.lock().unwrap();
        let parent = self.walk(parents)?;
        validate_fat_name(name)?;
        if self.find(&parent, name)?.is_some() { return Err(FsError::AlreadyExists(path.into())); }
        let c = self.alloc_cluster(&mut hint, None)?;
        let res = (|| {
            self.dev.write_at(self.cluster_offset(c), &vec![0u8; self.cs as usize])?;
            self.insert_set(&parent, name, &Self::proto(0x10, c, self.cs), &mut hint)
        })();
        if let Err(e) = res { let _ = self.release(&Chain { first: c, nofat: false, len: self.cs }); return Err(e); }
        self.touch_volume()?;
        drop(hint);
        self.stat(path)
    }

    pub(crate) fn w_write_file(&self, path: &str, src: &mut dyn Read, overwrite: bool) -> Result<DirEntry> {
        self.require_writable()?;
        let comps = normalize_path(path)?;
        let (name, parents) = comps.split_last().ok_or_else(|| FsError::InvalidName("root".into()))?;
        let mut hint = self.wlock.lock().unwrap();
        let parent = self.walk(parents)?;
        validate_fat_name(name)?;
        let existing = self.find(&parent, name)?;
        if let Some(l) = &existing {
            if l.entry.kind == EntryKind::Directory { return Err(FsError::IsADirectory(path.into())); }
            if !overwrite { return Err(FsError::AlreadyExists(path.into())); }
        }
        let cs = self.cs as usize;
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
            buf[n..].fill(0);
            let c = self.alloc_cluster(&mut hint, if last != 0 { Some(last) } else { None })?;
            if first == 0 { first = c; }
            last = c;
            self.dev.write_at(self.cluster_offset(c), &buf)?;
            total += n as u64;
            if n < cs { return Ok(()); }
        })();
        let cleanup = |first: u32| { if first != 0 { let _ = self.release(&Chain { first, nofat: false, len: 0 }); } };
        if let Err(e) = stream { cleanup(first); return Err(e); }

        let res = match existing {
            None => self.insert_set(&parent, name, &Self::proto(0x20, first, total), &mut hint),
            Some(l) => (|| {
                let old = l.chain;
                self.rewrite_set(&l.offs, |s| {
                    let (ts, tenms, utc) = now_ts();
                    s[0][12..16].copy_from_slice(&ts.to_le_bytes());
                    s[0][21] = tenms; s[0][23] = utc;
                    s[1][1] = 0x01;
                    s[1][8..16].copy_from_slice(&total.to_le_bytes());
                    s[1][24..32].copy_from_slice(&total.to_le_bytes());
                    s[1][20..24].copy_from_slice(&first.to_le_bytes());
                })?;
                if old.first != 0 { self.release(&old)?; }
                Ok(())
            })(),
        };
        if let Err(e) = res { cleanup(first); return Err(e); }
        self.touch_volume()?;
        drop(hint);
        self.stat(path)
    }

    pub(crate) fn w_remove(&self, path: &str) -> Result<()> {
        self.require_writable()?;
        let comps = normalize_path(path)?;
        let (name, parents) = comps.split_last().ok_or_else(|| FsError::InvalidName("root".into()))?;
        let _g = self.wlock.lock().unwrap();
        let parent = self.walk(parents)?;
        let l = self.find(&parent, name)?.ok_or_else(|| FsError::NotFound(path.into()))?;
        if l.entry.kind == EntryKind::Directory && !self.list(&self.dir_of(&l)?)?.is_empty() {
            return Err(FsError::NotEmpty(path.into()));
        }
        self.mark_deleted(&l.offs)?;
        if l.chain.first != 0 { self.release(&l.chain)?; }
        self.touch_volume()
    }

    pub(crate) fn w_rename(&self, from: &str, to: &str) -> Result<()> {
        self.require_writable()?;
        let f = normalize_path(from)?;
        let t = normalize_path(to)?;
        let mut hint = self.wlock.lock().unwrap();
        let (sname, sparents) = f.split_last().ok_or_else(|| FsError::InvalidName("root".into()))?;
        let (dname, dparents) = t.split_last().ok_or_else(|| FsError::InvalidName("root".into()))?;
        let sp = self.walk(sparents)?;
        let l = self.find(&sp, sname)?.ok_or_else(|| FsError::NotFound(from.into()))?;
        let lower = |v: &[&str]| v.iter().map(|s| s.to_lowercase()).collect::<Vec<_>>();
        if l.entry.kind == EntryKind::Directory && lower(&t).starts_with(&lower(&f)) {
            return Err(FsError::InvalidName("cannot move a directory into itself".into()));
        }
        let dp = self.walk(dparents)?;
        validate_fat_name(dname)?;
        let same_dir = dparents.iter().map(|s| s.to_lowercase()).eq(sparents.iter().map(|s| s.to_lowercase()));
        let same_entry = same_dir && self.name_eq(dname, sname);
        if !same_entry && self.find(&dp, dname)?.is_some() { return Err(FsError::AlreadyExists(to.into())); }

        let mut proto = [[0u8; 32]; 2];
        self.dev.read_at(l.offs[0], &mut proto[0])?;
        self.dev.read_at(l.offs[1], &mut proto[1])?;
        if same_entry { self.mark_deleted(&l.offs)?; self.insert_set(&dp, dname, &proto, &mut hint)?; }
        else { self.insert_set(&dp, dname, &proto, &mut hint)?; self.mark_deleted(&l.offs)?; }
        self.touch_volume()
    }
}
