//! Tree-sitter based Protocol Buffers parser

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{node_line, node_text, parse_tree, signature_line, text_end_line, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static PROTO_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_proto::LANGUAGE.into());

static PROTO_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&PROTO_LANGUAGE, include_str!("queries/proto.scm"))
        .expect("Failed to compile Proto tree-sitter query")
});

pub static PROTO_PARSER: ProtoParser = ProtoParser;

pub struct ProtoParser;

/// Comments and string literals.
static NON_CODE: super::NonCode = super::NonCode {
    language: &PROTO_LANGUAGE,
    prose: &["comment"],
    strings: &["string"],
    code: &[],
    keep: super::keep_no_string,
};

impl LanguageParser for ProtoParser {
    fn non_code(&self) -> Option<&'static super::NonCode> {
        Some(&NON_CODE)
    }

    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &PROTO_LANGUAGE)?;
        let mut symbols = Vec::new();
        let query = &*PROTO_QUERY;
        let mut cursor = QueryCursor::new();

        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_package_name = idx("package_name");
        let idx_option_name = idx("option_name");
        let idx_option_value = idx("option_value");
        let idx_service_name = idx("service_name");
        let idx_rpc_name = idx("rpc_name");
        let idx_rpc_request_type = idx("rpc_request_type");
        let idx_rpc_response_type = idx("rpc_response_type");
        let idx_definition = idx("definition");

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            let end_line = find_capture(m, idx_definition).map(|c| text_end_line(content, &c.node));

            // Package
            if let Some(cap) = find_capture(m, idx_package_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Package,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Option (e.g., java_package)
            if let Some(name_cap) = find_capture(m, idx_option_name) {
                if let Some(value_cap) = find_capture(m, idx_option_value) {
                    let opt_name = node_text(content, &name_cap.node);
                    let opt_value = node_text(content, &value_cap.node);
                    // Strip quotes from value
                    let clean_value = opt_value.trim_matches('"').trim_matches('\'');
                    let line = node_line(&name_cap.node);
                    symbols.push(ParsedSymbol {
                        name: format!("{}:{}", opt_name, clean_value),
                        kind: SymbolKind::Property,
                        line,
                        signature: signature_line(content, line),
                        parents: vec![],
                        end_line,
                    });
                }
                continue;
            }

            // Service
            if let Some(cap) = find_capture(m, idx_service_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Interface,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // RPC
            if let Some(name_cap) = find_capture(m, idx_rpc_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);

                let request_type = find_capture(m, idx_rpc_request_type)
                    .map(|c| node_text(content, &c.node))
                    .unwrap_or("");
                let response_type = find_capture(m, idx_rpc_response_type)
                    .map(|c| node_text(content, &c.node))
                    .unwrap_or("");

                let signature =
                    format!("rpc {}({}) returns ({})", name, request_type, response_type);

                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line,
                    signature,
                    parents: vec![],
                    end_line,
                });
                continue;
            }
        }

        // Walk the tree manually for messages and enums (to handle nesting)
        collect_messages_and_enums(content, &tree.root_node(), &[], &mut symbols);

        Ok(symbols)
    }
}

/// Recursively walk the tree to collect messages and enums with proper nesting paths.
///
/// For nested messages/enums, builds dot-separated names like `Outer.Inner`
/// and sets the parent relationship with `nested_in`.
fn collect_messages_and_enums(
    content: &str,
    node: &tree_sitter::Node,
    parent_path: &[String],
    symbols: &mut Vec<ParsedSymbol>,
) {
    let mut walk_cursor = node.walk();
    for child in node.children(&mut walk_cursor) {
        match child.kind() {
            "message" => {
                // Extract message name from message_name child
                if let Some(name) = extract_named_child_text(content, &child, "message_name") {
                    let full_name = if parent_path.is_empty() {
                        name.clone()
                    } else {
                        format!("{}.{}", parent_path.join("."), name)
                    };

                    let parents = if parent_path.last().is_some() {
                        // For nested parent, reconstruct the full parent path
                        let parent_full = parent_path.join(".");
                        vec![(parent_full, "nested_in".to_string())]
                    } else {
                        vec![]
                    };

                    let line = node_line(&child);
                    symbols.push(ParsedSymbol {
                        name: full_name.clone(),
                        kind: SymbolKind::Class,
                        line,
                        signature: signature_line(content, line),
                        parents,
                        end_line: Some(text_end_line(content, &child)),
                    });

                    // Recurse into message_body for nested messages/enums
                    let mut body_cursor = child.walk();
                    for body_child in child.children(&mut body_cursor) {
                        if body_child.kind() == "message_body" {
                            let mut new_path = parent_path.to_vec();
                            new_path.push(name.clone());
                            collect_messages_and_enums(content, &body_child, &new_path, symbols);
                        }
                    }
                }
            }
            "enum" => {
                if let Some(name) = extract_named_child_text(content, &child, "enum_name") {
                    let full_name = if parent_path.is_empty() {
                        name.clone()
                    } else {
                        format!("{}.{}", parent_path.join("."), name)
                    };

                    let line = node_line(&child);
                    symbols.push(ParsedSymbol {
                        name: full_name,
                        kind: SymbolKind::Enum,
                        line,
                        signature: signature_line(content, line),
                        parents: vec![],
                        end_line: Some(text_end_line(content, &child)),
                    });
                }
            }
            _ => {}
        }
    }
}

