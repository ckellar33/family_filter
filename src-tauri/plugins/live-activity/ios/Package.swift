// swift-tools-version:5.9

import PackageDescription

let package = Package(
  name: "tauri-plugin-live-activity",
  platforms: [
    .iOS(.v17)
  ],
  products: [
    .library(
      name: "tauri-plugin-live-activity",
      type: .static,
      targets: ["tauri-plugin-live-activity"])
  ],
  dependencies: [
    .package(name: "Tauri", path: "../.tauri/tauri-api")
  ],
  targets: [
    .target(
      name: "tauri-plugin-live-activity",
      dependencies: [
        .byName(name: "Tauri")
      ],
      path: "Sources")
  ]
)
