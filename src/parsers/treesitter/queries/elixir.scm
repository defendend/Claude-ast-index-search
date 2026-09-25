; Module definitions: defmodule MyModule do ... end
(call
  target: (identifier) @call_type
  (arguments (alias) @module_name)) @definition

; Function/macro definitions with regular call syntax: def foo(args), defp foo(args), defmacro foo(args), defmacrop foo(args)
(call
  target: (identifier) @def_type
  (arguments
    (call target: (identifier) @func_name))) @definition

; Zero-arity function/macro definitions (no parentheses): def foo do ... end
(call
  target: (identifier) @def_type_noargs
  (arguments
    (identifier) @func_name_noargs)) @definition

; Function/macro with guard clause: def foo(args) when guard
(call
  target: (identifier) @def_type_guard
  (arguments
    (binary_operator
      left: (call target: (identifier) @func_name_guard)
      operator: "when"))) @definition

; defstruct: defstruct [:field1, :field2]
(call
  target: (identifier) @struct_call) @definition

; Module attributes: @moduledoc, @doc, @callback, @type, @typep, @opaque, @spec
(unary_operator
  operator: "@"
  operand: (call
    target: (identifier) @attr_name)) @definition

; Simple module attributes (without arguments): @moduledoc false
(unary_operator
  operator: "@"
  operand: (call
    target: (identifier) @attr_name_simple)) @definition

; defimpl: defimpl Protocol, for: Module do ... end
(call
  target: (identifier) @impl_call
  (arguments (alias) @impl_protocol)) @definition
