# WSDL/XSD Commands Reference

ast-index supports parsing and indexing WSDL (Web Services Description Language) and XSD (XML Schema Definition) files.

## Supported Elements

| XML Element | Symbol Kind | Example |
|-------------|-------------|---------|
| `xsd:complexType name="..."` | Class | `ArrayOfString` → Class |
| `xsd:simpleType name="..."` (with enumeration) | Enum | `StatusEnum` → Enum |
| `xsd:simpleType name="..."` (without enumeration) | TypeAlias | `StringType` → TypeAlias |
| `xsd:element name="..."` (with inline complexType) | Class | `GetRequest` → Class |
| `wsdl:portType name="..."` | Interface | `ClientsPort` → Interface |
| `wsdl:operation name="..."` | Function | `Get` → Function |
| `wsdl:service name="..."` | Class | `ClientsService` → Class |
| `targetNamespace="..."` | Package | `clients` → Package |

## Template Toolkit Support

WSDL files may contain Template Toolkit directives (common in Yandex projects):

```xml
<xsd:import schemaLocation="[% API_SERVER_PATH %]/v[% api_version %]/general.xsd" />
[% FOREACH method IN [ '', 'Add' ] %]
    <xsd:complexType name="Strategy[% method %]">
[% END %]
```

These are automatically stripped before parsing. The parser extracts symbols from the clean XML structure.

## Core Commands

### Search Types

Find complex types and simple types:

```bash
ast-index class "ArrayOfString"       # Find complex type
ast-index search "Enum"               # Find enum types
ast-index symbol "Status"             # Find any status-related symbols
```

### Search Services

Find WSDL services and port types:

```bash
ast-index search "Service"            # Find all services
ast-index class "ClientsService"      # Find specific service
```

### Search Operations

Find WSDL operations (API methods):

```bash
ast-index symbol "Get"                # Find Get operations
ast-index callers "Update"            # Find where Update is called
```

### Find Enums

Find enumeration types (simpleType with xsd:enumeration):

```bash
ast-index search "FieldEnum"          # Find field enums
ast-index class "StatusEnum"          # Find status enum
```

## Example Workflow

```bash
# 1. Index WSDL/XSD directory
cd /path/to/wsdl/files
ast-index rebuild

# 2. Check index statistics
ast-index stats

# 3. Find all services
ast-index search "Service"

# 4. Find specific type
ast-index class "ClientGetItem"

# 5. Find enum values
ast-index search "StatusEnum"

# 6. Show file structure
ast-index outline "Clients.wsdl"
```

## XSD-Specific Patterns

### Complex Types

```xml
<xsd:complexType name="ArrayOfString">
    <xsd:sequence>
        <xsd:element name="Items" type="xsd:string"/>
    </xsd:sequence>
</xsd:complexType>
```

Indexed as: `ArrayOfString [class]`

### Enumeration Types

```xml
<xsd:simpleType name="StatusEnum">
    <xsd:restriction base="xsd:string">
        <xsd:enumeration value="ACTIVE"/>
        <xsd:enumeration value="DELETED"/>
    </xsd:restriction>
</xsd:simpleType>
```

Indexed as: `StatusEnum [enum]`

### Inline Elements

```xml
<xsd:element name="GetRequest">
    <xsd:complexType>
        <xsd:sequence>
            <xsd:element name="Id" type="xsd:long"/>
        </xsd:sequence>
    </xsd:complexType>
</xsd:element>
```

Indexed as: `GetRequest [class]`

## WSDL-Specific Patterns

### Port Types and Operations

```xml
<wsdl:portType name="ClientsPort">
    <wsdl:operation name="Get">
        <wsdl:input message="ns:GetRequest"/>
        <wsdl:output message="ns:GetResponse"/>
    </wsdl:operation>
</wsdl:portType>
```

Indexed as:
- `ClientsPort [interface]`
- `Get [function]`

### Services

```xml
<wsdl:service name="ClientsService">
    <wsdl:port name="ClientsPort" binding="ns:ClientsBinding"/>
</wsdl:service>
```

Indexed as: `ClientsService [class]`

## Performance

| Operation | Time |
|-----------|------|
| Rebuild (35 WSDL/XSD files) | ~70ms |
| Search type | ~2ms |
| Find usages | ~10ms |

## Limitations

Current implementation:
- Strips Template Toolkit before parsing
- Does not validate XML schema
- Does not resolve cross-file references
- Does not parse attribute definitions

## File Extensions

Supported extensions:
- `.wsdl` - Web Services Description Language
- `.xsd` - XML Schema Definition
