/// Compiled JSON Schema representation.
///
/// Parsed from `serde_json::Value` into compact Rust structs. Uses `Arc` for
/// shared ownership of sub-schemas, enabling cheap cloning during validation
/// and completion walking.
use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Draft detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SchemaDraft {
    Draft4,
    Draft6,
    #[default]
    Draft7,
    Draft2019_09,
    Draft2020_12,
}

impl SchemaDraft {
    pub fn from_schema_uri(uri: &str) -> Self {
        if uri.contains("draft-04") || uri.contains("draft/04") {
            SchemaDraft::Draft4
        } else if uri.contains("draft-06") || uri.contains("draft/06") {
            SchemaDraft::Draft6
        } else if uri.contains("draft-07") || uri.contains("draft/07") {
            SchemaDraft::Draft7
        } else if uri.contains("2019-09") {
            SchemaDraft::Draft2019_09
        } else if uri.contains("2020-12") {
            SchemaDraft::Draft2020_12
        } else {
            SchemaDraft::Draft7 // Default fallback.
        }
    }
}

// ---------------------------------------------------------------------------
// Schema types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SchemaType {
    String,
    Number,
    Integer,
    Boolean,
    Null,
    Array,
    Object,
}

impl SchemaType {
    pub fn as_str(self) -> &'static str {
        match self {
            SchemaType::String => "string",
            SchemaType::Number => "number",
            SchemaType::Integer => "integer",
            SchemaType::Boolean => "boolean",
            SchemaType::Null => "null",
            SchemaType::Array => "array",
            SchemaType::Object => "object",
        }
    }
}

/// A schema or a boolean (`true` = accept all, `false` = reject all).
#[derive(Debug, Clone)]
pub enum SchemaOrBool {
    Schema(Arc<JsonSchema>),
    Bool(bool),
}

impl SchemaOrBool {
    pub fn as_schema(&self) -> Option<&Arc<JsonSchema>> {
        match self {
            SchemaOrBool::Schema(s) => Some(s),
            SchemaOrBool::Bool(_) => None,
        }
    }

    pub fn is_false(&self) -> bool {
        matches!(self, SchemaOrBool::Bool(false))
    }
}

/// A dependency: either required properties or a sub-schema.
#[derive(Debug, Clone)]
pub enum Dependency {
    Properties(Vec<String>),
    Schema(Arc<JsonSchema>),
}