/// Extract text from a named child node type (e.g., "message_name" -> identifier text)
fn extract_named_child_text(
    content: &str,
    node: &tree_sitter::Node,
    child_kind: &str,
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == child_kind {
            // The name node contains an identifier child
            let mut inner_cursor = child.walk();
            for inner in child.children(&mut inner_cursor) {
                if inner.kind() == "identifier" {
                    return Some(node_text(content, &inner).to_string());
                }
            }
            // Fallback: use the whole child text
            return Some(node_text(content, &child).to_string());
        }
    }
    None
}

/// Find a capture by index in a match
fn find_capture<'a>(
    m: &'a tree_sitter::QueryMatch<'a, 'a>,
    idx: Option<u32>,
) -> Option<&'a tree_sitter::QueryCapture<'a>> {
    let idx = idx?;
    m.captures.iter().find(|c| c.index == idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_package() {
        let content = r#"
syntax = "proto3";
package direct.api.v6;
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "direct.api.v6" && s.kind == SymbolKind::Package),
            "expected package 'direct.api.v6', got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_message() {
        let content = r#"
syntax = "proto3";

message GetCampaignRequest {
    int64 campaign_id = 1;
}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "GetCampaignRequest" && s.kind == SymbolKind::Class),
            "expected message 'GetCampaignRequest', got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_nested_message() {
        let content = r#"
syntax = "proto3";

message Outer {
    message Inner {
        string value = 1;
    }
    Inner item = 1;
}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "Outer" && s.kind == SymbolKind::Class),
            "expected message 'Outer', got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "Outer.Inner" && s.kind == SymbolKind::Class),
            "expected nested message 'Outer.Inner', got: {:?}",
            symbols
        );
        // Check parent relationship
        let inner = symbols.iter().find(|s| s.name == "Outer.Inner").unwrap();
        assert!(
            inner
                .parents
                .iter()
                .any(|(p, k)| p == "Outer" && k == "nested_in"),
            "expected parent 'Outer' with 'nested_in', got: {:?}",
            inner.parents
        );
    }

    #[test]
    fn test_parse_deeply_nested_message() {
        let content = r#"
syntax = "proto3";

message Level1 {
    message Level2 {
        message Level3 {
            string value = 1;
        }
    }
}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Level1"));
        assert!(symbols.iter().any(|s| s.name == "Level1.Level2"));
        assert!(
            symbols.iter().any(|s| s.name == "Level1.Level2.Level3"),
            "expected deeply nested message 'Level1.Level2.Level3', got: {:?}",
            symbols
        );
        // Check parent of Level3
        let level3 = symbols
            .iter()
            .find(|s| s.name == "Level1.Level2.Level3")
            .unwrap();
        assert!(
            level3
                .parents
                .iter()
                .any(|(p, k)| p == "Level1.Level2" && k == "nested_in"),
            "expected parent 'Level1.Level2', got: {:?}",
            level3.parents
        );
    }

    #[test]
    fn test_parse_service() {
        let content = r#"
syntax = "proto3";

service CampaignService {
    rpc GetCampaign(GetCampaignRequest) returns (Campaign);
}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "CampaignService" && s.kind == SymbolKind::Interface),
            "expected service 'CampaignService', got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_rpc() {
        let content = r#"
syntax = "proto3";

