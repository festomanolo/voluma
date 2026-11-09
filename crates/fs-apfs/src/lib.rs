//! APFS read-only driver (current state of the first volume, or a chosen one).
//!
//! Every object read is verified with its Fletcher-64 checksum. The newest valid checkpoint
//! superblock is used. Not supported: encrypted volumes, snapshots (only the live state is
//! shown), transparently compressed files (listed, but reads fail), fusion drives, and
//! multi-volume browsing beyond choosing one volume. Symlinks appear as small files holding
//! the target path.

use block_device::BlockDevice;
use fs_core::{normalize_path, Capabilities, DirEntry, EntryKind, Filesystem, FsError, Result};
use std::sync::{Arc, Mutex};

fn le16(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o + 1]]) }
fn le32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) }
fn le64(b: &[u8], o: usize) -> u64 { u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) }

const ROOT_DIR: u64 = 2;
const T_INODE: u8 = 3;
const T_XATTR: u8 = 4;
const T_EXTENT: u8 = 8;
const T_DREC: u8 = 9;
const NODE_ROOT: u16 = 1;
const NODE_LEAF: u16 = 2;
const NODE_FIXED: u16 = 4;
const BTINFO: usize = 40;
const MAX_WALK_NODES: u32 = 1 << 20;

fn fletcher64(block: &[u8]) -> u64 {
    const M: u64 = 0xFFFF_FFFF;
    let (mut s1, mut s2) = (0u64, 0u64);
    for w in block[8..].chunks_exact(4) {
        s1 = (s1 + u32::from_le_bytes(w.try_into().unwrap()) as u64) % M;
        s2 = (s2 + s1) % M;
    }
    let c1 = M - (s1 + s2) % M;
    let c2 = M - (s1 + c1) % M;
    (c2 << 32) | c1
}

struct Node { d: Vec<u8>, flags: u16, level: u16, n: usize, toc: usize, key0: usize, val_end: usize, fixed: (usize, usize) }

impl Node {
    fn is_leaf(&self) -> bool { self.flags & NODE_LEAF != 0 }
    fn locs(&self, i: usize) -> Result<((usize, usize), (usize, usize))> {
        if i >= self.n { return Err(FsError::Corrupt("record index out of range")); }
        let (kloc, vloc) = if self.flags & NODE_FIXED != 0 {
            let e = self.toc + i * 4;
            if e + 4 > self.d.len() { return Err(FsError::Corrupt("table of contents overflow")); }
            let (k, v) = (le16(&self.d, e) as usize, le16(&self.d, e + 2) as usize);
            let vlen = if self.is_leaf() { self.fixed.1 } else { 8 };
            ((k, self.fixed.0), (v, vlen))
        } else {
            let e = self.toc + i * 8;
            if e + 8 > self.d.len() { return Err(FsError::Corrupt("table of contents overflow")); }
            ((le16(&self.d, e) as usize, le16(&self.d, e + 2) as usize), (le16(&self.d, e + 4) as usize, le16(&self.d, e + 6) as usize))
        };
        Ok((kloc, vloc))
    }
    fn rec(&self, i: usize) -> Result<(&[u8], &[u8])> {
        let ((ko, kl), (vo, vl)) = self.locs(i)?;
        let ks = self.key0.checked_add(ko).ok_or(FsError::Corrupt("key offset"))?;
        if ks + kl > self.d.len() { return Err(FsError::Corrupt("key outside node")); }
        // Offset 0xFFFF marks an absent value.
        if vo == 0xFFFF { return Ok((&self.d[ks..ks + kl], &[])); }
        let vs = self.val_end.checked_sub(vo).ok_or(FsError::Corrupt("value offset"))?;
        if vs + vl > self.d.len() || vs < self.key0 { return Err(FsError::Corrupt("value outside node")); }
        Ok((&self.d[ks..ks + kl], &self.d[vs..vs + vl]))
    }
}

pub struct ApfsFs {
    dev: Arc<dyn BlockDevice>,
    bs: u64,
    blocks: u64,
    vol_omap_root: u64,
    root_oid: u64,
    root_physical: bool,
    xid: u64,
    label: Option<String>,
    case_insensitive: bool,
    hashed_names: bool,
    extent_cache: Mutex<Option<(u64, Vec<(u64, u64, u64)>)>>,
    volume_names: Vec<String>,
}

