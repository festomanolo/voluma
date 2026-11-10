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

/// Lets windows group into native macOS tabs, and opens a new tab next to the current window (⌘T).
struct WindowTabbing: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let v = NSView()
        DispatchQueue.main.async { v.window?.tabbingMode = .preferred; v.window?.tabbingIdentifier = "voluma.main" }
        return v
    }
    func updateNSView(_ v: NSView, context: Context) {
        DispatchQueue.main.async { v.window?.tabbingMode = .preferred; v.window?.tabbingIdentifier = "voluma.main" }
    }

    @MainActor static func newTab() {
        guard let current = NSApp.keyWindow, let wc = current.windowController else { return }
        wc.newWindowForTab(nil)
        if let new = NSApp.keyWindow, new !== current {
            current.addTabbedWindow(new, ordered: .above)
            new.makeKeyAndOrderFront(nil)
        }
    }
}
