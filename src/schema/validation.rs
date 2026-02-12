/// Schema validation engine: validates tree-sitter nodes against a compiled
/// JsonSchema, producing validation errors with byte-offset locations.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use regex::Regex;
use tree_sitter::Node;

/// Server-wide regex cache to avoid recompiling the same pattern repeatedly
/// across validation passes. Persists for the lifetime of the server.
pub struct RegexCache {
    cache: HashMap<String, Option<Regex>>,
}

impl RegexCache {
    pub fn new() -> Self {
        RegexCache {
            cache: HashMap::new(),
        }
    }

    pub fn get(&mut self, pattern: &str) -> Option<&Regex> {
        self.cache
            .entry(pattern.to_string())
            .or_insert_with(|| Regex::new(pattern).ok())
            .as_ref()
    }
}

use super::types::*;
use crate::tree::{self, kinds};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
pub struct ValidationError {
    pub start_byte: usize,
    pub end_byte: usize,
    pub message: String,
    pub severity: Severity,
}

/// Validate a tree-sitter node against a schema.
pub fn validate<'a>(
    node: Node<'a>,
    source: &[u8],
    schema: &Arc<JsonSchema>,
    regex_cache: &mut RegexCache,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let ref_chain: HashSet<String> = HashSet::new();
    validate_node(node, source, schema, &mut errors, &ref_chain, regex_cache);
    warn_unknown_properties(node, source, schema, &mut errors, regex_cache);
    errors
}

fn validate_node(
    node: Node<'_>,
    source: &[u8],
    schema: &Arc<JsonSchema>,
    errors: &mut Vec<ValidationError>,
    ref_chain: &HashSet<String>,
    regex_cache: &mut RegexCache,
) {
    // Handle $ref: only clone the chain when we actually encounter a $ref.
    if let Some(ref reference) = schema.reference {
        if ref_chain.contains(reference) {
            return;
        }
        let mut chain = ref_chain.clone();
        chain.insert(reference.clone());
        validate_node_inner(node, source, schema, errors, &chain, regex_cache);
        return;
    }
    validate_node_inner(node, source, schema, errors, ref_chain, regex_cache);
}

fn validate_node_inner(
    node: Node<'_>,
    source: &[u8],
    schema: &Arc<JsonSchema>,
    errors: &mut Vec<ValidationError>,
    ref_chain: &HashSet<String>,
    regex_cache: &mut RegexCache,
) {
    // Deprecation warning.
    if schema.deprecated {
        let msg = schema
            .deprecation_message
            .as_deref()
            .unwrap_or("This property is deprecated.");
        errors.push(ValidationError {
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
            message: msg.to_string(),
            severity: Severity::Warning,
        });
    }

    // -- type --
    if !schema.types.is_empty() {
        let node_type = node_to_schema_type(node, source);
        // Integer is a subtype of Number in JSON Schema, so accept integers
        // when the schema expects "number".
        let matches = schema.types.iter().any(|t| {
            *t == node_type || (*t == SchemaType::Number && node_type == SchemaType::Integer)
        });
        if !matches {
            let expected: Vec<&str> = schema.types.iter().map(|t| t.as_str()).collect();
            errors.push(err(
                node,
                format!("Incorrect type. Expected {}.", expected.join(" | ")),
            ));
            return;
        }
    }

    // -- enum --
    if !schema.enum_values.is_empty() {
        let node_val = node_to_json_value(node, source);
        if !schema.enum_values.iter().any(|e| e == &node_val) {
            let allowed: Vec<String> = schema.enum_values.iter().map(|v| format!("{v}")).collect();
            errors.push(err(
                node,
                format!(
                    "Value is not accepted. Valid values: {}.",
                    allowed.join(", ")
                ),
            ));
        }
    }

    // -- const --
    if let Some(ref const_val) = schema.const_value {
        let node_val = node_to_json_value(node, source);
        if const_val != &node_val {
            errors.push(err(node, format!("Value must be {const_val}.")));
        }
    }

    // -- Kind-specific --
    match node.kind() {
        kinds::STRING => validate_string(node, source, schema, errors, regex_cache),
        kinds::NUMBER => validate_number(node, source, schema, errors),
        kinds::OBJECT => validate_object(node, source, schema, errors, ref_chain, regex_cache),
        kinds::ARRAY => validate_array(node, source, schema, errors, ref_chain, regex_cache),
        _ => {}
    }

    // -- Composition --
    validate_composition(node, source, schema, errors, ref_chain, regex_cache);

    // -- Conditional --
    validate_conditional(node, source, schema, errors, ref_chain, regex_cache);
}

// ---------------------------------------------------------------------------
// String
// ---------------------------------------------------------------------------

