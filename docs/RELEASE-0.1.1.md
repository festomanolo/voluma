Small fix release.

### Fixed
- Browsing a drive through the helper (NTFS writing, or reading an unmounted partition such as ext4) briefly showed a wrong "read-only" banner while the first listing loaded. It now says "Connecting through VOLUMA's driver..." until the listing arrives, then shows the real state.

### Also
- `pip install nolo-voluma` (or `pip install -U nolo-voluma`) installs this version, and `voluma` opens it.

### Install
Unzip, drag `VOLUMA.app` to Applications. The build is ad-hoc signed and **not notarised**: right-click the app and choose Open the first time, or run `xattr -cr /Applications/VOLUMA.app`. Requires macOS 14 or later. SHA-256 is in the attached `.sha256` file.

### Known limits (unchanged)
- NTFS writing is verified against ntfs-3g's tools, not Windows chkdsk. Back up anything irreplaceable.
- The universal build was built and tested on an Intel Mac; the Apple Silicon half has not been run on real Apple Silicon hardware.
- Compressed or encrypted NTFS files, hard links and alternate data streams are refused for writing. ext4, APFS and HFS+ are read-only.

Full changes: https://github.com/festomanolo/voluma/compare/v0.1.0...v0.1.1
