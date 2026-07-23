# Ruby Commands Reference

ast-index supports parsing and indexing Ruby source files (`.rb`).

## Supported Elements

| Ruby Element | Symbol Kind | Example |
|--------------|-------------|---------|
| `class ClassName` | Class | `User`, `Admin::Dashboard` |
| `module ModuleName` | Package | `Authenticatable` |
| `def method_name` | Function | `initialize`, `valid?`, `save!` |
| `def self.method_name` | Function | `self.call` |
| `attr_reader/writer/accessor` | Property | `:name`, `:email` |
| `CONSTANT` | Constant | `VERSION` |
| `require/require_relative` | Import | Imports |
| `include/extend/prepend` | Import | Mixins |

## Rails-Specific Elements

| Rails Element | Symbol Kind | Example |
|---------------|-------------|---------|
| `has_many/has_one/belongs_to` | Property | Associations |
| `scope :name` | Function | Scopes |
| `validates :field` | Annotation | Validations |
| `before_action :method` | Annotation | Callbacks |

## RSpec-Specific Elements

| RSpec Element | Symbol Kind | Example |
|---------------|-------------|---------|
| `describe "..."` | Class | Test suite |
| `context "..."` | Class | Nested context |
| `it "..."` | Function | Test case |
| `let(:name)` | Property | Test helper |

## Core Commands

### Search Classes

```bash
ast-index class "User"              # Find User class
ast-index class "Controller"        # Find controller classes
ast-index class "Service"           # Find service objects
ast-index search "ApplicationRecord"  # Find AR models
```

### Search Modules

```bash
ast-index search "module"           # Find all modules
ast-index search "Authenticatable"  # Find specific module
ast-index search "Concerns"         # Find Rails concerns
```

### Search Methods

```bash
ast-index symbol "initialize"       # Find constructors
ast-index symbol "call"             # Find call methods
ast-index callers "process"         # Find method callers
```

### Search Rails Patterns

```bash
ast-index search "has_many"         # Find associations
ast-index search "belongs_to"       # Find belongs_to
ast-index search "validates"        # Find validations
ast-index search "scope"            # Find scopes
ast-index search "before_action"    # Find callbacks
```

### Search RSpec Tests

```bash
ast-index search "describe"         # Find test suites
ast-index search "it"               # Find test cases
ast-index search "let"              # Find test helpers
```

### File Analysis

```bash
ast-index outline "user.rb"         # Show file structure
ast-index imports "controller.rb"   # Show require statements
```

## Example Workflow

```bash
# 1. Index Ruby project
cd /path/to/rails/app
ast-index rebuild

# 2. Check index statistics
ast-index stats

# 3. Find all models
ast-index search "ApplicationRecord"

# 4. Find all controllers
ast-index class "Controller"

# 5. Find usages of a class
ast-index usages "UserService"

# 6. Show model structure
ast-index outline "app/models/user.rb"

# 7. Find test files
ast-index search "RSpec.describe"
```

## Indexed Ruby Patterns

### Class Definition

```ruby
class User < ApplicationRecord
  attr_accessor :name

  def initialize(name)
    @name = name
  end
end
```

Indexed as:
- `User` [class] extends `ApplicationRecord`
- `:name` [property]
- `initialize` [function]

### Module Definition

```ruby
module Authenticatable
  def authenticate
    true
  end
end
```

Indexed as:
- `Authenticatable` [package]
- `authenticate` [function]

### Rails Model

```ruby
class Post < ApplicationRecord
  belongs_to :author
  has_many :comments

  validates :title
  validates :content

  scope :published, -> { where(published: true) }

  before_save :normalize_title
end
```

Indexed as:
- `Post` [class]
- `belongs_to :author` [property]
- `has_many :comments` [property]
- `validates :title` [annotation]
- `scope :published` [function]
- `before_save :normalize_title` [annotation]

### RSpec Test

```ruby
RSpec.describe User do
  describe "#valid?" do
    let(:user) { build(:user) }

    it "returns true for valid user" do
      expect(user).to be_valid
    end
  end
end
```

Indexed as:
- `describe "#valid?"` [class]
- `let(:user)` [property]
- `it "returns true for valid user"` [function]

### Service Object

```ruby
class CreateUserService
  def self.call(params)
    new(params).call
  end

  def initialize(params)
    @params = params
  end

  def call
    User.create(@params)
  end
end
```

Indexed as:
- `CreateUserService` [class]
- `self.call` [function]
- `initialize` [function]
- `call` [function]

## Import Handling

Both `require` and `require_relative` are tracked:

```ruby
require 'json'
require 'net/http'
require_relative './helpers'
require_relative '../models/user'
```

Use `ast-index imports "file.rb"` to see all imports.

## Mixins

Include, extend, and prepend are tracked:

```ruby
class User
  include Authenticatable
  extend ClassMethods
  prepend Trackable
end
```

Indexed as imports showing the relationship.

## Performance

| Operation | Time |
|-----------|------|
| Rebuild (500 Ruby files) | ~400ms |
| Search class | ~1ms |
| Find usages | ~5ms |
| File outline | ~1ms |
