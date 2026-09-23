; Import: import X
(import_statement
  name: (dotted_name) @import_name) @definition

; Import: import X as Y
(import_statement
  name: (aliased_import
    name: (dotted_name) @import_alias_original
    alias: (identifier) @import_alias_name)) @definition

; Import: from X import Y, Z
(import_from_statement
  module_name: (dotted_name) @import_from_module
  name: (dotted_name) @import_from_name) @definition

; Import: from X import Y as Z
(import_from_statement
  module_name: (dotted_name) @import_from_module_alias
  name: (aliased_import
    name: (dotted_name) @import_from_aliased_name)) @definition

; Class definition (with or without parents)
(class_definition
  name: (identifier) @class_name
  superclasses: (argument_list)? @class_parents) @definition

; Decorated class — get the decorator
(decorated_definition
  (decorator) @decorator @definition
  (class_definition))

; Decorated function — get the decorator
(decorated_definition
  (decorator) @func_decorator @definition
  (function_definition))

; Functions at module level
(module
  (function_definition
    name: (identifier) @func_name) @definition)

; Functions inside decorated_definition at module level
(module
  (decorated_definition
    (function_definition
      name: (identifier) @decorated_func_name) @definition))

; Functions inside class (methods)
(class_definition
  body: (block
    (function_definition
      name: (identifier) @method_name) @definition))

; Decorated methods inside class
(class_definition
  body: (block
    (decorated_definition
      (function_definition
        name: (identifier) @decorated_method_name) @definition)))

; Module-level assignments (constants and type aliases)
(module
  (expression_statement
    (assignment
      left: (identifier) @assignment_name
      right: (_) @assignment_value) @definition))