struct Rec { key: Vec<u8>, val: Vec<u8> }

impl ApfsFs {
    pub fn open(dev: Arc<dyn BlockDevice>) -> Result<Self> { Self::open_volume(dev, 0) }

    pub fn open_volume(dev: Arc<dyn BlockDevice>, index: usize) -> Result<Self> {
        let mut b0 = vec![0u8; 4096.min(dev.size() as usize)];
        if b0.len() < 4096 { return Err(FsError::Corrupt("too small for APFS")); }
        dev.read_at(0, &mut b0)?;
        if &b0[32..36] != b"NXSB" { return Err(FsError::Corrupt("not an APFS container")); }
        let bs = le32(&b0, 36) as u64;
        if !(4096..=65536).contains(&bs) || !bs.is_power_of_two() { return Err(FsError::Corrupt("bad APFS block size")); }
        let blocks = le64(&b0, 40);
        if blocks == 0 || blocks.saturating_mul(bs) > dev.size() { return Err(FsError::Corrupt("container larger than device")); }
        let mut fs = Self {
            dev, bs, blocks, vol_omap_root: 0, root_oid: 0, root_physical: false, xid: 0, label: None,
            case_insensitive: false, hashed_names: false, extent_cache: Mutex::new(None), volume_names: vec![],
        };
        // Newest valid checkpoint superblock from the descriptor area (block 0 may be stale).
        let mut sb = fs.read_obj(0)?;
        let (dblocks, dbase) = (le32(&sb, 104), le64(&sb, 112));
        if dblocks & 0x8000_0000 == 0 && dbase & (1 << 63) == 0 && dbase < blocks && dbase + dblocks as u64 <= blocks {
            for i in 0..dblocks as u64 {
                if let Ok(c) = fs.read_obj(dbase + i) {
                    if le32(&c, 24) & 0xFFFF == 1 && &c[32..36] == b"NXSB" && le64(&c, 16) > le64(&sb, 16) { sb = c; }
                }
            }
        }
        let xid = le64(&sb, 16);
        let omap_blk = fs.read_obj(le64(&sb, 160))?;
        let cont_omap_root = le64(&omap_blk, 48);
        let max_fs = (le32(&sb, 180) as usize).min(100);
        let mut vols = Vec::new();
        for i in 0..max_fs {
            let oid = le64(&sb, 184 + i * 8);
            if oid == 0 { continue; }
            let paddr = fs.omap_lookup(cont_omap_root, oid, xid)?;
            let v = fs.read_obj(paddr)?;
            if &v[32..36] != b"APSB" { return Err(FsError::Corrupt("bad volume superblock")); }
            vols.push(v);
        }
        fs.volume_names = vols.iter().map(|v| cstr(&v[704..960])).collect();
        let v = vols.get(index).ok_or(FsError::NotFound("no such APFS volume".into()))?;
        if le64(v, 264) & 1 == 0 { return Err(FsError::Unsupported("encrypted APFS volume")); }
        let incompat = le64(v, 56);
        fs.case_insensitive = incompat & 1 != 0;
        fs.hashed_names = incompat & 0x9 != 0;
        fs.label = Some(fs.volume_names[index].clone()).filter(|s| !s.is_empty());
        fs.xid = le64(v, 16);
        let vom = fs.read_obj(le64(v, 128))?;
        fs.vol_omap_root = le64(&vom, 48);
        fs.root_physical = le32(v, 116) & 0x4000_0000 != 0;
        fs.root_oid = le64(v, 136);
        Ok(fs)
    }

    pub fn volumes(&self) -> &[String] { &self.volume_names }

    fn read_obj(&self, paddr: u64) -> Result<Vec<u8>> {
        if paddr >= self.blocks { return Err(FsError::Corrupt("object outside container")); }
        let mut b = vec![0u8; self.bs as usize];
        self.dev.read_at(paddr * self.bs, &mut b)?;
        if fletcher64(&b) != le64(&b, 0) { return Err(FsError::Corrupt("object checksum mismatch")); }
        Ok(b)
    }

