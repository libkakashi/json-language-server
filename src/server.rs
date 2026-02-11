/// LSP server: wires all features together via tower-lsp.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::time::{Duration, sleep};
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};
use tracing::{debug, info, warn};

use crate::colors;
use crate::completion;
use crate::diagnostics;
use crate::document::DocumentStore;
use crate::folding;
use crate::formatting;
use crate::hover;
use crate::links;
use crate::schema::resolver::{self, SchemaAssociation, SchemaLookup, SchemaStore};
use crate::schema::types::JsonSchema;
use crate::schema::validation::{self, RegexCache};
use crate::selection;
use crate::symbols;
use crate::tree;

pub struct ServerState {
    pub documents: DocumentStore,
    pub schemas: SchemaStore,
}

/// Debounce delay for validation after edits.
const DEBOUNCE_MS: u64 = 75;

pub struct JsonLanguageServer {
    pub client: Client,
    pub state: Mutex<ServerState>,
    /// Server-wide regex cache, separate from state to avoid borrow conflicts.
    regex_cache: Mutex<RegexCache>,
    /// Monotonic counter per URI for debouncing validation.
    /// Each edit bumps the counter; validation only proceeds if
    /// the counter hasn't changed after the debounce delay.
    debounce_versions: Mutex<HashMap<Url, u64>>,
}

impl JsonLanguageServer {
    pub fn new(client: Client) -> Self {
        JsonLanguageServer {
            client,
            state: Mutex::new(ServerState {
                documents: DocumentStore::new(),
                schemas: SchemaStore::new(),
            }),
            regex_cache: Mutex::new(RegexCache::new()),
            debounce_versions: Mutex::new(HashMap::new()),
        }
    }

    /// Resolve a schema for a document, fetching over HTTP if needed.
    /// Handles lock/unlock around the blocking fetch.
    async fn resolve_schema(
        &self,
        doc_uri: &str,
        inline_schema_uri: Option<&str>,
    ) -> Option<Arc<JsonSchema>> {
        let lookup = {
            let mut state = self.state.lock().unwrap();
            state
                .schemas
                .schema_for_document(doc_uri, inline_schema_uri)
        };

        match lookup {
            SchemaLookup::Resolved(schema) => Some(schema),
            SchemaLookup::NeedsFetch(uri) => {
                let agent = self.state.lock().unwrap().schemas.http_agent();
                let fetch_uri = uri.clone();
                let raw =
                    tokio::task::spawn_blocking(move || resolver::fetch_schema(&agent, &fetch_uri))
                        .await
                        .ok()??;
                let schema = JsonSchema::from_value(&raw);
                self.state
                    .lock()
                    .unwrap()
                    .schemas
                    .insert_cache(uri, schema.clone());
                Some(schema)
            }
            SchemaLookup::None => None,
        }
    }

    /// Schedule a debounced validation. Bumps the version counter and waits;
    /// if another edit arrives during the delay, this call becomes a no-op.
    async fn debounced_validate(&self, uri: &Url) {
        let version = {
            let mut versions = self.debounce_versions.lock().unwrap();
            let v = versions.entry(uri.clone()).or_insert(0);
            *v += 1;
            *v
        };

        sleep(Duration::from_millis(DEBOUNCE_MS)).await;

        let current = {
            let versions = self.debounce_versions.lock().unwrap();
            versions.get(uri).copied().unwrap_or(0)
        };
        if current != version {
            return; // A newer edit superseded this one.
        }

        self.validate_and_publish(uri).await;
    }

