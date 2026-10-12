//! CMSのサーバー（`cms-server`）。
//!
//! - `cms-server init`: 秘密の値と暗号化の鍵を生成し、設定ファイルと鍵のファイルを書き出す（Q172）
//! - `cms-server serve`: 起動時の検査を行い、通ればHTTPの受け付けを始める
//! - `cms-server create-server-admin`: サーバー管理者を作る（サーバー上でだけ行える。Q100）

use cms_server::{auth, config, db, http, keys, migrate, startup, worker};

use std::io::{BufRead as _, Write as _};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use clap::{Parser, Subcommand};

use config::{Config, DatabaseConfig};

#[derive(Parser)]
#[command(name = "cms-server", version, about = "Headless CMS server")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate secrets and the encryption key, and write the config file.
    Init {
        /// Path of the config file to create.
        #[arg(long)]
        config: PathBuf,
        /// Path of the encryption key file to create. Keep it apart from database backups.
        #[arg(long)]
        key_file: PathBuf,
        /// Public HTTPS URL of the CMS.
        #[arg(long)]
        public_url: String,
        /// URL of the everyday database connection (without password).
        #[arg(long)]
        database_url: String,
        /// URL of the migration / role-creation connection (without password).
        #[arg(long)]
        migration_database_url: String,
        /// Address to listen on.
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: String,
    },
    /// Check the configuration and start serving.
    Serve {
        #[arg(long)]
        config: PathBuf,
    },
    /// Create a server administrator account.
    ///
    /// By default a strong password is generated and printed once.
    /// With --password-stdin, the password is read from the first line of standard input.
    CreateServerAdmin {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        email: String,
        /// Read the password from standard input instead of generating one.
        #[arg(long)]
        password_stdin: bool,
    },
}

/// 起動時の検査に通らなかったときの終了コード。
const EXIT_CONFIG: u8 = 2;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Init {
            config,
            key_file,
            public_url,
            database_url,
            migration_database_url,
            listen,
        } => {
            match init(
                &config,
                &key_file,
                public_url,
                database_url,
                migration_database_url,
                listen,
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e:#}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Serve { config } => serve_main(&config),
        Command::CreateServerAdmin {
            config,
            email,
            password_stdin,
        } => create_server_admin_main(&config, &email, password_stdin),
    }
}

fn random_secret() -> String {
    let mut bytes = [0u8; 24];
    rand::fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// 新しいファイルを作る（既にあれば上書きしない）。所有者だけが読める権限にする。
fn create_new_private(path: &Path, contents: &str) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("cannot create {} (it may already exist)", path.display()))?;
    file.write_all(contents.as_bytes())?;
    Ok(())
}

fn init(
    config_path: &Path,
    key_path: &Path,
    public_url: String,
    database_url: String,
    migration_database_url: String,
    listen: String,
) -> Result<()> {
    if config_path.exists() || key_path.exists() {
        bail!("refusing to overwrite an existing config or key file");
    }
    let config = Config {
        production: true,
        dev_mode: false,
        public_url,
        listen,
        log_level: "info".to_owned(),
        encryption_key_file: Some(key_path.to_path_buf()),
        database: DatabaseConfig {
            url: database_url,
            password: random_secret(),
        },
        migration_database: DatabaseConfig {
            url: migration_database_url,
            password: random_secret(),
        },
        trusted_proxies: Vec::new(),
        session: config::SessionConfig::default(),
        limits: config::LimitsConfig::default(),
        smtp: None,
    };
    let report = startup::check_config(&Config {
        encryption_key_file: None,
        ..config.clone()
    });
    if let Some(p) = report.problems.iter().find(|p| p.message.contains(".url")) {
        bail!("{}", p.message);
    }
    create_new_private(key_path, &keys::generate_key_file())?;
    let toml = toml::to_string_pretty(&config).context("cannot serialize config")?;
    create_new_private(
        config_path,
        &format!("# Generated by `cms-server init`.\n{toml}"),
    )?;
    println!("Wrote config to {}", config_path.display());
    println!("Wrote encryption key to {}", key_path.display());
    println!(
        "Store the encryption key separately from database backups. Without it, stored secrets cannot be read."
    );
    println!(
        "Set the generated database passwords on the PostgreSQL roles before starting the server."
    );
    Ok(())
}