    fn node(&self, paddr: u64, fixed: (usize, usize)) -> Result<Node> {
        let d = self.read_obj(paddr)?;
        let (flags, level, n) = (le16(&d, 32), le16(&d, 34), le32(&d, 36) as usize);
        let (toc_off, toc_len) = (le16(&d, 40) as usize, le16(&d, 42) as usize);
        let root = flags & NODE_ROOT != 0;
        let val_end = d.len() - if root { BTINFO } else { 0 };
        let toc = 56 + toc_off;
        let key0 = toc + toc_len;
        let entry = if flags & NODE_FIXED != 0 { 4 } else { 8 };
        if n > 1 << 16 || toc_len < n * entry || key0 > val_end { return Err(FsError::Corrupt("bad B-tree node")); }
        Ok(Node { d, flags, level, n, toc, key0, val_end, fixed })
    }

    fn omap_lookup(&self, root: u64, oid: u64, xid: u64) -> Result<u64> {
        let mut n = self.node(root, (16, 16))?;
        for _ in 0..16 {
            let mut pick = None;
            for i in 0..n.n {
                let (k, _) = n.rec(i)?;
                if k.len() < 16 { return Err(FsError::Corrupt("short omap key")); }
                if (le64(k, 0), le64(k, 8)) <= (oid, xid) { pick = Some(i) } else { break }
            }
            let i = pick.ok_or(FsError::NotFound(format!("object {oid}")))?;
            let (k, v) = n.rec(i)?;
            if n.is_leaf() {
                if le64(k, 0) != oid || v.len() < 16 || le32(v, 0) & 1 != 0 { return Err(FsError::NotFound(format!("object {oid}"))); }
                return Ok(le64(v, 8));
            }
            n = self.node(le64(v, 0), (16, 16))?;
        }
        Err(FsError::Corrupt("omap too deep"))
    }

    fn resolve_fs_oid(&self, oid: u64) -> Result<u64> { self.omap_lookup(self.vol_omap_root, oid, self.xid) }

    /// All records whose object id is `obj` and whose type satisfies `want`.
    fn collect(&self, obj: u64, want: &dyn Fn(u8) -> bool) -> Result<Vec<Rec>> {
        let root = if self.root_physical { self.root_oid } else { self.resolve_fs_oid(self.root_oid)? };
        let mut out = Vec::new();
        let mut budget = MAX_WALK_NODES;
        self.walk(root, obj, want, &mut out, 0, &mut budget)?;
        Ok(out)
    }

    fn walk(&self, paddr: u64, obj: u64, want: &dyn Fn(u8) -> bool, out: &mut Vec<Rec>, depth: u32, budget: &mut u32) -> Result<()> {
        if depth > 16 { return Err(FsError::Corrupt("fs tree too deep")); }
        *budget = budget.checked_sub(1).ok_or(FsError::Corrupt("fs tree walk too long"))?;
        let n = self.node(paddr, (0, 0))?;
        let id = |k: &[u8]| -> Result<u64> { if k.len() < 8 { Err(FsError::Corrupt("short key")) } else { Ok(le64(k, 0) & 0x0FFF_FFFF_FFFF_FFFF) } };
        if n.is_leaf() {
            for i in 0..n.n {
                let (k, v) = n.rec(i)?;
                if id(k)? == obj && want((le64(k, 0) >> 60) as u8) { out.push(Rec { key: k.to_vec(), val: v.to_vec() }); }
            }
            return Ok(());
        }
        if n.level == 0 { return Err(FsError::Corrupt("index node at level 0")); }
        for i in 0..n.n {
            let (k, v) = n.rec(i)?;
            if id(k)? > obj { break; }
            if i + 1 < n.n && id(n.rec(i + 1)?.0)? < obj { continue; }
            if v.len() < 8 { return Err(FsError::Corrupt("short index value")); }
            let child = if self.root_physical { le64(v, 0) } else { self.resolve_fs_oid(le64(v, 0))? };
            self.walk(child, obj, want, out, depth + 1, budget)?;
        }
        Ok(())
    }

