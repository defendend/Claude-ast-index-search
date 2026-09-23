; CREATE TABLE — Class
(create_table
  (object_reference) @table_name) @definition

; CREATE FUNCTION / CREATE PROCEDURE — Function
(create_function
  (object_reference) @func_name) @definition

; CREATE INDEX — Property
(create_index
  column: (identifier) @index_name) @definition

; CREATE TYPE — Class
(create_type
  (object_reference) @type_name) @definition
