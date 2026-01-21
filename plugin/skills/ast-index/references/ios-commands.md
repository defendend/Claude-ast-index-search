# iOS-Specific Commands

Commands for iOS/Swift/Objective-C projects.

## Storyboard & XIB Commands

**Find class usages in storyboards/xibs**:
```bash
ast-index storyboard-usages "MyViewController"
ast-index storyboard-usages "TableViewCell" --module "Features"
```

## Asset Commands (xcassets)

**Find iOS asset usages**:
```bash
ast-index asset-usages "AppIcon"
ast-index asset-usages --unused --module "MainApp"  # find unused assets
```

Supported asset types: imageset, colorset, appiconset, dataset

## SwiftUI Commands

**Find SwiftUI state properties**:
```bash
ast-index swiftui                    # all @State/@Binding/@Published
ast-index swiftui "State"            # filter by type
ast-index swiftui "userName"         # filter by name
```

Finds: @State, @Binding, @Published, @ObservedObject, @StateObject, @EnvironmentObject

## Swift Concurrency Commands

**Find async functions**:
```bash
ast-index async-funcs
ast-index async-funcs "fetch"
```

**Find @MainActor usages**:
```bash
ast-index main-actor
ast-index main-actor "ViewModel"
```

## Combine Commands

**Find Combine publishers**:
```bash
ast-index publishers                 # PassthroughSubject, CurrentValueSubject, AnyPublisher
ast-index publishers "state"
```

## Indexed Swift Constructs

- `class`, `struct`, `enum`, `protocol`, `actor`
- `extension` (indexed as `TypeName+Extension`)
- `func`, `init`, `var`, `let`, `typealias`
- Inheritance and protocol conformance

## Indexed Objective-C Constructs

- `@interface` with superclass and protocols
- `@protocol` definitions
- `@implementation`
- Methods (`-`/`+`), `@property`, `typedef`
- Categories (indexed as `TypeName+Category`)

## Module Detection

**SPM** - Parses `Package.swift`:
- `.target(name: "...")`, `.testTarget(name: "...")`, `.binaryTarget(name: "...")`

**CocoaPods** - Parses `Podfile` and `Podfile.lock`

**Carthage** - Parses `Cartfile` and `Cartfile.resolved`
