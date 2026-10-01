//! `sanad-kernel` binary: configuration from the environment, Postgres or
//! in-memory device store, graceful shutdown.

use std::process::ExitCode;
use std::sync::Arc;

use sanad_kernel::config::KernelConfig;
use sanad_kernel::devices::DeviceStore;
use sanad_kernel::{Kernel, router};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=warn")),
        )
        .json()
        .init();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "kernel failed");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = KernelConfig::from_env()?;
    if config.token_key.is_none() || config.nonce_key.is_none() {
        tracing::warn!(
            "using ephemeral token/nonce keys: development only, tokens die with this process"
        );
    }
    let devices = if let Some(url) = &config.database_url {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(16)
            .connect(url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        DeviceStore::Postgres(pool)
    } else {
        tracing::warn!("no DATABASE_URL: in-memory device store (development only)");
        DeviceStore::memory()
    };
    let bind = config.bind;
    let origin = config.public_origin.to_string();
    let kernel = Arc::new(Kernel::new(config, devices)?);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    // Machine-readable readiness line (tests and process supervisors parse it).
    println!(
        "sanad-kernel listening on {} (public origin {origin})",
        listener.local_addr()?
    );
    axum::serve(listener, router(kernel))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { () = ctrl_c => {}, () = term => {} }
}
