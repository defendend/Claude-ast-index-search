; Class declarations (class, struct, actor) with optional inheritance
(class_declaration
  declaration_kind: ["class" "struct" "actor"] @decl_kind
  name: (type_identifier) @class_name) @definition

; Enum declaration with optional inheritance
(class_declaration
  declaration_kind: "enum"
  name: (type_identifier) @enum_name) @definition

; Extension declaration
(class_declaration
  declaration_kind: "extension"
  name: (_) @ext_type) @definition

; Protocol declaration with optional inheritance
(protocol_declaration
  name: (type_identifier) @protocol_name) @definition

; Function declarations (top-level and in class/struct/actor/enum bodies)
(function_declaration
  name: (simple_identifier) @func_name) @definition

; Protocol function declarations
(protocol_function_declaration
  name: (simple_identifier) @func_name) @definition

; Init declarations
(init_declaration
  name: "init" @init_name) @definition

; Property declarations (var/let in class/struct/actor/enum bodies)
(property_declaration
  name: (pattern
    (simple_identifier) @prop_name)) @definition

; Protocol property declarations
(protocol_property_declaration
  name: (pattern
    (simple_identifier) @prop_name)) @definition

; Typealias declarations
(typealias_declaration
  name: (type_identifier) @typealias_name) @definition

; Import declarations: `import Foo`, `@testable import Foo.Bar` -> module `Foo`
(import_declaration
  (identifier . (simple_identifier) @import_name)) @definition
