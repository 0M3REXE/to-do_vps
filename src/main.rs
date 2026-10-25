//! # The executable — wiring only
//!
//! Real work lives in the `todo` library. This loads `.env`, sets up
//! tracing, opens the database, applies migrations, builds the router, and
//! serves. The router itself comes from `app::build_app` so that the test suite
//! drives the same routes, session layer and login guard that `cargo run` does.
//!
//! Middleware order is deliberate: the last `.layer()` call is outermost, so
//! CatchPanic sees a panic first and Compression wraps the final response.
//! The session and auth layers are already inside `build_app`, so they sit
//! *below* everything applied here — see the diagram in `src/app.rs`.

use todo::{app, db};
use std::time::Duration;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let addr = std::env::var("ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_owned());
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = app::AppConfig::from_env();
    let pool = connect_db().await;
    let app = app::build_app(pool, config);

    // Nothing below this point can recover, so bind and serve failures log the
    // cause and exit non-zero. A bare `.expect` would print the panic and bury
    // the real error in a backtrace; the io::Error carries the useful detail
    // (address in use, permission denied).
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(err) => {
            tracing::error!(%err, %addr, "failed to bind");
            std::process::exit(1);
        }
    };

    match listener.local_addr() {
        Ok(local) => tracing::info!(%local, "listening"),
        Err(err) => tracing::warn!(?err, "listening, but local_addr failed"),
    }

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let shutdown_signal = async move {
        shutdown_signal().await;
        let _ = shutdown_tx.send(());
    };
    let server = async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
    };
    tokio::pin!(server);
    tokio::pin!(shutdown_signal);

    // Keep serving until the signal arrives. Only then start the deadline for
    // draining in-flight requests; wrapping `server` directly in `timeout`
    // would stop the app 30 seconds after startup.
    let served = tokio::select! {
        result = &mut server => Some(result),
        _ = &mut shutdown_signal => {
            tokio::time::timeout(SHUTDOWN_DEADLINE, &mut server).await.ok()
        }
    };

    match served {
        Some(Ok(())) => {}
        Some(Err(err)) => {
            tracing::error!(?err, "server error");
            std::process::exit(1);
        }
        None => {
            tracing::error!("graceful shutdown exceeded its deadline");
            std::process::exit(1);
        }
    }
}

/// Opens the database named in the environment and applies any pending
/// migrations.
///
/// Migrations are embedded (`src/db.rs`), so a deploy needs no separate
/// migration step. A failure here is fatal: serving reads against a schema the
/// app does not recognise would fail later, less clearly.
async fn connect_db() -> todo::db::Db {
    match db::open_from_env().await {
        Ok(db) => {
            tracing::info!("database ready");
            db
        }
        Err(err) => {
            tracing::error!(?err, "could not open the database");
            std::process::exit(1);
        }
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    if let Err(err) = tokio::signal::ctrl_c().await {
        tracing::error!(?err, "failed to install ctrl-c handler");
        std::future::pending::<()>().await;
    }
    tracing::info!("shutdown signal received, draining");
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let terminate = async {
        match signal(SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(err) => {
                tracing::error!(?err, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate => {}
    }

    tracing::info!("shutdown signal received, draining");
}
