import SwiftUI

struct SimulatedBadge: View {
    var body: some View {
        Text("SIMULATED")
            .font(.system(size: 9, weight: .semibold)).tracking(0.6)
            .padding(.horizontal, 6).padding(.vertical, 2)
            .background(.orange.opacity(0.18), in: Capsule())
            .foregroundStyle(.orange)
            .help("Placeholder data; not read from real hardware")
    }
}

struct UsageBar: View {
    let fraction: Double
    var body: some View {
        GeometryReader { g in
            ZStack(alignment: .leading) {
                Capsule().fill(.quaternary)
                Capsule().fill(Color.accentColor).frame(width: g.size.width * min(max(fraction, 0), 1))
            }
        }.frame(height: 5)
    }
}

func formatBytes(_ n: Int64) -> String {
    ByteCountFormatter.string(fromByteCount: n, countStyle: .file)
}
