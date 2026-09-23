//! `beebox` — the daemon.
//!
//! The desktop shell spawns this as a child process; run on its own it serves
//! machines you only reach over SSH. Same core either way.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use beebox_core::app::App;
use beebox_core::http;
use beebox_core::ring::DEFAULT_MAX_LINES;
use beebox_core::store::Store;

#[derive(Parser)]
#[command(name = "beebox", about = "Web terminal for parallel agent CLIs")]
struct Args {
    /// Where to listen. Defaults to every interface: how the port is reached —
    /// Tailscale, a reverse proxy, plain LAN — is the user's decision, and
    /// hard-coding a range would just be in the way.
    #[arg(long, default_value = "0.0.0.0:17788")]
    listen: SocketAddr,

    /// State directory. Holds the layout database.
    #[arg(long)]
    home: Option<PathBuf>,

    /// Scrollback per pane. Generous by default: an agent working through a
    /// long task emits tens of thousands of lines, and being unable to scroll
    /// back to what it did an hour ago is a real failure.
    #[arg(long, default_value_t = DEFAULT_MAX_LINES)]
    scrollback: usize,

    /// Serve the UI from disk instead of the embedded copy. For development.
    #[arg(long)]
    ui: Option<PathBuf>,

    /// Exit when the process that spawned this one does.
    ///
    /// The desktop shell owns its daemon's lifetime, but it cannot clean up if
    /// it is killed outright — and a surviving daemon would keep the port and
    /// the terminals. Off by default: a daemon started from a shell is meant to
    /// outlive that shell.
    #[arg(long)]
    exit_with_parent: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "beebox_core=info".into()),
        )
        .init();

    let args = Args::parse();

    let home = args.home.unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".beebox")
    });
    let store = Store::open(&home.join("state.db"))?;

    let app = App::new_with_adapters(store, args.scrollback, &home);
    // PTYs get the hook URL in their environment, so the port must be known
    // before the first spawn — which bootstrap/resume below trigger.
    app.set_hook_port(args.listen.port());
    // Cleans up ghost workspaces but opens none: a fresh install comes up empty
    // and the UI prompts the owner to choose a folder.
    app.bootstrap().await?;
    // Restored panes come back without a process; give each one a shell before
    // anyone can type into it.
    app.resume_all().await?;

    // The pane footer's cwd/git follows the shell wherever it goes.
    tokio::spawn(beebox_core::cwd::poll_forever(app.clone()));
    // A pane whose shell exited cleanly closes itself. One watcher for the
    // whole daemon; doing it per connection would repeat the work per viewer.
    tokio::spawn(app.clone().watch_exits());

    if args.exit_with_parent {
        // Orphaned processes are reparented to launchd, so a ppid of 1 means
        // whoever started us is gone.
        tokio::spawn(async {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                // SAFETY: getppid takes no arguments and cannot fail.
                if unsafe { libc::getppid() } == 1 {
                    // No log line: the parent held our stdout and stderr, so
                    // both are broken pipes now. tracing's fallback report to
                    // stderr panics, the panic kills this task, and the daemon
                    // lived on as an orphan holding its port and terminals.
                    std::process::exit(0);
                }
            }
        });
    }

    let router = http::router(app.clone(), args.ui);
    let listener = tokio::net::TcpListener::bind(args.listen).await?;

    // The key is what separates "reachable" from "authorised". Printing it in
    // the URL is the whole handshake: whoever can read this terminal is the
    // owner, and nobody else gets in.
    let key = app.owner_key().to_string();
    tracing::info!("listening on {}", args.listen);
    tracing::info!("");
    tracing::info!("  open:  http://localhost:{}/?key={key}", args.listen.port());
    for addr in local_addrs(args.listen.port()) {
        tracing::info!("         {addr}/?key={key}");
    }
    tracing::info!("");
    tracing::info!("  the key grants full access — share links instead of this URL");

    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await?;

    Ok(())
}

/// Prints every address the port can be reached on, so the user can pick one
/// to share without hunting for `ifconfig`.
fn local_addrs(port: u16) -> Vec<String> {
    let Ok(out) = std::process::Command::new("ifconfig").output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("inet ")?;
            let ip = rest.split_whitespace().next()?;
            (ip != "127.0.0.1").then(|| format!("http://{ip}:{port}"))
        })
        .collect()
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
