use tower_lsp::{LspService, Server};
use tracing::info;
use tracing_subscriber::EnvFilter;

use json_language_server::server::JsonLanguageServer;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Initialize logging.  Set RUST_LOG=debug for verbose output.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("json_language_server=info")),
        )
        .with_writer(std::io::stderr)
        .init();

    info!(
        "json-language-server v{} starting",
        env!("CARGO_PKG_VERSION")
    );

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(JsonLanguageServer::new);

    Server::new(stdin, stdout, socket).serve(service).await;
}
