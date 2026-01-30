//! Protocol Buffers symbol parser
//!
//! Parses .proto files (proto2 and proto3) to extract:
//! - Messages (as Class, including nested)
//! - Services (as Interface)
//! - RPCs (as Function)
//! - Enums
//! - Imports
//! - Packages

use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

use crate::db::SymbolKind;
use super::ParsedSymbol;

/// Parse Protocol Buffers source code and extract symbols
pub fn parse_proto_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();

    // Detect proto version (proto3 vs proto2) - for future use
    let _is_proto3 = content.contains("syntax = \"proto3\"") || content.contains("syntax = 'proto3'");

    // Package declaration
    static PACKAGE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^package\s+([\w.]+)\s*;").unwrap());
    let package_re = &*PACKAGE_RE;

    // Import statements - used in parse_proto_imports, not in symbol extraction
    static IMPORT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?m)^import\s+(?:public\s+|weak\s+)?"([^"]+)"\s*;"#).unwrap());
    let _import_re = &*IMPORT_RE;

    // Message declaration (including nested)
    static MESSAGE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^(\s*)message\s+(\w+)\s*\{").unwrap());

    let message_re = &*MESSAGE_RE;

    // Service declaration
    static SERVICE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^service\s+(\w+)\s*\{").unwrap());

    let service_re = &*SERVICE_RE;

    // RPC declaration
    static RPC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^\s*rpc\s+(\w+)\s*\(\s*(?:stream\s+)?(\w+)\s*\)\s*returns\s*\(\s*(?:stream\s+)?(\w+)\s*\)"

    ).unwrap());

    let rpc_re = &*RPC_RE;

    // Enum declaration
    static ENUM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^(\s*)enum\s+(\w+)\s*\{").unwrap());

    let enum_re = &*ENUM_RE;

    // Option java_package (for cross-reference with Java)
    static JAVA_PACKAGE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?m)^option\s+java_package\s*=\s*"([^"]+)"\s*;"#).unwrap());
    let java_package_re = &*JAVA_PACKAGE_RE;

    let lines: Vec<&str> = content.lines().collect();

    // Track nesting context for nested messages
    let mut message_stack: Vec<(String, usize)> = Vec::new(); // (name, indent_level)

    for (line_num, line) in lines.iter().enumerate() {
        let line_num = line_num + 1;

        // Package
        if let Some(caps) = package_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Package,
                line: line_num,
                signature: line.trim().to_string(),
                parents: vec![],
            });
        }

        // Java package option (useful for cross-reference)
        if let Some(caps) = java_package_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            symbols.push(ParsedSymbol {
                name: format!("java_package:{}", name),
                kind: SymbolKind::Property,
                line: line_num,
                signature: line.trim().to_string(),
                parents: vec![],
            });
        }

        // Messages
        if let Some(caps) = message_re.captures(line) {
            let indent = caps.get(1).map(|m| m.as_str().len()).unwrap_or(0);
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();

            // Pop messages from stack that are at same or higher indent level
            while let Some((_, stack_indent)) = message_stack.last() {
                if indent <= *stack_indent {
                    message_stack.pop();
                } else {
                    break;
                }
            }

            // Build full name including parent messages
            let full_name = if message_stack.is_empty() {
                name.clone()
            } else {
                let parent_path: Vec<&str> = message_stack.iter().map(|(n, _)| n.as_str()).collect();
                format!("{}.{}", parent_path.join("."), name)
            };

            // Determine parent for nested messages
            let parents = if let Some((parent_name, _)) = message_stack.last() {
                vec![(parent_name.clone(), "nested_in".to_string())]
            } else {
                vec![]
            };

            symbols.push(ParsedSymbol {
                name: full_name.clone(),
                kind: SymbolKind::Class,
                line: line_num,
                signature: line.trim().to_string(),
                parents,
            });

            message_stack.push((full_name, indent));
        }

        // Services
        if let Some(caps) = service_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Interface,
                line: line_num,
                signature: line.trim().to_string(),
                parents: vec![],
            });
        }

        // RPCs
        if let Some(caps) = rpc_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            let request_type = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let response_type = caps.get(3).map(|m| m.as_str()).unwrap_or("");

            // Build signature with types
            let signature = format!(
                "rpc {}({}) returns ({})",
                name, request_type, response_type
            );

            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Function,
                line: line_num,
                signature,
                parents: vec![],
            });
        }

        // Enums
        if let Some(caps) = enum_re.captures(line) {
            let indent = caps.get(1).map(|m| m.as_str().len()).unwrap_or(0);
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();

            // Check if enum is nested inside a message
            let full_name = if !message_stack.is_empty() {
                // Find parent message at appropriate indent level
                let mut parent_path = String::new();
                for (msg_name, msg_indent) in &message_stack {
                    if *msg_indent < indent {
                        if parent_path.is_empty() {
                            parent_path = msg_name.clone();
                        } else {
                            parent_path = format!("{}.{}", parent_path, msg_name);
                        }
                    }
                }
                if parent_path.is_empty() {
                    name.clone()
                } else {
                    format!("{}.{}", parent_path, name)
                }
            } else {
                name.clone()
            };

            symbols.push(ParsedSymbol {
                name: full_name,
                kind: SymbolKind::Enum,
                line: line_num,
                signature: line.trim().to_string(),
                parents: vec![],
            });
        }

        // Track closing braces to pop message stack
        if line.trim() == "}" && !message_stack.is_empty() {
            // Simple heuristic: closing brace at certain indent pops the stack
            let line_indent = line.len() - line.trim_start().len();
            if let Some((_, stack_indent)) = message_stack.last() {
                if line_indent <= *stack_indent {
                    message_stack.pop();
                }
            }
        }
    }

    Ok(symbols)
}

