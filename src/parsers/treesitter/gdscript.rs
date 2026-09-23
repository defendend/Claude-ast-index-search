//! Tree-sitter based GDScript parser (Godot Engine)

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{line_text, node_line, node_text, parse_tree, text_end_line, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static GDSCRIPT_LANGUAGE: LazyLock<Language> =
    LazyLock::new(|| tree_sitter_gdscript::LANGUAGE.into());

static GDSCRIPT_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&GDSCRIPT_LANGUAGE, include_str!("queries/gdscript.scm"))
        .expect("Failed to compile GDScript tree-sitter query")
});

pub static GDSCRIPT_PARSER: GdscriptParser = GdscriptParser;

pub struct GdscriptParser;

impl LanguageParser for GdscriptParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &GDSCRIPT_LANGUAGE)?;
        let mut symbols = Vec::new();
        let mut cursor = QueryCursor::new();
        let query = &*GDSCRIPT_QUERY;

        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_class_name_decl = idx("class_name_decl");
        let idx_class_def_name = idx("class_def_name");
        let idx_extends_name = idx("extends_name");
        let idx_func_name = idx("func_name");
        let idx_signal_name = idx("signal_name");
        let idx_enum_name = idx("enum_name");
        let idx_const_name = idx("const_name");
        let idx_var_name = idx("var_name");
        let idx_export_var_name = idx("export_var_name");
        let idx_onready_var_name = idx("onready_var_name");
        let idx_definition = idx("definition");

        // Track extends for class parents
        let mut extends_parent: Option<String> = None;

        // First pass: find extends
        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());
        while let Some(m) = matches.next() {
            if let Some(cap) = find_capture(m, idx_extends_name) {
                extends_parent = Some(node_text(content, &cap.node).to_string());
            }
        }

        // Find constructor_definition nodes (they have no name field, always _init)
        let mut constructor_lines = std::collections::BTreeMap::new();
        {
            let mut walk = tree.root_node().walk();
            fn find_constructors(
                content: &str,
                cursor: &mut tree_sitter::TreeCursor,
                lines: &mut std::collections::BTreeMap<usize, usize>,
            ) {
                if cursor.node().kind() == "constructor_definition" {
                    let node = cursor.node();
                    lines.insert(node.start_position().row + 1, text_end_line(content, &node));
                }
                if cursor.goto_first_child() {
                    loop {
                        find_constructors(content, cursor, lines);
                        if !cursor.goto_next_sibling() {
                            break;
                        }
                    }
                    cursor.goto_parent();
                }
            }
            find_constructors(content, &mut walk, &mut constructor_lines);
        }
        for (line, end_line) in &constructor_lines {
            symbols.push(ParsedSymbol {
                name: "_init".to_string(),
                kind: SymbolKind::Function,
                line: *line,
                signature: line_text(content, *line).trim().to_string(),
                parents: vec![],
                end_line: Some(*end_line),
            });
        }

        // Second pass: extract symbols
        let mut cursor2 = QueryCursor::new();
        let mut matches = cursor2.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            let end_line = find_capture(m, idx_definition).map(|c| text_end_line(content, &c.node));

            // class_name MyClass
            if let Some(cap) = find_capture(m, idx_class_name_decl) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                let parents = extends_parent
                    .as_ref()
                    .map(|p| vec![(p.clone(), "extends".to_string())])
                    .unwrap_or_default();
                // `class_name` names the whole script: every function of the
                // file is a method of that class.
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Class,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents,
                    end_line: Some(text_end_line(content, &tree.root_node())),
                });
                continue;
            }

            // class InnerClass:
            if let Some(cap) = find_capture(m, idx_class_def_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Class,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // func
            if let Some(cap) = find_capture(m, idx_func_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // signal
            if let Some(cap) = find_capture(m, idx_signal_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Property,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // enum
            if let Some(cap) = find_capture(m, idx_enum_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Enum,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // const
            if let Some(cap) = find_capture(m, idx_const_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Constant,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // var / @export var / @onready var
            if let Some(cap) = find_capture(m, idx_var_name)
                .or_else(|| find_capture(m, idx_export_var_name))
                .or_else(|| find_capture(m, idx_onready_var_name))
            {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Property,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }
        }

        Ok(symbols)
    }
}

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
    fn test_parse_class_and_extends() {
        let content = r#"class_name Player
extends CharacterBody2D

func _ready():
    pass
"#;
        let symbols = GDSCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Player" && s.kind == SymbolKind::Class));
        let player = symbols.iter().find(|s| s.name == "Player").unwrap();
        assert_eq!(player.parents.len(), 1);
        assert_eq!(player.parents[0].0, "CharacterBody2D");
        assert!(symbols
            .iter()
            .any(|s| s.name == "_ready" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_signals() {
        let content = r#"class_name UI
extends Control

signal health_changed(new_health)
signal died
"#;
        let symbols = GDSCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "health_changed" && s.kind == SymbolKind::Property),
            "should find signal health_changed; got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.kind))
                .collect::<Vec<_>>()
        );
        assert!(symbols
            .iter()
            .any(|s| s.name == "died" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_enums_and_consts() {
        let content = r#"enum State { IDLE, RUNNING, JUMPING }
const MAX_SPEED = 300
const GRAVITY: float = 980.0
"#;
        let symbols = GDSCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "State" && s.kind == SymbolKind::Enum));
        assert!(symbols
            .iter()
            .any(|s| s.name == "MAX_SPEED" && s.kind == SymbolKind::Constant));
        assert!(symbols
            .iter()
            .any(|s| s.name == "GRAVITY" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_parse_variables() {
        let content = r#"var speed: float = 10.0
var health: int = 100
@export var damage: int = 25
@onready var sprite: Sprite2D = $Sprite2D
"#;
        let symbols = GDSCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "speed" && s.kind == SymbolKind::Property),
            "should find var speed; got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.kind))
                .collect::<Vec<_>>()
        );
        assert!(symbols
            .iter()
            .any(|s| s.name == "health" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "damage" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "sprite" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_functions() {
        let content = r#"func _init():
    pass

func _process(delta: float) -> void:
    pass

func take_damage(amount: int) -> void:
    health -= amount

static func create() -> Player:
    return Player.new()
"#;
        let symbols = GDSCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "_init" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "_process" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "take_damage" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "create" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_inner_class() {
        let content = r#"class_name Weapon
extends Node2D

class DamageInfo:
    var amount: int
    var type: String

func attack():
    var info = DamageInfo.new()
"#;
        let symbols = GDSCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Weapon" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "DamageInfo" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "attack" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_full_godot_script() {
        let content = r#"class_name Enemy
extends CharacterBody2D

signal defeated
signal health_changed(new_health)

enum State { IDLE, PATROL, CHASE, ATTACK }

const MAX_SPEED: float = 200.0
const ATTACK_RANGE: float = 50.0

@export var health: int = 100
@export var damage: int = 10
@onready var nav_agent: NavigationAgent2D = $NavigationAgent2D

var current_state: State = State.IDLE
var target: Node2D = null

func _ready() -> void:
    pass

func _physics_process(delta: float) -> void:
    match current_state:
        State.IDLE:
            _idle_state()
        State.CHASE:
            _chase_state(delta)

func take_damage(amount: int) -> void:
    health -= amount
    health_changed.emit(health)
    if health <= 0:
        defeated.emit()

func _idle_state() -> void:
    pass

func _chase_state(delta: float) -> void:
    pass
"#;
        let symbols = GDSCRIPT_PARSER.parse_symbols(content).unwrap();
        // Class with parent
        let enemy = symbols.iter().find(|s| s.name == "Enemy").unwrap();
        assert_eq!(enemy.kind, SymbolKind::Class);
        assert_eq!(enemy.parents[0].0, "CharacterBody2D");
        // Signals
        assert!(symbols
            .iter()
            .any(|s| s.name == "defeated" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "health_changed" && s.kind == SymbolKind::Property));
        // Enum
        assert!(symbols
            .iter()
            .any(|s| s.name == "State" && s.kind == SymbolKind::Enum));
        // Constants
        assert!(symbols
            .iter()
            .any(|s| s.name == "MAX_SPEED" && s.kind == SymbolKind::Constant));
        assert!(symbols
            .iter()
            .any(|s| s.name == "ATTACK_RANGE" && s.kind == SymbolKind::Constant));
        // Properties
        assert!(symbols
            .iter()
            .any(|s| s.name == "health" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "nav_agent" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "current_state" && s.kind == SymbolKind::Property));
        // Functions
        assert!(symbols
            .iter()
            .any(|s| s.name == "_ready" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "_physics_process" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "take_damage" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "_idle_state" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "_chase_state" && s.kind == SymbolKind::Function));
    }
}
