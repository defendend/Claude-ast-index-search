//! Rails models and the tables `db/schema.rb` declares: which model reads
//! which table, so `record.column` can resolve to the column definition.
//!
//! The mapping follows Active Record's own rules as far as an index can see
//! them: an explicit `self.table_name = "..."`, single-table inheritance (a
//! subclass of a concrete model shares its table), a model nested in another
//! model (`Order::Line` -> `order_lines`), and the pluralized, underscored
//! class name. What cannot be decided is reported rather than guessed.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// Uncountable words from Active Support's default inflections.
const UNCOUNTABLE: &[&str] = &[
    "equipment",
    "information",
    "rice",
    "money",
    "species",
    "series",
    "fish",
    "sheep",
    "jeans",
    "police",
];

/// Irregular singular -> plural pairs from Active Support's defaults.
const IRREGULAR: &[(&str, &str)] = &[
    ("person", "people"),
    ("man", "men"),
    ("child", "children"),
    ("sex", "sexes"),
    ("move", "moves"),
    ("zombie", "zombies"),
];

/// `HTTPClient` -> `http_client`, as `String#underscore` does for one segment.
pub fn underscore(segment: &str) -> String {
    let chars: Vec<char> = segment.chars().collect();
    let mut out = String::with_capacity(segment.len() + 4);
    for (position, &character) in chars.iter().enumerate() {
        if character.is_uppercase() && position > 0 {
            let previous = chars[position - 1];
            let next_lower = chars.get(position + 1).is_some_and(|c| c.is_lowercase());
            if previous.is_lowercase()
                || previous.is_ascii_digit()
                || (previous.is_uppercase() && next_lower)
            {
                out.push('_');
            }
        }
        out.extend(character.to_lowercase());
    }
    out
}

/// English plural of a snake_case word with Active Support's default rules
/// (`status` -> `statuses`, `person` -> `people`, `company` -> `companies`).
pub fn pluralize(word: &str) -> String {
    let last_word_start = word.rfind('_').map(|index| index + 1).unwrap_or(0);
    let (head, last) = word.split_at(last_word_start);
    if last.is_empty() || UNCOUNTABLE.contains(&word) {
        return word.to_string();
    }
    for (singular, plural) in IRREGULAR {
        if let Some(stem) = word.strip_suffix(singular) {
            return format!("{stem}{plural}");
        }
        if word.ends_with(plural) {
            return word.to_string();
        }
    }
    format!("{head}{}", pluralize_word(last))
}

fn pluralize_word(word: &str) -> String {
    let ends = |suffix: &str| word.ends_with(suffix);
    let stem = |len: usize| &word[..word.len() - len];
    let consonant_before = |len: usize| {
        let stem = stem(len);
        stem.ends_with("qu")
            || stem
                .chars()
                .next_back()
                .is_some_and(|c| !"aeiouy".contains(c))
    };
    if word == "ox" {
        return "oxen".to_string();
    }
    if word == "oxen" {
        return word.to_string();
    }
    if ends("quiz") {
        return format!("{word}zes");
    }
    if word == "mouse" || word == "louse" {
        return format!("{}ice", stem(4));
    }
    if word == "mice" || word == "lice" {
        return word.to_string();
    }
    for root in ["matr", "vert", "ind"] {
        for tail in ["ix", "ex"] {
            if ends(&format!("{root}{tail}")) {
                return format!("{}ices", stem(2));
            }
        }
    }
    if ends("x") || ends("ch") || ends("ss") || ends("sh") {
        return format!("{word}es");
    }
    if ends("y") && consonant_before(1) {
        return format!("{}ies", stem(1));
    }
    if ends("hive") {
        return format!("{word}s");
    }
    if ends("fe") && !ends("ffe") {
        return format!("{}ves", stem(2));
    }
    if (ends("lf") || ends("rf")) && word.len() > 2 {
        return format!("{}ves", stem(1));
    }
    if ends("sis") {
        return format!("{}ses", stem(3));
    }
    if ends("tum") || ends("ium") {
        return format!("{}a", stem(2));
    }
    if ends("ta") || ends("ia") {
        return word.to_string();
    }
    if ends("buffalo") || ends("tomato") {
        return format!("{word}es");
    }
    if ends("bus") {
        return format!("{word}es");
    }
    if ends("alias") || ends("status") {
        return format!("{word}es");
    }
    if ends("octopus") || ends("virus") {
        return format!("{}i", stem(2));
    }
    if ends("octopi") || ends("viri") {
        return word.to_string();
    }
    if word == "axis" || word == "testis" {
        return format!("{}es", stem(2));
    }
    if ends("s") {
        return word.to_string();
    }
    format!("{word}s")
}

