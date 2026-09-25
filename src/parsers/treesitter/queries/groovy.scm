; Package declaration
(package_declaration
  (identifier) @package_name) @definition
(package_declaration
  (scoped_identifier) @package_name) @definition

; Import declarations
(import_declaration
  (identifier) @import_path) @definition
(import_declaration
  (scoped_identifier) @import_path) @definition

; Class declarations
(class_declaration
  name: (identifier) @class_name) @definition

; Interface declarations
(interface_declaration
  name: (identifier) @interface_name) @definition

; Enum declarations
(enum_declaration
  name: (identifier) @enum_name) @definition

; Method declarations
(method_declaration
  name: (identifier) @method_name) @definition

; Constructor declarations
(constructor_declaration
  name: (identifier) @constructor_name) @definition

; Field declarations
(field_declaration
  declarator: (variable_declarator
    name: (identifier) @field_name)) @definition
