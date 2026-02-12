# json-language-server

A high-performance JSON Language Server written in Rust, implementing the [Language Server Protocol](https://microsoft.github.io/language-server-protocol/) (LSP). It provides rich editing features for JSON files including intelligent completions, hover information, schema validation, formatting, and more — with full support for JSON Schema drafts 4 through 2020-12.

Built on [tree-sitter](https://tree-sitter.github.io/) for fast, incremental, error-recovering parsing and [tower-lsp](https://github.com/ebkalderon/tower-lsp) for async LSP transport.

---

## Performance

Benchmarked against [vscode-json-languageservice](https://github.com/microsoft/vscode-json-languageservice), the Node.js JSON language service used by VS Code. All times are in milliseconds. Three document sizes were tested: small (1.1 KB), medium (50 KB), and large (500 KB).

### Latency

| Scenario | Size | Metric | json-language-server (Rust) | vscode-json-languageservice (Node) | Winner |
|---|---|---|--:|--:|---|
| **Startup** | — | p50 | 11.7 | 52.9 | Rust (4.5x) |
| | — | p95 | 14.2 | 55.8 | Rust (3.9x) |
| | — | mean | 11.1 | 53.4 | Rust (4.8x) |
| **Open + Diagnostics** | small | p50 | 33.4 | 304 | Rust (9.1x) |
| | small | p95 | 35.2 | 306 | Rust (8.7x) |
| | small | mean | 33.3 | 304 | Rust (9.1x) |
| | medium | p50 | 38.5 | 307 | Rust (8.0x) |
| | medium | p95 | 39.3 | 311 | Rust (7.9x) |
| | medium | mean | 37.6 | 307 | Rust (8.2x) |
| | large | p50 | 50.9 | 331 | Rust (6.5x) |
| | large | p95 | 51.9 | 344 | Rust (6.6x) |
| | large | mean | 50.1 | 333 | Rust (6.6x) |
| **Completion** | small | p50 | 0.097 | 0.319 | Rust (3.3x) |
| | small | p95 | 0.134 | 0.662 | Rust (4.9x) |
| | small | mean | 0.103 | 0.357 | Rust (3.5x) |
| | medium | p50 | 0.078 | 0.200 | Rust (2.6x) |
| | medium | p95 | 0.286 | 0.950 | Rust (3.3x) |
| | medium | mean | 0.269 | 0.503 | Rust (1.9x) |
| | large | p50 | 0.041 | 0.121 | Rust (3.0x) |
| | large | p95 | 1.14 | 2.21 | Rust (1.9x) |
| | large | mean | 1.11 | 0.867 | Node (1.3x) |
| **Hover** | small | p50 | 0.119 | 0.241 | Rust (2.0x) |
| | small | p95 | 0.197 | 0.620 | Rust (3.1x) |
| | small | mean | 0.136 | 0.307 | Rust (2.2x) |
| | medium | p50 | 0.063 | 0.182 | Rust (2.9x) |
| | medium | p95 | 0.275 | 0.709 | Rust (2.6x) |
| | medium | mean | 0.263 | 0.459 | Rust (1.7x) |
| | large | p50 | 0.050 | 0.118 | Rust (2.3x) |
| | large | p95 | 1.19 | 2.02 | Rust (1.7x) |
| | large | mean | 1.14 | 0.835 | Node (1.4x) |
| **Document Symbols** | small | p50 | 0.217 | 0.322 | Rust (1.5x) |
| | small | p95 | 0.305 | 0.848 | Rust (2.8x) |
| | small | mean | 0.235 | 0.396 | Rust (1.7x) |
| | medium | p50 | 2.15 | 2.29 | Rust (1.1x) |
| | medium | p95 | 2.90 | 4.33 | Rust (1.5x) |
| | medium | mean | 2.40 | 2.67 | Rust (1.1x) |
| | large | p50 | 20.2 | 21.0 | Rust (1.0x) |
| | large | p95 | 21.6 | 30.2 | Rust (1.4x) |
| | large | mean | 21.2 | 22.0 | Rust (1.0x) |
| **Edit + Diagnostics** | small | p50 | 1.10 | 304 | Rust (275x) |
| | small | p95 | 1.37 | 305 | Rust (223x) |
| | small | mean | 1.09 | 304 | Rust (279x) |
| | medium | p50 | 6.83 | 307 | Rust (45x) |
| | medium | p95 | 8.56 | 314 | Rust (37x) |
| | medium | mean | 6.82 | 308 | Rust (45x) |
| | large | p50 | 27.7 | 332 | Rust (12x) |
| | large | p95 | 38.3 | 342 | Rust (8.9x) |
| | large | mean | 28.1 | 333 | Rust (12x) |

### Memory (KB RSS)

| Phase | json-language-server (Rust) | vscode-json-languageservice (Node) | Ratio |
|---|--:|--:|---|
| Idle | 7,232 | 57,536 | 8.0x less |
| Peak (small) | 8,480 | 58,240 | 6.9x less |
| Peak (medium) | 9,552 | 61,056 | 6.4x less |
| Peak (large) | 21,120 | 81,504 | 3.9x less |

### Feature Parity

Comparison with [vscode-json-languageservice](https://github.com/microsoft/vscode-json-languageservice):

| Feature | json-language-server (Rust) | vscode-json-languageservice (Node) |
|---|:---:|:---:|
| JSON Schema validation | :white_check_mark: | :white_check_mark: |
| Schema drafts 4, 6, 7, 2019-09, 2020-12 | :white_check_mark: | :white_check_mark: |
| Code completion | :white_check_mark: | :white_check_mark: |
| Completion resolve | :x: | :white_check_mark: |
| Hover information | :white_check_mark: | :white_check_mark: |
| Document symbols | :white_check_mark: | :white_check_mark: |
| Document colors | :white_check_mark: | :white_check_mark: |
| Color presentations | :white_check_mark: | :white_check_mark: |
| Document formatting | :white_check_mark: | :white_check_mark: |
| Document sorting | :white_check_mark: | :white_check_mark: |
| Folding ranges | :white_check_mark: | :white_check_mark: |
| Selection ranges | :white_check_mark: | :white_check_mark: |
| Document links | :white_check_mark: | :white_check_mark: |
| Go to definition | :white_check_mark: | :white_check_mark: |
| Syntax diagnostics | :white_check_mark: | :white_check_mark: |
| `$ref` resolution | :white_check_mark: | :white_check_mark: |
| VS Code schema extensions | :white_check_mark: | :white_check_mark: |
| Schema matching / language status | :x: | :white_check_mark: |
| Incremental parsing (tree-sitter) | :white_check_mark: | :x: |
| Incremental document sync | :white_check_mark: | :x: |
| JSONC tolerance (comments, trailing commas) | :white_check_mark: | :white_check_mark: |

---

## Features

### Intelligent Completions

Context-aware completions powered by JSON Schema:

- **Property names** — suggests keys from `properties`, with required properties sorted first
- **Property values** — enum members, const values, booleans (`true`/`false`), `null`, and structural snippets for objects/arrays
- **Array items** — driven by `items` and `prefixItems` schemas
- **Default snippets** — custom code templates via the VS Code `defaultSnippets` extension
- **Descriptions** — each suggestion includes its schema description and type info
- **Deprecation markers** — deprecated properties are visually flagged

Trigger characters: `"`, `:`, ` ` (space)

### Hover Information

Hovering over any JSON value displays:

- **JSON Pointer path** (e.g. `/config/database/host`)
- **Description** from the schema (Markdown supported)
- **Type** (including union types like `string | number`)
- **Default value**
- **Allowed enum values** (up to 20 shown)
- **Deprecation warnings** with custom messages
- **Current value** preview

### Schema Validation

Comprehensive validation against JSON Schema with detailed error messages:

**Type checking**
- Validates against `type` (string, number, integer, boolean, null, array, object)
- Supports type unions (e.g. `["string", "null"]`)
- Recognizes integer as a subtype of number

**String constraints**
- `minLength` / `maxLength` (Unicode-aware character count)
- `pattern` (regex validation with custom error messages via `patternErrorMessage`)
- `format` validation for: `date-time`, `date`, `time`, `email`, `hostname`, `ipv4`, `ipv6`, `uri`, `uri-reference`, `color-hex`

**Numeric constraints**
- `minimum` / `maximum` (inclusive)
- `exclusiveMinimum` / `exclusiveMaximum` (Draft 4 boolean form and Draft 6+ numeric form)
- `multipleOf` (with floating-point tolerance)

**Object constraints**
- `required` properties
- `minProperties` / `maxProperties`
- `additionalProperties` (boolean or schema)
- `patternProperties` (regex-matched property names)
- `propertyNames` (schema for all keys)
- `dependencies`, `dependentRequired`, `dependentSchemas`

**Array constraints**
- `minItems` / `maxItems`
- `uniqueItems` (O(n) duplicate detection)
- `items` (single schema for all items)
- `prefixItems` (tuple validation)
- `contains` / `minContains` / `maxContains`

**Composition & conditional**
- `allOf` — all schemas must validate
- `anyOf` — at least one must validate
- `oneOf` — exactly one must validate
- `not` — must not validate
- `if` / `then` / `else` — conditional schema application

**Other**
- `enum` and `const` validation
- `deprecated` flag (reported as warnings)
- `$ref` resolution with circular reference detection
- Custom error messages via `errorMessage`

### Syntax Diagnostics

Reported without requiring a schema:

- **Syntax errors** — invalid JSON structure detected via tree-sitter's error-recovering parser
- **Missing tokens** — expected commas, colons, brackets, etc.
- **Duplicate keys** — warned per object scope
- **Trailing commas** — silently tolerated (JSONC-compatible)
- **Comments** — line (`//`) and block (`/* */`) comments tolerated
- **Descriptive messages** — context-aware messages like "Expected a value", "Single-quoted strings are not allowed in JSON", "Expected comma before this property"

### Document Formatting

- Full document and range formatting
- Walks the tree-sitter CST directly — no redundant serde_json round-trip
- Respects editor settings: `tabSize`, `insertSpaces`, `insertFinalNewline`
- Auto-detects existing indentation style (spaces vs tabs, indent width)
- Preserves string escapes and number formats exactly from source
- Only formats syntactically valid JSON

### Document Sorting

- Alphabetically sorts all object properties (recursively)
- Available as the `json.sort` command
- Preserves array order and detects current indentation style

### Document Symbols

- Hierarchical outline of the JSON structure
- Property names as symbols with type indicators
- Array items shown as `[0]`, `[1]`, etc.
- Detail text: type preview, property count for objects, item count for arrays

### Document Links

- Clickable links for `$ref` values pointing to HTTP/HTTPS URLs
- Detects any string value starting with `http://` or `https://`
- Internal `$ref` references (starting with `#`) support go-to-definition

### Go to Definition

- Navigates to the target of internal `$ref` pointers (e.g. `#/definitions/Address`)
- Resolves JSON Pointer paths within the same document

### Color Provider

- Detects CSS hex colors in string values
- Supports `#RGB`, `#RGBA`, `#RRGGBB`, and `#RRGGBBAA` formats
- Provides color presentations in hex, RGB/RGBA, and HSL/HSLA

### Folding Ranges

- Collapsible regions for multiline objects and arrays
- Nested folding support

### Selection Ranges

- Expand/contract selection following the JSON AST hierarchy
- Each expansion step selects the next parent node

---

## JSON Schema Support

### Supported Drafts

| Draft | `$schema` URI |
|-------|---------------|
| Draft 4 | `http://json-schema.org/draft-04/schema#` |
| Draft 6 | `http://json-schema.org/draft-06/schema#` |
| Draft 7 | `http://json-schema.org/draft-07/schema#` |
| 2019-09 | `https://json-schema.org/draft/2019-09/schema` |
| 2020-12 | `https://json-schema.org/draft/2020-12/schema` |

Draft 7 is used as the default when no draft can be detected.

### Schema Resolution

Schemas are resolved in priority order:

1. **Inline `$schema`** — the document's own `$schema` property value is used as the schema URI
2. **File-pattern associations** — glob patterns configured through editor settings (e.g. `*.tsconfig.json` → TypeScript config schema)
3. **No schema** — only syntax diagnostics are reported

### Schema Sources

- **HTTP/HTTPS** — remote schemas fetched asynchronously on a blocking thread pool (10-second timeout, LRU-cached per URI)
- **file://** — local filesystem schemas
- **`$ref` resolution** — external references resolved relative to the current schema URI; JSON Pointer fragments resolve within the target schema

### VS Code Schema Extensions

The server recognizes several non-standard extensions used by VS Code's JSON support:

- `markdownDescription` — preferred over `description` for richer hover text
- `doNotSuggest` — hides a property from completion suggestions
- `enumDescriptions` / `markdownEnumDescriptions` — per-value descriptions for enum completions
- `defaultSnippets` — custom completion templates with label, description, and body
- `errorMessage` / `patternErrorMessage` — custom error text for validation failures
- `deprecationMessage` — custom message shown for deprecated properties

---

## Architecture

```
src/
  main.rs          Entry point — single-threaded tokio runtime, stdin/stdout LSP transport
  lib.rs           Module declarations
  server.rs        LSP LanguageServer trait implementation, request routing, debounced validation
  document.rs      Document store, incremental text editing, incremental line index, position/offset conversion
  tree.rs          tree-sitter wrapper, JSON AST helpers, string unescaping, path/pointer utilities
  completion.rs    Context-aware completions from schema
  hover.rs         Hover information assembly
  diagnostics.rs   Syntax error and duplicate key detection, trailing comma/comment tolerance
  formatting.rs    Tree-sitter CST-based formatting, serde_json-based sorting
  links.rs         $ref and URL link detection, go-to-definition
  colors.rs        Hex color detection and presentation
  symbols.rs       Document symbol hierarchy
  folding.rs       Folding range computation
  selection.rs     Selection range chains
  schema/
    types.rs       JsonSchema struct, parsing from serde_json::Value, draft detection, path resolution
    validation.rs  Full schema validation engine, server-wide regex caching
    resolver.rs    Schema fetching (ureq), LRU caching, $ref resolution, glob matching
```

### Key Design Decisions

**tree-sitter for parsing** — provides incremental reparsing (only re-analyzes changed regions), error recovery (continues parsing after syntax errors), and a concrete syntax tree for precise position mapping.

**Incremental document sync** — the server uses LSP's incremental text document sync, applying edits directly to the source text and tree-sitter's `Tree.edit()` API for O(log n) re-parsing on each keystroke.

**Incremental line index** — rather than rebuilding the full line-start offset table on every edit, `LineIndex::update()` uses binary search to find affected lines, splices in new line starts from the replacement text, and shifts trailing offsets by the byte delta. This makes per-keystroke line index updates O(edit size) instead of O(document size). A full rebuild fallback is retained as a safety net.

**Tree-sitter based formatting** — the formatter walks the tree-sitter CST directly, copying leaf node text verbatim from source. This avoids an entire redundant `serde_json::from_str()` parse and halves peak memory during formatting. The serde_json round-trip is only retained for the sort command, which needs to reorder object keys.

**UTF-16 position handling** — LSP uses UTF-16 code units for column positions while tree-sitter uses byte offsets. The `LineIndex` in `document.rs` handles bidirectional conversion with pre-computed line start offsets.

**Schema path resolution** — a single `resolve_path_segment()` method on `JsonSchema` walks through properties, array items, composition schemas (allOf/anyOf/oneOf), and conditional schemas (if/then/else) to find the sub-schema at any cursor position. This is shared across completion and hover.

**Server-wide regex caching** — compiled regex patterns are stored in a dedicated `Mutex<RegexCache>` on the server, persisting across all validation passes for the server's lifetime. This means a `pattern` or `patternProperties` regex is compiled once and reused on every subsequent validation, rather than being recompiled on each keystroke.

**LRU schema cache** — fetched schemas are stored in an LRU cache (capacity 32) backed by the `lru` crate. When the cache is full, the least-recently-used schema is evicted. This prevents the thundering-herd problem where all cached schemas were previously cleared at once.

**Validation debouncing** — `did_change` events trigger a 75ms debounce before running validation. If another edit arrives within the window, the previous validation is cancelled. This cuts redundant validation work by 5-10x during active typing. `did_open` and `did_save` bypass debouncing for immediate feedback.

**Circular `$ref` detection** — each resolution chain maintains its own `HashSet` of visited URIs, cloned at branch points so that the same `$ref` can be used in sibling locations without false-positive cycle detection.

**JSONC tolerance** — trailing commas and comments (line and block) are silently accepted, matching the behavior of VS Code, tsconfig.json, and other JSONC-aware tools. Double commas and leading commas are still reported as errors.

---

## Building

```sh
cargo build --release
```

The release profile is configured for maximum optimization:

```toml
[profile.release]
opt-level = 3       # Full optimization
lto = "fat"         # Full cross-crate link-time optimization
codegen-units = 1   # Single codegen unit for better optimization
strip = true        # Strip debug symbols
panic = "abort"     # Remove unwinding machinery
overflow-checks = false
```

### Requirements

- Rust 2024 edition (1.85+)

---

## Usage

The server communicates over stdin/stdout using the LSP protocol. Point your editor's JSON language client at the binary.

### Logging

Set the `RUST_LOG` environment variable to control log verbosity:

```sh
RUST_LOG=debug json-language-server
```

### Schema Configuration

Configure schema associations through your editor's LSP settings. The server expects a `json.schemas` configuration with file glob patterns:

```json
{
  "json.schemas": [
    {
      "fileMatch": ["package.json"],
      "url": "https://json.schemastore.org/package.json"
    },
    {
      "fileMatch": ["tsconfig*.json"],
      "url": "https://json.schemastore.org/tsconfig.json"
    },
    {
      "fileMatch": [".eslintrc.json"],
      "url": "https://json.schemastore.org/eslintrc.json"
    }
  ]
}
```

Documents can also specify their own schema via the `$schema` property:

```json
{
  "$schema": "https://json.schemastore.org/package.json",
  "name": "my-package"
}
```

---

## Tests

```sh
cargo test
```

The test suite includes 195 tests covering all major features: parsing, incremental editing, formatting, sorting, validation (type checking, enums, constraints, composition, conditionals, format validation), syntax diagnostics (trailing commas, comments, descriptive error messages), duplicate key detection, colors, symbols, folding, selection ranges, links, go-to-definition, schema parsing, and tree-sitter utilities.

---

## Dependencies

| Crate | Purpose |
|-------|---------|
| `tower-lsp` | Async LSP protocol implementation |
| `tokio` | Async runtime (single-threaded, with timer support for debouncing) |
| `tree-sitter` | Incremental parser framework |
| `tree-sitter-json` | JSON grammar for tree-sitter |
| `serde` / `serde_json` | JSON schema parsing and value manipulation |
| `ureq` | HTTP schema fetching (native-tls) |
| `regex` | Pattern and patternProperties validation |
| `globset` | File-pattern schema association matching |
| `percent-encoding` | URI encoding/decoding for `$ref` resolution |
| `lru` | LRU cache for fetched schemas |
| `tracing` / `tracing-subscriber` | Structured logging with env-filter support |

---

## License

MIT
