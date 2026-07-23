# Matlab Commands Reference

ast-index supports parsing and indexing Matlab source files (`.m`).

## Supported Elements

| Matlab Element | Symbol Kind | Example |
|----------------|-------------|---------|
| `classdef ClassName` | Class | `Vehicle` → Class |
| `function name` | Function | `calculate` → Function |
| `properties` block members | Property | `Speed` → Property |
| `enumeration` members | Constant | `Red` → Constant |
| `events` block members | Property | `ButtonPressed` → Property |

## File Detection

`.m` files are shared between Matlab and Objective-C. ast-index automatically detects the language by inspecting file content:

- **Matlab markers**: `classdef`, `function`, `%` comments
- **ObjC markers**: `#import`, `@interface`, `//` comments

Mixed projects with both Matlab and ObjC `.m` files are handled correctly.

## Core Commands

### Search Classes

Find Matlab class definitions:

```bash
ast-index class "Vehicle"           # Find Vehicle class
ast-index class "handle"            # Find handle-derived classes
ast-index implementations "handle"  # Find all classes extending handle
```

### Search Functions

Find functions and methods:

```bash
ast-index symbol "calculate"        # Find functions containing "calculate"
ast-index callers "processData"     # Find callers of processData
```

### Class Hierarchy

```bash
ast-index hierarchy "Vehicle"       # Show Vehicle class hierarchy
ast-index implementations "handle"  # Find all handle subclasses
```

### File Analysis

Show file structure:

```bash
ast-index outline "Vehicle.m"       # Show class, properties, methods
ast-index usages "Vehicle"          # Find usages of Vehicle
```

## Example Workflow

```bash
# 1. Index Matlab project
cd /path/to/matlab/project
ast-index rebuild

# 2. Check index statistics
ast-index stats

# 3. Find all classes
ast-index search "classdef"

# 4. Find all functions
ast-index search "function"

# 5. Show file structure
ast-index outline "Vehicle.m"

# 6. Find implementations of a base class
ast-index implementations "handle"

# 7. Find usages
ast-index usages "Vehicle"
```

## Indexed Matlab Patterns

### Class Definition with Inheritance

```matlab
classdef Vehicle < handle
    properties
        Make
        Model
        Year
    end
    methods
        function obj = Vehicle(make, model, year)
            obj.Make = make;
            obj.Model = model;
            obj.Year = year;
        end
    end
    enumeration
        Car, Truck, SUV
    end
end
```

Indexed as:
- `Vehicle` [class] extends `handle`
- `Make`, `Model`, `Year` [property] member_of `Vehicle`
- `Vehicle` [function] member_of `Vehicle` (constructor)
- `Car`, `Truck`, `SUV` [constant] member_of `Vehicle`

### Standalone Function

```matlab
function result = myFunction(x, y)
    result = x + y;
end
```

Indexed as:
- `myFunction` [function]
