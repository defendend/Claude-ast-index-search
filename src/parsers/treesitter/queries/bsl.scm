; Procedure declaration (capture full node for export/annotation access)
(procedure_declaration
  name: (identifier) @proc_name) @proc_decl @definition

; Function declaration (capture full node for export/annotation access)
(function_declaration
  name: (identifier) @func_name) @func_decl @definition

; Module-level variable declaration
(source_file
  (var_declaration
    (var_name
      name: (identifier) @var_name)) @definition)

; Region
(region
  (region_start
    name: (identifier) @region_name))

; Standalone annotation (compilation directives & extension annotations)
(annotation
  name: (annotation_name) @annotation_name) @definition