fn validate_string(
    node: Node<'_>,
    source: &[u8],
    schema: &JsonSchema,
    errors: &mut Vec<ValidationError>,
    regex_cache: &mut RegexCache,
) {
    let val = match tree::string_value(node, source) {
        Some(s) => s,
        None => return,
    };

    if let Some(min) = schema.min_length
        && (val.chars().count() as u64) < min
    {
        errors.push(err(
            node,
            format!("String is shorter than the minimum length of {min}."),
        ));
    }
    if let Some(max) = schema.max_length
        && (val.chars().count() as u64) > max
    {
        errors.push(err(
            node,
            format!("String is longer than the maximum length of {max}."),
        ));
    }
    if let Some(ref pat) = schema.pattern
        && let Some(re) = regex_cache.get(pat)
        && !re.is_match(&val)
    {
        let msg = schema
            .pattern_error_message
            .clone()
            .unwrap_or_else(|| format!("String does not match the pattern '{pat}'."));
        errors.push(err(node, msg));
    }
    if let Some(ref fmt) = schema.format
        && !validate_format(&val, fmt)
    {
        errors.push(ValidationError {
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
            message: format!("String is not a valid '{fmt}'."),
            severity: Severity::Warning,
        });
    }
}

fn validate_format(val: &str, format: &str) -> bool {
    match format {
        "date-time" => {
            // RFC 3339: YYYY-MM-DDTHH:MM:SS with optional fractional seconds and timezone.
            val.len() >= 19
                && val.as_bytes().get(4) == Some(&b'-')
                && val.as_bytes().get(7) == Some(&b'-')
                && (val.as_bytes().get(10) == Some(&b'T') || val.as_bytes().get(10) == Some(&b't'))
                && val.as_bytes().get(13) == Some(&b':')
        }
        "date" => {
            // YYYY-MM-DD
            val.len() == 10
                && val.as_bytes().get(4) == Some(&b'-')
                && val.as_bytes().get(7) == Some(&b'-')
                && val[..4].bytes().all(|b| b.is_ascii_digit())
                && val[5..7].bytes().all(|b| b.is_ascii_digit())
                && val[8..10].bytes().all(|b| b.is_ascii_digit())
        }
        "time" => {
            // HH:MM:SS with optional fractional seconds and timezone.
            val.len() >= 8
                && val.as_bytes().get(2) == Some(&b':')
                && val.as_bytes().get(5) == Some(&b':')
                && val[..2].bytes().all(|b| b.is_ascii_digit())
        }
        "email" => {
            let parts: Vec<&str> = val.splitn(2, '@').collect();
            parts.len() == 2
                && !parts[0].is_empty()
                && parts[1].contains('.')
                && !parts[1].starts_with('.')
                && !parts[1].ends_with('.')
        }
        "hostname" => {
            !val.is_empty()
                && val.len() <= 253
                && val.split('.').all(|label| {
                    !label.is_empty()
                        && label.len() <= 63
                        && label
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                        && !label.starts_with('-')
                        && !label.ends_with('-')
                })
        }
        "ipv4" => {
            let parts: Vec<&str> = val.split('.').collect();
            parts.len() == 4
                && parts.iter().all(|p| {
                    !p.is_empty()
                        && (p.len() == 1 || !p.starts_with('0'))
                        && p.parse::<u16>().is_ok_and(|n| n <= 255)
                })
        }
        "ipv6" => {
            // Basic check: contains colons, right number of groups or uses :: shorthand.
            val.contains(':')
                && val.len() >= 2
                && val
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.')
        }
        "uri" => val.contains("://") || val.starts_with("urn:"),
        "uri-reference" => {
            val.contains("://")
                || val.starts_with('/')
                || val.starts_with('#')
                || val.starts_with("urn:")
        }
        "color-hex" => {
            val.starts_with('#')
                && matches!(val.len(), 4 | 5 | 7 | 9)
                && val[1..].bytes().all(|b| b.is_ascii_hexdigit())
        }
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// Number
// ---------------------------------------------------------------------------

fn validate_number(
    node: Node<'_>,
    source: &[u8],
    schema: &JsonSchema,
    errors: &mut Vec<ValidationError>,
) {
    let text = match node.utf8_text(source) {
        Ok(t) => t,
        Err(_) => return,
    };
    let val: f64 = match text.parse() {
        Ok(n) => n,
        Err(_) => return,
    };

    if let Some(min) = schema.minimum {
        let exclusive = matches!(schema.exclusive_minimum, Some(ExclusiveLimit::Bool(true)));
        if exclusive {
            if val <= min {
                errors.push(err(node, format!("Value must be greater than {min}.")));
            }
        } else if val < min {
            errors.push(err(node, format!("Value is below the minimum of {min}.")));
        }
    }
    if let Some(max) = schema.maximum {
        let exclusive = matches!(schema.exclusive_maximum, Some(ExclusiveLimit::Bool(true)));
        if exclusive {
            if val >= max {
                errors.push(err(node, format!("Value must be less than {max}.")));
            }
        } else if val > max {
            errors.push(err(node, format!("Value is above the maximum of {max}.")));
        }
    }
    if let Some(ExclusiveLimit::Number(exc_min)) = schema.exclusive_minimum
        && val <= exc_min
    {
        errors.push(err(node, format!("Value must be greater than {exc_min}.")));
    }
    if let Some(ExclusiveLimit::Number(exc_max)) = schema.exclusive_maximum
        && val >= exc_max
    {
        errors.push(err(node, format!("Value must be less than {exc_max}.")));
    }
    // multipleOf: use round-trip multiply instead of fract() to avoid
    // floating-point issues (e.g. 0.3 / 0.1 != exactly 3.0).
    if let Some(mult) = schema.multiple_of
        && mult != 0.0
    {
        let remainder = (val / mult).round() * mult;
        if (val - remainder).abs() > 1e-10 {
            errors.push(err(node, format!("Value is not a multiple of {mult}.")));
        }
    }
    if schema.types.contains(&SchemaType::Integer)
        && !schema.types.contains(&SchemaType::Number)
        && val.fract() != 0.0
    {
        errors.push(err(node, "Value must be an integer.".into()));
    }
}

/// Check whether a property name is known anywhere in the schema tree,
/// including composition sub-schemas (allOf/anyOf/oneOf) and conditionals.
fn schema_knows_property(schema: &JsonSchema, key: &str, regex_cache: &mut RegexCache) -> bool {
    if schema.properties.contains_key(key) {
        return true;
    }
    for (pattern, _) in &schema.pattern_properties {
        if let Some(re) = regex_cache.get(pattern) {
            if re.is_match(key) {
                return true;
            }
        }
    }
    if schema.additional_properties.is_some() {
        return true;
    }
    for sub in schema
        .all_of
        .iter()
        .chain(schema.any_of.iter())
        .chain(schema.one_of.iter())
    {
        if schema_knows_property(sub, key, regex_cache) {
            return true;
        }
    }
    if let Some(ref then_schema) = schema.then_schema {
        if schema_knows_property(then_schema, key, regex_cache) {
            return true;
        }
    }
    if let Some(ref else_schema) = schema.else_schema {
        if schema_knows_property(else_schema, key, regex_cache) {
            return true;
        }
    }
    false
}

/// Check whether a schema (or any of its composition sub-schemas) defines
/// any known properties, meaning we have enough information to warn about
/// unknown ones.
fn schema_has_known_properties(schema: &JsonSchema) -> bool {
    if !schema.properties.is_empty() || !schema.pattern_properties.is_empty() {
        return true;
    }
    for sub in schema
        .all_of
        .iter()
        .chain(schema.any_of.iter())
        .chain(schema.one_of.iter())
    {
        if schema_has_known_properties(sub) {
            return true;
        }
    }
    if let Some(ref then_schema) = schema.then_schema {
        if schema_has_known_properties(then_schema) {
            return true;
        }
    }
    if let Some(ref else_schema) = schema.else_schema {
        if schema_has_known_properties(else_schema) {
            return true;
        }
    }
    false
}

/// Walk the tree-sitter AST and emit warnings for object properties that are
/// not declared anywhere in the corresponding schema (including composition
/// and conditional sub-schemas). This runs as a separate pass after the main
/// validation so that the full schema context is available at each level.
fn warn_unknown_properties(
    node: Node<'_>,
    source: &[u8],
    schema: &Arc<JsonSchema>,
    errors: &mut Vec<ValidationError>,
    regex_cache: &mut RegexCache,
) {
    if node.kind() == kinds::OBJECT {
        // Only warn when the schema declares known properties and does not
        // already have an additionalProperties constraint (which is handled
        // by the main validation pass).
        if schema.additional_properties.is_none() && schema_has_known_properties(schema) {
            let mut cursor = node.walk();
            for pair in tree::object_pairs(node, &mut cursor) {
                let key_node = match pair.child_by_field_name("key") {
                    Some(k) => k,
                    None => continue,
                };
                let key_str = match tree::string_value(key_node, source) {
                    Some(k) => k,
                    None => continue,
                };
                if !schema_knows_property(schema, &key_str, regex_cache) {
                    errors.push(ValidationError {
                        start_byte: key_node.start_byte(),
                        end_byte: key_node.end_byte(),
                        message: format!("Unknown property \"{key_str}\"."),
                        severity: Severity::Warning,
                    });
                }
            }
        }

        // Recurse into child values with their resolved sub-schemas.
        let mut cursor = node.walk();
        for pair in tree::object_pairs(node, &mut cursor) {
            let key_str = match tree::pair_key_unescaped(pair, source) {
                Some(k) => k,
                None => continue,
            };
            let value_node = match tree::pair_value(pair) {
                Some(v) => v,
                None => continue,
            };
            if let Some(child_schema) = schema.resolve_path_segment(&key_str) {
                warn_unknown_properties(value_node, source, &child_schema, errors, regex_cache);
            }
        }
    } else if node.kind() == kinds::ARRAY {
        let mut cursor = node.walk();
        let items = tree::array_items(node, &mut cursor);
        for (i, item) in items.iter().enumerate() {
            // Try prefixItems first, then items.
            let child_schema = if let Some(ps) = schema.prefix_items.get(i) {
                Some(ps.clone())
            } else if let Some(ref items_schema) = schema.items {
                items_schema.as_schema().cloned()
            } else {
                None
            };
            if let Some(cs) = child_schema {
                warn_unknown_properties(*item, source, &cs, errors, regex_cache);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Object
// ---------------------------------------------------------------------------

fn validate_object(
    node: Node<'_>,
    source: &[u8],
    schema: &JsonSchema,
    errors: &mut Vec<ValidationError>,
    ref_chain: &HashSet<String>,
    regex_cache: &mut RegexCache,
) {
    let mut cursor = node.walk();
    let pairs = tree::object_pairs(node, &mut cursor);
    let prop_count = pairs.len() as u64;

    if let Some(min) = schema.min_properties
        && prop_count < min
    {
        errors.push(err(
            node,
            format!("Object has fewer than {min} properties."),
        ));
    }
    if let Some(max) = schema.max_properties
        && prop_count > max
    {
        errors.push(err(node, format!("Object has more than {max} properties.")));
    }

    // Collect present keys.
    let present_keys: HashSet<String> = pairs
        .iter()
        .filter_map(|p| tree::pair_key_unescaped(*p, source))
        .collect();

    // Required.
    for req in &schema.required {
        if !present_keys.contains(req) {
            errors.push(err(node, format!("Missing property \"{req}\".")));
        }
    }

    // Validate each pair.
    for pair in &pairs {
        let key_node = match pair.child_by_field_name("key") {
            Some(k) => k,
            None => continue,
        };
        let key_str = match tree::string_value(key_node, source) {
            Some(k) => k,
            None => continue,
        };
        let value_node = match tree::pair_value(*pair) {
            Some(v) => v,
            None => continue,
        };

        // Property names schema.
        if let Some(ref pn_schema) = schema.property_names {
            validate_node(key_node, source, pn_schema, errors, ref_chain, regex_cache);
        }

        let mut matched = false;

        // Named property.
        if let Some(prop_schema) = schema.properties.get(&key_str) {
            validate_node(
                value_node,
                source,
                prop_schema,
                errors,
                ref_chain,
                regex_cache,
            );
            matched = true;
        }

        // Pattern properties.
        for (pattern, pat_schema) in &schema.pattern_properties {
            if let Some(re) = regex_cache.get(pattern)
                && re.is_match(&key_str)
            {
                validate_node(
                    value_node,
                    source,
                    pat_schema,
                    errors,
                    ref_chain,
                    regex_cache,
                );
                matched = true;
            }
        }

        // Additional properties.
        if !matched {
            if let Some(ref ap) = schema.additional_properties {
                if ap.is_false() {
                    errors.push(err(
                        key_node,
                        format!("Property \"{key_str}\" is not allowed."),
                    ));
                } else if let Some(ap_schema) = ap.as_schema() {
                    validate_node(
                        value_node,
                        source,
                        ap_schema,
                        errors,
                        ref_chain,
                        regex_cache,
                    );
                }
            }
        }
    }

    // Dependencies.
    for (dep_key, dep) in &schema.dependencies {
        if present_keys.contains(dep_key) {
            match dep {
                Dependency::Properties(required) => {
                    for req in required {
                        if !present_keys.contains(req) {
                            errors.push(err(
                                node,
                                format!("Property \"{dep_key}\" requires \"{req}\"."),
                            ));
                        }
                    }
                }
                Dependency::Schema(dep_schema) => {
                    validate_node(node, source, dep_schema, errors, ref_chain, regex_cache);
                }
            }
        }
    }
    for (dep_key, required) in &schema.dependent_required {
        if present_keys.contains(dep_key) {
            for req in required {
                if !present_keys.contains(req) {
                    errors.push(err(
                        node,
                        format!("Property \"{dep_key}\" requires \"{req}\"."),
                    ));
                }
            }
        }
    }
    for (dep_key, dep_schema) in &schema.dependent_schemas {
        if present_keys.contains(dep_key) {
            validate_node(node, source, dep_schema, errors, ref_chain, regex_cache);
        }
    }
}

// ---------------------------------------------------------------------------
// Array
// ---------------------------------------------------------------------------

fn validate_array(
    node: Node<'_>,
    source: &[u8],
    schema: &JsonSchema,
    errors: &mut Vec<ValidationError>,
    ref_chain: &HashSet<String>,
    regex_cache: &mut RegexCache,
) {
    let mut cursor = node.walk();
    let items = tree::array_items(node, &mut cursor);
    let count = items.len() as u64;

    if let Some(min) = schema.min_items
        && count < min
    {
        errors.push(err(node, format!("Array has fewer than {min} items.")));
    }
    if let Some(max) = schema.max_items
        && count > max
    {
        errors.push(err(node, format!("Array has more than {max} items.")));
    }

    // Unique items — use serialized form as hash key for O(n) detection.
    if schema.unique_items && items.len() > 1 {
        let mut seen = HashSet::new();
        for item in &items {
            let val = node_to_json_value(*item, source);
            let key = serde_json::to_string(&val).unwrap_or_default();
            if !seen.insert(key) {
                errors.push(err(*item, "Duplicate array item.".into()));
            }
        }
    }

    // prefixItems (2020-12 tuple validation).
    let mut validated_up_to = 0;
    for (i, prefix_schema) in schema.prefix_items.iter().enumerate() {
        if let Some(item) = items.get(i) {
            validate_node(*item, source, prefix_schema, errors, ref_chain, regex_cache);
            validated_up_to = i + 1;
        }
    }

    // items.
    if let Some(ref items_schema) = schema.items {
        match items_schema.as_ref() {
            SchemaOrBool::Schema(s) => {
                for item in items.iter().skip(validated_up_to) {
                    validate_node(*item, source, s, errors, ref_chain, regex_cache);
                }
            }
            SchemaOrBool::Bool(false) => {
                for item in items.iter().skip(validated_up_to) {
                    errors.push(err(*item, "Additional items are not allowed.".into()));
                }
            }
            SchemaOrBool::Bool(true) => {}
        }
    }

    // contains.
    if let Some(ref contains_schema) = schema.contains {
        let mut match_count = 0u64;
        for item in &items {
            let mut temp = Vec::new();
            validate_node(
                *item,
                source,
                contains_schema,
                &mut temp,
                ref_chain,
                regex_cache,
            );
            if temp.is_empty() {
                match_count += 1;
            }
        }
        let min_contains = schema.min_contains.unwrap_or(1);
        if match_count < min_contains {
            errors.push(err(
                node,
                format!("Array must contain at least {min_contains} matching item(s)."),
            ));
        }
        if let Some(max_contains) = schema.max_contains
            && match_count > max_contains
        {
            errors.push(err(
                node,
                format!("Array must contain at most {max_contains} matching item(s)."),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Composition
// ---------------------------------------------------------------------------

fn validate_composition(
    node: Node<'_>,
    source: &[u8],
    schema: &JsonSchema,
    errors: &mut Vec<ValidationError>,
    ref_chain: &HashSet<String>,
    regex_cache: &mut RegexCache,
) {
    for sub in &schema.all_of {
        validate_node(node, source, sub, errors, ref_chain, regex_cache);
    }

    if !schema.any_of.is_empty() {
        let any_matches = schema.any_of.iter().any(|sub| {
            let mut temp = Vec::new();
            validate_node(node, source, sub, &mut temp, ref_chain, regex_cache);
            temp.is_empty()
        });
        if !any_matches {
            errors.push(err(
                node,
                "Matches none of the listed schemas (anyOf).".into(),
            ));
        }
    }

    if !schema.one_of.is_empty() {
        let match_count = schema
            .one_of
            .iter()
            .filter(|sub| {
                let mut temp = Vec::new();
                validate_node(node, source, sub, &mut temp, ref_chain, regex_cache);
                temp.is_empty()
            })
            .count();
        if match_count == 0 {
            errors.push(err(
                node,
                "Matches none of the listed schemas (oneOf).".into(),
            ));
        } else if match_count > 1 {
            errors.push(err(
                node,
                format!("Matches {match_count} schemas but should match exactly one (oneOf)."),
            ));
        }
    }

    if let Some(ref not_schema) = schema.not {
        let mut temp = Vec::new();
        validate_node(node, source, not_schema, &mut temp, ref_chain, regex_cache);
        if temp.is_empty() {
            errors.push(err(
                node,
                "Value matches a schema it should not (not).".into(),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Conditional
// ---------------------------------------------------------------------------

fn validate_conditional(
    node: Node<'_>,
    source: &[u8],
    schema: &JsonSchema,
    errors: &mut Vec<ValidationError>,
    ref_chain: &HashSet<String>,
    regex_cache: &mut RegexCache,
) {
    if let Some(ref if_schema) = schema.if_schema {
        let mut temp = Vec::new();
        validate_node(node, source, if_schema, &mut temp, ref_chain, regex_cache);
        if temp.is_empty() {
            if let Some(ref then_schema) = schema.then_schema {
                validate_node(node, source, then_schema, errors, ref_chain, regex_cache);
            }
        } else if let Some(ref else_schema) = schema.else_schema {
            validate_node(node, source, else_schema, errors, ref_chain, regex_cache);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn err(node: Node<'_>, message: String) -> ValidationError {
    ValidationError {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        message,
        severity: Severity::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::tree;

    fn validate_json(json: &str, schema_json: &str) -> Vec<ValidationError> {
        let doc = Document::new(json.into(), 0);
        let schema_val: serde_json::Value = serde_json::from_str(schema_json).unwrap();
        let schema = JsonSchema::from_value(&schema_val);
        let root = tree::root_value(&doc.tree).unwrap();
        let mut regex_cache = RegexCache::new();
        validate(root, doc.source(), &schema, &mut regex_cache)
    }

    // -- Type validation --

    #[test]
    fn type_string_valid() {
        let errors = validate_json(r#""hello""#, r#"{"type": "string"}"#);
        assert!(errors.is_empty());
    }

    #[test]
    fn type_string_invalid() {
        let errors = validate_json("42", r#"{"type": "string"}"#);
        assert!(!errors.is_empty());
        assert!(errors[0].message.contains("Incorrect type"));
    }

    #[test]
    fn type_number_valid() {
        let errors = validate_json("3.14", r#"{"type": "number"}"#);
        assert!(errors.is_empty());
    }

    #[test]
    fn type_integer_accepts_whole_number() {
        let errors = validate_json("42", r#"{"type": "integer"}"#);
        assert!(errors.is_empty());
    }

    #[test]
    fn type_integer_rejects_float() {
        let errors = validate_json("3.14", r#"{"type": "integer"}"#);
        assert!(!errors.is_empty());
    }

    #[test]
    fn type_boolean_valid() {
        let errors = validate_json("true", r#"{"type": "boolean"}"#);
        assert!(errors.is_empty());
    }

    #[test]
    fn type_null_valid() {
        let errors = validate_json("null", r#"{"type": "null"}"#);
        assert!(errors.is_empty());
    }

    #[test]
    fn type_array_valid() {
        let errors = validate_json("[1, 2]", r#"{"type": "array"}"#);
        assert!(errors.is_empty());
    }

    #[test]
    fn type_object_valid() {
        let errors = validate_json("{}", r#"{"type": "object"}"#);
        assert!(errors.is_empty());
    }

    #[test]
    fn type_union() {
        let errors = validate_json(r#""hi""#, r#"{"type": ["string", "number"]}"#);
        assert!(errors.is_empty());
        let errors = validate_json("42", r#"{"type": ["string", "number"]}"#);
        assert!(errors.is_empty());
        let errors = validate_json("true", r#"{"type": ["string", "number"]}"#);
        assert!(!errors.is_empty());
    }

    // -- Enum / Const --

    #[test]
    fn enum_valid() {
        let errors = validate_json(r#""red""#, r#"{"enum": ["red", "green", "blue"]}"#);
        assert!(errors.is_empty());
    }

    #[test]
    fn enum_invalid() {
        let errors = validate_json(r#""yellow""#, r#"{"enum": ["red", "green", "blue"]}"#);
        assert!(!errors.is_empty());
        assert!(errors[0].message.contains("not accepted"));
    }

    #[test]
    fn const_valid() {
        let errors = validate_json("42", r#"{"const": 42}"#);
        assert!(errors.is_empty());
    }

    #[test]
    fn const_invalid() {
        let errors = validate_json("43", r#"{"const": 42}"#);
        assert!(!errors.is_empty());
    }

    // -- String validation --

    #[test]
    fn string_min_length() {
        let errors = validate_json(r#""ab""#, r#"{"type": "string", "minLength": 3}"#);
        assert!(!errors.is_empty());
        assert!(errors[0].message.contains("shorter"));
    }

    #[test]
    fn string_max_length() {
        let errors = validate_json(r#""abcdef""#, r#"{"type": "string", "maxLength": 3}"#);
        assert!(!errors.is_empty());
        assert!(errors[0].message.contains("longer"));
    }

    #[test]
    fn string_pattern_valid() {
        let errors = validate_json(
            r#""abc123""#,
            r#"{"type": "string", "pattern": "^[a-z]+[0-9]+$"}"#,
        );
        assert!(errors.is_empty());
    }

    #[test]
    fn string_pattern_invalid() {
        let errors = validate_json(
            r#""123abc""#,
            r#"{"type": "string", "pattern": "^[a-z]+$"}"#,
        );
        assert!(!errors.is_empty());
    }

    // -- Number validation --

    #[test]
    fn number_minimum() {
        let errors = validate_json("5", r#"{"type": "number", "minimum": 10}"#);
        assert!(!errors.is_empty());
    }

    #[test]
    fn number_maximum() {
        let errors = validate_json("15", r#"{"type": "number", "maximum": 10}"#);
        assert!(!errors.is_empty());
    }

    #[test]
    fn number_exclusive_minimum_draft6() {
        let errors = validate_json("10", r#"{"type": "number", "exclusiveMinimum": 10}"#);
        assert!(!errors.is_empty());
        let errors = validate_json("11", r#"{"type": "number", "exclusiveMinimum": 10}"#);
        assert!(errors.is_empty());
    }

    #[test]
    fn number_exclusive_minimum_draft4() {
        let errors = validate_json(
            "10",
            r#"{"type": "number", "minimum": 10, "exclusiveMinimum": true}"#,
        );
        assert!(!errors.is_empty());
    }

    #[test]
    fn number_multiple_of() {
        let errors = validate_json("7", r#"{"type": "number", "multipleOf": 3}"#);
        assert!(!errors.is_empty());
        let errors = validate_json("9", r#"{"type": "number", "multipleOf": 3}"#);
        assert!(errors.is_empty());
    }

    #[test]
    fn number_multiple_of_float() {
        let errors = validate_json("0.3", r#"{"type": "number", "multipleOf": 0.1}"#);
        assert!(errors.is_empty()); // Should pass despite floating point.
    }

    // -- Object validation --

    #[test]
    fn object_required() {
        let errors = validate_json(
            r#"{"a": 1}"#,
            r#"{"type": "object", "required": ["a", "b"]}"#,
        );
        assert!(!errors.is_empty());
        assert!(errors.iter().any(|e| e.message.contains("\"b\"")));
    }

    #[test]
    fn object_additional_properties_false() {
        let errors = validate_json(
            r#"{"a": 1, "b": 2}"#,
            r#"{"type": "object", "properties": {"a": {}}, "additionalProperties": false}"#,
        );
        assert!(!errors.is_empty());
        assert!(errors.iter().any(|e| e.message.contains("\"b\"")));
    }

    #[test]
    fn object_min_max_properties() {
        let errors = validate_json(r#"{}"#, r#"{"type": "object", "minProperties": 1}"#);
        assert!(!errors.is_empty());
        let errors = validate_json(
            r#"{"a": 1, "b": 2}"#,
            r#"{"type": "object", "maxProperties": 1}"#,
        );
        assert!(!errors.is_empty());
    }

    #[test]
    fn object_property_validation() {
        let errors = validate_json(
            r#"{"name": 42}"#,
            r#"{"type": "object", "properties": {"name": {"type": "string"}}}"#,
        );
        assert!(!errors.is_empty());
        assert!(errors[0].message.contains("Incorrect type"));
    }

    // -- Array validation --

    #[test]
    fn array_min_items() {
        let errors = validate_json("[1]", r#"{"type": "array", "minItems": 2}"#);
        assert!(!errors.is_empty());
    }

    #[test]
    fn array_max_items() {
        let errors = validate_json("[1, 2, 3]", r#"{"type": "array", "maxItems": 2}"#);
        assert!(!errors.is_empty());
    }

    #[test]
    fn array_unique_items() {
        let errors = validate_json("[1, 2, 1]", r#"{"type": "array", "uniqueItems": true}"#);
        assert!(!errors.is_empty());
        assert!(errors.iter().any(|e| e.message.contains("Duplicate")));
    }

    #[test]
    fn array_items_schema() {
        let errors = validate_json(
            r#"[1, "two", 3]"#,
            r#"{"type": "array", "items": {"type": "number"}}"#,
        );
        assert!(!errors.is_empty());
    }

    // -- Composition --

    #[test]
    fn all_of() {
        let errors = validate_json(
            r#"{"a": 1}"#,
            r#"{"allOf": [{"required": ["a"]}, {"required": ["b"]}]}"#,
        );
        assert!(!errors.is_empty());
        assert!(errors.iter().any(|e| e.message.contains("\"b\"")));
    }

    #[test]
    fn any_of_valid() {
        let errors = validate_json(
            "42",
            r#"{"anyOf": [{"type": "string"}, {"type": "number"}]}"#,
        );
        assert!(errors.is_empty());
    }

    #[test]
    fn any_of_invalid() {
        let errors = validate_json(
            "true",
            r#"{"anyOf": [{"type": "string"}, {"type": "number"}]}"#,
        );
        assert!(!errors.is_empty());
        assert!(errors[0].message.contains("anyOf"));
    }

    #[test]
    fn one_of_valid() {
        let errors = validate_json(
            "42",
            r#"{"oneOf": [{"type": "string"}, {"type": "number"}]}"#,
        );
        assert!(errors.is_empty());
    }

    #[test]
    fn one_of_too_many_matches() {
        let errors = validate_json(
            "42",
            r#"{"oneOf": [{"type": "number"}, {"type": "integer"}]}"#,
        );
        assert!(!errors.is_empty());
        assert!(errors[0].message.contains("oneOf"));
    }

    #[test]
    fn not_schema() {
        let errors = validate_json("42", r#"{"not": {"type": "number"}}"#);
        assert!(!errors.is_empty());
        assert!(errors[0].message.contains("not"));
    }

    // -- Conditional --

    #[test]
    fn if_then_else() {
        let schema = r#"{
            "if": {"properties": {"type": {"const": "string"}}},
            "then": {"required": ["minLength"]},
            "else": {"required": ["minimum"]}
        }"#;
        let errors = validate_json(r#"{"type": "string"}"#, schema);
        assert!(!errors.is_empty());
        assert!(errors.iter().any(|e| e.message.contains("minLength")));
    }

    // -- Format validation --

    #[test]
    fn format_date_time_valid() {
        assert!(validate_format("2023-01-15T10:30:00Z", "date-time"));
    }

    #[test]
    fn format_date_time_invalid() {
        assert!(!validate_format("not a date", "date-time"));
    }

    #[test]
    fn format_date_valid() {
        assert!(validate_format("2023-01-15", "date"));
    }

    #[test]
    fn format_date_invalid() {
        assert!(!validate_format("2023/01/15", "date"));
    }

    #[test]
    fn format_email_valid() {
        assert!(validate_format("user@example.com", "email"));
    }

    #[test]
    fn format_email_invalid() {
        assert!(!validate_format("not-an-email", "email"));
        assert!(!validate_format("@example.com", "email"));
        assert!(!validate_format("user@", "email"));
    }

    #[test]
    fn format_ipv4_valid() {
        assert!(validate_format("192.168.1.1", "ipv4"));
    }

    #[test]
    fn format_ipv4_invalid() {
        assert!(!validate_format("256.1.1.1", "ipv4"));
        assert!(!validate_format("1.2.3", "ipv4"));
        assert!(!validate_format("01.2.3.4", "ipv4")); // Leading zeros.
    }

    #[test]
    fn format_uri_valid() {
        assert!(validate_format("https://example.com", "uri"));
        assert!(validate_format("urn:isbn:0451450523", "uri"));
    }

    #[test]
    fn format_uri_invalid() {
        assert!(!validate_format("not a uri", "uri"));
    }

    #[test]
    fn format_hostname_valid() {
        assert!(validate_format("example.com", "hostname"));
        assert!(validate_format("sub.example.com", "hostname"));
    }

    #[test]
    fn format_hostname_invalid() {
        assert!(!validate_format("-invalid.com", "hostname"));
        assert!(!validate_format("", "hostname"));
    }

    #[test]
    fn format_color_hex_valid() {
        assert!(validate_format("#fff", "color-hex"));
        assert!(validate_format("#ffffff", "color-hex"));
        assert!(validate_format("#ffffffFF", "color-hex"));
    }

    #[test]
    fn format_color_hex_invalid() {
        assert!(!validate_format("fff", "color-hex"));
        assert!(!validate_format("#ff", "color-hex"));
    }

    #[test]
    fn format_unknown_passes() {
        assert!(validate_format("anything", "x-custom"));
    }

    // -- Dependencies --

    #[test]
    fn dependencies_property() {
        let errors = validate_json(r#"{"a": 1}"#, r#"{"dependencies": {"a": ["b"]}}"#);
        assert!(!errors.is_empty());
        assert!(errors.iter().any(|e| e.message.contains("\"b\"")));
    }

    #[test]
    fn dependencies_not_triggered() {
        let errors = validate_json(r#"{"c": 1}"#, r#"{"dependencies": {"a": ["b"]}}"#);
        assert!(errors.is_empty());
    }

    // -- Deprecation --

    #[test]
    fn deprecated_property_warning() {
        let errors = validate_json(r#""old""#, r#"{"deprecated": true}"#);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].severity, Severity::Warning);
    }

    // -- Unknown property warnings --

    #[test]
    fn unknown_property_warning() {
        let errors = validate_json(
            r#"{"a": 1, "b": 2}"#,
            r#"{"type": "object", "properties": {"a": {}}}"#,
        );
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].severity, Severity::Warning);
        assert!(errors[0].message.contains("\"b\""));
    }

    #[test]
    fn unknown_property_no_warning_when_additional_properties_true() {
        let errors = validate_json(
            r#"{"a": 1, "b": 2}"#,
            r#"{"type": "object", "properties": {"a": {}}, "additionalProperties": true}"#,
        );
        assert!(errors.is_empty());
    }

    #[test]
    fn unknown_property_no_warning_when_no_properties_defined() {
        // Schema with no properties at all — no basis to warn.
        let errors = validate_json(r#"{"a": 1, "b": 2}"#, r#"{"type": "object"}"#);
        assert!(errors.is_empty());
    }

    #[test]
    fn unknown_property_allof_siblings() {
        // Property defined in a sibling allOf sub-schema should not warn.
        let errors = validate_json(
            r#"{"a": 1, "b": 2}"#,
            r#"{"allOf": [{"properties": {"a": {}}}, {"properties": {"b": {}}}]}"#,
        );
        assert!(errors.is_empty());
    }

    #[test]
    fn unknown_property_allof_unknown() {
        // Property not in any allOf sub-schema should warn.
        let errors = validate_json(
            r#"{"a": 1, "c": 3}"#,
            r#"{"allOf": [{"properties": {"a": {}}}, {"properties": {"b": {}}}]}"#,
        );
        let warnings: Vec<_> = errors
            .iter()
            .filter(|e| e.severity == Severity::Warning && e.message.contains("Unknown"))
            .collect();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("\"c\""));
    }

    #[test]
    fn unknown_property_nested_object() {
        let errors = validate_json(
            r#"{"nested": {"known": 1, "typo": 2}}"#,
            r#"{"type": "object", "properties": {"nested": {"type": "object", "properties": {"known": {}}}}}"#,
        );
        let warnings: Vec<_> = errors
            .iter()
            .filter(|e| e.severity == Severity::Warning && e.message.contains("Unknown"))
            .collect();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("\"typo\""));
    }

    #[test]
    fn unknown_property_pattern_properties_match() {
        // Property matching a patternProperty should not warn.
        let errors = validate_json(
            r#"{"x-custom": 1}"#,
            r#"{"type": "object", "properties": {"name": {}}, "patternProperties": {"^x-": {}}}"#,
        );
        assert!(errors.is_empty());
    }
}

fn node_to_schema_type(node: Node<'_>, source: &[u8]) -> SchemaType {
    match node.kind() {
        kinds::STRING => SchemaType::String,
        kinds::NUMBER => {
            if let Ok(text) = node.utf8_text(source)
                && let Ok(n) = text.parse::<f64>()
                && n.fract() == 0.0
            {
                return SchemaType::Integer;
            }
            SchemaType::Number
        }
        kinds::TRUE | kinds::FALSE => SchemaType::Boolean,
        kinds::NULL => SchemaType::Null,
        kinds::ARRAY => SchemaType::Array,
        kinds::OBJECT => SchemaType::Object,
        _ => SchemaType::Null,
    }
}

/// Convert a tree-sitter node to a serde_json::Value for comparison.
/// Only used for enum/const/uniqueItems checks.
fn node_to_json_value(node: Node<'_>, source: &[u8]) -> serde_json::Value {
    match node.kind() {
        kinds::STRING => {
            let val = tree::string_value(node, source).unwrap_or_default();
            serde_json::Value::String(val)
        }
        kinds::NUMBER => {
            let text = node.utf8_text(source).unwrap_or("0");
            if let Ok(n) = text.parse::<serde_json::Number>() {
                serde_json::Value::Number(n)
            } else {
                serde_json::Value::Null
            }
        }
        kinds::TRUE => serde_json::Value::Bool(true),
        kinds::FALSE => serde_json::Value::Bool(false),
        kinds::NULL => serde_json::Value::Null,
        kinds::OBJECT => {
            let mut map = serde_json::Map::new();
            let mut cursor = node.walk();
            for pair in tree::object_pairs(node, &mut cursor) {
                if let Some(key) = tree::pair_key_unescaped(pair, source)
                    && let Some(val_node) = tree::pair_value(pair)
                {
                    map.insert(key, node_to_json_value(val_node, source));
                }
            }
            serde_json::Value::Object(map)
        }
        kinds::ARRAY => {
            let mut cursor = node.walk();
            let items: Vec<serde_json::Value> = tree::array_items(node, &mut cursor)
                .iter()
                .map(|n| node_to_json_value(*n, source))
                .collect();
            serde_json::Value::Array(items)
        }
        _ => serde_json::Value::Null,
    }
}
