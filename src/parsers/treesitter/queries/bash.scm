; Function definitions: both `function name { }` and `name() { }`
(function_definition
  name: (word) @func_name) @definition

; Top-level variable assignments: NAME=value
(variable_assignment
  name: (variable_name) @var_name) @definition
