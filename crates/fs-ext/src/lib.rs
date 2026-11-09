//! ext2 / ext3 / ext4 read-only driver.
//!
//! Supported: classic indirect-block and extent-mapped files, sparse files and uninitialised
//! extents (read as zeros), 64-bit block numbers, flex_bg, meta_bg, large directories (linear
//! scan, so htree directories work), symlinks (shown as small files holding the target path).
//! Refused: volumes needing journal recovery, encrypted inodes, inline-data inodes, and any
//! unknown incompatible feature. Device nodes/FIFOs/sockets are listed as empty files.

use block_device::BlockDevice;
use fs_core::{normalize_path, Capabilities, DirEntry, EntryKind, Filesystem, FsError, Result};
use std::sync::Arc;

fn le16(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o + 1]]) }
fn le32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) }

const INCOMPAT_FILETYPE: u32 = 0x2;
const INCOMPAT_RECOVER: u32 = 0x4;
const INCOMPAT_META_BG: u32 = 0x10;
const INCOMPAT_EXTENTS: u32 = 0x40;
const INCOMPAT_64BIT: u32 = 0x80;
const INCOMPAT_MMP: u32 = 0x100;
const INCOMPAT_FLEX_BG: u32 = 0x200;
const INCOMPAT_CSUM_SEED: u32 = 0x2000;
const INCOMPAT_LARGEDIR: u32 = 0x4000;
const SUPPORTED_INCOMPAT: u32 = INCOMPAT_FILETYPE | INCOMPAT_META_BG | INCOMPAT_EXTENTS | INCOMPAT_64BIT
    | INCOMPAT_MMP | INCOMPAT_FLEX_BG | INCOMPAT_CSUM_SEED | INCOMPAT_LARGEDIR;
const RO_SPARSE_SUPER: u32 = 0x1;

const FL_ENCRYPT: u32 = 0x800;
const FL_EXTENTS: u32 = 0x80000;
const FL_INLINE: u32 = 0x1000_0000;
const MAX_DIR_BYTES: u64 = 512 << 20;

pub struct ExtFs {
    dev: Arc<dyn BlockDevice>,
    bs: u64,
    blocks_count: u64,
    first_data_block: u64,
    blocks_per_group: u64,
    inodes_per_group: u64,
    inodes_count: u64,
    inode_size: u64,
    desc_size: u64,
    first_meta_bg: u64,
    incompat: u32,
    ro_compat: u32,
    group_count: u64,
    label: Option<String>,
    version: &'static str,
}

#[derive(Debug, Clone)]
struct Inode { mode: u16, size: u64, flags: u32, block: [u8; 60], mtime: Option<i64>, crtime: Option<i64>, ino: u64 }

impl Inode {
    fn kind(&self) -> Option<EntryKind> {
        match self.mode & 0xF000 { 0x4000 => Some(EntryKind::Directory), 0x8000 | 0xA000 => Some(EntryKind::File), _ => None }
    }
    fn is_regular_data(&self) -> bool { matches!(self.mode & 0xF000, 0x8000 | 0xA000) }
}

fn ts(secs: u32, extra: Option<u32>) -> Option<i64> {
    if secs == 0 && extra.unwrap_or(0) == 0 { return None; }
    Some(secs as i32 as i64 + (((extra.unwrap_or(0) & 3) as i64) << 32))
}

