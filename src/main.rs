use tracing::error;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let is_repl = args.iter().any(|a| a == "--repl");
    let is_client = args.iter().any(|a| a == "--client");

    if is_repl {
        let mut repl = srrdb::repl::Repl::new();
        repl.run();
        return;
    }

    if is_client {
        let config = srrdb::config::Config::load();
        match srrdb::client::run_client(&config.host, config.port, &config.user, &config.password)
            .await
        {
            Ok(()) => {}
            Err(e) => {
                eprintln!("{e}");
            }
        }
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
