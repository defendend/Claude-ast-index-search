//! WSDL/XSD symbol parser
//!
//! Parses WSDL and XSD files to extract:
//! - Complex types (as Class)
//! - Simple types with enumeration (as Enum)
//! - Elements (as Class when they define inline types)
//! - Port types (as Interface)
//! - Operations (as Function)
//! - Services (as Class)
//!
//! Note: WSDL files in some projects may contain Template Toolkit directives
//! ([% ... %]) which are stripped before parsing.

use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

use crate::db::SymbolKind;
use super::ParsedSymbol;

/// Strip Template Toolkit directives from content
/// Handles both [% ... %] and [%- ... -%] patterns
fn strip_template_toolkit(content: &str) -> String {
    // First, remove multi-line BLOCK definitions
    static BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)\[%-?\s*BLOCK\s+\w+\s*-?%\].*?\[%-?\s*END\s*-?%\]").unwrap());

    let block_re = &*BLOCK_RE;
    let result = block_re.replace_all(content, "");

    // Remove FOREACH loops
    static FOREACH_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)\[%-?\s*FOREACH\s+[^%]+%\]").unwrap());

    let foreach_re = &*FOREACH_RE;
    let result = foreach_re.replace_all(&result, "");

    // Remove END tags
    static END_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[%-?\s*END\s*-?%\]").unwrap());

    let end_re = &*END_RE;
    let result = end_re.replace_all(&result, "");

    // Remove inline directives [% ... %] and [%- ... -%]
    static INLINE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[%-?[^%]*-?%\]").unwrap());

    let inline_re = &*INLINE_RE;
    let result = inline_re.replace_all(&result, "");

    // Remove PROCESS directives
    static PROCESS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[%-?\s*PROCESS\s+[^%]+%\]").unwrap());

    let process_re = &*PROCESS_RE;
    let result = process_re.replace_all(&result, "");

    result.to_string()
}