/// 設定を読み、起動時の検査を通ったら `body` を実行する。
fn with_checked_config<F, Fut>(config_path: &Path, what: &str, body: F) -> ExitCode
where
    F: FnOnce(Config) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let config = match Config::load(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e:#}");
            return ExitCode::from(EXIT_CONFIG);
        }
    };
    init_logging(&config.log_level);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: cannot start runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async move {
        if !run_startup_checks(&config).await {
            return ExitCode::from(EXIT_CONFIG);
        }
        match body(config).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                tracing::error!("{what}: {e:#}");
                ExitCode::FAILURE
            }
        }
    })
}

fn serve_main(config_path: &Path) -> ExitCode {
    with_checked_config(config_path, "server stopped", |config| async move {
        serve(config).await
    })
}

fn create_server_admin_main(config_path: &Path, email: &str, password_stdin: bool) -> ExitCode {
    let Some(email) = auth::users::normalize_email(email) else {
        eprintln!("error: invalid email address");
        return ExitCode::FAILURE;
    };
    let (password, generated) = if password_stdin {
        let mut line = String::new();
        if let Err(e) = std::io::stdin().lock().read_line(&mut line) {
            eprintln!("error: cannot read password from stdin: {e}");
            return ExitCode::FAILURE;
        }
        (line.trim_end_matches(['\r', '\n']).to_owned(), false)
    } else {
        (random_secret(), true)
    };
    if let Err(weakness) = auth::password::check_policy(&password) {
        eprintln!("error: {}", weakness.message());
        return ExitCode::FAILURE;
    }
    with_checked_config(
        config_path,
        "cannot create server administrator",
        |config| async move {
            let (pool, migration, _) = prepare_database(&config).await?;
            migration.close().await;
            let hash = auth::password::hash(password.clone()).await?;
            let id = auth::users::create(&pool, &email, &hash, true).await?;
            println!("Created server administrator {email} (id {id}).");
            if generated {
                println!("Password (shown only once): {password}");
            }
            println!("Set up multi-factor authentication after the first login.");
            Ok(())
        },
    )
}

fn init_logging(level: &str) {
    let filter = tracing_subscriber::EnvFilter::try_new(format!("cms_server={level},warn"))
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let ansi = std::io::IsTerminal::is_terminal(&std::io::stderr());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(ansi)
        .with_writer(std::io::stderr)
        .init();
}

/// 起動時の検査。止めるべき問題があれば、理由を表示して false を返す。
async fn run_startup_checks(config: &Config) -> bool {
    let mut report = startup::check_config(config);
    report
        .problems
        .extend(startup::check_databases(config).await.problems);
    for p in report.warnings(config.production) {
        tracing::warn!("startup check: {}", p.message);
    }
    let blocking = report.blocking(config.production);
    if blocking.is_empty() {
        return true;
    }
    eprintln!("error: refusing to start:");
    for p in blocking {
        eprintln!("  - {}", p.message);
    }
    false
}

/// 移行と付与は、移行・ロールの作成用の接続で行う（Q173）。
/// 共通のスキーマの移行と権限の付与を行い、(普段用のプール, 移行用のプール, 普段用のロール) を返す。
async fn prepare_database(config: &Config) -> Result<(sqlx::PgPool, sqlx::PgPool, db::Ident)> {
    let migration_pool = db::migration_pool(&config.migration_database)
        .await
        .context("cannot connect with migration_database")?;
    let app_pool = db::app_pool(&config.database)
        .await
        .context("cannot connect with database")?;
    let app_role = db::current_role(&app_pool).await?;
    let applied = migrate::run(&migration_pool).await?;
    if !applied.is_empty() {
        tracing::info!("applied migrations: {applied:?}");
    }
    migrate::apply_grants(&migration_pool, &app_role).await?;
    Ok((app_pool, migration_pool, app_role))
}

async fn serve(config: Config) -> Result<()> {
    let key_path = config
        .encryption_key_file
        .as_deref()
        .context("encryption_key_file is not set")?;
    let ring = keys::KeyRing::load(key_path)?;
    tracing::info!(
        "encryption key loaded (active version v{})",
        ring.active_version()
    );

    let (app_pool, migration_pool, app_role) = prepare_database(&config).await?;
    auth::password::warm_up();
    let listen = config.listen.clone();
    let state = http::AppState::new(app_pool, migration_pool, app_role, config, ring);
    worker::spawn(state.clone());
    let app = http::router(state);
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("cannot listen on {listen}"))?;
    tracing::info!("ready: listening on {}", listener.local_addr()?);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