impl ExtFs {
    pub fn open(dev: Arc<dyn BlockDevice>) -> Result<Self> {
        if dev.size() < 2048 { return Err(FsError::Corrupt("too small for ext")); }
        let mut sb = [0u8; 1024];
        dev.read_at(1024, &mut sb)?;
        if le16(&sb, 56) != 0xEF53 { return Err(FsError::Corrupt("not an ext superblock")); }
        let rev = le32(&sb, 76);
        let (compat, incompat, ro_compat) = if rev >= 1 { (le32(&sb, 92), le32(&sb, 96), le32(&sb, 100)) } else { (0, 0, 0) };
        let log = le32(&sb, 24);
        if log > 6 { return Err(FsError::Corrupt("invalid block size")); }
        let bs = 1024u64 << log;
        if incompat & INCOMPAT_RECOVER != 0 {
            return Err(FsError::Refused("filesystem needs journal recovery; mount it on Linux first"));
        }
        if incompat & !SUPPORTED_INCOMPAT != 0 {
            return Err(FsError::Unsupported("ext volume uses an unsupported incompatible feature"));
        }
        let wide = incompat & INCOMPAT_64BIT != 0;
        let desc_size = if wide { le16(&sb, 254) as u64 } else { 32 };
        if desc_size < 32 || desc_size > bs || !desc_size.is_power_of_two() { return Err(FsError::Corrupt("bad group descriptor size")); }
        let blocks_count = le32(&sb, 4) as u64 | if wide { (le32(&sb, 336) as u64) << 32 } else { 0 };
        let (bpg, ipg) = (le32(&sb, 32) as u64, le32(&sb, 40) as u64);
        let first_data_block = le32(&sb, 20) as u64;
        let inode_size = if rev >= 1 { le16(&sb, 88) as u64 } else { 128 };
        if bpg == 0 || ipg == 0 || bpg > 8 * bs || ipg > 8 * bs || blocks_count == 0 || blocks_count * bs > dev.size()
            || first_data_block >= blocks_count || (bs == 1024) != (first_data_block == 1)
            || inode_size < 128 || inode_size > bs || !inode_size.is_power_of_two() {
            return Err(FsError::Corrupt("inconsistent ext geometry"));
        }
        let group_count = (blocks_count - first_data_block).div_ceil(bpg);
        let inodes_count = le32(&sb, 0) as u64;
        if inodes_count != group_count * ipg && inodes_count > group_count * ipg { return Err(FsError::Corrupt("inode count mismatch")); }
        let label = {
            let l = &sb[120..136];
            let end = l.iter().position(|&b| b == 0).unwrap_or(16);
            let s = String::from_utf8_lossy(&l[..end]).trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        };
        let version = if incompat & (INCOMPAT_EXTENTS | INCOMPAT_64BIT | INCOMPAT_FLEX_BG) != 0 || ro_compat & 0x40 != 0 { "ext4" }
            else if compat & 0x4 != 0 { "ext3" } else { "ext2" };
        Ok(Self {
            dev, bs, blocks_count, first_data_block, blocks_per_group: bpg, inodes_per_group: ipg, inodes_count,
            inode_size, desc_size, first_meta_bg: le32(&sb, 260) as u64, incompat, ro_compat, group_count, label, version,
        })
    }

    pub fn variant(&self) -> &'static str { self.version }

    fn has_super(&self, g: u64) -> bool {
        if self.ro_compat & RO_SPARSE_SUPER == 0 || g <= 1 { return true; }
        [3u64, 5, 7].iter().any(|&b| { let mut n = b; while n < g { n *= b; } n == g })
    }

    fn inode_table(&self, g: u64) -> Result<u64> {
        if g >= self.group_count { return Err(FsError::Corrupt("group out of range")); }
        let dpb = self.bs / self.desc_size;
        let idx = g / dpb;
        let blk = if self.incompat & INCOMPAT_META_BG == 0 || idx < self.first_meta_bg {
            self.first_data_block + 1 + idx
        } else {
            let first = idx * dpb;
            self.first_data_block + first * self.blocks_per_group + self.has_super(first) as u64
        };
        if blk >= self.blocks_count { return Err(FsError::Corrupt("group descriptor outside volume")); }
        let mut d = vec![0u8; self.desc_size as usize];
        self.dev.read_at(blk * self.bs + (g % dpb) * self.desc_size, &mut d)?;
        let mut t = le32(&d, 8) as u64;
        if self.desc_size >= 64 { t |= (le32(&d, 0x28) as u64) << 32; }
        if t == 0 || t >= self.blocks_count { return Err(FsError::Corrupt("inode table outside volume")); }
        Ok(t)
    }

    fn inode(&self, ino: u64) -> Result<Inode> {
        if ino == 0 || ino > self.inodes_count { return Err(FsError::Corrupt("inode number out of range")); }
        let (g, i) = ((ino - 1) / self.inodes_per_group, (ino - 1) % self.inodes_per_group);
        let off = self.inode_table(g)? * self.bs + i * self.inode_size;
        if off + self.inode_size > self.blocks_count * self.bs { return Err(FsError::Corrupt("inode outside volume")); }
        let mut b = vec![0u8; self.inode_size as usize];
        self.dev.read_at(off, &mut b)?;
        let mode = le16(&b, 0);
        let size = le32(&b, 4) as u64 | if mode & 0xF000 == 0x8000 { (le32(&b, 108) as u64) << 32 } else { 0 };
        let extra = if self.inode_size > 128 { le16(&b, 128) as usize } else { 0 };
        let ex = |o: usize| if extra + 128 >= o + 4 && self.inode_size as usize >= o + 4 { Some(le32(&b, o)) } else { None };
        Ok(Inode {
            mode, size, flags: le32(&b, 32), block: b[40..100].try_into().unwrap(), ino,
            mtime: ts(le32(&b, 16), ex(140)),
            crtime: if self.inode_size as usize >= 152 && extra >= 24 { ts(le32(&b, 144), ex(148)) } else { None },
        })
    }