    fn inode_rec(&self, ino: u64) -> Result<Vec<u8>> {
        self.collect(ino, &|t| t == T_INODE)?.into_iter().next().map(|r| r.val).filter(|v| v.len() >= 92)
            .ok_or(FsError::Corrupt("inode record missing"))
    }

    fn dstream_size(v: &[u8]) -> Option<u64> {
        if v.len() < 96 { return None; }
        let (num, used) = (le16(v, 92) as usize, le16(v, 94) as usize);
        let hdr = 96 + num * 4;
        if hdr + used > v.len() && hdr > v.len() { return None; }
        let mut data = hdr;
        for i in 0..num {
            let (ty, sz) = (v[96 + i * 4], le16(v, 96 + i * 4 + 2) as usize);
            if ty == 8 && sz >= 8 && data + 8 <= v.len() { return Some(le64(v, data)); }
            data += (sz + 7) & !7;
        }
        None
    }

    fn symlink_target(&self, ino: u64) -> Result<Vec<u8>> {
        for r in self.collect(ino, &|t| t == T_XATTR)? {
            if r.key.len() < 10 { continue; }
            let nl = le16(&r.key, 8) as usize;
            if r.key.len() < 10 + nl || cstr(&r.key[10..10 + nl]) != "com.apple.fs.symlink" { continue; }
            if r.val.len() < 4 || le16(&r.val, 0) & 2 == 0 { return Err(FsError::Unsupported("non-embedded symlink target")); }
            let l = le16(&r.val, 2) as usize;
            if r.val.len() < 4 + l { return Err(FsError::Corrupt("short xattr")); }
            let mut t = r.val[4..4 + l].to_vec();
            while t.last() == Some(&0) { t.pop(); }
            return Ok(t);
        }
        Err(FsError::Corrupt("symlink without target"))
    }

    fn entry(&self, name: &str, ino: u64) -> Result<DirEntry> {
        let v = self.inode_rec(ino)?;
        let mode = le16(&v, 80);
        let kind = if mode & 0xF000 == 0x4000 { EntryKind::Directory } else { EntryKind::File };
        let size = match (mode & 0xF000, kind) {
            (_, EntryKind::Directory) => 0,
            (0xA000, _) => self.symlink_target(ino)?.len() as u64,
            (0x8000, _) => Self::dstream_size(&v).unwrap_or(0),
            _ => 0,
        };
        let secs = |ns: u64| if ns == 0 { None } else { Some((ns / 1_000_000_000) as i64) };
        Ok(DirEntry {
            name: name.to_string(), kind, size, modified: secs(le64(&v, 24)), created: secs(le64(&v, 16)),
            hidden: name.starts_with('.') || le32(&v, 68) & 0x8000 != 0, read_only: mode & 0o222 == 0, inode: ino,
        })
    }

    fn drecs(&self, dir: u64) -> Result<Vec<(String, u64)>> {
        let mut out = Vec::new();
        for r in self.collect(dir, &|t| t == T_DREC)? {
            let (nlen, off) = if self.hashed_names {
                if r.key.len() < 12 { return Err(FsError::Corrupt("short drec key")); }
                ((le32(&r.key, 8) & 0x3FF) as usize, 12)
            } else {
                if r.key.len() < 10 { return Err(FsError::Corrupt("short drec key")); }
                (le16(&r.key, 8) as usize, 10)
            };
            if off + nlen > r.key.len() || r.val.len() < 18 { return Err(FsError::Corrupt("bad directory record")); }
            out.push((cstr(&r.key[off..off + nlen]), le64(&r.val, 0)));
        }
        Ok(out)
    }

    fn name_eq(&self, a: &str, b: &str) -> bool { if self.case_insensitive { a.to_lowercase() == b.to_lowercase() } else { a == b } }

    fn find_dir(&self, comps: &[&str]) -> Result<u64> {
        let mut cur = ROOT_DIR;
        for c in comps {
            let (n, ino) = self.drecs(cur)?.into_iter().find(|(n, _)| self.name_eq(n, c)).ok_or_else(|| FsError::NotFound((*c).into()))?;
            let e = self.entry(&n, ino)?;
            if e.kind != EntryKind::Directory { return Err(FsError::NotADirectory((*c).into())); }
            cur = ino;
        }
        Ok(cur)
    }