service CampaignService {
    rpc GetCampaign(GetCampaignRequest) returns (Campaign);
    rpc ListCampaigns(ListCampaignsRequest) returns (ListCampaignsResponse);
}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();
        let get = symbols.iter().find(|s| s.name == "GetCampaign").unwrap();
        assert_eq!(get.kind, SymbolKind::Function);
        assert!(
            get.signature.contains("GetCampaignRequest"),
            "signature should contain request type: {}",
            get.signature
        );
        assert!(
            get.signature.contains("Campaign"),
            "signature should contain response type: {}",
            get.signature
        );

        let list = symbols.iter().find(|s| s.name == "ListCampaigns").unwrap();
        assert_eq!(list.kind, SymbolKind::Function);
        assert!(list.signature.contains("ListCampaignsRequest"));
        assert!(list.signature.contains("ListCampaignsResponse"));
    }

    #[test]
    fn test_parse_stream_rpc() {
        let content = r#"
syntax = "proto3";

service StreamService {
    rpc StreamEvents(stream EventRequest) returns (stream EventResponse);
}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();
        let rpc = symbols.iter().find(|s| s.name == "StreamEvents").unwrap();
        assert_eq!(rpc.kind, SymbolKind::Function);
        assert!(
            rpc.signature.contains("EventRequest"),
            "signature should contain request type: {}",
            rpc.signature
        );
        assert!(
            rpc.signature.contains("EventResponse"),
            "signature should contain response type: {}",
            rpc.signature
        );
    }

    #[test]
    fn test_parse_enum() {
        let content = r#"
syntax = "proto3";

enum Status {
    UNKNOWN = 0;
    ACTIVE = 1;
    DELETED = 2;
}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "Status" && s.kind == SymbolKind::Enum),
            "expected enum 'Status', got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_nested_enum() {
        let content = r#"
syntax = "proto3";

message Response {
    enum Status {
        OK = 0;
        ERROR = 1;
    }
    Status status = 1;
}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Enum && s.name == "Response.Status"),
            "expected nested enum 'Response.Status', got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_java_package_option() {
        let content = r#"
syntax = "proto3";
package api.v1;
option java_package = "com.example.api.v1";

message Request {}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "java_package:com.example.api.v1"
                    && s.kind == SymbolKind::Property),
            "expected option 'java_package:com.example.api.v1', got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "api.v1" && s.kind == SymbolKind::Package),
            "expected package 'api.v1', got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_proto2_message() {
        let content = r#"
package NDirect.ChangeAgency;

