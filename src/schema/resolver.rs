/// Schema resolver: maps document URIs to schemas, fetches remote schemas,
/// resolves $ref, and caches results.
use std::num::NonZeroUsize;
use std::sync::Arc;

use globset::{Glob, GlobMatcher};
use lru::LruCache;
use tracing::{debug, warn};

use super::types::JsonSchema;
use crate::document::Document;
use crate::tree::{self, kinds};

/// A configured association between file patterns and a schema.
#[derive(Debug, Clone)]
pub struct SchemaAssociation {
    pub file_match: Vec<String>,
    pub uri: String,
    pub schema: Option<Arc<JsonSchema>>,
}

/// Maximum number of cached schemas before eviction.
const MAX_SCHEMA_CACHE: usize = 32;

/// Manages schema associations, fetching, and caching.
pub struct SchemaStore {
    associations: Vec<(Vec<GlobMatcher>, String, Option<Arc<JsonSchema>>)>,
    cache: LruCache<String, Arc<JsonSchema>>,
    http: Option<ureq::Agent>,
}

impl SchemaStore {
    pub fn new() -> Self {
        SchemaStore {
            associations: Vec::new(),
            cache: LruCache::new(NonZeroUsize::new(MAX_SCHEMA_CACHE).unwrap()),
            http: None,
        }
    }

    /// Get or lazily create the HTTP agent (cheap clone via Arc internals).
    pub fn http_agent(&mut self) -> ureq::Agent {
        self.http
            .get_or_insert_with(|| {
                ureq::Agent::config_builder()
                    .timeout_global(Some(std::time::Duration::from_secs(10)))
                    .build()
                    .new_agent()
            })
            .clone()
    }

    pub fn set_associations(&mut self, assocs: Vec<SchemaAssociation>) {
        self.associations.clear();
        for assoc in assocs {
            let matchers: Vec<GlobMatcher> = assoc
                .file_match
                .iter()
                .filter_map(|pat| {
                    Glob::new(pat)
                        .map(|g| g.compile_matcher())
                        .map_err(|e| warn!("invalid glob pattern '{}': {}", pat, e))
                        .ok()
                })
                .collect();
            self.associations.push((matchers, assoc.uri, assoc.schema));
        }
    }

