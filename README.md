# json-language-server

A high-performance JSON Language Server written in Rust, implementing the [Language Server Protocol](https://microsoft.github.io/language-server-protocol/) (LSP). Provides rich editing features for JSON files with full support for JSON Schema drafts 4 through 2020-12.

Built on [tree-sitter](https://tree-sitter.github.io/) for fast, incremental, error-recovering parsing and [lsp-server](https://github.com/rust-lang/rust-analyzer/tree/master/lib/lsp-server) for synchronous LSP transport.

---

## Performance

Benchmarked against [vscode-json-languageservice](https://github.com/microsoft/vscode-json-languageservice), the Node.js JSON language service used by VS Code. All times are in milliseconds. Three document sizes were tested: small (1.1 KB), medium (50 KB), and large (500 KB).

### Latency

| Scenario | Size | Metric | json-language-server (Rust) | vscode-json-languageservice (Node) | Winner |
|---|---|---|--:|--:|---|
| **Startup** | — | p50 | 7.06 | 51.5 | Rust (7.3x) |
| | — | p95 | 7.68 | 56.8 | Rust (7.4x) |
| | — | mean | 7.06 | 53.2 | Rust (7.5x) |
| **Open + Diagnostics** | small | p50 | 0.426 | 305 | Rust (715x) |
| | small | p95 | 0.523 | 306 | Rust (585x) |
| | small | mean | 0.436 | 305 | Rust (699x) |
| | medium | p50 | 4.07 | 310 | Rust (76x) |
| | medium | p95 | 4.42 | 313 | Rust (71x) |
| | medium | mean | 4.14 | 310 | Rust (75x) |
| | large | p50 | 37.1 | 338 | Rust (9.1x) |
| | large | p95 | 37.9 | 349 | Rust (9.2x) |
| | large | mean | 37.1 | 339 | Rust (9.1x) |
| **Edit + Diagnostics** | small | p50 | 0.856 | 305 | Rust (356x) |
| | small | p95 | 1.80 | 307 | Rust (170x) |
| | small | mean | 1.45 | 305 | Rust (210x) |
| | medium | p50 | 7.48 | 309 | Rust (41x) |
| | medium | p95 | 11.9 | 314 | Rust (26x) |
| | medium | mean | 8.55 | 310 | Rust (36x) |
| | large | p50 | 33.9 | 334 | Rust (9.9x) |
| | large | p95 | 56.8 | 346 | Rust (6.1x) |
| | large | mean | 36.3 | 337 | Rust (9.3x) |
| **Completion** | small | p50 | 0.047 | 0.237 | Rust (5.0x) |
| | small | p95 | 0.122 | 0.761 | Rust (6.3x) |
| | small | mean | 0.065 | 0.367 | Rust (5.6x) |
| | medium | p50 | 0.039 | 0.207 | Rust (5.3x) |
| | medium | p95 | 0.152 | 0.762 | Rust (5.0x) |
| | medium | mean | 0.140 | 0.477 | Rust (3.4x) |
| | large | p50 | 0.057 | 0.134 | Rust (2.4x) |
| | large | p95 | 1.10 | 2.12 | Rust (1.9x) |
| | large | mean | 1.05 | 0.902 | Node (1.2x) |
| **Hover** | small | p50 | 0.043 | 0.192 | Rust (4.4x) |
| | small | p95 | 0.066 | 0.518 | Rust (7.9x) |
| | small | mean | 0.047 | 0.265 | Rust (5.6x) |
| | medium | p50 | 0.039 | 0.123 | Rust (3.1x) |
| | medium | p95 | 0.162 | 0.589 | Rust (3.6x) |
| | medium | mean | 0.138 | 0.332 | Rust (2.4x) |
| | large | p50 | 0.049 | 0.177 | Rust (3.6x) |
| | large | p95 | 1.11 | 0.975 | Node (1.1x) |
| | large | mean | 1.05 | 0.897 | Node (1.2x) |
| **Document Symbols** | small | p50 | 0.200 | 0.273 | Rust (1.4x) |
| | small | p95 | 0.369 | 0.573 | Rust (1.6x) |
| | small | mean | 0.220 | 0.295 | Rust (1.3x) |
| | medium | p50 | 2.16 | 2.34 | Rust (1.1x) |
| | medium | p95 | 2.89 | 3.17 | Rust (1.1x) |
| | medium | mean | 2.45 | 2.66 | Rust (1.1x) |
| | large | p50 | 20.4 | 20.4 | Node (1.0x) |
| | large | p95 | 21.9 | 29.8 | Rust (1.4x) |
| | large | mean | 21.4 | 21.3 | Node (1.0x) |

### Memory (KB RSS)

| Phase | json-language-server (Rust) | vscode-json-languageservice (Node) | Ratio |
|---|--:|--:|---|
| Idle | 7,232 | 57,536 | 8.0x less |
| Peak (small) | 8,480 | 58,240 | 6.9x less |
| Peak (medium) | 9,552 | 61,056 | 6.4x less |
| Peak (large) | 21,120 | 81,504 | 3.9x less |

### Feature Parity

Comparison with [vscode-json-languageservice](https://github.com/microsoft/vscode-json-languageservice):

| Feature | Rust | Node |
|---|:---:|:---:|
| JSON Schema validation (drafts 4, 6, 7, 2019-09, 2020-12) | :white_check_mark: | :white_check_mark: |
| Code completion | :white_check_mark: | :white_check_mark: |
| Completion resolve | :x: | :white_check_mark: |
| Hover information | :white_check_mark: | :white_check_mark: |
| Document symbols | :white_check_mark: | :white_check_mark: |
| Document colors / color presentations | :white_check_mark: | :white_check_mark: |
| Formatting / sorting | :white_check_mark: | :white_check_mark: |
| Folding / selection ranges | :white_check_mark: | :white_check_mark: |
| Document links / go to definition | :white_check_mark: | :white_check_mark: |
| Syntax diagnostics | :white_check_mark: | :white_check_mark: |
| `$ref` resolution | :white_check_mark: | :white_check_mark: |
| VS Code schema extensions | :white_check_mark: | :white_check_mark: |
| JSONC tolerance (comments, trailing commas) | :white_check_mark: | :white_check_mark: |
| Schema matching / language status | :x: | :white_check_mark: |
| Incremental parsing (tree-sitter) | :white_check_mark: | :x: |
| Incremental document sync | :white_check_mark: | :x: |

---

## Building

```sh
cargo build --release
```

Or install directly:

```sh
cargo install --path .
```

Release profile: `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`, `strip = true`, `panic = "abort"`.

Requires Rust 2024 edition (1.85+).

### Zed

After `cargo install --path .`, add to your Zed settings (`~/.config/zed/settings.json`):

```json
{
  "lsp": {
    "json-language-server": {
      "binary": {
        "path": "json-language-server",
        "arguments": []
      }
    }
  }
}
```

### VS Code

Point a generic LSP client extension (e.g. [vscode-languageclient](https://github.com/microsoft/vscode-languageserver-node)) at the `json-language-server` binary. The server communicates over stdin/stdout.

### Schema Configuration

Configure schema associations through your editor's LSP settings:

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
    }
  ]
}
```

Documents can also specify their own schema via the `$schema` property.

### Logging

```sh
RUST_LOG=debug json-language-server
```

---

## Architecture

```
src/
  main.rs          Entry point — stdin/stdout LSP transport
  server.rs        LSP request routing, debounced validation
  document.rs      Document store, incremental editing, line index
  tree.rs          tree-sitter wrapper, JSON AST helpers
  completion.rs    Context-aware completions from schema
  hover.rs         Hover information assembly
  diagnostics.rs   Syntax errors, duplicate keys, trailing comma/comment tolerance
  formatting.rs    CST-based formatting, serde_json-based sorting
  links.rs         $ref / URL links, go-to-definition
  colors.rs        Hex color detection and presentation
  symbols.rs       Document symbol hierarchy
  folding.rs       Folding ranges
  selection.rs     Selection ranges
  schema/
    types.rs       Schema parsing, draft detection, path resolution
    validation.rs  Full validation engine, server-wide regex caching
    resolver.rs    Schema fetching (ureq), caching, $ref resolution, glob matching
```

### Design Decisions

- **tree-sitter parsing** — incremental reparsing (only changed regions), error recovery, concrete syntax tree for precise position mapping.
- **Incremental document sync** — LSP incremental sync with `Tree.edit()` for O(log n) re-parsing per keystroke.
- **CST-based formatting** — walks tree-sitter CST directly, copies leaf text verbatim. No redundant `serde_json` round-trip (only used for sort).
- **Schema path resolution** — single `resolve_path_segment()` walks properties, items, composition, and conditional schemas. Shared by completion and hover.
- **Server-wide regex cache** — compiled patterns persist across all validation passes for the server's lifetime.
- **Validation debouncing** — 50ms debounce on `did_change`; `did_open` and `did_save` bypass for immediate feedback.
- **Circular `$ref` detection** — per-chain `HashSet` of visited URIs, cloned at branch points to avoid false positives.
- **JSONC tolerance** — trailing commas and comments silently accepted. Double/leading commas still reported.

---

## Tests

```sh
cargo test
```

195 tests covering parsing, incremental editing, formatting, sorting, validation, syntax diagnostics, colors, symbols, folding, selection ranges, links, go-to-definition, and schema parsing.

---

## Dependencies

| Crate | Purpose |
|-------|---------|
| `lsp-server` / `lsp-types` | LSP protocol |
| `crossbeam-channel` | Message passing |
| `tree-sitter` / `tree-sitter-json` | Incremental parsing |
| `line-index` | UTF-16 position conversion |
| `serde` / `serde_json` | Schema parsing and value manipulation |
| `ureq` | HTTP schema fetching |
| `regex` | Pattern validation |
| `globset` | File-pattern matching |
| `parking_lot` | Synchronization primitives |
| `tracing` / `tracing-subscriber` | Structured logging |

---

## License

MIT
