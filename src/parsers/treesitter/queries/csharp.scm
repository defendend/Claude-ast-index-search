; Namespace declaration (block style)
(namespace_declaration
  name: (_) @namespace_name) @definition

; File-scoped namespace declaration
(file_scoped_namespace_declaration
  name: (_) @namespace_name) @definition

; Using directive (imports)
(using_directive) @using_dir @definition

; Class declaration
(class_declaration
  name: (identifier) @class_name) @class_decl @definition

; Interface declaration
(interface_declaration
  name: (identifier) @interface_name) @interface_decl @definition

; Struct declaration
(struct_declaration
  name: (identifier) @struct_name) @definition

; Record declaration
(record_declaration
  name: (identifier) @record_name) @record_decl @definition

; Enum declaration
(enum_declaration
  name: (identifier) @enum_name) @definition

; Method declaration
(method_declaration
  name: (identifier) @method_name) @definition

; Constructor declaration
(constructor_declaration
  name: (identifier) @constructor_name) @definition

; Property declaration
(property_declaration
  name: (identifier) @property_name) @definition

; Field declaration
(field_declaration) @field_decl @definition

; Event field declaration (event EventHandler OnData;)
(event_field_declaration) @event_field_decl @definition

; Event declaration (event with accessors)
(event_declaration
  name: (identifier) @event_name) @definition

; Delegate declaration
(delegate_declaration
  name: (identifier) @delegate_name) @definition

; Attribute list
(attribute_list
  (attribute
    name: (_) @attr_name) @definition)
