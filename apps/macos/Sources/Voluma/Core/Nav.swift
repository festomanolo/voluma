import SwiftUI

/// Lets the window-level Back button defer to the folder history of whichever browser is on screen.
@MainActor
final class NavState: ObservableObject {
    @Published var innerCanBack = false
    var innerBack: (() -> Void)?
    /// Swap the open browser for another target without adding a history entry (used when NTFS writing is enabled).
    var replaceBrowse: ((BrowseTarget) -> Void)?
    /// Leave the browser and go back to the page it was opened from.
    var closeBrowser: (() -> Void)?
}
