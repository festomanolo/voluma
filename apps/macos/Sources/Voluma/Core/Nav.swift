import SwiftUI

/// Lets the window-level Back button defer to the folder history of whichever browser is on screen.
@MainActor
final class NavState: ObservableObject {
    @Published var innerCanBack = false
    var innerBack: (() -> Void)?
}