/// What the index knows about one Ruby class (all reopenings merged).
#[derive(Clone, Debug, Default)]
pub struct ModelClass {
    /// Superclass as written (`ActiveRecord::Base`, `::ApplicationRecord`).
    pub superclass_written: Option<String>,
    /// Namespace path of the superclass when the graph resolved it.
    pub superclass: Option<String>,
    /// `self.table_name = "..."`.
    pub explicit_table: Option<String>,
    /// `self.abstract_class = true`.
    pub abstract_class: bool,
    /// Defined only under test directories.
    pub test_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkRule {
    Explicit,
    Inherited,
    Nested,
    Convention,
    Prefixed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MissingTable {
    pub model: String,
    pub table: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AmbiguousModel {
    pub model: String,
    pub tables: Vec<String>,
}

/// How `db/schema.rb` tables were matched to models, stored with the graph.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SchemaLinkSummary {
    pub tables: u64,
    pub columns: u64,
    pub tables_linked: u64,
    pub models_linked: u64,
    /// Models linked per rule.
    pub by_rule: BTreeMap<LinkRule, u64>,
    pub tables_without_model: Vec<String>,
    pub models_without_table: Vec<MissingTable>,
    pub ambiguous_models: Vec<AmbiguousModel>,
}

enum Outcome {
    Linked(String, LinkRule),
    Missing(String),
    Ambiguous(Vec<String>),
    NotModel,
}

struct Linker<'a> {
    classes: &'a HashMap<String, ModelClass>,
    tables: &'a HashSet<String>,
    prefixes: &'a HashMap<String, String>,
    active_record: HashMap<String, bool>,
    outcomes: HashMap<String, Outcome>,
}

fn is_base_name(written: &str) -> bool {
    let name = written.trim_start_matches("::");
    name == "ActiveRecord::Base"
}

impl<'a> Linker<'a> {
    fn is_active_record(&mut self, qual: &str, depth: usize) -> bool {
        if is_base_name(qual) {
            return true;
        }
        if let Some(&known) = self.active_record.get(qual) {
            return known;
        }
        let Some(class) = self.classes.get(qual) else {
            return false;
        };
        let result = depth < 16
            && (class.superclass_written.as_deref().is_some_and(|written| {
                is_base_name(written)
                    || (class.superclass.is_none()
                        && written
                            .trim_start_matches("::")
                            .ends_with("ApplicationRecord"))
            }) || class
                .superclass
                .clone()
                .is_some_and(|parent| self.is_active_record(&parent, depth + 1)));
        self.active_record.insert(qual.to_string(), result);
        result
    }

    fn table_of(&mut self, qual: &str) -> Option<String> {
        match self.outcome(qual, 0) {
            Outcome::Linked(table, _) => Some(table.clone()),
            _ => None,
        }
    }

    fn outcome(&mut self, qual: &str, depth: usize) -> &Outcome {
        if !self.outcomes.contains_key(qual) {
            let outcome = self.compute(qual, depth);
            self.outcomes.insert(qual.to_string(), outcome);
        }
        &self.outcomes[qual]
    }