    pub fn reset_schema(&mut self, uri: &str) {
        self.cache.pop(uri);
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    /// Determine the schema URI for a document (sync, no fetching).
    ///
    /// Returns either a cached/inline schema directly, or the URI that needs
    /// to be fetched. The caller is responsible for calling `fetch_and_cache`
    /// if `SchemaLookup::NeedsFetch` is returned.
    pub fn schema_for_document(
        &mut self,
        doc_uri: &str,
        inline_schema_uri: Option<&str>,
    ) -> SchemaLookup {
        // 1. Check for $schema property.
        if let Some(schema_uri) = inline_schema_uri {
            if let Some(cached) = self.cache.get(schema_uri) {
                return SchemaLookup::Resolved(cached.clone());
            }
            return SchemaLookup::NeedsFetch(schema_uri.to_string());
        }

        // 2. Check file-pattern associations.
        if let Some((uri, inline)) = self.match_association(doc_uri) {
            if let Some(schema) = inline {
                return SchemaLookup::Resolved(schema);
            }
            if let Some(cached) = self.cache.get(&uri) {
                return SchemaLookup::Resolved(cached.clone());
            }
            return SchemaLookup::NeedsFetch(uri);
        }

        SchemaLookup::None
    }

    /// Insert a fetched schema into the cache. LRU eviction is automatic.
    pub fn insert_cache(&mut self, uri: String, schema: Arc<JsonSchema>) {
        self.cache.put(uri, schema);
    }

    fn match_association(&self, doc_uri: &str) -> Option<(String, Option<Arc<JsonSchema>>)> {
        for (matchers, uri, inline) in &self.associations {
            for matcher in matchers {
                if matcher.is_match(doc_uri) {
                    return Some((uri.clone(), inline.clone()));
                }
            }
        }
        None
    }

    /// Resolve a `$ref` within a schema.
    pub fn resolve_ref(
        &mut self,
        reference: &str,
        current_schema: &Arc<JsonSchema>,
        base_uri: &str,
    ) -> RefLookup {
        let (external, fragment) = match reference.find('#') {
            Some(idx) => (&reference[..idx], &reference[idx + 1..]),
            None => (reference, ""),
        };

        if external.is_empty() {
            let schema = current_schema.clone();
            if fragment.is_empty() || fragment == "/" {
                return RefLookup::Resolved(Some(schema));
            }
            return RefLookup::Resolved(resolve_pointer(&schema, fragment));
        }

        let resolved_uri = resolve_relative_uri(base_uri, external);
        if let Some(cached) = self.cache.get(&resolved_uri) {
            let schema = cached.clone();
            if fragment.is_empty() || fragment == "/" {
                return RefLookup::Resolved(Some(schema));
            }
            return RefLookup::Resolved(resolve_pointer(&schema, fragment));
        }

        RefLookup::NeedsFetch {
            uri: resolved_uri,
            fragment: fragment.to_string(),
        }
    }
}

/// Result of a synchronous schema lookup.
pub enum SchemaLookup {
    Resolved(Arc<JsonSchema>),
    NeedsFetch(String),
    None,
}

/// Result of a synchronous $ref resolution.
pub enum RefLookup {
    Resolved(Option<Arc<JsonSchema>>),
    NeedsFetch { uri: String, fragment: String },
}

impl Default for SchemaStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Fetch a schema from HTTP or file (blocking).
pub fn fetch_schema(agent: &ureq::Agent, uri: &str) -> Option<serde_json::Value> {
    if uri.starts_with("http://") || uri.starts_with("https://") {
        match agent.get(uri).call() {
            Ok(resp) => match resp.into_body().read_json() {
                Ok(val) => Some(val),
                Err(e) => {
                    warn!("failed to parse schema from {}: {}", uri, e);
                    None
                }
            },
            Err(e) => {
                warn!("failed to fetch schema {}: {}", uri, e);
                None
            }
        }
    } else if uri.starts_with("file://") {
        let path = uri.strip_prefix("file://").unwrap_or(uri);
        match std::fs::read_to_string(path) {
            Ok(content) => serde_json::from_str(&content).ok(),
            Err(e) => {
                warn!("failed to read schema file {}: {}", path, e);
                None
            }
        }
    } else {
        debug!("unsupported schema URI scheme: {}", uri);
        None
    }
}

/// Extract `$schema` property from the root object.
pub fn extract_schema_property(doc: &Document) -> Option<String> {
    let root = tree::root_value(&doc.tree)?;
    if root.kind() != kinds::OBJECT {
        return None;
    }
    let mut cursor = root.walk();
    for pair in tree::object_pairs(root, &mut cursor) {
        if tree::pair_key(pair, doc.source())? == "$schema" {
            let val = tree::pair_value(pair)?;
            if val.kind() == kinds::STRING {
                return tree::string_value(val, doc.source());
            }
        }
    }
    None
}

/// Resolve a JSON Pointer fragment within a compiled schema.
fn resolve_pointer(schema: &Arc<JsonSchema>, pointer: &str) -> Option<Arc<JsonSchema>> {
    let path = pointer.strip_prefix('/').unwrap_or(pointer);
    let segments: Vec<&str> = path.split('/').collect();

    let mut current = schema.clone();
    for segment in &segments {
        let decoded = percent_encoding::percent_decode_str(segment)
            .decode_utf8()
            .ok()?;
        let key = decoded.replace("~1", "/").replace("~0", "~");

        if let Some(found) = current.definitions.get(&*key) {
            current = found.clone();
        } else if let Some(found) = current.defs.get(&*key) {
            current = found.clone();
        } else if let Some(found) = current.properties.get(&*key) {
            current = found.clone();
        } else {
            return None;
        }
    }

    Some(current)
}

fn resolve_relative_uri(base: &str, relative: &str) -> String {
    if relative.contains("://") {
        return relative.to_string();
    }
    if let Some(base_dir) = base.rfind('/') {
        format!("{}/{}", &base[..base_dir], relative)
    } else {
        relative.to_string()
    }
}
