// Harness around StorageScanner (the code Storage Lens uses). Prints one JSON object: bytes, files, seconds.
import Foundation
let p = ScanProgress()
let r = StorageScanner.scan(root: URL(fileURLWithPath: CommandLine.arguments[1]), progress: p)
print("{\"bytes\":\(r.root.size),\"files\":\(r.root.itemCount),\"seconds\":\(String(format: "%.3f", r.seconds)),\"unreadable\":\(r.unreadableFolders)}")
