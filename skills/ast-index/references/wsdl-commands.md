# WSDL/XSD Commands Reference

ast-index supports parsing and indexing WSDL and XSD files (`.wsdl`, `.xsd`).

## Supported Elements

### WSDL Elements

| WSDL Element | Symbol Kind | Example |
|--------------|-------------|---------|
| `<service name="...">` | Interface | `UserService` → Interface |
| `<portType name="...">` | Interface | `UserPortType` → Interface |
| `<operation name="...">` | Function | `getUser` → Function |
| `<binding name="...">` | Class | `UserBinding` → Class |
| `<message name="...">` | Class | `GetUserRequest` → Class |
| `<import location="...">` | Import | `types.xsd` → Import |

### XSD Elements

| XSD Element | Symbol Kind | Example |
|-------------|-------------|---------|
| `<complexType name="...">` | Class | `UserType` → Class |
| `<simpleType name="...">` | Class | `StatusType` → Class |
| `<element name="...">` | Property | `userId` → Property |
| `<attribute name="...">` | Property | `version` → Property |
| `<import schemaLocation="...">` | Import | `common.xsd` → Import |

## Core Commands

### Search Services

Find WSDL service definitions:

```bash
ast-index class "Service"           # Find all services
ast-index search "UserService"      # Find user service
```

### Search Operations

Find WSDL operations:

```bash
ast-index symbol "get"              # Find get* operations
ast-index symbol "create"           # Find create* operations
ast-index usages "getUser"          # Find operation usages
```

### Search Types

Find XSD type definitions:

```bash
ast-index class "Type"              # Find all types
ast-index search "Request"          # Find request types
ast-index search "Response"         # Find response types
```

### File Analysis

```bash
ast-index outline "service.wsdl"    # Show services, operations, messages
ast-index outline "types.xsd"       # Show complex/simple types
ast-index imports "api.wsdl"        # Show imports and includes
```

## Example Workflow

```bash
# 1. Index WSDL/XSD files
cd /path/to/wsdl/project
ast-index rebuild

# 2. Check index statistics
ast-index stats

# 3. Find all services
ast-index search "Service"

# 4. Find all operations
ast-index symbol "operation"

# 5. Show WSDL structure
ast-index outline "api/UserService.wsdl"

# 6. Find type usages
ast-index usages "UserType"
```

## WSDL Patterns

### Service Definition

```xml
<wsdl:service name="UserService">
    <wsdl:port name="UserPort" binding="tns:UserBinding">
        <soap:address location="http://example.com/user"/>
    </wsdl:port>
</wsdl:service>
```

Indexed as:
- `UserService` [interface]
- `UserPort` [property]

### Port Type and Operations

```xml
<wsdl:portType name="UserPortType">
    <wsdl:operation name="getUser">
        <wsdl:input message="tns:GetUserRequest"/>
        <wsdl:output message="tns:GetUserResponse"/>
    </wsdl:operation>
    <wsdl:operation name="createUser">
        <wsdl:input message="tns:CreateUserRequest"/>
        <wsdl:output message="tns:CreateUserResponse"/>
    </wsdl:operation>
</wsdl:portType>
```

Indexed as:
- `UserPortType` [interface]
- `getUser` [function] with parent `UserPortType`
- `createUser` [function] with parent `UserPortType`

### Messages

```xml
<wsdl:message name="GetUserRequest">
    <wsdl:part name="userId" type="xsd:string"/>
</wsdl:message>
<wsdl:message name="GetUserResponse">
    <wsdl:part name="user" element="tns:User"/>
</wsdl:message>
```

Indexed as:
- `GetUserRequest` [class]
- `GetUserResponse` [class]

## XSD Patterns

### Complex Type

```xml
<xsd:complexType name="UserType">
    <xsd:sequence>
        <xsd:element name="id" type="xsd:string"/>
        <xsd:element name="name" type="xsd:string"/>
        <xsd:element name="email" type="xsd:string" minOccurs="0"/>
    </xsd:sequence>
    <xsd:attribute name="version" type="xsd:int"/>
</xsd:complexType>
```

Indexed as:
- `UserType` [class]
- `id`, `name`, `email` [property]
- `version` [property]

### Simple Type with Restriction

```xml
<xsd:simpleType name="StatusType">
    <xsd:restriction base="xsd:string">
        <xsd:enumeration value="ACTIVE"/>
        <xsd:enumeration value="INACTIVE"/>
    </xsd:restriction>
</xsd:simpleType>
```

Indexed as:
- `StatusType` [class]

## Import Handling

```xml
<!-- WSDL imports -->
<wsdl:import namespace="http://example.com/types" location="types.wsdl"/>

<!-- XSD imports -->
<xsd:import namespace="http://example.com/common" schemaLocation="common.xsd"/>
<xsd:include schemaLocation="local-types.xsd"/>
```

```bash
ast-index imports "service.wsdl"    # Shows all imports
ast-index usages "common.xsd"       # Find where XSD is imported
```

## Performance

| Operation | Time |
|-----------|------|
| Rebuild (20 WSDL/XSD files) | ~80ms |
| Search type | ~1ms |
| Find usages | ~3ms |
| File outline | ~1ms |