message TChangeAgencyRequest {
    message TChangeAgencyRequestItem {
        optional uint64 client_id = 1;
        optional uint64 new_agency_client_id = 2;
    }
    repeated TChangeAgencyRequestItem items = 1;
}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();

        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Package && s.name == "NDirect.ChangeAgency"),
            "expected package, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Class && s.name == "TChangeAgencyRequest"),
            "expected outer message, got: {:?}",
            symbols
        );
        assert!(
            symbols.iter().any(|s| s.kind == SymbolKind::Class && s.name.contains("TChangeAgencyRequestItem")),
            "expected nested message, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_full_proto3_file() {
        let content = r#"
syntax = "proto3";
package direct.api.v6;

option java_package = "com.example.api";

message GetCampaignRequest {
    int64 campaign_id = 1;
    message Nested {
        string value = 1;
    }
    enum Status {
        UNKNOWN = 0;
        ACTIVE = 1;
    }
}

service CampaignService {
    rpc GetCampaign(GetCampaignRequest) returns (Campaign);
    rpc StreamEvents(stream EventRequest) returns (stream EventResponse);
}

enum GlobalEnum {
    UNSPECIFIED = 0;
}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();

        // Package
        assert!(symbols
            .iter()
            .any(|s| s.name == "direct.api.v6" && s.kind == SymbolKind::Package));

        // Option
        assert!(symbols
            .iter()
            .any(|s| s.name == "java_package:com.example.api" && s.kind == SymbolKind::Property));

        // Messages
        assert!(symbols
            .iter()
            .any(|s| s.name == "GetCampaignRequest" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "GetCampaignRequest.Nested" && s.kind == SymbolKind::Class));

        // Nested enum
        assert!(symbols
            .iter()
            .any(|s| s.name == "GetCampaignRequest.Status" && s.kind == SymbolKind::Enum));

        // Service
        assert!(symbols
            .iter()
            .any(|s| s.name == "CampaignService" && s.kind == SymbolKind::Interface));

        // RPCs
        assert!(symbols
            .iter()
            .any(|s| s.name == "GetCampaign" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "StreamEvents" && s.kind == SymbolKind::Function));

        // Global enum
        assert!(symbols
            .iter()
            .any(|s| s.name == "GlobalEnum" && s.kind == SymbolKind::Enum));
    }

    #[test]
    fn test_comments_ignored() {
        let content = r#"
syntax = "proto3";

// message Commented {}
/* message AlsoCommented {} */
message RealMessage {
    string field = 1;
}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealMessage"));
        assert!(!symbols.iter().any(|s| s.name == "Commented"));
        assert!(!symbols.iter().any(|s| s.name == "AlsoCommented"));
    }

    #[test]
    fn test_parse_multiple_services() {
        let content = r#"
syntax = "proto3";

service UserService {
    rpc CreateUser(CreateUserRequest) returns (CreateUserResponse);
    rpc GetUser(GetUserRequest) returns (User);
}

service AdminService {
    rpc DeleteUser(DeleteUserRequest) returns (Empty);
}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserService" && s.kind == SymbolKind::Interface));
        assert!(symbols
            .iter()
            .any(|s| s.name == "AdminService" && s.kind == SymbolKind::Interface));
        assert!(symbols
            .iter()
            .any(|s| s.name == "CreateUser" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "GetUser" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "DeleteUser" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_rpc_signature_types() {
        let content = r#"
syntax = "proto3";

service UserService {
    rpc CreateUser(CreateUserRequest) returns (CreateUserResponse);
    rpc GetUser(GetUserRequest) returns (User);
    rpc DeleteUser(DeleteUserRequest) returns (google.protobuf.Empty);
}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();
        let create_rpc = symbols.iter().find(|s| s.name == "CreateUser").unwrap();
        assert!(create_rpc.signature.contains("CreateUserRequest"));
        assert!(create_rpc.signature.contains("CreateUserResponse"));

        let delete_rpc = symbols.iter().find(|s| s.name == "DeleteUser").unwrap();
        assert!(delete_rpc.signature.contains("DeleteUserRequest"));
    }

    #[test]
    fn test_parse_empty_message() {
        let content = r#"
syntax = "proto3";
message Empty {}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Empty" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_nested_message_parent_relation() {
        let content = r#"
message Outer {
    message Middle {
        message Inner {
            string val = 1;
        }
    }
}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();

        let outer = symbols.iter().find(|s| s.name == "Outer").unwrap();
        assert!(
            outer.parents.is_empty(),
            "top-level message should have no parents"
        );

        let middle = symbols.iter().find(|s| s.name == "Outer.Middle").unwrap();
        assert!(
            middle
                .parents
                .iter()
                .any(|(p, k)| p == "Outer" && k == "nested_in"),
            "Middle should be nested_in Outer: {:?}",
            middle.parents
        );

        let inner = symbols
            .iter()
            .find(|s| s.name == "Outer.Middle.Inner")
            .unwrap();
        assert!(
            inner
                .parents
                .iter()
                .any(|(p, k)| p == "Outer.Middle" && k == "nested_in"),
            "Inner should be nested_in Outer.Middle: {:?}",
            inner.parents
        );
    }

    #[test]
    fn test_parse_multiple_top_level_messages() {
        let content = r#"
syntax = "proto3";

message Request {
    string id = 1;
}

message Response {
    string data = 1;
}

message Error {
    int32 code = 1;
    string message = 2;
}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Request" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Response" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Error" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_enum_inside_nested_message() {
        let content = r#"
message Outer {
    message Inner {
        enum Priority {
            LOW = 0;
            HIGH = 1;
        }
    }
}
"#;
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "Outer.Inner.Priority" && s.kind == SymbolKind::Enum),
            "expected deeply nested enum 'Outer.Inner.Priority', got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_line_numbers() {
        let content =
            "syntax = \"proto3\";\npackage test;\n\nmessage Foo {\n    string bar = 1;\n}\n";
        let symbols = PROTO_PARSER.parse_symbols(content).unwrap();
        let pkg = symbols
            .iter()
            .find(|s| s.name == "test" && s.kind == SymbolKind::Package)
            .unwrap();
        assert_eq!(pkg.line, 2, "package should be on line 2");
        let msg = symbols
            .iter()
            .find(|s| s.name == "Foo" && s.kind == SymbolKind::Class)
            .unwrap();
        assert_eq!(msg.line, 4, "message should be on line 4");
    }
}
