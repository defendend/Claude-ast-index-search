; Class definition
(class_definition
  name: (identifier) @class_name) @definition

; Function definition (identifier form)
(function_definition
  name: (identifier) @func_name) @definition

; Function definition (property_name form, e.g., set.Prop / get.Prop)
(function_definition
  name: (property_name) @func_name) @definition

; Properties block with property declarations
(property
  name: (identifier) @property_name) @definition

; Enumeration members
(enum
  (identifier) @enum_name) @definition

; Events: one `events` block lists several, so each event spans only its name
(events
  (identifier) @event_name @definition)
