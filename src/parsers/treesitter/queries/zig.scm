; Function declaration — `fn foo(...) ReturnType { ... }` or signature-only
(function_declaration
  name: (identifier) @func_name) @definition

; Top-level / container-scope variable or constant declaration.
; Zig has no dedicated struct/enum/union statement — type definitions are
; expressions assigned to a `const`, so `variable_declaration` covers types too.
; The identifier is not a named field, but it is the first direct
; `identifier` child of `variable_declaration`.
(variable_declaration
  (identifier) @var_name) @definition

; `test "does X" { ... }` — name is a direct (unfielded) string child.
(test_declaration
  (string) @test_name) @definition

; `test identifier { ... }` — bare-identifier form.
(test_declaration
  (identifier) @test_ident) @definition

; Struct / enum / union field declaration.
(container_field
  name: (identifier) @field_name) @definition
