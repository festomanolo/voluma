import Foundation
import SwiftUI

extension Listing.Entry {
    var createdSort: Int64 { created ?? modified ?? 0 }
    var tagsSort: String { (tags ?? []).first ?? "~" }   // untagged items sort last
}

/// Finder's "Sort By" choices. The Table's sort order is the single source of truth; these map to and from it.
enum SortKey: String, CaseIterable, Identifiable {
    case name = "Name", kind = "Kind", modified = "Date Modified", created = "Date Created", size = "Size", tags = "Tags"
    var id: String { rawValue }

    func comparators(ascending: Bool) -> [KeyPathComparator<Listing.Entry>] {
        let o: SortOrder = ascending ? .forward : .reverse
        switch self {
        case .name: return [KeyPathComparator(\Listing.Entry.name, comparator: .localizedStandard, order: o)]
        case .kind: return [KeyPathComparator(\Listing.Entry.kindText, comparator: .localizedStandard, order: o), KeyPathComparator(\Listing.Entry.name, comparator: .localizedStandard)]
        case .modified: return [KeyPathComparator(\Listing.Entry.modifiedSort, order: o)]
        case .created: return [KeyPathComparator(\Listing.Entry.createdSort, order: o)]
        case .size: return [KeyPathComparator(\Listing.Entry.size, order: o)]
        case .tags: return [KeyPathComparator(\Listing.Entry.tagsSort, order: o), KeyPathComparator(\Listing.Entry.name, comparator: .localizedStandard)]
        }
    }

    static func of(_ order: [KeyPathComparator<Listing.Entry>]) -> SortKey? {
        guard let k = order.first?.keyPath else { return nil }
        if k == \Listing.Entry.name { return .name }
        if k == \Listing.Entry.kindText { return .kind }
        if k == \Listing.Entry.modifiedSort { return .modified }
        if k == \Listing.Entry.createdSort { return .created }
        if k == \Listing.Entry.size { return .size }
        if k == \Listing.Entry.tagsSort { return .tags }
        return nil
    }
}

/// Finder's "Group By" choices: the listing is cut into titled segments.
enum GroupKey: String, CaseIterable, Identifiable {
    case none = "None", name = "Name", kind = "Kind", modified = "Date Modified", created = "Date Created", size = "Size", tags = "Tags"
    var id: String { rawValue }
    var sortKey: SortKey? { switch self { case .none: nil; case .name: .name; case .kind: .kind; case .modified: .modified; case .created: .created; case .size: .size; case .tags: .tags } }
}

struct EntryGroup: Identifiable {
    let id: String; let title: String; var entries: [Listing.Entry]
}

enum Grouper {
    /// Cuts already-sorted entries into segments. Segment order: dates newest first, sizes smallest first, text A to Z
    /// (flipped when `reverse`); the order inside a segment is the order passed in.
    static func group(_ entries: [Listing.Entry], by key: GroupKey, reverse: Bool, now: Date = Date()) -> [EntryGroup] {
        var buckets: [String: (rank: Int, entries: [Listing.Entry])] = [:]
        var order: [String] = []
        for e in entries {
            let (rank, title) = bucket(e, key, now)
            if buckets[title] == nil { buckets[title] = (rank, []); order.append(title) }
            buckets[title]!.entries.append(e)
        }
        var out = order.map { t -> (Int, String, [Listing.Entry]) in (buckets[t]!.rank, t, buckets[t]!.entries) }
        out.sort { a, b in a.0 != b.0 ? a.0 < b.0 : a.1.localizedStandardCompare(b.1) == .orderedAscending }
        if reverse { out.reverse() }
        return out.map { EntryGroup(id: $0.1, title: $0.1, entries: $0.2) }
    }

    private static func bucket(_ e: Listing.Entry, _ key: GroupKey, _ now: Date) -> (Int, String) {
        switch key {
        case .none: return (0, "")
        case .name:
            let c = e.name.trimmingCharacters(in: CharacterSet(charactersIn: ".")).first.map { String($0).uppercased() } ?? "#"
            return c.rangeOfCharacter(from: .letters) != nil ? (1, c) : (0, "#")
        case .kind: return (0, e.kindText)
        case .modified: return dateBucket(e.modified, now)
        case .created: return dateBucket(e.created ?? e.modified, now)
        case .size:
            if e.isDir { return (0, "Folders") }
            let s = e.size
            switch s {
            case 0: return (1, "Empty")
            case ..<(100 << 10): return (2, "Under 100 KB")
            case ..<(1 << 20): return (3, "100 KB to 1 MB")
            case ..<(10 << 20): return (4, "1 to 10 MB")
            case ..<(100 << 20): return (5, "10 to 100 MB")
            case ..<(1 << 30): return (6, "100 MB to 1 GB")
            default: return (7, "Over 1 GB")
            }
        case .tags:
            guard let t = e.tags?.first else { return (1000, "No Tags") }
            let base = t.components(separatedBy: "\n").first ?? t
            if let i = FinderTag.allCases.firstIndex(where: { $0.rawValue == base }) { return (i, base) }
            return (100, base)
        }
    }

    static func dateBucket(_ ts: Int64?, _ now: Date) -> (Int, String) {
        guard let ts else { return (99_999, "No Date") }
        let cal = Calendar.current
        let d = Date(timeIntervalSince1970: TimeInterval(ts))
        let days = cal.dateComponents([.day], from: cal.startOfDay(for: d), to: cal.startOfDay(for: now)).day ?? 0
        if days <= 0 { return (0, "Today") }
        if days == 1 { return (1, "Yesterday") }
        if days <= 7 { return (2, "Previous 7 Days") }
        if days <= 30 { return (3, "Previous 30 Days") }
        let (y, ny) = (cal.component(.year, from: d), cal.component(.year, from: now))
        if y == ny {
            let m = cal.component(.month, from: d), nm = cal.component(.month, from: now)
            let f = DateFormatter(); f.dateFormat = "LLLL"
            return (100 + (nm - m), f.string(from: d))
        }
        return (1000 + (ny - y), String(y))
    }
}
