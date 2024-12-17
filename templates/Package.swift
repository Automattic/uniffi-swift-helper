// swift-tools-version: 6.0

import Foundation
import PackageDescription

let projectName = "{{ project_name }}"
let packageName = "{{ package_name }}"
let ffiModuleName = "{{ ffi_module_name }}"

// SPM targets
let libraryTargetName = packageName
let swiftWrapperTargetName = "\(packageName)Internal"
let testTargetName = "\(packageName)Tests"

// Source code paths
let ffiSwiftWrapperSourcePath = "target/\(ffiModuleName)/swift-wrapper"
let ffiXCFrameworkPath = "target/\(ffiModuleName)/\(ffiModuleName).xcframework"

let ffiVersion: FFIVersion = .local

#if os(Linux)
let ffiTarget: Target = .systemLibrary(
        name: ffiModuleName,
        path: "target/release/\(ffiModuleName)-linux/"
    )
#elseif os(macOS)
let ffiTarget: Target = ffiVersion.target
#endif

var package = Package(
    name: packageName,
    platforms: [
        .iOS(.v13),
        .macOS(.v11),
        .tvOS(.v13),
        .watchOS(.v8)
    ],
    products: [
        .library(
            name: packageName,
            targets: [libraryTargetName]
        )
    ],
    dependencies: [],
    targets: [
        {% for target in targets %}
        .target(
            name: "{{ target.name }}",
            dependencies: [
                .target(name: swiftWrapperTargetName),
                {% for dep in target.dependencies %}
                .target(name: "{{ dep }}"),
                {% endfor %}
            ],
            path: "{{ target.library_source_path }}",
            swiftSettings: [
                .enableExperimentalFeature("StrictConcurrency"),
            ]
        ),
        {% endfor %}

        .target(
            name: swiftWrapperTargetName,
            dependencies: [
                .target(name: ffiTarget.name)
            ],
            path: ffiSwiftWrapperSourcePath,
            swiftSettings: [
                .swiftLanguageMode(.v5)
            ]
        ),
        ffiTarget,
        {% for target in targets %}
            .testTarget(
                name: "{{ target.name }}Tests",
                dependencies: [
                    .target(name: "{{ target.name }}"),
                    .target(name: ffiTarget.name)
                ],
                path: "{{ target.test_source_path }}",
                resources: [
                    {% if target.has_test_resources %}
                    .process("Resources")
                    {% endif %}
                ]
            )
        {% endfor %}
    ]
)

// MARK: - Enable local development toolings

let localDevelopment = ffiVersion.isLocal

if localDevelopment {
    try enableSwiftLint()
}

// MARK: - Helpers

enum FFIVersion {
    case local
    case release(version: String, checksum: String)

    var isLocal: Bool {
        if case .local = self {
            return true
        }
        return false
    }

    var target: Target {
        switch self {
        case .local:
            return .binaryTarget(name: ffiModuleName, path: ffiXCFrameworkPath)
        case let .release(version, checksum):
            return .binaryTarget(
                name: ffiModuleName,
                url: "https://cdn.a8c-ci.services/\(projectName)/\(version)/\(ffiModuleName).xcframework.zip",
                checksum: checksum
            )
        }
    }
}

// Add SwiftLint to the package so that we can see linting issues directly from Xcode.
@MainActor
func enableSwiftLint() throws {
#if os(macOS)
    let filePath = URL(string:"./.swiftlint.yml", relativeTo: URL(filePath: #filePath))!
    let version = try String(contentsOf: filePath, encoding: .utf8)
        .split(separator: "\n")
        .first(where: { $0.starts(with: "swiftlint_version") })?
        .split(separator: ":")
        .last?
        .trimmingCharacters(in: .whitespaces)
    guard let version else {
        fatalError("Can't find swiftlint_version in .swiftlint.yml")
    }

    package.dependencies.append(.package(url: "https://github.com/realm/SwiftLint", exact: .init(version)!))

    var platforms = package.platforms ?? []
    if let mac = platforms.firstIndex(where: { $0 == .macOS(.v11) }) {
        platforms.remove(at: mac)
        platforms.append(.macOS(.v12))
    }
    package.platforms = platforms

    if let target = package.targets.first(where: { $0.name == "WordPressAPI" }) {
        target.plugins = (target.plugins ?? []) + [.plugin(name: "SwiftLintBuildToolPlugin", package: "SwiftLint")]
    }
#endif
}
