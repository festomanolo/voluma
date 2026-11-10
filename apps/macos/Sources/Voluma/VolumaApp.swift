import SwiftUI
import AppKit

@main
struct VolumaApp: App {
    var body: some Scene {
        WindowGroup { ContentView().background(WindowTabbing()) }   // ⌘N opens a new window (built in)
            .windowToolbarStyle(.unified)
            .commands {
                CommandGroup(after: .newItem) {
                    Button("New Tab") { WindowTabbing.newTab() }.keyboardShortcut("t", modifiers: .command)
                }
            }
    }
}
