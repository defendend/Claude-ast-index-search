//! Tree-sitter based BSL (1C:Enterprise) parser

use anyhow::Result;
use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{
    line_text, node_line, node_text, parse_tree, signature_line, text_end_line, LanguageParser,
};
use crate::db::SymbolKind;
use crate::parsers::{truncate_context, FileType, ParsedRef, ParsedSymbol};

// Link the tree-sitter-bsl C library (compiled via build.rs)
unsafe extern "C" {
    fn tree_sitter_bsl() -> *const tree_sitter::ffi::TSLanguage;
}

fn bsl_language() -> Language {
    unsafe { Language::from_raw(tree_sitter_bsl()) }
}

static BSL_LANGUAGE: LazyLock<Language> = LazyLock::new(bsl_language);

static BSL_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&BSL_LANGUAGE, include_str!("queries/bsl.scm"))
        .expect("Failed to compile BSL tree-sitter query")
});

pub static BSL_PARSER: BslParser = BslParser;

pub struct BslParser;

/// Extract 1C module name from BSL file path.
///
/// In 1C:Enterprise configurations, module names are encoded in the directory structure
/// (NOT in the file content). This function parses the path to derive a qualified module
/// name matching 1C metadata conventions.
///
/// Path patterns:
/// - `CommonModules/Name/Ext/Module.bsl`     → `Name` (common module)
/// - `Documents/Name/Ext/ObjectModule.bsl`   → `Документ.Name`
/// - `Documents/Name/Ext/ManagerModule.bsl`  → `Документ.Name.МенеджерОбъекта`
/// - `Documents/Name/Forms/F/Ext/Form/Module.bsl` → `Документ.Name.Форма.F`
/// - `Catalogs/Name/Ext/ObjectModule.bsl`    → `Справочник.Name`
/// - `Enums/Name/...`                         → `Перечисление.Name`
/// - `InformationRegisters/Name/...`          → `РегистрСведений.Name`
/// - `AccumulationRegisters/Name/...`         → `РегистрНакопления.Name`
/// - `Reports/Name/...`                       → `Отчет.Name`
/// - `DataProcessors/Name/...`                → `Обработка.Name`
/// - `ChartsOfAccounts/Name/...`              → `ПланСчетов.Name`
/// - `Constants/Name/...`                     → `Константа.Name`
/// - `ExchangePlans/Name/...`                 → `ПланОбмена.Name`
/// - `BusinessProcesses/Name/...`             → `БизнесПроцесс.Name`
/// - `Tasks/Name/...`                         → `Задача.Name`
pub fn extract_bsl_module_name(path: &str) -> Option<String> {
    // Normalize path separators
    let normalized = path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').collect();

    // Find the metadata collection prefix (e.g., "CommonModules", "Documents")
    // Skip leading directories like "src", "configuration", etc.
    let (collection_idx, collection) = parts.iter().enumerate().find_map(|(i, &p)| match p {
        "CommonModules"
        | "Documents"
        | "Catalogs"
        | "Enums"
        | "InformationRegisters"
        | "AccumulationRegisters"
        | "AccountingRegisters"
        | "CalculationRegisters"
        | "Reports"
        | "DataProcessors"
        | "ChartsOfAccounts"
        | "ChartsOfCharacteristicTypes"
        | "ChartsOfCalculationTypes"
        | "Constants"
        | "ExchangePlans"
        | "BusinessProcesses"
        | "Tasks"
        | "DocumentJournals"
        | "Subsystems"
        | "CommonForms"
        | "SettingsStorages"
        | "WebServices"
        | "HTTPServices"
        | "CommonCommands"
        | "CommandGroups"
        | "FilterCriteria"
        | "EventSubscriptions"
        | "ScheduledJobs"
        | "FunctionalOptions"
        | "FunctionalOptionsParameters"
        | "DefinedTypes"
        | "Sequences"
        | "DocumentNumerators"
        | "Roles"
        | "ExternalDataSources" => Some((i, p)),
        _ => None,
    })?;

    // The name comes right after the collection
    let name = parts.get(collection_idx + 1)?;
    if name.is_empty() {
        return None;
    }

    // Map collection to Russian prefix (matching 1C conventions)
    let prefix = match collection {
        "CommonModules" => return Some(name.to_string()), // common modules have no prefix
        "Documents" => "Документ",
        "Catalogs" => "Справочник",
        "Enums" => "Перечисление",
        "InformationRegisters" => "РегистрСведений",
        "AccumulationRegisters" => "РегистрНакопления",
        "AccountingRegisters" => "РегистрБухгалтерии",
        "CalculationRegisters" => "РегистрРасчета",
        "Reports" => "Отчет",
        "DataProcessors" => "Обработка",
        "ChartsOfAccounts" => "ПланСчетов",
        "ChartsOfCharacteristicTypes" => "ПланВидовХарактеристик",
        "ChartsOfCalculationTypes" => "ПланВидовРасчета",
        "Constants" => "Константа",
        "ExchangePlans" => "ПланОбмена",
        "BusinessProcesses" => "БизнесПроцесс",
        "Tasks" => "Задача",
        "DocumentJournals" => "ЖурналДокументов",
        "Subsystems" => "Подсистема",
        "CommonForms" => "ОбщаяФорма",
        "SettingsStorages" => "ХранилищеНастроек",
        "WebServices" => "WebСервис",
        "HTTPServices" => "HTTPСервис",
        "CommonCommands" => "ОбщаяКоманда",
        "CommandGroups" => "ГруппаКоманд",
        "FilterCriteria" => "КритерийОтбора",
        "EventSubscriptions" => "ПодпискаНаСобытие",
        "ScheduledJobs" => "РегламентноеЗадание",
        "FunctionalOptions" => "ФункциональнаяОпция",
        "FunctionalOptionsParameters" => "ПараметрФункциональнойОпции",
        "DefinedTypes" => "ОпределяемыйТип",
        "Sequences" => "Последовательность",
        "DocumentNumerators" => "Нумератор",
        "Roles" => "Роль",
        "ExternalDataSources" => "ВнешнийИсточникДанных",
        _ => return None,
    };

    // Optionally append the module kind (ObjectModule, ManagerModule, Form, etc.)
    let suffix = derive_module_suffix(&parts, collection_idx + 2);
    if suffix.is_empty() {
        Some(format!("{}.{}", prefix, name))
    } else {
        Some(format!("{}.{}.{}", prefix, name, suffix))
    }
}

