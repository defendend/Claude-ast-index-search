; Class definitions (class, case class, abstract class)
(class_definition
  name: (identifier) @class_name) @class_decl @definition

; Object definitions (object, case object)
(object_definition
  name: (identifier) @object_name) @object_decl @definition

; Trait definitions
(trait_definition
  name: (identifier) @trait_name) @trait_decl @definition

; Enum definitions (Scala 3)
(enum_definition
  name: (identifier) @enum_name) @definition

; Function definitions (def foo = ...)
(function_definition
  name: (identifier) @func_name) @definition

; Function declarations (abstract def)
(function_declaration
  name: (identifier) @func_decl_name) @definition

; Val definitions (val x = ...)
(val_definition
  pattern: (identifier) @val_name) @definition

; Val declarations (abstract val x: T)
(val_declaration
  name: (identifier) @val_decl_name) @definition

; Var definitions (var x = ...)
(var_definition
  pattern: (identifier) @var_name) @definition

; Var declarations (abstract var x: T)
(var_declaration
  name: (identifier) @var_decl_name) @definition

; Type definitions (type aliases)
(type_definition
  name: (type_identifier) @type_name) @definition

; Given definitions (Scala 3)
(given_definition
  name: (identifier) @given_name) @definition
