// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "DeskBridgeMac",
    platforms: [
        .macOS(.v14)
    ],
    products: [
        .executable(name: "DeskBridgeMac", targets: ["DeskBridgeMac"])
    ],
    targets: [
        .executableTarget(name: "DeskBridgeMac")
    ]
)

