//! HFS+ / HFSX read-only driver.
//!
//! Refuses volumes whose journal holds unreplayed transactions (the on-disk structures may be
//! stale until macOS replays it). Transparently compressed (decmpfs) files are reported but
//! cannot be read. Names are shown exactly as stored (Unicode NFD); an on-disk ':' is what BSD
//! calls '/'. Hard links are resolved through the private metadata directory.

use block_device::BlockDevice;
use fs_core::{normalize_path, Capabilities, DirEntry, EntryKind, Filesystem, FsError, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const HFS_EPOCH_TO_UNIX: i64 = 2_082_844_800;
const ROOT_ID: u32 = 2;
const MAX_NODES_VISITED: u32 = 1 << 22;

fn be16(b: &[u8], o: usize) -> u16 { u16::from_be_bytes([b[o], b[o + 1]]) }
fn be32(b: &[u8], o: usize) -> u32 { u32::from_be_bytes(b[o..o + 4].try_into().unwrap()) }
fn be64(b: &[u8], o: usize) -> u64 { u64::from_be_bytes(b[o..o + 8].try_into().unwrap()) }

#[derive(Clone, Debug, Default)]
struct Fork { size: u64, extents: Vec<(u32, u32)>, compressed: bool }

#[derive(Clone, Copy, Debug)]
struct Bt { node_size: usize, root: u32, total_nodes: u32 }

struct Item { name: String, units: Vec<u16>, rtype: u16, data: Vec<u8> }

pub struct HfsPlusFs {
    dev: Arc<dyn BlockDevice>,
    bs: u64,
    total_blocks: u32,
    case_sensitive: bool,
    label: Option<String>,
    ext_fork: Fork,
    cat_fork: Fork,
    cat: Bt,
    ext: Option<Bt>,
    cache: Mutex<HashMap<u64, Fork>>,
    private_dir: Mutex<Option<Option<u32>>>,
}

fn hfs_time(t: u32) -> Option<i64> { if t == 0 { None } else { Some(t as i64 - HFS_EPOCH_TO_UNIX) } }

impl HfsPlusFs {
    pub fn open(dev: Arc<dyn BlockDevice>) -> Result<Self> {
        let mut vh = [0u8; 512];
        if dev.size() < 1024 + 512 { return Err(FsError::Corrupt("too small for HFS+")); }
        dev.read_at(1024, &mut vh)?;
        let sig = be16(&vh, 0);
        let case_sensitive = match sig { 0x482B => false, 0x4858 => true, _ => return Err(FsError::Corrupt("not an HFS+ volume header")) };
        let bs = be32(&vh, 40) as u64;
        let total_blocks = be32(&vh, 44);
        if bs < 512 || !bs.is_power_of_two() || total_blocks == 0 || bs * total_blocks as u64 > dev.size() + bs {
            return Err(FsError::Corrupt("inconsistent HFS+ geometry"));
        }
        let attrs = be32(&vh, 4);
        if attrs & (1 << 11) != 0 { return Err(FsError::Refused("volume is marked software-locked/inconsistent")); }
        if attrs & (1 << 13) != 0 { Self::check_journal(&*dev, be32(&vh, 12) as u64, bs)?; }

        let parse = |o: usize| -> (u64, u32, Vec<(u32, u32)>) {
            let ex = (0..8).map(|i| (be32(&vh, o + 16 + i * 8), be32(&vh, o + 20 + i * 8))).filter(|e| e.1 != 0).collect();
            (be64(&vh, o), be32(&vh, o + 12), ex)
        };
        let (esz, _, eex) = parse(192);
        let (csz, _, cex) = parse(272);
        let mut fs = Self {
            dev, bs, total_blocks, case_sensitive, label: None,
            ext_fork: Fork { size: esz, extents: eex, compressed: false },
            cat_fork: Fork { size: csz, extents: cex, compressed: false },
            cat: Bt { node_size: 0, root: 0, total_nodes: 0 }, ext: None,
            cache: Mutex::new(HashMap::new()), private_dir: Mutex::new(None),
        };
        fs.ext = if esz > 0 { Some(fs.open_bt(&fs.ext_fork)?) } else { None };
        fs.cat = fs.open_bt(&fs.cat_fork)?;
        // The catalog may itself have overflow extents.
        let (_, tb, _) = parse(272);
        fs.cat_fork = fs.complete_fork(4, 0, csz, tb, fs.cat_fork.extents.clone())?;
        fs.label = fs.find_volume_name();
        Ok(fs)
    }

    fn check_journal(dev: &dyn BlockDevice, info_block: u64, bs: u64) -> Result<()> {
        let mut ji = [0u8; 52];
        dev.read_at(info_block * bs, &mut ji)?;
        let flags = be32(&ji, 0);
        if flags & 1 == 0 { return Err(FsError::Refused("journal is on an external device")); }
        let off = be64(&ji, 36);
        let mut jh = [0u8; 44];
        if off.checked_add(44).map_or(true, |e| e > dev.size()) { return Err(FsError::Corrupt("journal outside volume")); }
        dev.read_at(off, &mut jh)?;
        // Written in the creating host's byte order; the endian field says which.
        let le_endian = u32::from_le_bytes(jh[4..8].try_into().unwrap());
        let big = match (u32::from_le_bytes(jh[0..4].try_into().unwrap()), le_endian) {
            (0x4a4e_4c78, 0x1234_5678) => false,
            (0x786c_4e4a, 0x7856_3412) => true,
            _ => return Err(FsError::Corrupt("bad journal header")),
        };
        let (start, end) = if big { (be64(&jh, 8), be64(&jh, 16)) } else {
            (u64::from_le_bytes(jh[8..16].try_into().unwrap()), u64::from_le_bytes(jh[16..24].try_into().unwrap()))
        };
        if start != end { return Err(FsError::Refused("journal has unreplayed transactions; mount the volume on macOS first")); }
        Ok(())
    }

    // ---- fork I/O ----
    fn read_fork_raw(&self, fork: &Fork, off: u64, buf: &mut [u8]) -> Result<()> {
        let (mut pos, mut done, mut base) = (off, 0usize, 0u64);
        for &(start, count) in &fork.extents {
            let len = count as u64 * self.bs;
            if start as u64 + count as u64 > self.total_blocks as u64 { return Err(FsError::Corrupt("extent outside volume")); }
            if pos < base + len && done < buf.len() {
                let within = pos - base;
                let n = ((len - within) as usize).min(buf.len() - done);
                self.dev.read_at(start as u64 * self.bs + within, &mut buf[done..done + n])?;
                done += n; pos += n as u64;
            }
            base += len;
            if done == buf.len() { return Ok(()); }
        }
        Err(FsError::Corrupt("read beyond fork extents"))
    }

    fn complete_fork(&self, id: u32, ftype: u8, size: u64, total_blocks: u32, first: Vec<(u32, u32)>) -> Result<Fork> {
        let mut extents = first;
        let mut have: u64 = extents.iter().map(|e| e.1 as u64).sum();
        let mut guard = 0;
        while have < total_blocks as u64 {
            guard += 1;
            let (Some(bt), true) = (self.ext, guard < 4096) else { return Err(FsError::Corrupt("missing extent overflow record")) };
            let target = (id, ftype, have as u32);
            let mut found: Option<Vec<(u32, u32)>> = None;
            let ef = &self.ext_fork;
            self.scan(&bt, ef, &|k| key_tuple(k) < target, |k, d| {
                let kt = key_tuple(k);
                if kt < target { return Ok(true); }
                if kt == target && d.len() >= 64 {
                    found = Some((0..8).map(|i| (be32(d, i * 8), be32(d, i * 8 + 4))).filter(|e| e.1 != 0).collect());
                }
                Ok(false)
            })?;
            let more = found.ok_or(FsError::Corrupt("missing extent overflow record"))?;
            if more.is_empty() { return Err(FsError::Corrupt("empty overflow record")); }
            have += more.iter().map(|e| e.1 as u64).sum::<u64>();
            extents.extend(more);
        }
        Ok(Fork { size, extents, compressed: false })
    }

    // ---- B-tree ----
    fn open_bt(&self, fork: &Fork) -> Result<Bt> {
        let mut h = [0u8; 512];
        self.read_fork_raw(fork, 0, &mut h)?;
        let ns = be16(&h, 32) as usize;
        if !ns.is_power_of_two() || !(512..=65536).contains(&ns) { return Err(FsError::Corrupt("bad B-tree node size")); }
        Ok(Bt { node_size: ns, root: be32(&h, 16), total_nodes: be32(&h, 36) })
    }

    fn node(&self, bt: &Bt, fork: &Fork, n: u32) -> Result<Vec<u8>> {
        if n >= bt.total_nodes { return Err(FsError::Corrupt("B-tree node out of range")); }
        let mut b = vec![0u8; bt.node_size];
        self.read_fork_raw(fork, n as u64 * bt.node_size as u64, &mut b)?;
        Ok(b)
    }

    /// Record byte ranges of a node.
    fn records(node: &[u8]) -> Result<Vec<(usize, usize)>> {
        let n = be16(node, 10) as usize;
        let tbl = node.len().checked_sub(2 * (n + 1)).filter(|&t| t >= 14).ok_or(FsError::Corrupt("bad record count"))?;
        let off = |i: usize| be16(node, node.len() - 2 * (i + 1)) as usize;
        let mut v = Vec::with_capacity(n);
        for i in 0..n {
            let (s, e) = (off(i), off(i + 1));
            if s < 14 || e < s || e > tbl { return Err(FsError::Corrupt("bad record offsets")); }
            v.push((s, e));
        }
        Ok(v)
    }

    /// Descends to the leaf where keys `>= target` begin (`lt` is true for keys strictly below the
    /// target), then visits records forward until `f` returns `false`.
    fn scan<F>(&self, bt: &Bt, fork: &Fork, lt: &dyn Fn(&[u8]) -> bool, mut f: F) -> Result<()>
    where F: FnMut(&[u8], &[u8]) -> Result<bool> {
        if bt.root == 0 { return Ok(()); }
        let mut n = bt.root;
        let mut node;
        let mut depth = 0;
        loop {
            node = self.node(bt, fork, n)?;
            match node[8] as i8 {
                -1 => break,
                0 => {
                    depth += 1;
                    if depth > 16 { return Err(FsError::Corrupt("B-tree too deep")); }
                    let recs = Self::records(&node)?;
                    if recs.is_empty() { return Err(FsError::Corrupt("empty index node")); }
                    let mut pick = 0;
                    for (i, &(s, e)) in recs.iter().enumerate() {
                        let (key, _) = split_rec(&node[s..e])?;
                        if lt(key) { pick = i } else { break }
                    }
                    let (s, e) = recs[pick];
                    let (_, data) = split_rec(&node[s..e])?;
                    if data.len() < 4 { return Err(FsError::Corrupt("short index record")); }
                    n = be32(data, 0);
                }
                _ => return Err(FsError::Corrupt("unexpected B-tree node kind")),
            }
        }
        let mut visited = 0u32;
        loop {
            for (s, e) in Self::records(&node)? {
                let (key, data) = split_rec(&node[s..e])?;
                if !f(key, data)? { return Ok(()); }
            }
            let next = be32(&node, 0);
            if next == 0 { return Ok(()); }
            visited += 1;
            if visited > MAX_NODES_VISITED { return Err(FsError::Corrupt("B-tree leaf chain loop")); }
            node = self.node(bt, fork, next)?;
            if node[8] as i8 != -1 { return Err(FsError::Corrupt("leaf chain reaches non-leaf")); }
        }
    }

    fn children(&self, parent: u32) -> Result<Vec<Item>> {
        let mut out = Vec::new();
        self.scan(&self.cat, &self.cat_fork, &|k| cat_parent(k) < parent, |k, d| {
            let p = cat_parent(k);
            if p < parent { return Ok(true); }
            if p > parent { return Ok(false); }
            let nl = be16(k, 6) as usize;
            if k.len() < 8 + nl * 2 || d.len() < 2 { return Err(FsError::Corrupt("bad catalog record")); }
            let units: Vec<u16> = (0..nl).map(|i| be16(k, 8 + i * 2)).collect();
            out.push(Item { name: String::from_utf16_lossy(&units), units, rtype: be16(d, 0), data: d.to_vec() });
            Ok(true)
        })?;
        Ok(out)
    }

    fn find_volume_name(&self) -> Option<String> {
        self.children(ROOT_ID).ok()?.into_iter().find(|i| i.rtype == 3 && i.units.is_empty()).and_then(|i| {
            if i.data.len() < 10 { return None; }
            let nl = be16(&i.data, 8) as usize;
            if i.data.len() < 10 + nl * 2 { return None; }
            let u: Vec<u16> = (0..nl).map(|k| be16(&i.data, 10 + k * 2)).collect();
            let s = String::from_utf16_lossy(&u);
            if s.is_empty() { None } else { Some(s) }
        })
    }

    fn fork_from(&self, rec: &[u8], at: usize, id: u32, ftype: u8, compressed: bool) -> Result<Fork> {
        if rec.len() < at + 80 { return Err(FsError::Corrupt("short fork data")); }
        let size = be64(rec, at);
        let total = be32(rec, at + 12);
        let ex: Vec<(u32, u32)> = (0..8).map(|i| (be32(rec, at + 16 + i * 8), be32(rec, at + 20 + i * 8))).filter(|e| e.1 != 0).collect();
        let mut f = self.complete_fork(id, ftype, size, total, ex)?;
        f.compressed = compressed;
        Ok(f)
    }

    fn private_dir_id(&self) -> Result<Option<u32>> {
        let mut g = self.private_dir.lock().unwrap();
        if let Some(v) = *g { return Ok(v); }
        let want: Vec<u16> = "\0\0\0\0HFS+ Private Data".encode_utf16().collect();
        let v = self.children(ROOT_ID)?.into_iter().find(|i| i.rtype == 1 && i.units == want).map(|i| be32(&i.data, 8));
        *g = Some(v);
        Ok(v)
    }

    fn make_entry(&self, it: &Item) -> Result<Option<DirEntry>> {
        let d = &it.data;
        let hidden_name = it.name.starts_with('.');
        match it.rtype {
            1 => {
                if d.len() < 88 { return Err(FsError::Corrupt("short folder record")); }
                Ok(Some(DirEntry {
                    name: it.name.clone(), kind: EntryKind::Directory, size: 0,
                    modified: hfs_time(be32(d, 16)), created: hfs_time(be32(d, 12)),
                    hidden: hidden_name || be16(d, 64 + 8) & 0x4000 != 0, read_only: false,
                    inode: be32(d, 8) as u64,
                }))
            }
            2 => {
                if d.len() < 248 { return Err(FsError::Corrupt("short file record")); }
                let (mut id, mut rec) = (be32(d, 8), d.to_vec());
                // Hard link: type 'hlnk' creator 'hfs+' -> iNode<special> in the private directory.
                if &d[48..52] == b"hlnk" && &d[52..56] == b"hfs+" {
                    let n = be32(d, 44);
                    if let Some(pd) = self.private_dir_id()? {
                        let want = format!("iNode{n}");
                        let target = self.children(pd)?.into_iter().find(|i| i.rtype == 2 && i.name == want)
                            .ok_or(FsError::Corrupt("dangling hard link"))?;
                        if target.data.len() < 248 { return Err(FsError::Corrupt("short file record")); }
                        id = be32(&target.data, 8);
                        rec = target.data;
                    }
                }
                let compressed = rec[32 + 9] & 0x20 != 0;
                let fork = self.fork_from(&rec, 88, id, 0x00, compressed)?;
                let e = DirEntry {
                    name: it.name.clone(), kind: EntryKind::File, size: fork.size,
                    modified: hfs_time(be32(d, 16)), created: hfs_time(be32(d, 12)),
                    hidden: hidden_name || be16(d, 48 + 8) & 0x4000 != 0, read_only: false,
                    inode: id as u64,
                };
                self.cache.lock().unwrap().insert(e.inode, fork);
                Ok(Some(e))
            }
            _ => Ok(None),
        }
    }

    fn name_eq(&self, a: &str, b: &str) -> bool {
        if self.case_sensitive { a == b } else { a == b || a.to_lowercase() == b.to_lowercase() }
    }

    fn list_id(&self, id: u32) -> Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        for it in self.children(id)? {
            if id == ROOT_ID && it.name.starts_with('\0') { continue; }
            if let Some(e) = self.make_entry(&it)? { out.push(e); }
        }
        Ok(out)
    }

    fn walk_dir(&self, comps: &[&str]) -> Result<u32> {
        let mut id = ROOT_ID;
        for c in comps {
            let e = self.list_id(id)?.into_iter().find(|e| self.name_eq(&e.name, c)).ok_or_else(|| FsError::NotFound((*c).into()))?;
            if e.kind != EntryKind::Directory { return Err(FsError::NotADirectory((*c).into())); }
            id = e.inode as u32;
        }
        Ok(id)
    }
}

