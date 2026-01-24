# TypeScript/JavaScript Commands Reference

ast-index supports parsing and indexing TypeScript and JavaScript files:
- `.ts`, `.tsx` - TypeScript and TSX (React)
- `.js`, `.jsx` - JavaScript and JSX (React)
- `.mjs`, `.cjs` - ES modules and CommonJS
- `.vue` - Vue Single File Components
- `.svelte` - Svelte components

## Supported Elements

| Element | Symbol Kind | Example |
|---------|-------------|---------|
| `class ClassName` | Class | `UserService` |
| `interface IName` | Interface | `IUserRepository` |
| `type TypeName = ...` | TypeAlias | `UserDTO` |
| `enum EnumName` | Enum | `UserStatus` |
| `function funcName()` | Function | `processData` |
| `const func = () => {}` | Function | Arrow functions |
| `const Component = () => <div/>` | Class | React components (PascalCase) |
| `function useHook()` | Function | React hooks (useXxx) |
| `@Decorator` | Annotation | NestJS/Angular decorators |
| `namespace NS` | Namespace | TypeScript namespaces |
| `const CONST = value` | Constant | Module constants |
| `import/export` | Import/Export | Module imports/exports |

## Core Commands

### Search Classes and Components

```bash
ast-index class "Service"           # Find service classes
ast-index class "Component"         # Find React/Vue components
ast-index class "Controller"        # Find NestJS controllers
ast-index search "Repository"       # Find repository classes
```

### Search Interfaces and Types

```bash
ast-index class "Props"             # Find prop interfaces
ast-index symbol "DTO"              # Find DTOs
ast-index symbol "Response"         # Find type aliases
```

### Search Functions

```bash
ast-index symbol "fetch"            # Find fetch functions
ast-index symbol "handle"           # Find handlers
ast-index callers "processOrder"    # Find function callers
```

### Search React-Specific

```bash
ast-index search "use"              # Find React hooks (useXxx)
ast-index class "Button"            # Find Button component
ast-index class "Modal"             # Find Modal component
ast-index usages "useState"         # Find useState usages
```

### Search Decorators (NestJS/Angular)

```bash
ast-index search "@Controller"      # Find controllers
ast-index search "@Injectable"      # Find services
ast-index search "@Component"       # Find Angular components
ast-index search "@Module"          # Find modules
```

### File Analysis

```bash
ast-index outline "service.ts"      # Show file structure
ast-index imports "component.tsx"   # Show all imports
ast-index exports "index.ts"        # Show all exports
```

## Framework-Specific Patterns

### React

```typescript
// Functional component - indexed as Class
const UserCard: FC<UserCardProps> = ({ user }) => {
  return <div>{user.name}</div>;
};

// Hook - indexed as Function
function useUser(id: string) {
  const [user, setUser] = useState<User | null>(null);
  return user;
}

// Component with forwardRef
const Button = forwardRef<HTMLButtonElement, ButtonProps>((props, ref) => {
  return <button ref={ref} {...props} />;
});
```

Indexed as:
- `UserCard` [class] - React component
- `useUser` [function] - React hook
- `Button` [class] - React component

### Vue 3 (Composition API)

```vue
<script setup lang="ts">
import { ref, computed } from 'vue';

interface Props {
  title: string;
}

const props = defineProps<Props>();
const count = ref(0);
const doubled = computed(() => count.value * 2);
</script>
```

ast-index extracts the `<script>` section and indexes:
- `Props` [interface]
- `count` [constant]
- `doubled` [constant]

### Svelte

```svelte
<script lang="ts">
  export let name: string;
  export let count = 0;

  function increment() {
    count += 1;
  }
</script>
```

Indexed as:
- `name` [property] - exported prop
- `count` [property] - exported prop
- `increment` [function]

### NestJS

```typescript
@Controller('users')
@Injectable()
export class UserController {
  constructor(private readonly userService: UserService) {}

  @Get(':id')
  async findOne(@Param('id') id: string): Promise<User> {
    return this.userService.findById(id);
  }
}
```

Indexed as:
- `@Controller` [annotation]
- `@Injectable` [annotation]
- `UserController` [class]
- `findOne` [function]
- `@Get` [annotation]

### Angular

```typescript
@Component({
  selector: 'app-user',
  templateUrl: './user.component.html'
})
export class UserComponent implements OnInit {
  @Input() userId!: string;

  ngOnInit(): void {
    this.loadUser();
  }
}
```

Indexed as:
- `@Component` [annotation]
- `UserComponent` [class] extends `OnInit`
- `ngOnInit` [function]

## Example Workflow

```bash
# 1. Index web project
cd /path/to/react/project
ast-index rebuild

# 2. Check index statistics
ast-index stats

# 3. Find all components
ast-index search "Component" --kind class

# 4. Find all hooks
ast-index search "use" --kind function

# 5. Find usages of a hook
ast-index usages "useAuth"

# 6. Show component structure
ast-index outline "UserProfile.tsx"

# 7. Find implementations of interface
ast-index implementations "UserRepository"

# 8. Find who calls a function
ast-index callers "fetchUser"
```

## Import/Export Handling

Both named and default exports are tracked:

```typescript
// Named exports
export { UserService, UserController };
export type { UserDTO, UserResponse };

// Default export
export default class App extends Component {}

// Re-exports
export * from './user';
export { default as Button } from './Button';
```

Use `ast-index exports "index.ts"` to see all exports with line numbers.

## TypeScript-Specific

### Type Aliases

```typescript
type UserID = string;
type UserMap = Map<UserID, User>;
type AsyncResult<T> = Promise<Result<T, Error>>;
```

Search with:
```bash
ast-index symbol "ID"
ast-index symbol "Map"
ast-index symbol "Result"
```

### Enums

```typescript
enum UserStatus {
  Active = 'ACTIVE',
  Inactive = 'INACTIVE',
  Pending = 'PENDING'
}
```

Search with:
```bash
ast-index symbol "Status"
ast-index symbol "UserStatus"
```

### Namespaces

```typescript
namespace API {
  export interface Request {}
  export interface Response {}
  export function call() {}
}
```

Search with:
```bash
ast-index symbol "API"
ast-index search "API.Request"
```

## Performance

| Operation | Time |
|-----------|------|
| Rebuild (1000 TS files) | ~800ms |
| Search class/component | ~1ms |
| Find usages | ~5ms |
| File outline | ~1ms |
| Find implementations | ~5ms |
