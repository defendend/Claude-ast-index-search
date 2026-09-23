; Struct definition
(struct_item
  name: (type_identifier) @struct_name) @definition

; Enum definition
(enum_item
  name: (type_identifier) @enum_name) @definition

; Trait definition
(trait_item
  name: (type_identifier) @trait_name) @definition

; Impl Trait for Type
(impl_item
  trait: (_) @impl_trait
  type: (_) @impl_trait_type) @definition

; Impl Type (self impl, no trait)
(impl_item
  !trait
  type: (_) @impl_self_type) @definition

; Standalone function
(function_item
  name: (identifier) @func_name) @definition

; Function signature in trait (no body)
(function_signature_item
  name: (identifier) @func_sig_name) @definition

; Macro definition
(macro_definition
  name: (identifier) @macro_name) @definition

; Type alias
(type_item
  name: (type_identifier) @type_alias_name) @definition

; Constant
(const_item
  name: (identifier) @const_name) @definition

; Static
(static_item
  name: (identifier) @static_name) @definition

; Module
(mod_item
  name: (identifier) @mod_name) @definition

; Use declaration with scoped path
(use_declaration
  argument: (scoped_identifier) @use_path) @definition

; Use declaration with use_as_clause containing scoped path
(use_declaration
  argument: (use_as_clause
    path: (scoped_identifier) @use_alias_path)) @definition

; Attribute item (for derive and other attributes)
(attribute_item
  (attribute) @attr) @definition