    // ---- block mapping ----
    fn read_block_u32s(&self, blk: u64) -> Result<Vec<u8>> {
        if blk == 0 || blk >= self.blocks_count { return Err(FsError::Corrupt("indirect block outside volume")); }
        let mut b = vec![0u8; self.bs as usize];
        self.dev.read_at(blk * self.bs, &mut b)?;
        Ok(b)
    }

    /// Maps logical block -> (physical block, contiguous run length, zero-filled) or `None` for a hole.
    fn map(&self, inode: &Inode, lblk: u64) -> Result<Option<(u64, u64, bool)>> {
        if inode.flags & FL_EXTENTS != 0 { return self.map_extent(&inode.block, lblk, 0); }
        let per = self.bs / 4;
        let idx = |b: &[u8], i: u64| le32(b, (i * 4) as usize) as u64;
        let ok = |p: u64| -> Result<Option<(u64, u64, bool)>> {
            if p == 0 { return Ok(None); }
            if p >= self.blocks_count { return Err(FsError::Corrupt("data block outside volume")); }
            Ok(Some((p, 1, false)))
        };
        if lblk < 12 { return ok(le32(&inode.block, (lblk * 4) as usize) as u64); }
        let mut l = lblk - 12;
        for level in 1..=3u32 {
            let span = per.pow(level);
            if l < span {
                let mut blk = le32(&inode.block, (12 + level as usize - 1) * 4) as u64;
                for depth in (0..level).rev() {
                    if blk == 0 { return Ok(None); }
                    let b = self.read_block_u32s(blk)?;
                    let sub = per.pow(depth);
                    blk = idx(&b, (l / sub) % per);
                    l %= sub.max(1);
                }
                return ok(blk);
            }
            l -= span;
        }
        Err(FsError::Corrupt("logical block beyond triple-indirect range"))
    }

    fn map_extent(&self, node: &[u8], lblk: u64, depth_seen: u32) -> Result<Option<(u64, u64, bool)>> {
        if depth_seen > 6 || node.len() < 12 || le16(node, 0) != 0xF30A { return Err(FsError::Corrupt("bad extent header")); }
        let (entries, depth) = (le16(node, 2) as usize, le16(node, 6));
        if 12 + entries * 12 > node.len() { return Err(FsError::Corrupt("extent entries overflow node")); }
        let e = |i: usize| &node[12 + i * 12..24 + i * 12];
        if depth == 0 {
            for i in 0..entries {
                let (first, raw_len) = (le32(e(i), 0) as u64, le16(e(i), 4) as u64);
                let (len, uninit) = if raw_len > 32768 { (raw_len - 32768, true) } else { (raw_len, false) };
                if lblk >= first && lblk < first + len {
                    let start = le32(e(i), 8) as u64 | (le16(e(i), 6) as u64) << 32;
                    if start + len > self.blocks_count { return Err(FsError::Corrupt("extent outside volume")); }
                    let within = lblk - first;
                    return Ok(Some((start + within, len - within, uninit)));
                }
            }
            return Ok(None);
        }
        // Index node: last entry whose first logical block <= lblk.
        let mut pick = None;
        for i in 0..entries { if le32(e(i), 0) as u64 <= lblk { pick = Some(i) } else { break } }
        let Some(i) = pick else { return Ok(None) };
        let leaf = le32(e(i), 4) as u64 | (le16(e(i), 8) as u64) << 32;
        let b = self.read_block_u32s(leaf)?;
        self.map_extent(&b, lblk, depth_seen + 1)
    }

    fn read_range(&self, inode: &Inode, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if inode.flags & FL_ENCRYPT != 0 { return Err(FsError::Unsupported("encrypted inode")); }
        if inode.flags & FL_INLINE != 0 { return Err(FsError::Unsupported("inline-data inode")); }
        if offset >= inode.size || buf.is_empty() { return Ok(0); }
        let want = (buf.len() as u64).min(inode.size - offset) as usize;
        // Fast symlink: target lives in i_block.
        if inode.mode & 0xF000 == 0xA000 && inode.size < 60 && inode.flags & FL_EXTENTS == 0 && self.is_fast_symlink(inode) {
            buf[..want].copy_from_slice(&inode.block[offset as usize..offset as usize + want]);
            return Ok(want);
        }
        let mut done = 0usize;
        while done < want {
            let pos = offset + done as u64;
            let (lblk, within) = (pos / self.bs, pos % self.bs);
            match self.map(inode, lblk)? {
                None => {
                    let n = ((self.bs - within) as usize).min(want - done);
                    buf[done..done + n].fill(0); done += n;
                }
                Some((p, run, uninit)) => {
                    let n = ((run * self.bs - within).min((want - done) as u64)) as usize;
                    if uninit { buf[done..done + n].fill(0); }
                    else { self.dev.read_at(p * self.bs + within, &mut buf[done..done + n])?; }
                    done += n;
                }
            }
        }
        Ok(want)
    }