/// Derive the module-kind suffix for qualified names (e.g., МенеджерОбъекта, Форма.FormName).
fn derive_module_suffix(parts: &[&str], start: usize) -> String {
    // Typical layouts:
    //   Documents/Name/Ext/ObjectModule.bsl       → ""  (base is the object itself)
    //   Documents/Name/Ext/ManagerModule.bsl      → "МенеджерОбъекта"
    //   Documents/Name/Forms/FName/Ext/Form/Module.bsl → "Форма.FName"
    //   Documents/Name/Commands/CName/...         → "Команда.CName"
    if parts.len() <= start {
        return String::new();
    }

    // Check for Forms/Commands/Templates nested under the object
    for (i, &p) in parts[start..].iter().enumerate() {
        match p {
            "Forms" => {
                if let Some(form_name) = parts.get(start + i + 1) {
                    return format!("Форма.{}", form_name);
                }
            }
            "Commands" => {
                if let Some(cmd_name) = parts.get(start + i + 1) {
                    return format!("Команда.{}", cmd_name);
                }
            }
            "Templates" => {
                if let Some(tpl_name) = parts.get(start + i + 1) {
                    return format!("Макет.{}", tpl_name);
                }
            }
            _ => {}
        }
    }

    // Check filename for module-kind suffix
    let file_name = parts.last().copied().unwrap_or("");
    match file_name {
        "ManagerModule.bsl" => "МенеджерОбъекта".to_string(),
        "RecordSetModule.bsl" => "НаборЗаписей".to_string(),
        "ValueManagerModule.bsl" => "МенеджерЗначения".to_string(),
        "CommandModule.bsl" => "МодульКоманды".to_string(),
        _ => String::new(),
    }
}

/// Extract annotation text from a declaration node's preceding sibling or child
fn extract_annotation(content: &str, decl_node: &tree_sitter::Node) -> Option<(String, usize)> {
    // Look for annotation child within the declaration
    let mut cursor = decl_node.walk();
    for child in decl_node.children(&mut cursor) {
        if child.kind() == "annotation" {
            let ann_text = node_text(content, &child);
            let ann_line = node_line(&child);
            return Some((ann_text.to_string(), ann_line));
        }
    }
    None
}

