use anyhow::Result;
use regex::Regex;

use crate::db::SymbolKind;
use super::ParsedSymbol;

/// Parse Ruby source file and extract symbols
pub fn parse_ruby_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();

    // Class definition: class ClassName < ParentClass
    let class_re = Regex::new(
        r"(?m)^[ \t]*class\s+([A-Z][A-Za-z0-9_]*(?:::[A-Z][A-Za-z0-9_]*)*)(?:\s*<\s*([A-Z][A-Za-z0-9_:]*))?"
    )?;

    // Module definition: module ModuleName
    let module_re = Regex::new(
        r"(?m)^[ \t]*module\s+([A-Z][A-Za-z0-9_]*(?:::[A-Z][A-Za-z0-9_]*)*)"
    )?;

    // Instance method: def method_name
    let def_re = Regex::new(
        r"(?m)^[ \t]*def\s+([a-z_][a-z0-9_]*[?!=]?)\s*(?:\([^)]*\))?"
    )?;

    // Class method: def self.method_name
    let def_self_re = Regex::new(
        r"(?m)^[ \t]*def\s+self\.([a-z_][a-z0-9_]*[?!=]?)\s*(?:\([^)]*\))?"
    )?;

    // Attribute accessors: attr_reader, attr_writer, attr_accessor
    let attr_re = Regex::new(
        r"(?m)^[ \t]*attr_(reader|writer|accessor)\s+:([a-z_][a-z0-9_]*)"
    )?;

    // Constants: CONSTANT_NAME = value
    let const_re = Regex::new(
        r"(?m)^[ \t]*([A-Z][A-Z0-9_]*)\s*="
    )?;

    // RSpec describe/context blocks
    let rspec_describe_re = Regex::new(
        r#"(?m)^[ \t]*(describe|context)\s+['"]([^'"]+)['"]"#
    )?;

    // RSpec it/specify blocks
    let rspec_it_re = Regex::new(
        r#"(?m)^[ \t]*(it|specify)\s+['"]([^'"]+)['"]"#
    )?;

    // RSpec let/let!/subject
    let rspec_let_re = Regex::new(
        r"(?m)^[ \t]*(let|let!|subject)\s*\(\s*:([a-z_][a-z0-9_]*)\s*\)"
    )?;

    // Rails scope
    let rails_scope_re = Regex::new(
        r"(?m)^[ \t]*scope\s+:([a-z_][a-z0-9_]*)"
    )?;

    // Rails has_many/has_one/belongs_to
    let rails_assoc_re = Regex::new(
        r"(?m)^[ \t]*(has_many|has_one|belongs_to|has_and_belongs_to_many)\s+:([a-z_][a-z0-9_]*)"
    )?;

    // Rails callbacks: before_action, after_action, etc.
    let rails_callback_re = Regex::new(
        r"(?m)^[ \t]*(before_action|after_action|around_action|before_create|after_create|before_save|after_save|before_destroy|after_destroy|before_validation|after_validation)\s+:([a-z_][a-z0-9_]*)"
    )?;

    // Rails validates
    let rails_validates_re = Regex::new(
        r"(?m)^[ \t]*validates\s+:([a-z_][a-z0-9_]*)"
    )?;

    // require/require_relative
    let require_re = Regex::new(
        r#"(?m)^[ \t]*require(?:_relative)?\s+['"]([^'"]+)['"]"#
    )?;

    // include/extend/prepend
    let include_re = Regex::new(
        r"(?m)^[ \t]*(include|extend|prepend)\s+([A-Z][A-Za-z0-9_:]*)"
    )?;

    let lines: Vec<&str> = content.lines().collect();

    // Parse classes
    for cap in class_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let parent = cap.get(2).map(|m| m.as_str());
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        let parents: Vec<(String, String)> = parent
            .map(|p| vec![(p.to_string(), "extends".to_string())])
            .unwrap_or_default();

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Class,
            line,
            signature: line_text.trim().to_string(),
            parents,
        });
    }

    // Parse modules
    for cap in module_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Package, // Module -> Package
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse class methods (def self.xxx)
    for cap in def_self_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: format!("self.{}", name),
            kind: SymbolKind::Function,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse instance methods
    for cap in def_re.captures_iter(content) {
        let full_match = cap.get(0).unwrap().as_str();
        // Skip if this is a class method (already handled)
        if full_match.contains("self.") {
            continue;
        }

        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Function,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse attribute accessors
    for cap in attr_re.captures_iter(content) {
        let attr_type = cap.get(1).unwrap().as_str();
        let name = cap.get(2).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);

        symbols.push(ParsedSymbol {
            name: format!(":{}", name),
            kind: SymbolKind::Property,
            line,
            signature: format!("attr_{} :{}", attr_type, name),
            parents: vec![],
        });
    }

    // Parse constants
    for cap in const_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Skip if it looks like a class/module name assignment
        if line_text.contains("class ") || line_text.contains("module ") {
            continue;
        }

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Constant,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse RSpec describe/context
    for cap in rspec_describe_re.captures_iter(content) {
        let keyword = cap.get(1).unwrap().as_str();
        let desc = cap.get(2).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: format!("{} \"{}\"", keyword, desc),
            kind: SymbolKind::Class, // Test suites as classes
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse RSpec it/specify
    for cap in rspec_it_re.captures_iter(content) {
        let keyword = cap.get(1).unwrap().as_str();
        let desc = cap.get(2).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: format!("{} \"{}\"", keyword, desc),
            kind: SymbolKind::Function, // Test cases as functions
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse RSpec let/let!/subject
    for cap in rspec_let_re.captures_iter(content) {
        let keyword = cap.get(1).unwrap().as_str();
        let name = cap.get(2).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: format!("{}(:{})\"", keyword, name),
            kind: SymbolKind::Property,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse Rails scopes
    for cap in rails_scope_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: format!("scope :{}", name),
            kind: SymbolKind::Function,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse Rails associations
    for cap in rails_assoc_re.captures_iter(content) {
        let assoc_type = cap.get(1).unwrap().as_str();
        let name = cap.get(2).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: format!("{} :{}", assoc_type, name),
            kind: SymbolKind::Property,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse Rails callbacks
    for cap in rails_callback_re.captures_iter(content) {
        let callback_type = cap.get(1).unwrap().as_str();
        let name = cap.get(2).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: format!("{} :{}", callback_type, name),
            kind: SymbolKind::Annotation,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse Rails validates
    for cap in rails_validates_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: format!("validates :{}", name),
            kind: SymbolKind::Annotation,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse require statements
    for cap in require_re.captures_iter(content) {
        let path = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: path.to_string(),
            kind: SymbolKind::Import,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse include/extend/prepend
    for cap in include_re.captures_iter(content) {
        let keyword = cap.get(1).unwrap().as_str();
        let module_name = cap.get(2).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: format!("{} {}", keyword, module_name),
            kind: SymbolKind::Import,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    Ok(symbols)
}

fn find_line_number(content: &str, byte_offset: usize) -> usize {
    content[..byte_offset].matches('\n').count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_class() {
        let content = r#"
class User < ApplicationRecord
  attr_accessor :name

  def initialize(name)
    @name = name
  end
end

class Admin::Dashboard
  VERSION = "1.0"
end
"#;
        let symbols = parse_ruby_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "User" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "Admin::Dashboard" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.parents.iter().any(|(p, _)| p == "ApplicationRecord")));
    }

    #[test]
    fn test_parse_module() {
        let content = r#"
module Authenticatable
  def authenticate
    true
  end
end

module Admin::Helpers
  def admin?
    true
  end
end
"#;
        let symbols = parse_ruby_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Authenticatable" && s.kind == SymbolKind::Package));
        assert!(symbols.iter().any(|s| s.name == "Admin::Helpers" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_parse_methods() {
        let content = r#"
class Service
  def self.call(params)
    new(params).call
  end

  def call
    process
  end

  def valid?
    true
  end

  def save!
    persist
  end

  private

  def process
    # do something
  end
end
"#;
        let symbols = parse_ruby_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "self.call"));
        assert!(symbols.iter().any(|s| s.name == "call" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "valid?"));
        assert!(symbols.iter().any(|s| s.name == "save!"));
        assert!(symbols.iter().any(|s| s.name == "process"));
    }

    #[test]
    fn test_parse_rspec() {
        let content = r##"
RSpec.describe User do
  describe "#valid?" do
    let(:user) { build(:user) }

    it "returns true for valid user" do
      expect(user).to be_valid
    end

    context "when name is blank" do
      it "returns false" do
        user.name = ""
        expect(user).not_to be_valid
      end
    end
  end
end
"##;
        let symbols = parse_ruby_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name.contains("describe") && s.name.contains("#valid?")));
        assert!(symbols.iter().any(|s| s.name.contains("it") && s.name.contains("returns true")));
        assert!(symbols.iter().any(|s| s.name.contains("context") && s.name.contains("when name is blank")));
        assert!(symbols.iter().any(|s| s.name.contains("let") && s.name.contains("user")));
    }

    #[test]
    fn test_parse_rails_model() {
        let content = r#"
class Post < ApplicationRecord
  belongs_to :author
  has_many :comments
  has_one :featured_image

  validates :title
  validates :content

  scope :published, -> { where(published: true) }
  scope :recent, -> { order(created_at: :desc) }

  before_save :normalize_title
  after_create :notify_subscribers

  def publish!
    update(published: true)
  end
end
"#;
        let symbols = parse_ruby_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Post" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "belongs_to :author"));
        assert!(symbols.iter().any(|s| s.name == "has_many :comments"));
        assert!(symbols.iter().any(|s| s.name == "has_one :featured_image"));
        assert!(symbols.iter().any(|s| s.name == "validates :title"));
        assert!(symbols.iter().any(|s| s.name == "scope :published"));
        assert!(symbols.iter().any(|s| s.name == "before_save :normalize_title"));
        assert!(symbols.iter().any(|s| s.name == "publish!"));
    }

    #[test]
    fn test_parse_require() {
        let content = r#"
require 'json'
require 'net/http'
require_relative './helpers'
require_relative '../models/user'
"#;
        let symbols = parse_ruby_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "json" && s.kind == SymbolKind::Import));
        assert!(symbols.iter().any(|s| s.name == "net/http" && s.kind == SymbolKind::Import));
        assert!(symbols.iter().any(|s| s.name == "./helpers" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn test_parse_include_extend() {
        let content = r#"
class User
  include Authenticatable
  extend ClassMethods
  prepend Trackable
end
"#;
        let symbols = parse_ruby_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "include Authenticatable"));
        assert!(symbols.iter().any(|s| s.name == "extend ClassMethods"));
        assert!(symbols.iter().any(|s| s.name == "prepend Trackable"));
    }
}
