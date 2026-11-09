//! End-to-end: a raw NTFS image is attached as a block device (hdiutil -nomount, owned by the test user), the real
//! `voluma-helper` binary serves it over its Unix socket, and ntfs-3g judges the result. macOS + VOLUMA_NTFS3G needed.
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

fn tool(name: &str) -> Option<PathBuf> {
    let d = std::env::var("VOLUMA_NTFS3G").ok()?;
    ["", "bin", "sbin"].iter().map(|s| Path::new(&d).join(s).join(name)).find(|p| p.is_file())
}

struct Attached(String);
impl Drop for Attached { fn drop(&mut self) { let _ = Command::new("hdiutil").args(["detach", &self.0, "-force"]).output(); } }
struct Helper(Child);
impl Drop for Helper { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }

struct Client { r: BufReader<UnixStream>, w: UnixStream }
impl Client {
    fn call(&mut self, v: Value) -> Value {
        writeln!(self.w, "{v}").unwrap();
        let mut l = String::new(); self.r.read_line(&mut l).unwrap();
        serde_json::from_str(&l).unwrap()
    }
}

fn setup() -> Option<(tempfile::TempDir, PathBuf, Attached, PathBuf, Helper, Client)> {
    if !cfg!(target_os = "macos") { eprintln!("SKIPPED: needs macOS"); return None; }
    let (Some(mkntfs), Some(pop)) = (tool("mkntfs"), tool("ntfs_populate")) else { eprintln!("SKIPPED: ntfs-3g tools not found (set VOLUMA_NTFS3G)"); return None; };
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("README.TXT"), b"hello voluma\n").unwrap();
    std::fs::write(src.join("docs/a.txt"), b"a").unwrap();
    let img = d.path().join("n.img");
    std::fs::File::create(&img).unwrap().set_len(40 << 20).unwrap();
    assert!(Command::new(mkntfs).args(["-F", "-Q", "-L", "HELPER"]).arg(&img).output().unwrap().status.success());
    assert!(Command::new(pop).arg(&img).arg(&src).output().unwrap().status.success());
    let o = Command::new("hdiutil").args(["attach", "-imagekey", "diskimage-class=CRawDiskImage", "-nomount"]).arg(&img).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let dev = String::from_utf8_lossy(&o.stdout).split_whitespace().next().unwrap().to_string();
    let att = Attached(dev.clone());
    let stage = d.path().join("stage");
    std::fs::create_dir(&stage).unwrap();
    let uid = unsafe { libc::getuid() };
    let child = Command::new(env!("CARGO_BIN_EXE_voluma-helper"))
        .args(["serve", "--device", &dev, "--stage", stage.to_str().unwrap(), "--uid", &uid.to_string(), "--parent", &std::process::id().to_string(), "--idle", "60"])
        .spawn().unwrap();
    let helper = Helper(child);
    let sock = stage.join("helper.sock");
    let t = Instant::now();
    while !sock.exists() { assert!(t.elapsed() < Duration::from_secs(10), "helper never came up"); std::thread::sleep(Duration::from_millis(50)); }
    let s = UnixStream::connect(&sock).unwrap();
    let c = Client { r: BufReader::new(s.try_clone().unwrap()), w: s };
    Some((d, img, att, stage, helper, c))
}

fn ok(v: &Value) -> &Value { assert!(v.get("error").is_none(), "{v}"); &v["ok"] }

