import SwiftUI
import AppKit

@main
struct VolumaApp: App {
    var body: some Scene {
        WindowGroup { ContentView().background(WindowTabbing()) }   // ⌘N opens a new window (built in)
            .windowToolbarStyle(.unified)
    }
}