    fn compute(&mut self, qual: &str, depth: usize) -> Outcome {
        let Some(class) = self.classes.get(qual).cloned() else {
            return Outcome::NotModel;
        };
        if let Some(table) = &class.explicit_table {
            let table = table.rsplit('.').next().unwrap_or(table).to_string();
            return if self.tables.contains(&table) {
                Outcome::Linked(table, LinkRule::Explicit)
            } else {
                Outcome::Missing(table)
            };
        }
        if depth > 16
            || class.abstract_class
            || is_base_name(qual)
            || !self.is_active_record(qual, 0)
        {
            return Outcome::NotModel;
        }
        if let Some(parent) = &class.superclass {
            if let Outcome::Linked(table, _) = self.outcome(parent, depth + 1) {
                return Outcome::Linked(table.clone(), LinkRule::Inherited);
            }
        }
        let (namespace, own) = match qual.rsplit_once("::") {
            Some((namespace, own)) => (namespace, own),
            None => ("", qual),
        };
        let plural = pluralize(&underscore(own));
        // `table_name_prefix` of the nearest enclosing namespace that has one.
        let mut declared_prefix: Option<&str> = None;
        let mut scope = namespace;
        while !scope.is_empty() {
            if let Some(prefix) = self.prefixes.get(scope) {
                declared_prefix = Some(prefix);
                break;
            }
            scope = scope.rsplit_once("::").map(|(head, _)| head).unwrap_or("");
        }
        let concrete_outer = self
            .classes
            .get(namespace)
            .is_some_and(|outer| !outer.abstract_class && outer.explicit_table.is_none())
            && self.is_active_record(namespace, 0);
        // Active Record prefixes a model nested in a concrete model with the
        // outer table's singular; for a conventionally named outer model that
        // is its underscored name.
        if concrete_outer {
            let outer = namespace.rsplit("::").next().unwrap_or(namespace);
            let table = format!(
                "{}{}_{plural}",
                declared_prefix.unwrap_or(""),
                underscore(outer)
            );
            return if self.tables.contains(&table) {
                Outcome::Linked(table, LinkRule::Nested)
            } else {
                Outcome::Missing(table)
            };
        }
        if let Some(prefix) = declared_prefix {
            let table = format!("{prefix}{plural}");
            return if self.tables.contains(&table) {
                Outcome::Linked(table, LinkRule::Prefixed)
            } else {
                Outcome::Missing(table)
            };
        }
        // Without a declared prefix, a namespaced model may still read a
        // namespaced table through a prefix defined outside the index.
        let prefixed = (!namespace.is_empty()).then(|| {
            let prefix: Vec<String> = namespace.split("::").map(underscore).collect();
            format!("{}_{plural}", prefix.join("_"))
        });
        let plain_exists = self.tables.contains(&plural);
        let prefixed_exists = prefixed
            .as_ref()
            .is_some_and(|table| self.tables.contains(table));
        match (plain_exists, prefixed_exists, prefixed) {
            (true, true, Some(prefixed)) => Outcome::Ambiguous(vec![plural, prefixed]),
            (true, _, _) => Outcome::Linked(plural, LinkRule::Convention),
            (false, true, Some(prefixed)) => Outcome::Linked(prefixed, LinkRule::Prefixed),
            _ => Outcome::Missing(plural),
        }
    }
}

/// Match every model class to a table name from `tables`; returns the links
/// (class namespace path -> table name) and what could not be matched.
/// `prefixes` maps a namespace to its `table_name_prefix`.
pub fn link_models(
    classes: &HashMap<String, ModelClass>,
    tables: &HashSet<String>,
    prefixes: &HashMap<String, String>,
    columns: u64,
) -> (HashMap<String, String>, SchemaLinkSummary) {
    let mut linker = Linker {
        classes,
        tables,
        prefixes,
        active_record: HashMap::new(),
        outcomes: HashMap::new(),
    };
    let mut names: Vec<&String> = classes.keys().collect();
    names.sort();
    let mut links = HashMap::new();
    let mut rules: BTreeMap<LinkRule, u64> = BTreeMap::new();
    let mut summary = SchemaLinkSummary {
        tables: tables.len() as u64,
        columns,
        ..SchemaLinkSummary::default()
    };
    for qual in names {
        let test_only = classes[qual].test_only;
        linker.table_of(qual);
        match &linker.outcomes[qual.as_str()] {
            Outcome::Linked(table, rule) => {
                links.insert(qual.clone(), table.clone());
                *rules.entry(*rule).or_default() += 1;
            }
            Outcome::Missing(table) if !test_only => {
                summary.models_without_table.push(MissingTable {
                    model: qual.clone(),
                    table: table.clone(),
                })
            }
            Outcome::Ambiguous(candidates) if !test_only => {
                summary.ambiguous_models.push(AmbiguousModel {
                    model: qual.clone(),
                    tables: candidates.clone(),
                })
            }
            _ => {}
        }
    }
    let linked: HashSet<&String> = links.values().collect();
    let mut without: Vec<String> = tables
        .iter()
        .filter(|table| !linked.contains(table))
        .cloned()
        .collect();
    without.sort();
    summary.tables_linked = linked.len() as u64;
    summary.models_linked = links.len() as u64;
    summary.tables_without_model = without;
    summary.by_rule = rules;
    (links, summary)
}

/// Column names an Active Record reader or attribute method named `name`
/// can stand for: `status`, `status?`, `status_changed?`,
/// `saved_change_to_status?`, `will_save_change_to_status?`.
pub fn column_candidates(name: &str) -> Vec<&str> {
    let Some(base) = name.strip_suffix('?') else {
        return if name.ends_with('!') {
            Vec::new()
        } else {
            vec![name]
        };
    };
    let mut out = vec![base];
    for suffix in ["_changed", "_previously_changed"] {
        if let Some(column) = base.strip_suffix(suffix) {
            out.push(column);
        }
    }
    for prefix in ["saved_change_to_", "will_save_change_to_"] {
        if let Some(column) = base.strip_prefix(prefix) {
            out.push(column);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pluralize_follows_active_support_defaults() {
        let cases = [
            ("applicant", "applicants"),
            ("company", "companies"),
            ("status", "statuses"),
            ("job_status", "job_statuses"),
            ("person", "people"),
            ("sales_person", "sales_people"),
            ("address", "addresses"),
            ("box", "boxes"),
            ("match", "matches"),
            ("day", "days"),
            ("key", "keys"),
            ("analysis", "analyses"),
            ("datum", "data"),
            ("medium", "media"),
            ("wife", "wives"),
            ("half", "halves"),
            ("information", "information"),
            ("child", "children"),
            ("alias", "aliases"),
            ("bus", "buses"),
            ("index", "indices"),
            ("mouse", "mice"),
            ("quiz", "quizzes"),
            ("news", "news"),
        ];
        for (singular, plural) in cases {
            assert_eq!(pluralize(singular), plural, "{singular}");
        }
        assert_eq!(underscore("HTTPClient"), "http_client");
        assert_eq!(underscore("JobStatus"), "job_status");
    }

    fn class(superclass: Option<&str>, resolved: Option<&str>) -> ModelClass {
        ModelClass {
            superclass_written: superclass.map(str::to_string),
            superclass: resolved.map(str::to_string),
            ..ModelClass::default()
        }
    }

    #[test]
    fn models_link_by_every_active_record_rule() {
        let mut classes: HashMap<String, ModelClass> = HashMap::new();
        classes.insert(
            "ApplicationRecord".into(),
            ModelClass {
                abstract_class: true,
                ..class(Some("ActiveRecord::Base"), None)
            },
        );
        let base = |name: &str| {
            (
                name.to_string(),
                class(Some("ApplicationRecord"), Some("ApplicationRecord")),
            )
        };
        classes.extend([
            base("Person"),
            base("Order"),
            base("Order::Line"),
            base("Billing::Invoice"),
            base("Ledger::Entry"),
            base("Orphan"),
            base("Shop::Models::Item"),
        ]);
        classes.insert(
            "Customer".into(),
            ModelClass {
                explicit_table: Some("legacy.clients".into()),
                ..class(Some("ApplicationRecord"), Some("ApplicationRecord"))
            },
        );
        classes.insert("Admin".into(), class(Some("Person"), Some("Person")));
        classes.insert("Report".into(), class(Some("Base"), None));
        let tables: HashSet<String> = [
            "people",
            "orders",
            "order_lines",
            "invoices",
            "billing_invoices",
            "ledger_entries",
            "clients",
            "audits",
            "shop_items",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        let prefixes: HashMap<String, String> = [("Shop".to_string(), "shop_".to_string())]
            .into_iter()
            .collect();
        let (links, summary) = link_models(&classes, &tables, &prefixes, 0);
        assert_eq!(links["Person"], "people");
        assert_eq!(links["Admin"], "people");
        assert_eq!(links["Order::Line"], "order_lines");
        assert_eq!(links["Ledger::Entry"], "ledger_entries");
        assert_eq!(links["Customer"], "clients");
        assert_eq!(links["Shop::Models::Item"], "shop_items");
        assert!(!links.contains_key("ApplicationRecord"));
        assert!(!links.contains_key("Report"));
        assert!(!links.contains_key("Billing::Invoice"));
        assert_eq!(summary.ambiguous_models[0].model, "Billing::Invoice");
        assert_eq!(summary.models_without_table[0].model, "Orphan");
        assert_eq!(
            summary.tables_without_model,
            vec!["audits", "billing_invoices", "invoices"]
        );
    }

    #[test]
    fn attribute_methods_name_their_column() {
        assert_eq!(column_candidates("email"), vec!["email"]);
        assert_eq!(
            column_candidates("saved_change_to_status?"),
            vec!["saved_change_to_status", "status"]
        );
        assert_eq!(
            column_candidates("status_changed?"),
            vec!["status_changed", "status"]
        );
        assert!(column_candidates("save!").is_empty());
    }
}
