use lsp_server::Connection;
use tracing::info;
use tracing_subscriber::EnvFilter;

use light_json_lsp::server::JsonLanguageServer;

fn main() {
    // Initialize logging.  Set RUST_LOG=debug for verbose output.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("light_json_lsp=info")),
        )
        .with_writer(std::io::stderr)
        .init();

    info!(
        "json-language-server v{} starting",
        env!("CARGO_PKG_VERSION")
    );

    let (connection, io_threads) = Connection::stdio();

    let server = JsonLanguageServer::new(connection);
    server.run();
    drop(server);

    io_threads.join().unwrap();
}
