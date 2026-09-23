; Class definition: class Foo or class Foo < Bar
(class
  name: (_) @class_name
  superclass: (superclass (_) @class_parent)?) @class_node

; Module definition: module Foo
(module
  name: (_) @module_name) @module_node

; Instance method: def method_name
(method
  name: (_) @method_name) @method_node

; Singleton method: def self.method_name
(singleton_method
  object: (_) @singleton_object
  name: (_) @singleton_method_name) @singleton_method_node

; Assignment: Name = value or Scope::Name = value (top-level or inside class/module)
(assignment
  left: [(constant) (scope_resolution)] @assign_const_name
  right: (_) @assign_const_value) @assign_const_node

; Call expressions (DSL methods like require, include, attr_reader, etc.)
; We capture the method name and first argument for all call nodes
(call
  method: (_) @call_method
  arguments: (argument_list . (_) @call_first_arg)?)
