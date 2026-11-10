// Command-line harness for FastTransfer: `fasttransfer copy|move SRC... DSTDIR`.
// Build: tests/bench/fasttransfer.sh
import Foundation

let a = Array(CommandLine.arguments.dropFirst())
guard a.count >= 3, a[0] == "copy" || a[0] == "move" else { print("usage: fasttransfer copy|move SRC... DSTDIR"); exit(2) }
let mode: FastTransfer.Mode = a[0] == "move" ? .move : .copy
let srcs = a[1..<(a.count - 1)].map { URL(fileURLWithPath: $0) }
let dst = URL(fileURLWithPath: a.last!)
let p = TransferProgress()
let r = FastTransfer.run(srcs, into: dst, mode: mode, progress: p)
let mb = Double(r.bytes) / 1_048_576
print(String(format: "%@ %d files, %.1f MB in %.3fs = %.0f MB/s, %.0f files/s · cloned %d · renamed %d · errors %d%@",
             a[0], r.files, mb, r.seconds, mb / max(r.seconds, 1e-6), Double(r.files) / max(r.seconds, 1e-6), r.cloned, r.renamed, r.errors,
             r.firstError.map { " (\($0))" } ?? ""))
exit(r.errors == 0 ? 0 : 1)