/// Parse WSDL/XSD source code and extract symbols
pub fn parse_wsdl_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();

    // Strip Template Toolkit directives
    let clean_content = strip_template_toolkit(content);

    // Complex type: <xsd:complexType name="TypeName">
    static COMPLEX_TYPE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r#"<xsd:complexType\s+name\s*=\s*"([^"]+)""#

    ).unwrap());

    let complex_type_re = &*COMPLEX_TYPE_RE;

    // Simple type with enumeration: <xsd:simpleType name="EnumName">
    static SIMPLE_TYPE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r#"<xsd:simpleType\s+name\s*=\s*"([^"]+)""#

    ).unwrap());

    let simple_type_re = &*SIMPLE_TYPE_RE;

    // Element with name (can define inline complex type)
    static ELEMENT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r#"<xsd:element\s+name\s*=\s*"([^"]+)""#

    ).unwrap());

    let element_re = &*ELEMENT_RE;

    // Port type: <wsdl:portType name="PortName">
    static PORT_TYPE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r#"<wsdl:portType\s+name\s*=\s*"([^"]+)""#

    ).unwrap());

    let port_type_re = &*PORT_TYPE_RE;

    // Operation: <wsdl:operation name="OperationName">
    static OPERATION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r#"<wsdl:operation\s+name\s*=\s*"([^"]+)""#

    ).unwrap());

    let operation_re = &*OPERATION_RE;

    // Service: <wsdl:service name="ServiceName">
    static SERVICE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r#"<wsdl:service\s+name\s*=\s*"([^"]+)""#

    ).unwrap());

    let service_re = &*SERVICE_RE;

    // Target namespace
    static NAMESPACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r#"targetNamespace\s*=\s*"([^"]+)""#

    ).unwrap());

    let namespace_re = &*NAMESPACE_RE;

    // Check if content contains enumeration (to detect enums vs regular simple types)
    let has_enumeration = |start_line: usize, lines: &[&str]| -> bool {
        for i in start_line..lines.len().min(start_line + 20) {
            if lines[i].contains("</xsd:simpleType>") {
                break;
            }
            if lines[i].contains("<xsd:enumeration") {
                return true;
            }
        }
        false
    };

    // Check if element has inline complexType (not just a type reference)
    let has_inline_type = |start_line: usize, lines: &[&str]| -> bool {
        for i in start_line..lines.len().min(start_line + 5) {
            let line = lines[i];
            if line.contains("/>") && !line.contains("<xsd:complexType") {
                return false; // Self-closing element without inline type
            }
            if line.contains("<xsd:complexType") {
                return true;
            }
            if line.contains("</xsd:element>") {
                return false;
            }
        }
        false
    };

    let lines: Vec<&str> = clean_content.lines().collect();
    let original_lines: Vec<&str> = content.lines().collect();

    // Extract namespace for context
    let mut namespace = String::new();
    for line in &original_lines {
        if let Some(caps) = namespace_re.captures(line) {
            namespace = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            break;
        }
    }

    for (line_num, line) in lines.iter().enumerate() {
        let line_num = line_num + 1;

        // Complex types -> Class
        if let Some(caps) = complex_type_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Class,
                    line: line_num,
                    signature: line.trim().to_string(),
                    parents: vec![],
                });
            }
        }

        // Simple types -> Enum (if has enumeration) or TypeAlias
        if let Some(caps) = simple_type_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() {
                let kind = if has_enumeration(line_num - 1, &lines) {
                    SymbolKind::Enum
                } else {
                    SymbolKind::TypeAlias
                };
                symbols.push(ParsedSymbol {
                    name,
                    kind,
                    line: line_num,
                    signature: line.trim().to_string(),
                    parents: vec![],
                });
            }
        }

        // Elements with inline complex type -> Class
        if let Some(caps) = element_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() && has_inline_type(line_num - 1, &lines) {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Class,
                    line: line_num,
                    signature: line.trim().to_string(),
                    parents: vec![],
                });
            }
        }

        // Port types -> Interface
        if let Some(caps) = port_type_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Interface,
                    line: line_num,
                    signature: line.trim().to_string(),
                    parents: vec![],
                });
            }
        }

        // Operations -> Function
        if let Some(caps) = operation_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Function,
                    line: line_num,
                    signature: line.trim().to_string(),
                    parents: vec![],
                });
            }
        }

        // Services -> Class
        if let Some(caps) = service_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Class,
                    line: line_num,
                    signature: line.trim().to_string(),
                    parents: vec![],
                });
            }
        }
    }

    // Add namespace as Package if found
    if !namespace.is_empty() {
        // Extract short name from namespace URL
        let short_name = namespace
            .rsplit('/')
            .next()
            .unwrap_or(&namespace)
            .to_string();

        symbols.push(ParsedSymbol {
            name: short_name,
            kind: SymbolKind::Package,
            line: 1,
            signature: format!("targetNamespace=\"{}\"", namespace),
            parents: vec![],
        });
    }

    Ok(symbols)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_template_toolkit() {
        let content = r#"
<xsd:import schemaLocation="[% API_SERVER_PATH %]/v[% api_version %]/general.xsd" />
[% strategy_settings = { foo => 1 } %]
<xsd:complexType name="TestType">
[%- BLOCK foo -%]
  some content
[%- END -%]
</xsd:complexType>
"#;
        let result = strip_template_toolkit(content);
        assert!(!result.contains("[%"));
        assert!(result.contains("TestType"));
    }

    #[test]
    fn test_parse_xsd_complex_type() {
        let content = r#"
<?xml version="1.0" encoding="UTF-8"?>
<xsd:schema xmlns:xsd="http://www.w3.org/2001/XMLSchema" targetNamespace="http://api.example.com/v1">
    <xsd:complexType name="ArrayOfString">
        <xsd:sequence>
            <xsd:element name="Items" type="xsd:string"/>
        </xsd:sequence>
    </xsd:complexType>
</xsd:schema>
"#;
        let symbols = parse_wsdl_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Class && s.name == "ArrayOfString"));
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Package && s.name == "v1"));
    }

    #[test]
    fn test_parse_xsd_enum() {
        let content = r#"
<xsd:simpleType name="StatusEnum">
    <xsd:restriction base="xsd:string">
        <xsd:enumeration value="ACTIVE"/>
        <xsd:enumeration value="DELETED"/>
    </xsd:restriction>
</xsd:simpleType>
"#;
        let symbols = parse_wsdl_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Enum && s.name == "StatusEnum"));
    }

    #[test]
    fn test_parse_wsdl_port_and_operation() {
        let content = r#"
<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/">
    <wsdl:portType name="ClientsPort">
        <wsdl:operation name="Get">
            <wsdl:input message="ns:GetRequest"/>
            <wsdl:output message="ns:GetResponse"/>
        </wsdl:operation>
        <wsdl:operation name="Update">
            <wsdl:input message="ns:UpdateRequest"/>
        </wsdl:operation>
    </wsdl:portType>
    <wsdl:service name="ClientsService">
        <wsdl:port name="ClientsPort"/>
    </wsdl:service>
</wsdl:definitions>
"#;
        let symbols = parse_wsdl_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Interface && s.name == "ClientsPort"));
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Function && s.name == "Get"));
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Function && s.name == "Update"));
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Class && s.name == "ClientsService"));
    }

    #[test]
    fn test_parse_element_with_inline_type() {
        let content = r#"
<xsd:element name="GetRequest">
    <xsd:complexType>
        <xsd:sequence>
            <xsd:element name="Id" type="xsd:long"/>
        </xsd:sequence>
    </xsd:complexType>
</xsd:element>
<xsd:element name="SimpleRef" type="xsd:string"/>
"#;
        let symbols = parse_wsdl_symbols(content).unwrap();
        // GetRequest has inline complexType, should be indexed
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Class && s.name == "GetRequest"));
        // SimpleRef is just a type reference, should not be indexed as class
        assert!(!symbols.iter().any(|s| s.name == "SimpleRef"));
    }
}