/// VS Code snippet extension.
#[derive(Debug, Clone, Default)]
pub struct DefaultSnippet {
    pub label: Option<String>,
    pub description: Option<String>,
    pub body: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Main schema struct
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct JsonSchema {
    // -- Metadata --
    pub id: Option<String>,
    pub schema_draft: Option<String>,
    pub draft: SchemaDraft,
    pub title: Option<String>,
    pub description: Option<String>,
    pub markdown_description: Option<String>,
    pub default: Option<serde_json::Value>,
    pub examples: Vec<serde_json::Value>,
    pub deprecated: bool,
    pub deprecation_message: Option<String>,
    pub error_message: Option<String>,
    pub pattern_error_message: Option<String>,
    pub do_not_suggest: bool,
    pub enum_descriptions: Vec<String>,
    pub markdown_enum_descriptions: Vec<String>,

    // -- Type --
    pub types: Vec<SchemaType>,

    // -- Enum / const --
    pub enum_values: Vec<serde_json::Value>,
    pub const_value: Option<serde_json::Value>,

    // -- Numeric --
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub exclusive_minimum: Option<ExclusiveLimit>,
    pub exclusive_maximum: Option<ExclusiveLimit>,
    pub multiple_of: Option<f64>,

    // -- String --
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub pattern: Option<String>,
    pub format: Option<String>,

    // -- Array --
    pub items: Option<Box<SchemaOrBool>>,
    pub prefix_items: Vec<Arc<JsonSchema>>,
    pub additional_items: Option<Box<SchemaOrBool>>,
    pub min_items: Option<u64>,
    pub max_items: Option<u64>,
    pub unique_items: bool,
    pub contains: Option<Arc<JsonSchema>>,
    pub min_contains: Option<u64>,
    pub max_contains: Option<u64>,

    // -- Object --
    pub properties: HashMap<String, Arc<JsonSchema>>,
    pub required: Vec<String>,
    pub additional_properties: Option<Box<SchemaOrBool>>,
    pub pattern_properties: Vec<(String, Arc<JsonSchema>)>,
    pub property_names: Option<Arc<JsonSchema>>,
    pub min_properties: Option<u64>,
    pub max_properties: Option<u64>,
    pub dependencies: HashMap<String, Dependency>,
    pub dependent_required: HashMap<String, Vec<String>>,
    pub dependent_schemas: HashMap<String, Arc<JsonSchema>>,

    // -- Composition --
    pub all_of: Vec<Arc<JsonSchema>>,
    pub any_of: Vec<Arc<JsonSchema>>,
    pub one_of: Vec<Arc<JsonSchema>>,
    pub not: Option<Arc<JsonSchema>>,

    // -- Conditional --
    pub if_schema: Option<Arc<JsonSchema>>,
    pub then_schema: Option<Arc<JsonSchema>>,
    pub else_schema: Option<Arc<JsonSchema>>,

    // -- References --
    pub reference: Option<String>,

    // -- Definitions --
    pub definitions: HashMap<String, Arc<JsonSchema>>,
    pub defs: HashMap<String, Arc<JsonSchema>>,

    // -- VS Code extensions --
    pub default_snippets: Vec<DefaultSnippet>,
}

/// Draft 4 uses boolean exclusiveMinimum/Maximum, draft 6+ uses numbers.
#[derive(Debug, Clone)]
pub enum ExclusiveLimit {
    /// Draft 4: boolean flag (true means the minimum/maximum itself is exclusive).
    Bool(bool),
    /// Draft 6+: the exclusive boundary value.
    Number(f64),
}

// ---------------------------------------------------------------------------
// Schema path resolution
// ---------------------------------------------------------------------------

impl JsonSchema {
    /// Resolve a single path segment (property name or array index) against this
    /// schema. Walks into allOf/anyOf/oneOf and if/then/else to find the sub-schema.
    /// Used by both completion and hover to navigate from root schema to cursor position.
    pub fn resolve_path_segment(self: &Arc<Self>, seg: &str) -> Option<Arc<JsonSchema>> {
        // Direct property.
        if let Some(prop) = self.properties.get(seg) {
            return Some(prop.clone());
        }

        // Array index.
        if let Ok(idx) = seg.parse::<usize>() {
            if let Some(ps) = self.prefix_items.get(idx) {
                return Some(ps.clone());
            }
            if let Some(ref items) = self.items
                && let Some(s) = items.as_schema()
            {
                return Some(s.clone());
            }
        }

        // Walk into composition schemas.
        for sub in self
            .all_of
            .iter()
            .chain(self.any_of.iter())
            .chain(self.one_of.iter())
        {
            if let Some(result) = sub.resolve_path_segment(seg) {
                return Some(result);
            }
        }

        // if/then/else
        if let Some(ref then_schema) = self.then_schema
            && let Some(result) = then_schema.resolve_path_segment(seg)
        {
            return Some(result);
        }
        if let Some(ref else_schema) = self.else_schema
            && let Some(result) = else_schema.resolve_path_segment(seg)
        {
            return Some(result);
        }

        // Additional properties.
        if let Some(ref ap) = self.additional_properties
            && let Some(s) = ap.as_schema()
        {
            return Some(s.clone());
        }

        None
    }
}

// ---------------------------------------------------------------------------
// Parsing from serde_json::Value
// ---------------------------------------------------------------------------

impl JsonSchema {
    pub fn from_value(val: &serde_json::Value) -> Arc<JsonSchema> {
        match val {
            serde_json::Value::Bool(true) => Arc::new(JsonSchema::default()),
            serde_json::Value::Bool(false) => {
                // "false" schema rejects everything — model as not:{}.
                Arc::new(JsonSchema {
                    not: Some(Arc::new(JsonSchema::default())),
                    ..Default::default()
                })
            }
            serde_json::Value::Object(map) => Arc::new(parse_schema_object(map)),
            _ => Arc::new(JsonSchema::default()),
        }
    }
}

fn parse_schema_object(map: &serde_json::Map<String, serde_json::Value>) -> JsonSchema {
    let mut s = JsonSchema::default();

    // -- Draft detection --
    if let Some(v) = str_field(map, "$schema") {
        s.draft = SchemaDraft::from_schema_uri(v);
        s.schema_draft = Some(v.to_string());
    }

    // -- Metadata --
    s.id = str_field(map, "$id")
        .or_else(|| str_field(map, "id"))
        .map(String::from);
    s.title = str_field(map, "title").map(String::from);
    s.description = str_field(map, "description").map(String::from);
    s.markdown_description = str_field(map, "markdownDescription").map(String::from);
    s.default = map.get("default").cloned();
    s.examples = map
        .get("examples")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    s.deprecated = map
        .get("deprecated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    s.deprecation_message = str_field(map, "deprecationMessage").map(String::from);
    s.error_message = str_field(map, "errorMessage").map(String::from);
    s.pattern_error_message = str_field(map, "patternErrorMessage").map(String::from);
    s.do_not_suggest = map
        .get("doNotSuggest")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    s.enum_descriptions = str_array_field(map, "enumDescriptions");
    s.markdown_enum_descriptions = str_array_field(map, "markdownEnumDescriptions");

    // -- type --
    match map.get("type") {
        Some(serde_json::Value::String(t)) => {
            if let Some(st) = parse_schema_type(t) {
                s.types = vec![st];
            }
        }
        Some(serde_json::Value::Array(arr)) => {
            s.types = arr
                .iter()
                .filter_map(|v| v.as_str().and_then(parse_schema_type))
                .collect();
        }
        _ => {}
    }

    // -- enum / const --
    s.enum_values = map
        .get("enum")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    s.const_value = map.get("const").cloned();

    // -- numeric --
    s.minimum = map.get("minimum").and_then(|v| v.as_f64());
    s.maximum = map.get("maximum").and_then(|v| v.as_f64());
    s.exclusive_minimum = map.get("exclusiveMinimum").map(|v| {
        if let Some(b) = v.as_bool() {
            ExclusiveLimit::Bool(b)
        } else {
            ExclusiveLimit::Number(v.as_f64().unwrap_or(0.0))
        }
    });
    s.exclusive_maximum = map.get("exclusiveMaximum").map(|v| {
        if let Some(b) = v.as_bool() {
            ExclusiveLimit::Bool(b)
        } else {
            ExclusiveLimit::Number(v.as_f64().unwrap_or(0.0))
        }
    });
    s.multiple_of = map.get("multipleOf").and_then(|v| v.as_f64());

    // -- string --
    s.min_length = map.get("minLength").and_then(|v| v.as_u64());
    s.max_length = map.get("maxLength").and_then(|v| v.as_u64());
    s.pattern = str_field(map, "pattern").map(String::from);
    s.format = str_field(map, "format").map(String::from);

    // -- array --
    s.items = map.get("items").map(|v| Box::new(parse_schema_or_bool(v)));
    s.prefix_items = map
        .get("prefixItems")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(JsonSchema::from_value).collect())
        .unwrap_or_default();
    s.additional_items = map
        .get("additionalItems")
        .map(|v| Box::new(parse_schema_or_bool(v)));
    s.min_items = map.get("minItems").and_then(|v| v.as_u64());
    s.max_items = map.get("maxItems").and_then(|v| v.as_u64());
    s.unique_items = map
        .get("uniqueItems")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    s.contains = map.get("contains").map(JsonSchema::from_value);
    s.min_contains = map.get("minContains").and_then(|v| v.as_u64());
    s.max_contains = map.get("maxContains").and_then(|v| v.as_u64());

    // -- object --
    s.properties = map
        .get("properties")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), JsonSchema::from_value(v)))
                .collect()
        })
        .unwrap_or_default();
    s.required = str_array_field(map, "required");
    s.additional_properties = map
        .get("additionalProperties")
        .map(|v| Box::new(parse_schema_or_bool(v)));
    s.pattern_properties = map
        .get("patternProperties")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), JsonSchema::from_value(v)))
                .collect()
        })
        .unwrap_or_default();
    s.property_names = map.get("propertyNames").map(JsonSchema::from_value);
    s.min_properties = map.get("minProperties").and_then(|v| v.as_u64());
    s.max_properties = map.get("maxProperties").and_then(|v| v.as_u64());

    // -- dependencies --
    if let Some(obj) = map.get("dependencies").and_then(|v| v.as_object()) {
        for (k, dep) in obj {
            let d = if let Some(arr) = dep.as_array() {
                Dependency::Properties(
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                )
            } else {
                Dependency::Schema(JsonSchema::from_value(dep))
            };
            s.dependencies.insert(k.clone(), d);
        }
    }
    if let Some(obj) = map.get("dependentRequired").and_then(|v| v.as_object()) {
        for (k, arr) in obj {
            if let Some(arr) = arr.as_array() {
                let props = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
                s.dependent_required.insert(k.clone(), props);
            }
        }
    }
    if let Some(obj) = map.get("dependentSchemas").and_then(|v| v.as_object()) {
        s.dependent_schemas = obj
            .iter()
            .map(|(k, v)| (k.clone(), JsonSchema::from_value(v)))
            .collect();
    }

    // -- composition --
    s.all_of = schema_array_field(map, "allOf");
    s.any_of = schema_array_field(map, "anyOf");
    s.one_of = schema_array_field(map, "oneOf");
    s.not = map.get("not").map(JsonSchema::from_value);

    // -- conditional --
    s.if_schema = map.get("if").map(JsonSchema::from_value);
    s.then_schema = map.get("then").map(JsonSchema::from_value);
    s.else_schema = map.get("else").map(JsonSchema::from_value);

    // -- $ref --
    s.reference = str_field(map, "$ref").map(String::from);

    // -- definitions --
    s.definitions = schema_object_field(map, "definitions");
    s.defs = schema_object_field(map, "$defs");

    // -- VS Code extensions --
    s.default_snippets = map
        .get("defaultSnippets")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_object())
                .map(|obj| DefaultSnippet {
                    label: obj.get("label").and_then(|v| v.as_str()).map(String::from),
                    description: obj
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    body: obj.get("body").cloned(),
                })
                .collect()
        })
        .unwrap_or_default();
    s
}