fn key_tuple(k: &[u8]) -> (u32, u8, u32) {
    if k.len() < 12 { return (0, 0, 0); }
    (be32(k, 4), k[2], be32(k, 8))
}
fn cat_parent(k: &[u8]) -> u32 { if k.len() < 6 { 0 } else { be32(k, 2) } }

/// Splits a B-tree record into (key incl. length field, data).
fn split_rec(r: &[u8]) -> Result<(&[u8], &[u8])> {
    if r.len() < 2 { return Err(FsError::Corrupt("short record")); }
    let kl = be16(r, 0) as usize + 2;
    if kl > r.len() { return Err(FsError::Corrupt("key exceeds record")); }
    let ds = (kl + 1) & !1;
    Ok((&r[..kl], &r[ds.min(r.len())..]))
}

impl Filesystem for HfsPlusFs {
    fn name(&self) -> &'static str { if self.case_sensitive { "HFSX" } else { "HFS+" } }
    fn label(&self) -> Option<String> { self.label.clone() }
    fn capabilities(&self) -> Capabilities { Capabilities::READ_ONLY }

    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        let id = self.walk_dir(&normalize_path(path)?)?;
        self.list_id(id)
    }

    fn stat(&self, path: &str) -> Result<DirEntry> {
        let comps = normalize_path(path)?;
        let Some((last, parents)) = comps.split_last() else {
            return Ok(DirEntry { name: "/".into(), kind: EntryKind::Directory, size: 0, modified: None, created: None, hidden: false, read_only: false, inode: ROOT_ID as u64 });
        };
        let id = self.walk_dir(parents)?;
        self.list_id(id)?.into_iter().find(|e| self.name_eq(&e.name, last)).ok_or_else(|| FsError::NotFound(path.into()))
    }

    fn read_at(&self, entry: &DirEntry, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if entry.kind == EntryKind::Directory { return Err(FsError::IsADirectory(entry.name.clone())); }
        let fork = self.cache.lock().unwrap().get(&entry.inode).cloned()
            .ok_or(FsError::Corrupt("entry was not obtained from this volume (stat/read_dir first)"))?;
        if fork.compressed { return Err(FsError::Unsupported("transparently compressed (decmpfs) file")); }
        if offset >= fork.size || buf.is_empty() { return Ok(0); }
        let n = (buf.len() as u64).min(fork.size - offset) as usize;
        self.read_fork_raw(&fork, offset, &mut buf[..n])?;
        Ok(n)
    }
}
