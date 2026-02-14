; === Classes ===

; class Name extends X implements Y { }
(class_declaration
  name: (type_identifier) @class_name) @class_node

; abstract class Name extends X { }
(abstract_class_declaration
  name: (type_identifier) @abstract_class_name) @abstract_class_node

; export class / export abstract class (wrapped in export_statement)
(export_statement
  (class_declaration
    name: (type_identifier) @export_class_name) @export_class_node)

(export_statement
  (abstract_class_declaration
    name: (type_identifier) @export_abstract_class_name) @export_abstract_class_node)

; === Interfaces ===

; interface Name extends X { }
(interface_declaration
  name: (type_identifier) @interface_name) @interface_node

(export_statement
  (interface_declaration
    name: (type_identifier) @export_interface_name) @export_interface_node)

; === Type aliases ===

; type Name = ...
(type_alias_declaration
  name: (type_identifier) @type_alias_name)

(export_statement
  (type_alias_declaration
    name: (type_identifier) @export_type_alias_name))

; === Enums ===

; enum Name { }
(enum_declaration
  name: (identifier) @enum_name)

(export_statement
  (enum_declaration
    name: (identifier) @export_enum_name))

; === Functions ===

; function name(...) { }
(function_declaration
  name: (identifier) @func_name)

(export_statement
  (function_declaration
    name: (identifier) @export_func_name))

; === Arrow functions as const/let ===

; const name = (...) => { }  or  const name = async (...) => { }
(lexical_declaration
  (variable_declarator
    name: (identifier) @arrow_func_name
    value: (arrow_function)))

(export_statement
  (lexical_declaration
    (variable_declarator
      name: (identifier) @export_arrow_func_name
      value: (arrow_function))))

; const name: Type = (...) => { }
; (type annotated arrow functions are still captured by the above patterns)

; === Constants (ALL_CAPS at module level) ===

; const API_URL = ...
(lexical_declaration
  (variable_declarator
    name: (identifier) @const_name
    value: (_) @const_value))

(export_statement
  (lexical_declaration
    (variable_declarator
      name: (identifier) @export_const_name
      value: (_) @export_const_value)))

; === Namespaces ===

; namespace Name { }
(internal_module
  name: (identifier) @namespace_name)

(export_statement
  (internal_module
    name: (identifier) @export_namespace_name))

; === Imports ===

; import ... from 'module'
(import_statement
  source: (string) @import_source)

; === Decorators ===

; @DecoratorName  or  @DecoratorName(...)
(decorator
  (identifier) @decorator_id)

(decorator
  (call_expression
    function: (identifier) @decorator_call_id))

; === Class methods ===

; method() {}, constructor() {}, get x() {}, set x(v) {}, static m() {}, async m() {}
(method_definition
  name: (property_identifier) @method_name) @method_node

; #privateMethod() {}
(method_definition
  name: (private_property_identifier) @private_method_name) @private_method_node

; === Class properties/fields ===

; prop: Type = value
(public_field_definition
  name: (property_identifier) @field_name) @field_node

; #privateProp: Type
(public_field_definition
  name: (private_property_identifier) @private_field_name) @private_field_node

; === Abstract methods ===

; abstract method(): void
(abstract_method_signature
  name: (property_identifier) @abstract_method_name) @abstract_method_node
