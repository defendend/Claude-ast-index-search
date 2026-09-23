; Classes and interfaces (grammar uses class_declaration for both)
; Modifiers like enum, data, sealed come via modifiers/class_modifier
(class_declaration
  name: (identifier) @class_name) @class_decl @definition

; Object declarations (singleton)
(object_declaration
  name: (identifier) @object_name) @object_decl @definition

; Function declarations (including suspend, extension, etc.)
(function_declaration
  name: (identifier) @func_name) @definition

; Property declarations (val/var)
(property_declaration
  (variable_declaration
    (identifier) @property_name)) @definition

; Type alias
(type_alias
  type: (identifier) @typealias_name) @definition

; Java-style field declarations (public static final Type NAME = val)
; These appear as property_declaration in Kotlin grammar too but
; for Java files parsed as Kotlin, we handle ALL_CAPS in Rust code
