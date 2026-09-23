; Function assignment with <- operator
; name <- function(...) { ... }
(binary_operator
  lhs: (identifier) @func_name_arrow
  operator: "<-"
  rhs: (function_definition)) @definition

; Function assignment with = operator
; name = function(...) { ... }
(binary_operator
  lhs: (identifier) @func_name_equals
  operator: "="
  rhs: (function_definition)) @definition

; Function assignment with <<- operator (global assignment)
; name <<- function(...) { ... }
(binary_operator
  lhs: (identifier) @func_name_global
  operator: "<<-"
  rhs: (function_definition)) @definition

; library() calls as imports — identifier argument
(call
  function: (identifier) @_lib_fn
  arguments: (arguments
    (argument
      value: (identifier) @import_library_name))
  (#eq? @_lib_fn "library")) @definition

; require() calls as imports — identifier argument
(call
  function: (identifier) @_req_fn
  arguments: (arguments
    (argument
      value: (identifier) @import_require_name))
  (#eq? @_req_fn "require")) @definition

; library() with string argument
(call
  function: (identifier) @_lib_fn_str
  arguments: (arguments
    (argument
      value: (string) @import_library_str))
  (#eq? @_lib_fn_str "library")) @definition

; require() with string argument
(call
  function: (identifier) @_req_fn_str
  arguments: (arguments
    (argument
      value: (string) @import_require_str))
  (#eq? @_req_fn_str "require")) @definition

; setClass() — S4 class definitions
(call
  function: (identifier) @_setclass_fn
  arguments: (arguments
    (argument
      value: (string) @s4_class_name))
  (#eq? @_setclass_fn "setClass")) @definition

; setMethod() — S4 method definitions
(call
  function: (identifier) @_setmethod_fn
  arguments: (arguments
    (argument
      value: (string) @s4_method_name))
  (#eq? @_setmethod_fn "setMethod")) @definition

; R6Class() — R6 class definitions with <-
; Name <- R6Class(...)
(binary_operator
  lhs: (identifier) @r6_class_name
  operator: "<-"
  rhs: (call
    function: (identifier) @_r6class_fn
    (#eq? @_r6class_fn "R6Class"))) @definition

; R6Class with = assignment
(binary_operator
  lhs: (identifier) @r6_class_name_eq
  operator: "="
  rhs: (call
    function: (identifier) @_r6class_fn_eq
    (#eq? @_r6class_fn_eq "R6Class"))) @definition

; setGeneric() — S4 generic definitions
(call
  function: (identifier) @_setgeneric_fn
  arguments: (arguments
    (argument
      value: (string) @s4_generic_name))
  (#eq? @_setgeneric_fn "setGeneric")) @definition
