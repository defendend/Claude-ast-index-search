; Function/macro/generic/method definitions with simple name
; Matches: (defun my-func ...) (defmacro my-macro ...) (defgeneric gf ...) (defmethod m ...)
(defun_header
  keyword: (defun_keyword) @kw
  function_name: (sym_lit) @func_name) @definition

; Function/macro definitions with package-qualified name
; Matches: (defun my-pkg:my-func ...)
(defun_header
  keyword: (defun_keyword) @kw_pkg
  function_name: (package_lit
    symbol: (sym_lit) @func_name_pkg)) @definition

; Class definitions: (defclass ClassName (superclasses) slots)
(list_lit . [(sym_lit) (package_lit)] @_kw
          . (sym_lit) @class_name
  (#match? @_kw "(?i)^(cl:)?defclass$")) @definition

; Structure definitions: (defstruct StructName ...)
(list_lit . [(sym_lit) (package_lit)] @_kw
          . (sym_lit) @struct_name
  (#match? @_kw "(?i)^(cl:)?defstruct$")) @definition

; Variable/parameter definitions: (defvar *name* ...) (defparameter *name* ...)
(list_lit . [(sym_lit) (package_lit)] @_kw
          . (sym_lit) @var_name
  (#match? @_kw "(?i)^(cl:)?(defvar|defparameter)$")) @definition

; Constant definitions: (defconstant +name+ ...)
(list_lit . [(sym_lit) (package_lit)] @_kw
          . (sym_lit) @const_name
  (#match? @_kw "(?i)^(cl:)?defconstant$")) @definition

; Package definitions with keyword name: (defpackage :my-package ...)
(list_lit . [(sym_lit) (package_lit)] @_kw
          . (kwd_lit) @pkg_name_kwd
  (#match? @_kw "(?i)^(cl:)?defpackage$")) @definition

; Package definitions with symbol name: (defpackage my-package ...)
(list_lit . [(sym_lit) (package_lit)] @_kw
          . (sym_lit) @pkg_name_sym
  (#match? @_kw "(?i)^(cl:)?defpackage$")) @definition