    fn is_fast_symlink(&self, inode: &Inode) -> bool {
        // i_blocks == 0 (no extra blocks, ignoring EA block) is the rule; we re-read it cheaply.
        let (g, i) = ((inode.ino - 1) / self.inodes_per_group, (inode.ino - 1) % self.inodes_per_group);
        let Ok(t) = self.inode_table(g) else { return false };
        let mut b = [0u8; 4];
        self.dev.read_at(t * self.bs + i * self.inode_size + 28, &mut b).is_ok() && u32::from_le_bytes(b) == 0
    }

    fn dir_entries(&self, dir: &Inode) -> Result<Vec<(u64, String)>> {
        if dir.kind() != Some(EntryKind::Directory) { return Err(FsError::NotADirectory(format!("inode {}", dir.ino))); }
        if dir.flags & FL_ENCRYPT != 0 { return Err(FsError::Unsupported("encrypted directory")); }
        if dir.flags & FL_INLINE != 0 { return Err(FsError::Unsupported("inline-data directory")); }
        if dir.size > MAX_DIR_BYTES || dir.size % self.bs != 0 { return Err(FsError::Corrupt("bad directory size")); }
        let mut out = Vec::new();
        let mut blk = vec![0u8; self.bs as usize];
        for l in 0..dir.size / self.bs {
            match self.map(dir, l)? {
                Some((p, _, false)) => self.dev.read_at(p * self.bs, &mut blk)?,
                _ => return Err(FsError::Corrupt("hole in directory")),
            }
            let mut o = 0usize;
            while o + 8 <= blk.len() {
                let ino = le32(&blk, o) as u64;
                let mut rl = le16(&blk, o + 4) as usize;
                if self.bs == 65536 && (rl == 0 || rl == 65535) { rl = 65536; }
                let nl = blk[o + 6] as usize;
                if rl < 8 || rl % 4 != 0 || o + rl > blk.len() || 8 + nl > rl { return Err(FsError::Corrupt("bad directory entry")); }
                if ino != 0 {
                    let name = String::from_utf8_lossy(&blk[o + 8..o + 8 + nl]).into_owned();
                    if name != "." && name != ".." { out.push((ino, name)); }
                }
                o += rl;
            }
        }
        Ok(out)
    }

    fn entry_of(&self, name: &str, ino: u64) -> Result<DirEntry> {
        let i = self.inode(ino)?;
        let kind = i.kind().unwrap_or(EntryKind::File);
        Ok(DirEntry {
            name: name.to_string(), kind,
            size: if kind == EntryKind::Directory || !i.is_regular_data() { 0 } else { i.size },
            modified: i.mtime, created: i.crtime,
            hidden: name.starts_with('.'), read_only: i.mode & 0o222 == 0, inode: ino,
        })
    }

    fn lookup_dir(&self, comps: &[&str]) -> Result<Inode> {
        let mut cur = self.inode(2)?;
        for c in comps {
            let ino = self.dir_entries(&cur)?.into_iter().find(|(_, n)| n == c).ok_or_else(|| FsError::NotFound((*c).into()))?.0;
            cur = self.inode(ino)?;
            if cur.kind() != Some(EntryKind::Directory) { return Err(FsError::NotADirectory((*c).into())); }
        }
        Ok(cur)
    }
}

impl Filesystem for ExtFs {
    fn name(&self) -> &'static str { self.version }
    fn label(&self) -> Option<String> { self.label.clone() }
    fn capabilities(&self) -> Capabilities { Capabilities::READ_ONLY }

    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        let d = self.lookup_dir(&normalize_path(path)?)?;
        self.dir_entries(&d)?.into_iter().map(|(ino, n)| self.entry_of(&n, ino)).collect()
    }

    fn stat(&self, path: &str) -> Result<DirEntry> {
        let comps = normalize_path(path)?;
        let Some((last, parents)) = comps.split_last() else {
            let mut e = self.entry_of("/", 2)?; e.hidden = false; return Ok(e);
        };
        let d = self.lookup_dir(parents)?;
        let (ino, _) = self.dir_entries(&d)?.into_iter().find(|(_, n)| n == last).ok_or_else(|| FsError::NotFound(path.into()))?;
        self.entry_of(last, ino)
    }

    fn read_at(&self, entry: &DirEntry, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if entry.kind == EntryKind::Directory { return Err(FsError::IsADirectory(entry.name.clone())); }
        let i = self.inode(entry.inode)?;
        if !i.is_regular_data() { return Ok(0); }
        self.read_range(&i, offset, buf)
    }
}