/// Build enriched signature including annotation line if present
fn build_signature(
    content: &str,
    decl_line: usize,
    annotation: &Option<(String, usize)>,
    is_async: bool,
) -> String {
    let base = line_text(content, decl_line).trim().to_string();
    let mut parts = Vec::new();
    if let Some((ann_text, _)) = annotation {
        parts.push(ann_text.clone());
    }
    if is_async && !base.contains("Асинх") && !base.contains("Async") {
        parts.push("Асинх".to_string());
    }
    if parts.is_empty() {
        base
    } else {
        parts.push(base);
        parts.join(" ")
    }
}

/// Check if declaration node has Export keyword
fn has_export(decl_node: &tree_sitter::Node) -> bool {
    decl_node.child_by_field_name("export").is_some()
}

/// Check if declaration node has Async keyword
fn has_async(decl_node: &tree_sitter::Node) -> bool {
    decl_node.child_by_field_name("async").is_some()
}

impl LanguageParser for BslParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &BSL_LANGUAGE)?;
        let mut symbols = Vec::new();
        let query = &*BSL_QUERY;
        let mut cursor = QueryCursor::new();

        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_proc_name = idx("proc_name");
        let idx_proc_decl = idx("proc_decl");
        let idx_func_name = idx("func_name");
        let idx_func_decl = idx("func_decl");
        let idx_var_name = idx("var_name");
        let idx_region_name = idx("region_name");
        let idx_annotation_name = idx("annotation_name");
        let idx_definition = idx("definition");

        // Track annotation lines already emitted as part of proc/func
        let mut emitted_annotation_lines: HashSet<usize> = HashSet::new();

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            let end_line = find_capture(m, idx_definition).map(|c| text_end_line(content, &c.node));

            // Procedure → SymbolKind::Procedure (P2)
            if let Some(name_cap) = find_capture(m, idx_proc_name) {
                let decl_cap = find_capture(m, idx_proc_decl);
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);

                let annotation = decl_cap.and_then(|c| extract_annotation(content, &c.node));
                let is_export = decl_cap.map(|c| has_export(&c.node)).unwrap_or(false);
                let is_async = decl_cap.map(|c| has_async(&c.node)).unwrap_or(false);

                let mut sig = build_signature(content, line, &annotation, is_async);
                if is_export && !sig.contains("Экспорт") && !sig.contains("Export") {
                    sig.push_str(" Экспорт");
                }

                // Emit annotation as separate symbol (P3, P5)
                if let Some((ref ann_text, ann_line)) = annotation {
                    emitted_annotation_lines.insert(ann_line);
                    symbols.push(ParsedSymbol {
                        name: ann_text.clone(),
                        kind: SymbolKind::Annotation,
                        line: ann_line,
                        signature: signature_line(content, ann_line),
                        parents: vec![],
                        end_line: Some(ann_line),
                    });
                }

                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Procedure,
                    line,
                    signature: sig,
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Function → SymbolKind::Function (P2)
            if let Some(name_cap) = find_capture(m, idx_func_name) {
                let decl_cap = find_capture(m, idx_func_decl);
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);

                let annotation = decl_cap.and_then(|c| extract_annotation(content, &c.node));
                let is_export = decl_cap.map(|c| has_export(&c.node)).unwrap_or(false);
                let is_async = decl_cap.map(|c| has_async(&c.node)).unwrap_or(false);

                let mut sig = build_signature(content, line, &annotation, is_async);
                if is_export && !sig.contains("Экспорт") && !sig.contains("Export") {
                    sig.push_str(" Экспорт");
                }

                // Emit annotation as separate symbol (P3, P5)
                if let Some((ref ann_text, ann_line)) = annotation {
                    emitted_annotation_lines.insert(ann_line);
                    symbols.push(ParsedSymbol {
                        name: ann_text.clone(),
                        kind: SymbolKind::Annotation,
                        line: ann_line,
                        signature: signature_line(content, ann_line),
                        parents: vec![],
                        end_line: Some(ann_line),
                    });
                }

                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line,
                    signature: sig,
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Variable
            if let Some(cap) = find_capture(m, idx_var_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Property,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Region (as package/namespace grouping)
            if let Some(cap) = find_capture(m, idx_region_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                // A region only folds code: with a range it would become a
                // namespace and hide `Module.Procedure` behind its own name.
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Package,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line: None,
                });
                continue;
            }

            // Standalone annotation not already emitted with a proc/func (P3, P5)
            if let Some(cap) = find_capture(m, idx_annotation_name) {
                let line = node_line(&cap.node);
                if !emitted_annotation_lines.contains(&line) {
                    // Get the full annotation text including &
                    let ann_node = cap.node.parent();
                    let ann_text = ann_node
                        .map(|n| node_text(content, &n).to_string())
                        .unwrap_or_else(|| format!("&{}", node_text(content, &cap.node)));
                    symbols.push(ParsedSymbol {
                        name: ann_text,
                        kind: SymbolKind::Annotation,
                        line,
                        signature: signature_line(content, line),
                        parents: vec![],
                        end_line,
                    });
                }
                continue;
            }
        }

        Ok(symbols)
    }

    fn extract_refs_for_lang(
        &self,
        content: &str,
        defined: &[ParsedSymbol],
        _file_type: FileType,
    ) -> Result<Vec<ParsedRef>> {
        self.extract_refs(content, defined)
    }

    fn extract_refs(&self, content: &str, defined: &[ParsedSymbol]) -> Result<Vec<ParsedRef>> {
        // Only the line a symbol is declared on is skipped: a procedure called
        // further down its own module is a real usage.
        let declarations: HashSet<(&str, usize)> =
            defined.iter().map(|s| (s.name.as_str(), s.line)).collect();

        // Match identifiers: Cyrillic (А-яЁё) and Latin (A-Za-z), digits, underscores
        static BSL_IDENT_RE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"([A-Za-z\p{Cyrillic}_][A-Za-z0-9\p{Cyrillic}_]*)\s*\(").unwrap()
        });
        // CamelCase or Cyrillic type references (not followed by open paren — those are calls above)
        static BSL_TYPE_RE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?:^|[\s=,;(])([A-ZА-ЯЁ][A-Za-z0-9\p{Cyrillic}_]*)").unwrap()
        });

        static BSL_KEYWORDS: LazyLock<HashSet<String>> = LazyLock::new(|| {
            [
                // Russian keywords (per 1C:Enterprise 8.3.27 docs, section 4.2.4.6)
                "Если",
                "Тогда",
                "ИначеЕсли",
                "Иначе",
                "КонецЕсли",
                "Для",
                "Каждого",
                "Из",
                "По",
                "Цикл",
                "КонецЦикла",
                "Пока",
                "Процедура",
                "КонецПроцедуры",
                "Функция",
                "КонецФункции",
                "Перем",
                "Возврат",
                "Продолжить",
                "Прервать",
                "Попытка",
                "Исключение",
                "КонецПопытки",
                "ВызватьИсключение",
                "Новый",
                "Выполнить",
                "Не",
                "И",
                "Или",
                "Истина",
                "Ложь",
                "Неопределено",
                "Null",
                "Экспорт",
                "Знач",
                "Перейти",
                "Асинх",
                "Ждать",
                "ДобавитьОбработчик",
                "УдалитьОбработчик",
                // English keywords
                "If",
                "Then",
                "ElsIf",
                "Else",
                "EndIf",
                "For",
                "Each",
                "In",
                "To",
                "Do",
                "EndDo",
                "While",
                "Procedure",
                "EndProcedure",
                "Function",
                "EndFunction",
                "Var",
                "Return",
                "Continue",
                "Break",
                "Try",
                "Except",
                "EndTry",
                "Raise",
                "New",
                "Execute",
                "Not",
                "And",
                "Or",
                "True",
                "False",
                "Undefined",
                "Export",
                "Val",
                "Goto",
                "Async",
                "Await",
                "AddHandler",
                "RemoveHandler",
            ]
            .into_iter()
            .map(str::to_lowercase)
            .collect()
        });
        // BSL keywords are case-insensitive: `НЕ` and `ИЛИ` are as common as
        // `Не` and `Или`.
        let keywords = &*BSL_KEYWORDS;

        let mut refs = Vec::new();
        let mut seen: HashSet<(String, usize)> = HashSet::new();

        for (line_num, line) in content.lines().enumerate() {
            let line_num = line_num + 1;
            let trimmed = line.trim();

            if trimmed.len() > 2000 {
                continue;
            }
            if trimmed.starts_with("//") {
                continue;
            }

            // Calls first, then type references (CamelCase / Cyrillic
            // uppercase start); a name is recorded once per line. Procedures
            // and functions are not values in BSL, so a name without `(` is
            // only a reference as a module or object (`ЮТест.ОжидаетЧто`) or
            // as the type after `Новый`; any other capitalized word is a
            // variable or a parameter, however many modules define it.
            let calls = BSL_IDENT_RE.captures_iter(line).filter_map(|c| c.get(1));
            let types = BSL_TYPE_RE
                .captures_iter(line)
                .filter_map(|c| c.get(1))
                .filter(|m| {
                    line[m.end()..].trim_start().starts_with('.') || follows_new(&line[..m.start()])
                });
            for found in calls.chain(types) {
                let name = found.as_str();
                if name.is_empty()
                    || keywords.contains(&name.to_lowercase())
                    || declarations.contains(&(name, line_num))
                {
                    continue;
                }
                if seen.insert((name.to_string(), line_num)) {
                    refs.push(ParsedRef {
                        name: name.to_string(),
                        line: line_num,
                        context: truncate_context(trimmed),
                    });
                }
            }
        }

        Ok(refs)
    }
}