#[test]
fn helper_serves_an_ntfs_block_device() {
    let Some((d, img, att, stage, mut helper, mut c)) = setup() else { return };
    assert_eq!(c.call(json!({"op": "ping"}))["ok"], "pong");
    let l = c.call(json!({"op": "list", "path": "/"}));
    assert_eq!(ok(&l)["filesystem"], "NTFS", "{l}");
    assert!(ok(&l)["entries"].as_array().unwrap().iter().any(|e| e["name"] == "README.TXT"));
    assert!(ok(&l)["write_blocked"].is_null(), "clean volume: {l}");

    // write: mkdir, import (via the staging dir), rename, export
    ok(&c.call(json!({"op": "mkdir", "path": "/pasted"})));
    std::fs::write(stage.join("in.bin"), vec![0x5Au8; 700_000]).unwrap();
    let r = c.call(json!({"op": "import", "src": "in.bin", "path": "/pasted/big.bin"}));
    assert_eq!(ok(&r)["bytes"], 700_000, "{r}");
    assert!(c.call(json!({"op": "import", "src": "in.bin", "path": "/pasted/big.bin"})).get("error").is_some(), "no silent overwrite");
    ok(&c.call(json!({"op": "rename", "path": "/pasted/big.bin", "to": "/pasted/renamed.bin"})));
    let e = c.call(json!({"op": "export", "path": "/pasted/renamed.bin", "dst": "out.bin"}));
    assert_eq!(ok(&e)["bytes"], 700_000, "{e}");
    assert_eq!(std::fs::read(stage.join("out.bin")).unwrap(), vec![0x5Au8; 700_000]);
    ok(&c.call(json!({"op": "mkdir", "path": "/gone"})));
    ok(&c.call(json!({"op": "delete", "path": "/gone"})));

    // abuse attempts must all be refused
    for bad in ["../../etc/hosts", "/etc/hosts", "sub/x", "..", "helper.sock", ""] {
        let r = c.call(json!({"op": "import", "src": bad, "path": "/evil"}));
        assert!(r.get("error").is_some(), "import src {bad:?} must be refused: {r}");
        let r = c.call(json!({"op": "export", "path": "/README.TXT", "dst": bad}));
        assert!(r.get("error").is_some(), "export dst {bad:?} must be refused: {r}");
    }
    std::os::unix::fs::symlink("/etc/hosts", stage.join("link")).unwrap();
    assert!(c.call(json!({"op": "import", "src": "link", "path": "/evil"})).get("error").is_some(), "symlinks are refused");
    assert!(c.call(json!({"op": "nonsense"})).get("error").is_some());
    assert!(c.call(json!({"op": "list", "path": "/evil"})).get("error").is_some(), "nothing was written");

    assert_eq!(c.call(json!({"op": "quit"}))["ok"], "bye");
    helper.0.wait().unwrap();
    assert!(!stage.join("helper.sock").exists(), "socket removed on exit");
    // a clean (non-forced) detach must succeed: nothing may still hold the device
    let o = Command::new("hdiutil").args(["detach", &att.0]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    if std::env::var("VOLUMA_KEEP_DIR").is_ok() { let _ = std::fs::copy(&img, Path::new(&std::env::var("VOLUMA_KEEP_DIR").unwrap()).join("helper.img")); }
    // ntfs-3g agrees
    let cat = Command::new(tool("ntfscat").unwrap()).arg("-f").arg(&img).arg("/pasted/renamed.bin").output().unwrap();
    assert_eq!(cat.stdout.len(), 700_000, "ntfscat: {}", String::from_utf8_lossy(&cat.stderr));
    assert_eq!(cat.stdout, vec![0x5Au8; 700_000]);
    let o = Command::new(tool("ntfsresize").unwrap()).args(["-i", "-f"]).arg(&img).output().unwrap();
    assert!(o.status.success(), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    drop(d);
}

#[test]
fn helper_rejects_bad_configuration() {
    let d = tempfile::tempdir().unwrap();
    let run = |dev: &str| Command::new(env!("CARGO_BIN_EXE_voluma-helper"))
        .args(["serve", "--device", dev, "--stage", d.path().to_str().unwrap(), "--uid", "0", "--parent", "1"]).output().unwrap().status.success();
    for bad in ["/etc/passwd", "/dev/null", "/dev/disk", "/dev/diskXs1", "/dev/disk1s", "/dev/disk1/../../etc", "disk1"] {
        assert!(!run(bad), "device {bad:?} must be refused");
    }
    // stage dir owned by someone else than --uid is refused (we are not uid 0)
    assert!(!run("/dev/disk99"));
}
