/// LSP server: wires all features together via lsp-server.
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crossbeam_channel::Sender;
use lsp_server::{Connection, ExtractError, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{self, Notification as _};
use lsp_types::request::{self, Request as _};
use lsp_types::*;
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
use crate::tree;

pub struct ServerState {
    pub documents: DocumentStore,
    pub schemas: SchemaStore,
}

/// Debounce delay for validation after edits.
const DEBOUNCE_MS: u64 = 75;

/// Shared state that background threads (debounced validation) can access.
struct Shared {
    state: RwLock<ServerState>,
    regex_cache: Mutex<RegexCache>,
    debounce_versions: Mutex<HashMap<Uri, u64>>,
}

pub struct JsonLanguageServer {
    connection: Connection,
    shared: Arc<Shared>,
}

impl JsonLanguageServer {
    pub fn new(connection: Connection) -> Self {
        JsonLanguageServer {
            connection,
            shared: Arc::new(Shared {
                state: RwLock::new(ServerState {
                    documents: DocumentStore::new(),
                    schemas: SchemaStore::new(),
                }),
                regex_cache: Mutex::new(RegexCache::new()),
                debounce_versions: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Run the server: initialize, then enter the main loop.
    pub fn run(&self) {
        let caps = self.server_capabilities();
        let init_result = InitializeResult {
            server_info: Some(ServerInfo {
                name: "json-language-server".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: caps,
        };
        let init_json = serde_json::to_value(init_result).unwrap();
        let (id, _params) = self.connection.initialize_start().unwrap();

        // Send the initialize response ourselves instead of using
        // initialize_finish(), which blocks until it receives `initialized`.
        // Some clients (and benchmarks) may send `shutdown` before
        // `initialized`, so we handle `initialized` in the main loop instead.
        let resp = Response::new_ok(id, init_json);
        self.connection
            .sender
            .send(Message::Response(resp))
            .unwrap();

        info!("json-language-server initialized");
        self.send_notification::<notification::LogMessage>(LogMessageParams {
            typ: MessageType::INFO,
            message: "JSON Language Server ready".into(),
        });

        self.main_loop();
        info!("json-language-server shutting down");
    }

    fn server_capabilities(&self) -> ServerCapabilities {
        ServerCapabilities {
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
        }
    }

    // -----------------------------------------------------------------------
    // Main loop
    // -----------------------------------------------------------------------

    fn main_loop(&self) {
        for msg in &self.connection.receiver {
            match msg {
                Message::Request(req) => {
                    if self.connection.handle_shutdown(&req).unwrap_or(true) {
                        return;
                    }
                    self.dispatch_request(req);
                }
                Message::Notification(not) => {
                    self.dispatch_notification(not);
                }
                Message::Response(_resp) => {
                    // Responses to our outgoing requests (e.g. workspace/applyEdit).
                }
                Message::PreSerialized(_) => {
                    // PreSerialized messages are outgoing only; never received.
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Request dispatch
    // -----------------------------------------------------------------------

    fn dispatch_request(&self, req: Request) {
        fn cast<R: request::Request>(
            req: Request,
        ) -> Result<(RequestId, R::Params), ExtractError<Request>> {
            req.extract(R::METHOD)
        }

        let req = match cast::<request::HoverRequest>(req) {
            Ok((id, params)) => return self.on_hover(id, params),
            Err(ExtractError::MethodMismatch(req)) => req,
            Err(ExtractError::JsonError { .. }) => return,
        };
        let req = match cast::<request::Completion>(req) {
            Ok((id, params)) => return self.on_completion(id, params),
            Err(ExtractError::MethodMismatch(req)) => req,
            Err(ExtractError::JsonError { .. }) => return,
        };
        let req = match cast::<request::DocumentSymbolRequest>(req) {
            Ok((id, params)) => return self.on_document_symbol(id, params),
            Err(ExtractError::MethodMismatch(req)) => req,
            Err(ExtractError::JsonError { .. }) => return,
        };
        let req = match cast::<request::Formatting>(req) {
            Ok((id, params)) => return self.on_formatting(id, params),
            Err(ExtractError::MethodMismatch(req)) => req,
            Err(ExtractError::JsonError { .. }) => return,
        };
        let req = match cast::<request::RangeFormatting>(req) {
            Ok((id, params)) => return self.on_range_formatting(id, params),
            Err(ExtractError::MethodMismatch(req)) => req,
            Err(ExtractError::JsonError { .. }) => return,
        };
        let req = match cast::<request::DocumentColor>(req) {
            Ok((id, params)) => return self.on_document_color(id, params),
            Err(ExtractError::MethodMismatch(req)) => req,
            Err(ExtractError::JsonError { .. }) => return,
        };
        let req = match cast::<request::ColorPresentationRequest>(req) {
            Ok((id, params)) => return self.on_color_presentation(id, params),
            Err(ExtractError::MethodMismatch(req)) => req,
            Err(ExtractError::JsonError { .. }) => return,
        };
        let req = match cast::<request::FoldingRangeRequest>(req) {
            Ok((id, params)) => return self.on_folding_range(id, params),
            Err(ExtractError::MethodMismatch(req)) => req,
            Err(ExtractError::JsonError { .. }) => return,
        };
        let req = match cast::<request::SelectionRangeRequest>(req) {
            Ok((id, params)) => return self.on_selection_range(id, params),
            Err(ExtractError::MethodMismatch(req)) => req,
            Err(ExtractError::JsonError { .. }) => return,
        };
        let req = match cast::<request::DocumentLinkRequest>(req) {
            Ok((id, params)) => return self.on_document_link(id, params),
            Err(ExtractError::MethodMismatch(req)) => req,
            Err(ExtractError::JsonError { .. }) => return,
        };
        let req = match cast::<request::GotoDefinition>(req) {
            Ok((id, params)) => return self.on_goto_definition(id, params),
            Err(ExtractError::MethodMismatch(req)) => req,
            Err(ExtractError::JsonError { .. }) => return,
        };
        match cast::<request::ExecuteCommand>(req) {
            Ok((id, params)) => return self.on_execute_command(id, params),
            Err(ExtractError::MethodMismatch(_req)) => {}
            Err(ExtractError::JsonError { .. }) => return,
        };
    }

    // -----------------------------------------------------------------------
    // Notification dispatch
    // -----------------------------------------------------------------------

    fn dispatch_notification(&self, not: Notification) {
        fn cast<N: notification::Notification>(
            not: Notification,
        ) -> Result<N::Params, ExtractError<Notification>> {
            not.extract(N::METHOD)
        }

        let not = match cast::<notification::DidOpenTextDocument>(not) {
            Ok(params) => return self.on_did_open(params),
            Err(ExtractError::MethodMismatch(not)) => not,
            Err(ExtractError::JsonError { .. }) => return,
        };
        let not = match cast::<notification::DidChangeTextDocument>(not) {
            Ok(params) => return self.on_did_change(params),
            Err(ExtractError::MethodMismatch(not)) => not,
            Err(ExtractError::JsonError { .. }) => return,
        };
        let not = match cast::<notification::DidSaveTextDocument>(not) {
            Ok(params) => return self.on_did_save(params),
            Err(ExtractError::MethodMismatch(not)) => not,
            Err(ExtractError::JsonError { .. }) => return,
        };
        let not = match cast::<notification::DidCloseTextDocument>(not) {
            Ok(params) => return self.on_did_close(params),
            Err(ExtractError::MethodMismatch(not)) => not,
            Err(ExtractError::JsonError { .. }) => return,
        };
        match cast::<notification::DidChangeConfiguration>(not) {
            Ok(params) => return self.on_did_change_configuration(params),
            Err(ExtractError::MethodMismatch(_not)) => {}
            Err(ExtractError::JsonError { .. }) => return,
        };
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn send_response<T: serde::Serialize>(&self, id: RequestId, result: T) {
        let resp = Response::new_ok(id, result);
        self.connection.sender.send(Message::Response(resp)).ok();
    }

    fn send_notification<N: notification::Notification>(&self, params: N::Params) {
        let not = Notification::new(N::METHOD.into(), params);
        self.connection.sender.send(Message::Notification(not)).ok();
    }

    /// Resolve a schema for a document, fetching over HTTP if needed.
    fn resolve_schema(
        &self,
        doc_uri: &str,
        inline_schema_uri: Option<&str>,
    ) -> Option<Arc<JsonSchema>> {
        let lookup = {
            let mut state = self.shared.state.write();
            state
                .schemas
                .schema_for_document(doc_uri, inline_schema_uri)
        };

        match lookup {
            SchemaLookup::Resolved(schema) => Some(schema),
            SchemaLookup::NeedsFetch(uri) => {
                let agent = self.shared.state.write().schemas.http_agent();
                let raw = resolver::fetch_schema(&agent, &uri)?;
                let schema = JsonSchema::from_value(&raw);
                self.shared
                    .state
                    .write()
                    .schemas
                    .insert_cache(uri, schema.clone());
                Some(schema)
            }
            SchemaLookup::None => None,
        }
    }

    /// Schedule a debounced validation on a background thread.
    fn debounced_validate(&self, uri: Uri) {
        let version = {
            let mut versions = self.shared.debounce_versions.lock();
            let v = versions.entry(uri.clone()).or_insert(0);
            *v += 1;
            *v
        };

        let sender = self.connection.sender.clone();
        let shared = Arc::clone(&self.shared);

        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(DEBOUNCE_MS));

            // Check if a newer edit superseded this one.
            let current = {
                let versions = shared.debounce_versions.lock();
                versions.get(&uri).copied().unwrap_or(0)
            };
            if current != version {
                return;
            }

            validate_and_publish(&uri, &shared.state, &shared.regex_cache, &sender);
        });
    }

    /// Run validation immediately and publish diagnostics.
    fn validate_and_publish(&self, uri: &Uri) {
        validate_and_publish(
            uri,
            &self.shared.state,
            &self.shared.regex_cache,
            &self.connection.sender,
        );
    }

    // -----------------------------------------------------------------------
    // Document sync
    // -----------------------------------------------------------------------

    fn on_did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        debug!("did_open: {}", uri.as_str());
        {
            let mut state = self.shared.state.write();
            state.documents.open(
                params.text_document.uri,
                params.text_document.text,
                params.text_document.version,
            );
        }
        self.validate_and_publish(&uri);
    }

    fn on_did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        debug!("did_change: {}", uri.as_str());
        {
            let mut state = self.shared.state.write();
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
        self.debounced_validate(uri);
    }

    fn on_did_save(&self, params: DidSaveTextDocumentParams) {
        debug!("did_save: {}", params.text_document.uri.as_str());
        self.validate_and_publish(&params.text_document.uri);
    }

    fn on_did_close(&self, params: DidCloseTextDocumentParams) {
        debug!("did_close: {}", params.text_document.uri.as_str());
        {
            let mut state = self.shared.state.write();
            state.documents.close(&params.text_document.uri);
        }
        self.send_notification::<notification::PublishDiagnostics>(PublishDiagnosticsParams {
            uri: params.text_document.uri,
            diagnostics: Vec::new(),
            version: None,
        });
    }

    // -----------------------------------------------------------------------
    // Configuration
    // -----------------------------------------------------------------------

    fn on_did_change_configuration(&self, params: DidChangeConfigurationParams) {
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

            let mut state = self.shared.state.write();
            state.schemas.set_associations(associations);
        }
    }

    // -----------------------------------------------------------------------
    // Hover
    // -----------------------------------------------------------------------

    fn on_hover(&self, id: RequestId, params: HoverParams) {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;

        let (offset, inline_schema) = {
            let state = self.shared.state.read();
            let doc = match state.documents.get(uri) {
                Some(d) => d,
                None => return self.send_response(id, Option::<Hover>::None),
            };
            let offset = doc.offset_of(pos);
            let inline = crate::schema::resolver::extract_schema_property(doc);
            (offset, inline)
        };

        let uri_str = uri.as_str().to_string();
        let schema = self.resolve_schema(&uri_str, inline_schema.as_deref());

        let state = self.shared.state.read();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return self.send_response(id, Option::<Hover>::None),
        };

        let result = hover::hover(doc, offset, schema.as_ref());
        self.send_response(id, result);
    }

    // -----------------------------------------------------------------------
    // Completion
    // -----------------------------------------------------------------------

    fn on_completion(&self, id: RequestId, params: CompletionParams) {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;

        let (offset, inline_schema) = {
            let state = self.shared.state.read();
            let doc = match state.documents.get(uri) {
                Some(d) => d,
                None => return self.send_response(id, Option::<CompletionResponse>::None),
            };
            let offset = doc.offset_of(pos);
            let inline = crate::schema::resolver::extract_schema_property(doc);
            (offset, inline)
        };

        let uri_str = uri.as_str().to_string();
        let schema = self.resolve_schema(&uri_str, inline_schema.as_deref());

        let state = self.shared.state.read();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return self.send_response(id, Option::<CompletionResponse>::None),
        };

        let items = completion::completions(doc, offset, schema.as_ref());
        let result = if items.is_empty() {
            None
        } else {
            Some(CompletionResponse::Array(items))
        };
        self.send_response(id, result);
    }

    // -----------------------------------------------------------------------
    // Document symbols
    // -----------------------------------------------------------------------

    fn on_document_symbol(&self, id: RequestId, params: DocumentSymbolParams) {
        let uri = &params.text_document.uri;
        let state = self.shared.state.read();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return self.send_response(id, Option::<DocumentSymbolResponse>::None),
        };
        // Write symbols directly into the JSON-RPC envelope buffer —
        // avoids a second allocation + memcpy of the entire result.
        let mut buf = Response::start_preserialized(id);
        crate::symbols::write_document_symbols(doc, &mut buf);
        let msg = Response::finish_preserialized(buf);
        self.connection.sender.send(msg).ok();
    }

    // -----------------------------------------------------------------------
    // Formatting
    // -----------------------------------------------------------------------

    fn on_formatting(&self, id: RequestId, params: DocumentFormattingParams) {
        let uri = &params.text_document.uri;
        let state = self.shared.state.read();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return self.send_response(id, Option::<Vec<TextEdit>>::None),
        };
        let result = formatting::format_document(doc, &params.options);
        self.send_response(id, Some(result));
    }

    fn on_range_formatting(&self, id: RequestId, params: DocumentRangeFormattingParams) {
        let uri = &params.text_document.uri;
        let state = self.shared.state.read();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return self.send_response(id, Option::<Vec<TextEdit>>::None),
        };
        let result = formatting::format_range(doc, params.range, &params.options);
        self.send_response(id, Some(result));
    }

    // -----------------------------------------------------------------------
    // Colors
    // -----------------------------------------------------------------------

    fn on_document_color(&self, id: RequestId, params: DocumentColorParams) {
        let uri = &params.text_document.uri;
        let state = self.shared.state.read();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return self.send_response(id, Vec::<ColorInformation>::new()),
        };
        let result = colors::document_colors(doc);
        self.send_response(id, result);
    }

    fn on_color_presentation(&self, id: RequestId, params: ColorPresentationParams) {
        let result = colors::color_presentations(params.color);
        self.send_response(id, result);
    }

    // -----------------------------------------------------------------------
    // Folding
    // -----------------------------------------------------------------------

    fn on_folding_range(&self, id: RequestId, params: FoldingRangeParams) {
        let uri = &params.text_document.uri;
        let state = self.shared.state.read();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return self.send_response(id, Option::<Vec<FoldingRange>>::None),
        };
        let result = folding::folding_ranges(doc);
        self.send_response(id, Some(result));
    }

    // -----------------------------------------------------------------------
    // Selection ranges
    // -----------------------------------------------------------------------

    fn on_selection_range(&self, id: RequestId, params: SelectionRangeParams) {
        let uri = &params.text_document.uri;
        let state = self.shared.state.read();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return self.send_response(id, Option::<Vec<SelectionRange>>::None),
        };
        let result = selection::selection_ranges(doc, &params.positions);
        self.send_response(id, Some(result));
    }

    // -----------------------------------------------------------------------
    // Document links
    // -----------------------------------------------------------------------

    fn on_document_link(&self, id: RequestId, params: DocumentLinkParams) {
        let uri = &params.text_document.uri;
        let state = self.shared.state.read();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return self.send_response(id, Option::<Vec<DocumentLink>>::None),
        };
        let result = links::document_links(doc);
        self.send_response(id, Some(result));
    }

    // -----------------------------------------------------------------------
    // Go to definition
    // -----------------------------------------------------------------------

    fn on_goto_definition(&self, id: RequestId, params: GotoDefinitionParams) {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;

        let state = self.shared.state.read();
        let doc = match state.documents.get(uri) {
            Some(d) => d,
            None => return self.send_response(id, Option::<GotoDefinitionResponse>::None),
        };

        let offset = doc.offset_of(pos);
        let result = match links::find_definition(doc, offset) {
            Some(mut loc) => {
                loc.uri = uri.clone();
                Some(GotoDefinitionResponse::Scalar(loc))
            }
            None => None,
        };
        self.send_response(id, result);
    }

    // -----------------------------------------------------------------------
    // Execute command (sort)
    // -----------------------------------------------------------------------

    fn on_execute_command(&self, id: RequestId, params: ExecuteCommandParams) {
        match params.command.as_str() {
            "json.sort" => {
                let edit = {
                    if let Some(uri_val) = params.arguments.first()
                        && let Some(uri_str) = uri_val.as_str()
                        && let Ok(uri) = Uri::from_str(uri_str)
                    {
                        let state = self.shared.state.read();
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
                    // Send workspace/applyEdit request (fire-and-forget).
                    let params = ApplyWorkspaceEditParams {
                        label: Some("Sort JSON".into()),
                        edit,
                    };
                    let req = Request::new(
                        RequestId::from("apply-edit".to_string()),
                        request::ApplyWorkspaceEdit::METHOD.into(),
                        serde_json::to_value(params).unwrap(),
                    );
                    self.connection.sender.send(Message::Request(req)).ok();
                }
                self.send_response(id, Option::<serde_json::Value>::None);
            }
            _ => {
                warn!("unknown command: {}", params.command);
                self.send_response(id, Option::<serde_json::Value>::None);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Free function for validation (callable from background threads)
// ---------------------------------------------------------------------------

fn validate_and_publish(
    uri: &Uri,
    state: &RwLock<ServerState>,
    regex_cache: &Mutex<RegexCache>,
    sender: &Sender<Message>,
) {
    let (mut diags, version, needs_schema, uri_str, inline_schema) = {
        let state = state.read();

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
        let schema = {
            let lookup = {
                let mut state = state.write();
                state
                    .schemas
                    .schema_for_document(&uri_str, inline_schema.as_deref())
            };

            match lookup {
                SchemaLookup::Resolved(schema) => Some(schema),
                SchemaLookup::NeedsFetch(fetch_uri) => {
                    let agent = state.write().schemas.http_agent();
                    let raw = resolver::fetch_schema(&agent, &fetch_uri);
                    raw.map(|r| {
                        let schema = JsonSchema::from_value(&r);
                        state
                            .write()
                            .schemas
                            .insert_cache(fetch_uri, schema.clone());
                        schema
                    })
                }
                SchemaLookup::None => None,
            }
        };

        let state = state.read();
        let mut regex_cache = regex_cache.lock();
        if let (Some(schema), Some(doc)) = (schema, state.documents.get(uri))
            && let Some(root) = tree::root_value(&doc.tree)
        {
            let val_errors = validation::validate(root, doc.source(), &schema, &mut regex_cache);
            for ve in &val_errors {
                diags.push(lsp_types::Diagnostic {
                    range: doc.range_of(ve.start_byte, ve.end_byte),
                    severity: Some(match ve.severity {
                        validation::Severity::Error => DiagnosticSeverity::ERROR,
                        validation::Severity::Warning => DiagnosticSeverity::WARNING,
                    }),
                    source: Some("json".into()),
                    message: ve.message.clone(),
                    ..lsp_types::Diagnostic::default()
                });
            }
        }
    }

    let params = PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics: diags,
        version,
    };
    let not = Notification::new(
        notification::PublishDiagnostics::METHOD.into(),
        serde_json::to_value(params).unwrap(),
    );
    sender.send(Message::Notification(not)).ok();
}