// -- Helpers --

fn str_field<'a>(
    map: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    map.get(key).and_then(|v| v.as_str())
}

fn str_array_field(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> Vec<String> {
    map.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn schema_array_field(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Vec<Arc<JsonSchema>> {
    map.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(JsonSchema::from_value).collect())
        .unwrap_or_default()
}

fn schema_object_field(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> HashMap<String, Arc<JsonSchema>> {
    map.get(key)
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), JsonSchema::from_value(v)))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_schema_or_bool(val: &serde_json::Value) -> SchemaOrBool {
    match val {
        serde_json::Value::Bool(b) => SchemaOrBool::Bool(*b),
        _ => SchemaOrBool::Schema(JsonSchema::from_value(val)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_detection() {
        assert_eq!(
            SchemaDraft::from_schema_uri("http://json-schema.org/draft-04/schema#"),
            SchemaDraft::Draft4
        );
        assert_eq!(
            SchemaDraft::from_schema_uri("http://json-schema.org/draft-06/schema#"),
            SchemaDraft::Draft6
        );
        assert_eq!(
            SchemaDraft::from_schema_uri("http://json-schema.org/draft-07/schema#"),
            SchemaDraft::Draft7
        );
        assert_eq!(
            SchemaDraft::from_schema_uri("https://json-schema.org/draft/2019-09/schema"),
            SchemaDraft::Draft2019_09
        );
        assert_eq!(
            SchemaDraft::from_schema_uri("https://json-schema.org/draft/2020-12/schema"),
            SchemaDraft::Draft2020_12
        );
        assert_eq!(SchemaDraft::from_schema_uri("unknown"), SchemaDraft::Draft7);
    }

    #[test]
    fn parse_empty_schema() {
        let val: serde_json::Value = serde_json::from_str("{}").unwrap();
        let schema = JsonSchema::from_value(&val);
        assert!(schema.properties.is_empty());
        assert!(schema.types.is_empty());
    }

    #[test]
    fn parse_boolean_schemas() {
        let true_schema = JsonSchema::from_value(&serde_json::Value::Bool(true));
        assert!(true_schema.not.is_none()); // Accepts everything.

        let false_schema = JsonSchema::from_value(&serde_json::Value::Bool(false));
        assert!(false_schema.not.is_some()); // Rejects everything.
    }

    #[test]
    fn parse_type_string() {
        let val: serde_json::Value = serde_json::from_str(r#"{"type": "string"}"#).unwrap();
        let schema = JsonSchema::from_value(&val);
        assert_eq!(schema.types, vec![SchemaType::String]);
    }

    #[test]
    fn parse_type_array() {
        let val: serde_json::Value =
            serde_json::from_str(r#"{"type": ["string", "number"]}"#).unwrap();
        let schema = JsonSchema::from_value(&val);
        assert_eq!(schema.types.len(), 2);
    }

    #[test]
    fn parse_properties() {
        let val: serde_json::Value = serde_json::from_str(
            r#"{"properties": {"name": {"type": "string"}, "age": {"type": "integer"}}}"#,
        )
        .unwrap();
        let schema = JsonSchema::from_value(&val);
        assert_eq!(schema.properties.len(), 2);
        assert!(schema.properties.contains_key("name"));
        assert!(schema.properties.contains_key("age"));
    }

    #[test]
    fn parse_required() {
        let val: serde_json::Value =
            serde_json::from_str(r#"{"required": ["name", "email"]}"#).unwrap();
        let schema = JsonSchema::from_value(&val);
        assert_eq!(schema.required, vec!["name", "email"]);
    }

    #[test]
    fn parse_enum() {
        let val: serde_json::Value =
            serde_json::from_str(r#"{"enum": ["red", "green", "blue"]}"#).unwrap();
        let schema = JsonSchema::from_value(&val);
        assert_eq!(schema.enum_values.len(), 3);
    }

    #[test]
    fn parse_const() {
        let val: serde_json::Value = serde_json::from_str(r#"{"const": 42}"#).unwrap();
        let schema = JsonSchema::from_value(&val);
        assert_eq!(schema.const_value, Some(serde_json::json!(42)));
    }

    #[test]
    fn parse_numeric_constraints() {
        let val: serde_json::Value =
            serde_json::from_str(r#"{"minimum": 0, "maximum": 100, "multipleOf": 5}"#).unwrap();
        let schema = JsonSchema::from_value(&val);
        assert_eq!(schema.minimum, Some(0.0));
        assert_eq!(schema.maximum, Some(100.0));
        assert_eq!(schema.multiple_of, Some(5.0));
    }

    #[test]
    fn parse_exclusive_limit_draft4_bool() {
        let val: serde_json::Value =
            serde_json::from_str(r#"{"exclusiveMinimum": true, "exclusiveMaximum": false}"#)
                .unwrap();
        let schema = JsonSchema::from_value(&val);
        assert!(matches!(
            schema.exclusive_minimum,
            Some(ExclusiveLimit::Bool(true))
        ));
        assert!(matches!(
            schema.exclusive_maximum,
            Some(ExclusiveLimit::Bool(false))
        ));
    }

    #[test]
    fn parse_exclusive_limit_draft6_number() {
        let val: serde_json::Value =
            serde_json::from_str(r#"{"exclusiveMinimum": 0, "exclusiveMaximum": 100}"#).unwrap();
        let schema = JsonSchema::from_value(&val);
        assert!(matches!(
            schema.exclusive_minimum,
            Some(ExclusiveLimit::Number(n)) if n == 0.0
        ));
        assert!(matches!(
            schema.exclusive_maximum,
            Some(ExclusiveLimit::Number(n)) if n == 100.0
        ));
    }

    #[test]
    fn parse_string_constraints() {
        let val: serde_json::Value = serde_json::from_str(
            r#"{"minLength": 1, "maxLength": 255, "pattern": "^[a-z]+$", "format": "email"}"#,
        )
        .unwrap();
        let schema = JsonSchema::from_value(&val);
        assert_eq!(schema.min_length, Some(1));
        assert_eq!(schema.max_length, Some(255));
        assert_eq!(schema.pattern.as_deref(), Some("^[a-z]+$"));
        assert_eq!(schema.format.as_deref(), Some("email"));
    }

    #[test]
    fn parse_composition() {
        let val: serde_json::Value = serde_json::from_str(
            r#"{"allOf": [{"type": "object"}], "anyOf": [{"type": "string"}, {"type": "number"}], "oneOf": [{}]}"#,
        )
        .unwrap();
        let schema = JsonSchema::from_value(&val);
        assert_eq!(schema.all_of.len(), 1);
        assert_eq!(schema.any_of.len(), 2);
        assert_eq!(schema.one_of.len(), 1);
    }

    #[test]
    fn parse_definitions() {
        let val: serde_json::Value = serde_json::from_str(
            r#"{"definitions": {"Foo": {"type": "string"}}, "$defs": {"Bar": {"type": "number"}}}"#,
        )
        .unwrap();
        let schema = JsonSchema::from_value(&val);
        assert!(schema.definitions.contains_key("Foo"));
        assert!(schema.defs.contains_key("Bar"));
    }

    #[test]
    fn parse_vscode_extensions() {
        let val: serde_json::Value = serde_json::from_str(
            r##"{"markdownDescription": "# Title", "doNotSuggest": true, "deprecationMessage": "Use X instead"}"##,
        )
        .unwrap();
        let schema = JsonSchema::from_value(&val);
        assert_eq!(schema.markdown_description.as_deref(), Some("# Title"));
        assert!(schema.do_not_suggest);
        assert_eq!(schema.deprecation_message.as_deref(), Some("Use X instead"));
    }

    #[test]
    fn parse_dependencies() {
        let val: serde_json::Value =
            serde_json::from_str(r#"{"dependencies": {"a": ["b", "c"], "d": {"type": "object"}}}"#)
                .unwrap();
        let schema = JsonSchema::from_value(&val);
        assert_eq!(schema.dependencies.len(), 2);
        assert!(matches!(
            schema.dependencies.get("a"),
            Some(Dependency::Properties(p)) if p.len() == 2
        ));
        assert!(matches!(
            schema.dependencies.get("d"),
            Some(Dependency::Schema(_))
        ));
    }

    #[test]
    fn parse_conditional() {
        let val: serde_json::Value = serde_json::from_str(
            r#"{"if": {"type": "string"}, "then": {"minLength": 1}, "else": {"type": "number"}}"#,
        )
        .unwrap();
        let schema = JsonSchema::from_value(&val);
        assert!(schema.if_schema.is_some());
        assert!(schema.then_schema.is_some());
        assert!(schema.else_schema.is_some());
    }

    #[test]
    fn schema_or_bool() {
        let s = SchemaOrBool::Bool(false);
        assert!(s.is_false());
        assert!(s.as_schema().is_none());

        let s = SchemaOrBool::Schema(Arc::new(JsonSchema::default()));
        assert!(!s.is_false());
        assert!(s.as_schema().is_some());
    }

    #[test]
    fn parse_array_constraints() {
        let val: serde_json::Value = serde_json::from_str(
            r#"{"items": {"type": "string"}, "minItems": 1, "maxItems": 10, "uniqueItems": true}"#,
        )
        .unwrap();
        let schema = JsonSchema::from_value(&val);
        assert!(schema.items.is_some());
        assert_eq!(schema.min_items, Some(1));
        assert_eq!(schema.max_items, Some(10));
        assert!(schema.unique_items);
    }

    #[test]
    fn parse_prefix_items() {
        let val: serde_json::Value =
            serde_json::from_str(r#"{"prefixItems": [{"type": "string"}, {"type": "number"}]}"#)
                .unwrap();
        let schema = JsonSchema::from_value(&val);
        assert_eq!(schema.prefix_items.len(), 2);
    }

    #[test]
    fn parse_ref() {
        let val: serde_json::Value =
            serde_json::from_str(r##"{"$ref": "#/definitions/Foo"}"##).unwrap();
        let schema = JsonSchema::from_value(&val);
        assert_eq!(schema.reference.as_deref(), Some("#/definitions/Foo"));
    }

    #[test]
    fn parse_default_snippets() {
        let val: serde_json::Value =
            serde_json::from_str(r#"{"defaultSnippets": [{"label": "empty", "body": {}}]}"#)
                .unwrap();
        let schema = JsonSchema::from_value(&val);
        assert_eq!(schema.default_snippets.len(), 1);
        assert_eq!(schema.default_snippets[0].label.as_deref(), Some("empty"));
    }
}

fn parse_schema_type(s: &str) -> Option<SchemaType> {
    match s {
        "string" => Some(SchemaType::String),
        "number" => Some(SchemaType::Number),
        "integer" => Some(SchemaType::Integer),
        "boolean" => Some(SchemaType::Boolean),
        "null" => Some(SchemaType::Null),
        "array" => Some(SchemaType::Array),
        "object" => Some(SchemaType::Object),
        _ => None,
    }
}
