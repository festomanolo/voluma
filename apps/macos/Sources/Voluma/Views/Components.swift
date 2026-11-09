import SwiftUI

struct UsageBar: View {
    let fraction: Double
    var body: some View {
        GeometryReader { g in
            ZStack(alignment: .leading) {
                Capsule().fill(.quaternary)
                Capsule().fill(fraction > 0.9 ? Color.red : Color.accentColor).frame(width: g.size.width * min(max(fraction, 0), 1))
            }
        }.frame(height: 5)
    }
}

func formatBytes(_ n: Int64) -> String {
    ByteCountFormatter.string(fromByteCount: n, countStyle: .file)
}