    /// Run syntax + schema validation and push diagnostics to the client.
    async fn validate_and_publish(&self, uri: &Url) {
        let (mut diags, version, needs_schema, uri_str, inline_schema) = {
            let state = self.state.lock().unwrap();

            let doc = match state.documents.get(uri) {
                Some(d) => d,
                None => return,
            };

            let diags = diagnostics::syntax_diagnostics(doc);
            let version = Some(doc.version);
            let needs_schema = diags.is_empty() && tree::root_value(&doc.tree).is_some();
            let uri_str = uri.as_str().to_string();
            let inline_schema = if needs_schema {
                crate::schema::resolver::extract_schema_property(doc)
            } else {
                None
            };

            (diags, version, needs_schema, uri_str, inline_schema)
        };

        if needs_schema {
            let schema = self
                .resolve_schema(&uri_str, inline_schema.as_deref())
                .await;

            let state = self.state.lock().unwrap();
            let mut regex_cache = self.regex_cache.lock().unwrap();
            if let (Some(schema), Some(doc)) = (schema, state.documents.get(uri))
                && let Some(root) = tree::root_value(&doc.tree)
            {
                let val_errors =
                    validation::validate(root, doc.source(), &schema, &mut regex_cache);
                for ve in &val_errors {
                    diags.push(Diagnostic {
                        range: doc.range_of(ve.start_byte, ve.end_byte),
                        severity: Some(match ve.severity {
                            validation::Severity::Error => DiagnosticSeverity::ERROR,
                            validation::Severity::Warning => DiagnosticSeverity::WARNING,
                        }),
                        source: Some("json".into()),
                        message: ve.message.clone(),
                        ..Diagnostic::default()
                    });
                }
            }
        }

        self.client
            .publish_diagnostics(uri.clone(), diags, version)
            .await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for JsonLanguageServer {
    async fn initialize(&self, _params: InitializeParams) -> Result<InitializeResult> {
        info!("json-language-server initializing");

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "json-language-server".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        will_save: None,
                        will_save_wait_until: None,
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(false),
                        })),
                    },
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec!["\"".into(), ":".into(), " ".into()]),
                    all_commit_characters: None,
                    work_done_progress_options: Default::default(),
                    ..Default::default()
                }),
                document_symbol_provider: Some(OneOf::Left(true)),
                document_formatting_provider: Some(OneOf::Left(true)),
                document_range_formatting_provider: Some(OneOf::Left(true)),
                color_provider: Some(ColorProviderCapability::Simple(true)),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
                document_link_provider: Some(DocumentLinkOptions {
                    resolve_provider: Some(false),
                    work_done_progress_options: Default::default(),
                }),
                definition_provider: Some(OneOf::Left(true)),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec!["json.sort".into()],
                    work_done_progress_options: Default::default(),
                }),
                ..ServerCapabilities::default()
            },
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        info!("json-language-server initialized");
        self.client
            .log_message(MessageType::INFO, "JSON Language Server ready")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        info!("json-language-server shutting down");
        Ok(())
    }

    // -- Document sync --

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        debug!("did_open: {}", uri);
        {
            let mut state = self.state.lock().unwrap();
            state.documents.open(
                params.text_document.uri.clone(),
                params.text_document.text,
                params.text_document.version,
            );
        }
        self.validate_and_publish(&uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        debug!("did_change: {}", uri);
        {
            let mut state = self.state.lock().unwrap();
            if let Some(doc) = state.documents.get_mut(&uri) {
                for change in params.content_changes {
                    match change.range {
                        Some(range) => {
                            doc.apply_edit(range, &change.text, params.text_document.version);
                        }
                        None => {
                            doc.replace_full(change.text, params.text_document.version);
                        }
                    }
                }
            }
        }
        self.debounced_validate(&uri).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        debug!("did_save: {}", params.text_document.uri);
        self.validate_and_publish(&params.text_document.uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        debug!("did_close: {}", params.text_document.uri);
        {
            let mut state = self.state.lock().unwrap();
            state.documents.close(&params.text_document.uri);
        }
        self.client
            .publish_diagnostics(params.text_document.uri, Vec::new(), None)
            .await;
    }

    // -- Configuration --

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        debug!("did_change_configuration");
        if let Some(schemas) = params
            .settings
            .as_object()
            .and_then(|o| o.get("json"))
            .and_then(|v| v.as_object())
            .and_then(|o| o.get("schemas"))
            .and_then(|v| v.as_array())
        {
            let associations: Vec<SchemaAssociation> = schemas
                .iter()
                .filter_map(|entry| {
                    let obj = entry.as_object()?;
                    let uri = obj
                        .get("url")
                        .or_else(|| obj.get("uri"))?
                        .as_str()?
                        .to_string();
                    let file_match: Vec<String> = obj
                        .get("fileMatch")?
                        .as_array()?
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                    Some(SchemaAssociation {
                        file_match,
                        uri,
                        schema: None,
                    })
                })
                .collect();

            let mut state = self.state.lock().unwrap();
            state.schemas.set_associations(associations);
        }
    }

    // -- Hover --

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;

        let (offset, inline_schema) = {
            let state = self.state.lock().unwrap();
            let doc = match state.documents.get(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            let offset = doc.offset_of(pos);
            let inline = crate::schema::resolver::extract_schema_property(doc);
            (offset, inline)
        };

        let uri_str = uri.as_str().to_string();
        let schema = self
            .resolve_schema(&uri_str, inline_schema.as_deref())
            .await;

        let state = self.state.lock().unwrap();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return Ok(None),
        };

        Ok(hover::hover(doc, offset, schema.as_ref()))
    }

    // -- Completion --

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;

        let (offset, inline_schema) = {
            let state = self.state.lock().unwrap();
            let doc = match state.documents.get(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            let offset = doc.offset_of(pos);
            let inline = crate::schema::resolver::extract_schema_property(doc);
            (offset, inline)
        };

        let uri_str = uri.as_str().to_string();
        let schema = self
            .resolve_schema(&uri_str, inline_schema.as_deref())
            .await;

        let state = self.state.lock().unwrap();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return Ok(None),
        };

        let items = completion::completions(doc, offset, schema.as_ref());
        if items.is_empty() {
            Ok(None)
        } else {
            Ok(Some(CompletionResponse::Array(items)))
        }
    }

    // -- Document symbols --

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().unwrap();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        Ok(Some(DocumentSymbolResponse::Nested(
            symbols::document_symbols(doc),
        )))
    }

    // -- Formatting --

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().unwrap();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        Ok(Some(formatting::format_document(doc, &params.options)))
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().unwrap();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        Ok(Some(formatting::format_range(
            doc,
            params.range,
            &params.options,
        )))
    }

    // -- Colors --

    async fn document_color(&self, params: DocumentColorParams) -> Result<Vec<ColorInformation>> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().unwrap();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return Ok(Vec::new()),
        };
        Ok(colors::document_colors(doc))
    }

    async fn color_presentation(
        &self,
        params: ColorPresentationParams,
    ) -> Result<Vec<ColorPresentation>> {
        Ok(colors::color_presentations(params.color))
    }

    // -- Folding --

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().unwrap();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        Ok(Some(folding::folding_ranges(doc)))
    }

    // -- Selection ranges --

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().unwrap();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        Ok(Some(selection::selection_ranges(doc, &params.positions)))
    }

    // -- Document links --

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().unwrap();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        Ok(Some(links::document_links(doc)))
    }

    // -- Go to definition --

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;

        let state = self.state.lock().unwrap();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return Ok(None),
        };

        let offset = doc.offset_of(pos);
        match links::find_definition(doc, offset) {
            Some(mut loc) => {
                loc.uri = uri.clone();
                Ok(Some(GotoDefinitionResponse::Scalar(loc)))
            }
            None => Ok(None),
        }
    }

    // -- Execute command (sort) --

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        match params.command.as_str() {
            "json.sort" => {
                let edit = {
                    if let Some(uri_val) = params.arguments.first()
                        && let Some(uri_str) = uri_val.as_str()
                        && let Ok(uri) = Url::parse(uri_str)
                    {
                        let state = self.state.lock().unwrap();
                        if let Some(doc) = state.documents.get(&uri) {
                            let edits = formatting::sort_document(doc);
                            if !edits.is_empty() {
                                let mut changes = std::collections::HashMap::new();
                                changes.insert(uri, edits);
                                Some(WorkspaceEdit {
                                    changes: Some(changes),
                                    ..Default::default()
                                })
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };
                if let Some(edit) = edit {
                    let _ = self.client.apply_edit(edit).await;
                }
                Ok(None)
            }
            _ => {
                warn!("unknown command: {}", params.command);
                Ok(None)
            }
        }
    }
}