/// Whether `before` ends with the `Новый` / `New` keyword, so the next word
/// names a type: `Новый Структура`, `New Array`.
fn follows_new(before: &str) -> bool {
    let word = before
        .trim_end()
        .rsplit(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or("");
    word.to_lowercase() == "новый" || word.eq_ignore_ascii_case("new")
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
    fn test_extract_bsl_module_name_common_module() {
        assert_eq!(
            extract_bsl_module_name("CommonModules/ГосударственныеКонтракты/Ext/Module.bsl"),
            Some("ГосударственныеКонтракты".to_string())
        );
    }

    #[test]
    fn test_extract_bsl_module_name_document_object() {
        assert_eq!(
            extract_bsl_module_name("Documents/РегистрацияОбязательств/Ext/ObjectModule.bsl"),
            Some("Документ.РегистрацияОбязательств".to_string())
        );
    }

    #[test]
    fn test_extract_bsl_module_name_document_manager() {
        assert_eq!(
            extract_bsl_module_name("Documents/Заказ/Ext/ManagerModule.bsl"),
            Some("Документ.Заказ.МенеджерОбъекта".to_string())
        );
    }

    #[test]
    fn test_extract_bsl_module_name_catalog() {
        assert_eq!(
            extract_bsl_module_name("Catalogs/Товары/Ext/ObjectModule.bsl"),
            Some("Справочник.Товары".to_string())
        );
    }

    #[test]
    fn test_extract_bsl_module_name_form() {
        assert_eq!(
            extract_bsl_module_name("Documents/Заказ/Forms/ФормаДокумента/Ext/Form/Module.bsl"),
            Some("Документ.Заказ.Форма.ФормаДокумента".to_string())
        );
    }

    #[test]
    fn test_extract_bsl_module_name_info_register() {
        assert_eq!(
            extract_bsl_module_name(
                "InformationRegisters/ЦеныНоменклатуры/Ext/RecordSetModule.bsl"
            ),
            Some("РегистрСведений.ЦеныНоменклатуры.НаборЗаписей".to_string())
        );
    }

    #[test]
    fn test_extract_bsl_module_name_nested_src() {
        // Prefixed with configuration/src folder — should still work
        assert_eq!(
            extract_bsl_module_name("src/cf/CommonModules/УтилитыСистемы/Ext/Module.bsl"),
            Some("УтилитыСистемы".to_string())
        );
    }

    #[test]
    fn test_extract_bsl_module_name_backslash() {
        // Windows-style paths
        assert_eq!(
            extract_bsl_module_name("CommonModules\\Утилиты\\Ext\\Module.bsl"),
            Some("Утилиты".to_string())
        );
    }

    #[test]
    fn test_extract_bsl_module_name_unknown() {
        // Non-1C BSL file → None
        assert_eq!(extract_bsl_module_name("scripts/helper.bsl"), None);
    }

    #[test]
    fn test_extract_bsl_module_name_enum() {
        assert_eq!(
            extract_bsl_module_name("Enums/СтатусыЗаказов/Ext/ManagerModule.bsl"),
            Some("Перечисление.СтатусыЗаказов.МенеджерОбъекта".to_string())
        );
    }

    #[test]
    fn test_parse_procedure_ru() {
        let content = "Процедура МояПроцедура()\nКонецПроцедуры\n";
        // Debug: parse tree
        let tree = parse_tree(content, &BSL_LANGUAGE).unwrap();
        eprintln!("DEBUG tree: {}", tree.root_node().to_sexp());
        let symbols = BSL_PARSER.parse_symbols(content).unwrap();
        eprintln!(
            "DEBUG symbols: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.kind))
                .collect::<Vec<_>>()
        );
        assert!(symbols
            .iter()
            .any(|s| s.name == "МояПроцедура" && s.kind == SymbolKind::Procedure));
    }

    #[test]
    fn test_parse_function_en() {
        let content = "Function GetData() Export\n    Return 42;\nEndFunction\n";
        let symbols = BSL_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "GetData" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_procedure_en() {
        let content = "Procedure DoWork(Param1)\n    // work\nEndProcedure\n";
        let symbols = BSL_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "DoWork" && s.kind == SymbolKind::Procedure));
    }

    #[test]
    fn test_parse_variable() {
        let content = "Перем МояПеременная;\n";
        let symbols = BSL_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "МояПеременная" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_region() {
        let content = "#Область ОбработчикиСобытий\n#КонецОбласти\n";
        let symbols = BSL_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "ОбработчикиСобытий" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_parse_complex_module() {
        let content = r#"
Перем МодульнаяПеременная;

#Область ОбработчикиСобытий

Процедура ПриСозданииНаСервере(Отказ, СтандартнаяОбработка)
КонецПроцедуры

Функция ПолучитьДанные() Экспорт
    Возврат 42;
КонецФункции

#КонецОбласти
"#;
        let symbols = BSL_PARSER.parse_symbols(content).unwrap();
        let procs: Vec<_> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Procedure)
            .collect();
        let funcs: Vec<_> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Function)
            .collect();
        let props: Vec<_> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Property)
            .collect();
        let pkgs: Vec<_> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Package)
            .collect();
        assert_eq!(procs.len(), 1, "should have 1 procedure");
        assert_eq!(funcs.len(), 1, "should have 1 function");
        assert!(props.len() >= 1);
        assert_eq!(pkgs.len(), 1);
    }

    #[test]
    fn test_procedure_vs_function() {
        let content = r#"Процедура Обработать()
КонецПроцедуры

Функция ПолучитьДанные()
    Возврат 1;
КонецФункции
"#;
        let symbols = BSL_PARSER.parse_symbols(content).unwrap();
        let proc = symbols.iter().find(|s| s.name == "Обработать").unwrap();
        let func = symbols.iter().find(|s| s.name == "ПолучитьДанные").unwrap();
        assert_eq!(proc.kind, SymbolKind::Procedure);
        assert_eq!(func.kind, SymbolKind::Function);
    }

    #[test]
    fn test_export_in_signature() {
        let content = "Функция ПолучитьДанные() Экспорт\n    Возврат 42;\nКонецФункции\n";
        let symbols = BSL_PARSER.parse_symbols(content).unwrap();
        let func = symbols.iter().find(|s| s.name == "ПолучитьДанные").unwrap();
        assert!(
            func.signature.contains("Экспорт"),
            "signature should contain Экспорт: {}",
            func.signature
        );
    }

    #[test]
    fn test_export_en_in_signature() {
        let content = "Function GetData() Export\n    Return 42;\nEndFunction\n";
        let symbols = BSL_PARSER.parse_symbols(content).unwrap();
        let func = symbols.iter().find(|s| s.name == "GetData").unwrap();
        assert!(
            func.signature.contains("Export"),
            "signature should contain Export: {}",
            func.signature
        );
    }

    #[test]
    fn test_compilation_directive() {
        let content = "&НаСервере\nПроцедура ОбработатьНаСервере()\nКонецПроцедуры\n";
        let symbols = BSL_PARSER.parse_symbols(content).unwrap();
        // Procedure should exist
        let proc = symbols.iter().find(|s| s.name == "ОбработатьНаСервере");
        assert!(proc.is_some(), "should find procedure");
        let proc = proc.unwrap();
        assert_eq!(proc.kind, SymbolKind::Procedure);
        // Signature should include the directive
        assert!(
            proc.signature.contains("НаСервере"),
            "signature should include directive: {}",
            proc.signature
        );
        // Annotation should be emitted as separate symbol
        let annotations: Vec<_> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Annotation)
            .collect();
        assert!(!annotations.is_empty(), "should have annotation symbol");
    }

    #[test]
    fn test_compilation_directive_en() {
        let content = "&AtClient\nProcedure OnOpen()\nEndProcedure\n";
        let symbols = BSL_PARSER.parse_symbols(content).unwrap();
        let proc = symbols.iter().find(|s| s.name == "OnOpen");
        assert!(proc.is_some(), "should find procedure");
        let proc = proc.unwrap();
        assert!(
            proc.signature.contains("AtClient"),
            "signature should include directive: {}",
            proc.signature
        );
    }

    #[test]
    fn test_extract_refs_cyrillic() {
        let content = r#"Процедура Обработать()
    Результат = ПолучитьДанные();
    ЗаписатьВЖурнал(Результат);
КонецПроцедуры
"#;
        let symbols = BSL_PARSER.parse_symbols(content).unwrap();
        let refs = BSL_PARSER.extract_refs(content, &symbols).unwrap();
        assert!(
            refs.iter().any(|r| r.name == "ПолучитьДанные"),
            "should find ПолучитьДанные call"
        );
        assert!(
            refs.iter().any(|r| r.name == "ЗаписатьВЖурнал"),
            "should find ЗаписатьВЖурнал call"
        );
    }

    #[test]
    fn test_extract_refs_skips_keywords() {
        let content = "Если Истина Тогда\n    Возврат Неопределено;\nКонецЕсли\n";
        let symbols = BSL_PARSER.parse_symbols(content).unwrap();
        let refs = BSL_PARSER.extract_refs(content, &symbols).unwrap();
        assert!(!refs.iter().any(|r| r.name == "Если"));
        assert!(!refs.iter().any(|r| r.name == "Истина"));
        assert!(!refs.iter().any(|r| r.name == "Неопределено"));
    }

    #[test]
    fn test_extract_refs_mixed() {
        let content = r#"Процедура Тест()
    Данные = GetData();
    Запись = Новый ТаблицаЗначений;
КонецПроцедуры
"#;
        let symbols = BSL_PARSER.parse_symbols(content).unwrap();
        let refs = BSL_PARSER.extract_refs(content, &symbols).unwrap();
        assert!(
            refs.iter().any(|r| r.name == "GetData"),
            "should find English call"
        );
        assert!(
            refs.iter().any(|r| r.name == "ТаблицаЗначений"),
            "should find Cyrillic type ref"
        );
    }

    #[test]
    fn test_async_procedure() {
        let content = "Асинх Процедура ЗагрузитьДанные()\nКонецПроцедуры\n";
        let symbols = BSL_PARSER.parse_symbols(content).unwrap();
        let proc = symbols.iter().find(|s| s.name == "ЗагрузитьДанные");
        assert!(proc.is_some(), "should find async procedure");
        let proc = proc.unwrap();
        assert_eq!(proc.kind, SymbolKind::Procedure);
        assert!(
            proc.signature.contains("Асинх"),
            "signature should contain Асинх: {}",
            proc.signature
        );
    }

    #[test]
    fn test_async_function_en() {
        let content = "Async Function LoadData() Export\n    Return 42;\nEndFunction\n";
        let symbols = BSL_PARSER.parse_symbols(content).unwrap();
        let func = symbols.iter().find(|s| s.name == "LoadData");
        assert!(func.is_some(), "should find async function");
        let func = func.unwrap();
        assert_eq!(func.kind, SymbolKind::Function);
        assert!(
            func.signature.contains("Async"),
            "signature should contain Async: {}",
            func.signature
        );
    }
}
