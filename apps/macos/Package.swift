// swift-tools-version:6.0
import PackageDescription

let package = Package(
    name: "Voluma",
    platforms: [.macOS(.v14)],
    targets: [
        .target(name: "CVoluma", path: "Sources/CVoluma"),
        .executableTarget(
            name: "Voluma",
            dependencies: ["CVoluma"],
            path: "Sources/Voluma",
            swiftSettings: [.swiftLanguageMode(.v5)],
            linkerSettings: [.linkedLibrary("voluma_ffi")]
        ),
    ]
)
