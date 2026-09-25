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
  name: (type_identifier) @type_alias_name) @type_alias_node

(export_statement
  (type_alias_declaration
    name: (type_identifier) @export_type_alias_name) @export_type_alias_node)

; === Enums ===

; enum Name { }
(enum_declaration
  name: (identifier) @enum_name) @enum_node

(export_statement
  (enum_declaration
    name: (identifier) @export_enum_name) @export_enum_node)

; === Functions ===

; function name(...) { }
(function_declaration
  name: (identifier) @func_name) @func_node

(export_statement
  (function_declaration
    name: (identifier) @export_func_name) @export_func_node)

; === Arrow functions as const/let ===

; const name = (...) => { }  or  const name = async (...) => { }
(lexical_declaration
  (variable_declarator
    name: (identifier) @arrow_func_name
    value: (arrow_function)) @arrow_func_node)

(export_statement
  (lexical_declaration
    (variable_declarator
      name: (identifier) @export_arrow_func_name
      value: (arrow_function)) @export_arrow_func_node))

; const name: Type = (...) => { }
; (type annotated arrow functions are still captured by the above patterns)

; === Constants (ALL_CAPS at module level) ===

; const API_URL = ...
(lexical_declaration
  (variable_declarator
    name: (identifier) @const_name
    value: (_) @const_value) @const_node)

(export_statement
  (lexical_declaration
    (variable_declarator
      name: (identifier) @export_const_name
      value: (_) @export_const_value) @export_const_node))

; === Namespaces ===

; namespace Name { }
(internal_module
  name: (identifier) @namespace_name) @namespace_node

(export_statement
  (internal_module
    name: (identifier) @export_namespace_name) @export_namespace_node)

; === Imports ===

; import ... from 'module'
(import_statement
  source: (string) @import_source) @import_node

; === Decorators ===

; @DecoratorName  or  @DecoratorName(...)
(decorator
  (identifier) @decorator_id) @decorator_node

(decorator
  (call_expression
    function: (identifier) @decorator_call_id)) @decorator_call_node

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

; === Ambient declarations (declare keyword, .d.ts files) ===

; declare function name(...)
(ambient_declaration
  (function_signature
    name: (identifier) @func_name) @func_node)

; export declare function name(...)
(export_statement
  (ambient_declaration
    (function_signature
      name: (identifier) @export_func_name) @export_func_node))

; export declare class Name {}
(export_statement
  (ambient_declaration
    (class_declaration
      name: (type_identifier) @export_class_name) @export_class_node))

; export declare abstract class Name {}
(export_statement
  (ambient_declaration
    (abstract_class_declaration
      name: (type_identifier) @export_abstract_class_name) @export_abstract_class_node))

; export declare interface Name {}
(export_statement
  (ambient_declaration
    (interface_declaration
      name: (type_identifier) @export_interface_name) @export_interface_node))

; export declare type Name = ...
(export_statement
  (ambient_declaration
    (type_alias_declaration
      name: (type_identifier) @export_type_alias_name) @export_type_alias_node))

; export declare enum Name {}
(export_statement
  (ambient_declaration
    (enum_declaration
      name: (identifier) @export_enum_name) @export_enum_node))

; export declare namespace Name {}
(export_statement
  (ambient_declaration
    (internal_module
      name: (identifier) @export_namespace_name) @export_namespace_node))

; export declare const NAME: Type (ambient, may have no value)
(export_statement
  (ambient_declaration
    (lexical_declaration
      (variable_declarator
        name: (identifier) @export_ambient_const_name) @export_ambient_const_node)))

; === Export default ===

; export default expression  (identifier, call_expression, object, etc.)
(export_statement
  value: (_) @export_default_value)
