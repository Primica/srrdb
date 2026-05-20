use tracing::error;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let is_repl = args.iter().any(|a| a == "--repl");

    if is_repl {
        let mut repl = srrdb::repl::Repl::new();
        repl.run();
        return;
    }

    let config = srrdb::config::Config::load();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .parse_lossy(&config.log_level),
        )
        .init();

    match srrdb::server::listener::start(config).await {
        Ok(()) => {}
        Err(e) => {
            error!("Server error: {e}");
        }
    }
}