/// Parse imports from proto file
#[allow(dead_code)]
pub fn parse_proto_imports(content: &str) -> Result<Vec<(String, usize)>> {
    let mut imports = Vec::new();
    static PROTO_IMPORT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?m)^import\s+(?:public\s+|weak\s+)?"([^"]+)"\s*;"#).unwrap());
    let import_re = &*PROTO_IMPORT_RE;

    for (line_num, line) in content.lines().enumerate() {
        if let Some(caps) = import_re.captures(line) {
            let path = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            imports.push((path, line_num + 1));
        }
    }

    Ok(imports)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let symbols = parse_proto_symbols(content).unwrap();

        // Should find package
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Package && s.name == "NDirect.ChangeAgency"));

        // Should find outer message
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Class && s.name == "TChangeAgencyRequest"));

        // Should find nested message
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Class && s.name.contains("TChangeAgencyRequestItem")));
    }

    #[test]
    fn test_parse_proto3_service() {
        let content = r#"
syntax = "proto3";
package direct.api.v6.services;

service CampaignService {
    rpc GetCampaign(GetCampaignRequest) returns (Campaign);
    rpc ListCampaigns(ListCampaignsRequest) returns (ListCampaignsResponse);
}

message GetCampaignRequest {
    int64 campaign_id = 2;
}
"#;
        let symbols = parse_proto_symbols(content).unwrap();

        // Should find service
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Interface && s.name == "CampaignService"));

        // Should find RPCs
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Function && s.name == "GetCampaign"));
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Function && s.name == "ListCampaigns"));

        // Should find message
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Class && s.name == "GetCampaignRequest"));
    }

    #[test]
    fn test_parse_proto_enum() {
        let content = r#"
enum Status {
    UNKNOWN = 0;
    ACTIVE = 1;
    DELETED = 2;
}
"#;
        let symbols = parse_proto_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Enum && s.name == "Status"));
    }

    #[test]
    fn test_parse_proto_imports() {
        let content = r#"
import "google/protobuf/timestamp.proto";
import public "other/file.proto";
import weak "weak/dep.proto";
"#;
        let imports = parse_proto_imports(content).unwrap();
        assert_eq!(imports.len(), 3);
        assert!(imports.iter().any(|(p, _)| p == "google/protobuf/timestamp.proto"));
        assert!(imports.iter().any(|(p, _)| p == "other/file.proto"));
    }
}