    fn extents(&self, private_id: u64) -> Result<Vec<(u64, u64, u64)>> {
        if let Some((id, v)) = &*self.extent_cache.lock().unwrap() { if *id == private_id { return Ok(v.clone()); } }
        let mut v: Vec<(u64, u64, u64)> = Vec::new();
        for r in self.collect(private_id, &|t| t == T_EXTENT)? {
            if r.key.len() < 16 || r.val.len() < 16 { return Err(FsError::Corrupt("short extent record")); }
            v.push((le64(&r.key, 8), le64(&r.val, 0) & 0x00FF_FFFF_FFFF_FFFF, le64(&r.val, 8)));
        }
        v.sort();
        *self.extent_cache.lock().unwrap() = Some((private_id, v.clone()));
        Ok(v)
    }
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

impl Filesystem for ApfsFs {
    fn name(&self) -> &'static str { "APFS" }
    fn label(&self) -> Option<String> { self.label.clone() }
    fn capabilities(&self) -> Capabilities { Capabilities::READ_ONLY }

    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        let d = self.find_dir(&normalize_path(path)?)?;
        self.drecs(d)?.into_iter().map(|(n, ino)| self.entry(&n, ino)).collect()
    }

    fn stat(&self, path: &str) -> Result<DirEntry> {
        let comps = normalize_path(path)?;
        let Some((last, parents)) = comps.split_last() else {
            let mut e = self.entry("/", ROOT_DIR)?; e.hidden = false; return Ok(e);
        };
        let d = self.find_dir(parents)?;
        let (n, ino) = self.drecs(d)?.into_iter().find(|(n, _)| self.name_eq(n, last)).ok_or_else(|| FsError::NotFound(path.into()))?;
        self.entry(&n, ino)
    }

    fn read_at(&self, entry: &DirEntry, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if entry.kind == EntryKind::Directory { return Err(FsError::IsADirectory(entry.name.clone())); }
        let v = self.inode_rec(entry.inode)?;
        let mode = le16(&v, 80);
        if le32(&v, 68) & 0x20 != 0 { return Err(FsError::Unsupported("transparently compressed (decmpfs) file")); }
        if mode & 0xF000 == 0xA000 {
            let t = self.symlink_target(entry.inode)?;
            if offset >= t.len() as u64 { return Ok(0); }
            let n = buf.len().min(t.len() - offset as usize);
            buf[..n].copy_from_slice(&t[offset as usize..offset as usize + n]);
            return Ok(n);
        }
        if mode & 0xF000 != 0x8000 { return Ok(0); }
        let size = Self::dstream_size(&v).unwrap_or(0);
        if offset >= size || buf.is_empty() { return Ok(0); }
        let want = (buf.len() as u64).min(size - offset) as usize;
        let exts = self.extents(le64(&v, 8))?;
        let mut done = 0usize;
        while done < want {
            let pos = offset + done as u64;
            // Extent covering `pos`, if any (otherwise a hole -> zeros up to the next extent).
            let idx = exts.partition_point(|e| e.0 <= pos);
            let cur = idx.checked_sub(1).map(|i| exts[i]).filter(|e| pos < e.0 + e.1);
            match cur {
                Some((lo, len, phys)) => {
                    let n = ((lo + len - pos) as usize).min(want - done);
                    if phys == 0 { buf[done..done + n].fill(0); }
                    else {
                        let start = phys.checked_mul(self.bs).and_then(|s| s.checked_add(pos - lo)).ok_or(FsError::Corrupt("extent overflow"))?;
                        if start + n as u64 > self.blocks * self.bs { return Err(FsError::Corrupt("extent outside container")); }
                        self.dev.read_at(start, &mut buf[done..done + n])?;
                    }
                    done += n;
                }
                None => {
                    let next = exts.get(idx).map(|e| e.0).unwrap_or(size);
                    let n = ((next - pos) as usize).min(want - done).max(1);
                    buf[done..done + n].fill(0); done += n;
                }
            }
        }
        Ok(want)
    }
}
