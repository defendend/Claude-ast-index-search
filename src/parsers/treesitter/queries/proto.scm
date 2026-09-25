; Package declaration
(package
  (full_ident) @package_name) @definition

; Top-level option (e.g., option java_package = "com.example";)
(source_file
  (option
    (identifier) @option_name
    (constant) @option_value) @definition)

; Service declaration
(service
  (service_name
    (identifier) @service_name)) @definition

; RPC declaration (inside service)
(rpc
  (rpc_name
    (identifier) @rpc_name)
  (message_or_enum_type) @rpc_request_type
  (message_or_enum_type) @rpc_response_type) @definition
