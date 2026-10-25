//! CMSのサーバー（`cms-server`）。
//!
//! - `cms-server init`: 秘密の値と暗号化の鍵を生成し、設定ファイルと鍵のファイルを書き出す（Q172）
//! - `cms-server serve`: 起動時の検査を行い、通ればHTTPの受け付けを始める
//! - `cms-server create-server-admin`: サーバー管理者を作る（サーバー上でだけ行える。Q100）
//! - `cms-server reset-password` / `reset-mfa`: 利用者のパスワードを決め直す・多要素認証を解除する。
//!   サーバー管理者の回復は、ネットワーク越しではなくこれで行う（INV-AUTH-10）

use cms_server::{audit, auth, config, db, http, keys, migrate, startup, worker};
use uuid::Uuid;

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
        /// Directory to store media files in.
        #[arg(long, default_value = "/var/lib/cms/media")]
        media_root: PathBuf,
        /// Directory of the built admin UI (`nuxt generate` output) to serve at /admin/.
        #[arg(long)]
        admin_dist_dir: Option<PathBuf>,
    },
    /// Check the configuration and start serving.
    Serve {
        #[arg(long)]
        config: PathBuf,
    },
    /// Set a new password for a user (the way to recover a server administrator).
    ///
    /// By default a strong password is generated and printed once.
    /// With --password-stdin, the password is read from the first line of standard input.
    /// All sessions of the user are revoked.
    ResetPassword {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        email: String,
        #[arg(long)]
        password_stdin: bool,
    },
    /// Remove multi-factor authentication from a user. All sessions of the user are revoked.
    ResetMfa {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        email: String,
    },
    /// Emergency stop: revoke every session and every personal access token of all users.
    ///
    /// Everyone has to sign in again. Run this when sessions or tokens may have leaked.
    RevokeAllSessions {
        #[arg(long)]
        config: PathBuf,
        /// Confirm the operation (required).
        #[arg(long)]
        yes: bool,
    },
    /// Emergency stop: replace the secret of every active delivery API key.
    ///
    /// The old values stop working immediately. The new values are printed once, one key per line
    /// (workspace id, key id, name, new key), tab-separated. Update the sites that use them.
    RotateApiKeys {
        #[arg(long)]
        config: PathBuf,
        /// Only the keys of this workspace.
        #[arg(long)]
        workspace: Option<Uuid>,
        /// Confirm the operation (required).
        #[arg(long)]
        yes: bool,
    },
    /// Key rotation, step 1: add a new encryption key to the key file.
    ///
    /// New secrets are encrypted with the newest key. Restart every server process so that all of them
    /// load the new key, then run `reencrypt-secrets`.
    AddEncryptionKey {
        #[arg(long)]
        config: PathBuf,
    },
    /// Key rotation, step 2: re-encrypt stored secrets with the newest key.
    ///
    /// Safe to run more than once. Prints which key versions are still in use.
    ReencryptSecrets {
        #[arg(long)]
        config: PathBuf,
        /// Confirm the operation (required).
        #[arg(long)]
        yes: bool,
    },
    /// Key rotation, step 3: remove an old encryption key from the key file.
    ///
    /// Refused while stored secrets still use that version. Restart every server process afterwards.
    RemoveEncryptionKey {
        #[arg(long)]
        config: PathBuf,
        /// The version to remove (the number after `v` in the key file).
        #[arg(long)]
        version: u32,
        /// Confirm the operation (required).
        #[arg(long)]
        yes: bool,
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
            media_root,
            admin_dist_dir,
        } => {
            match init(
                &config,
                &key_file,
                InitOptions {
                    public_url,
                    database_url,
                    migration_database_url,
                    listen,
                    media_root,
                    admin_dist_dir,
                },
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
        Command::ResetPassword {
            config,
            email,
            password_stdin,
        } => reset_password_main(&config, &email, password_stdin),
        Command::ResetMfa { config, email } => reset_mfa_main(&config, &email),
        Command::RevokeAllSessions { config, yes } => revoke_all_sessions_main(&config, yes),
        Command::RotateApiKeys {
            config,
            workspace,
            yes,
        } => rotate_api_keys_main(&config, workspace, yes),
        Command::AddEncryptionKey { config } => add_encryption_key_main(&config),
        Command::ReencryptSecrets { config, yes } => reencrypt_secrets_main(&config, yes),
        Command::RemoveEncryptionKey {
            config,
            version,
            yes,
        } => remove_encryption_key_main(&config, version, yes),
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

/// `init` で設定ファイルに書く値。
struct InitOptions {
    public_url: String,
    database_url: String,
    migration_database_url: String,
    listen: String,
    media_root: PathBuf,
    admin_dist_dir: Option<PathBuf>,
}

fn init(config_path: &Path, key_path: &Path, options: InitOptions) -> Result<()> {
    let InitOptions {
        public_url,
        database_url,
        migration_database_url,
        listen,
        media_root,
        admin_dist_dir,
    } = options;
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
        default_locale: cms_server::locale::Locale::default(),
        auth: config::AuthConfig::default(),
        smtp: None,
        login_history: config::LoginHistoryConfig::default(),
        media: config::MediaConfig::at(media_root),
        delivery: config::DeliveryConfig::default(),
        egress: config::EgressConfig::default(),
        admin: config::AdminConfig {
            dist_dir: admin_dist_dir,
        },
        scripts: config::ScriptsConfig::default(),
        audit_export: config::AuditExportConfig::default(),
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
    // 長さは設定を読む前に確かめる（パターンとメールアドレス・サイト名は、設定を読んでから確かめる）
    if let Err(weakness) = auth::password::check_policy(&password, &[]) {
        eprintln!("error: {}", weakness.message());
        return ExitCode::FAILURE;
    }
    with_checked_config(
        config_path,
        "cannot create server administrator",
        |config| async move {
            let words = auth::password::personal_words(&email, &config.public_url);
            let refs: Vec<&str> = words.iter().map(String::as_str).collect();
            if let Err(weakness) = auth::password::check_policy(&password, &refs) {
                bail!("{}", weakness.message());
            }
            let (pool, migration, _) = prepare_database(&config).await?;
            migration.close().await;
            let hash = auth::password::hash(password.clone()).await?;
            let id = auth::users::create(&pool, &email, &hash, true).await?;
            println!("Created server administrator {email} (id {id}).");
            if generated {
                println!("Password (shown only once): {password}");
            }
            println!(
                "Set up multi-factor authentication right after the first login; administrative operations require it."
            );
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
/// パスワードを標準入力から読むか、生成する。(パスワード, 生成したか)
fn read_or_generate_password(password_stdin: bool) -> Result<(String, bool)> {
    if password_stdin {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .context("cannot read password from stdin")?;
        Ok((line.trim_end_matches(['\r', '\n']).to_owned(), false))
    } else {
        Ok((random_secret(), true))
    }
}

/// メールアドレスから利用者のIDを引く（CLI用）。
async fn find_user_for_cli(pool: &sqlx::PgPool, email: &str) -> Result<Uuid> {
    let email = auth::users::normalize_email(email).context("invalid email address")?;
    sqlx::query_scalar("SELECT id FROM cms.users WHERE email = $1")
        .bind(&email)
        .fetch_optional(pool)
        .await?
        .context("no user with this email")
}

fn reset_password_main(config_path: &Path, email: &str, password_stdin: bool) -> ExitCode {
    let (password, generated) = match read_or_generate_password(password_stdin) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    let email = email.to_owned();
    with_checked_config(config_path, "cannot reset password", |config| async move {
        let words = auth::password::personal_words(&email, &config.public_url);
        let refs: Vec<&str> = words.iter().map(String::as_str).collect();
        if let Err(weakness) = auth::password::check_policy(&password, &refs) {
            bail!("{}", weakness.message());
        }
        let (pool, migration, _) = prepare_database(&config).await?;
        migration.close().await;
        let user_id = find_user_for_cli(&pool, &email).await?;
        let hash = auth::password::hash(password.clone()).await?;
        let mut tx = pool.begin().await?;
        auth::users::set_password(&mut *tx, user_id, &hash).await?;
        auth::session::revoke_all(&mut *tx, user_id).await?;
        audit::record(
            &mut *tx,
            None,
            None,
            audit::action::PASSWORD_RESET_COMPLETED,
            &[user_id],
        )
        .await?;
        tx.commit().await?;
        println!("Password reset for {email}. All sessions were revoked.");
        if generated {
            println!("New password (shown only once): {password}");
        }
        Ok(())
    })
}

fn reset_mfa_main(config_path: &Path, email: &str) -> ExitCode {
    let email = email.to_owned();
    with_checked_config(
        config_path,
        "cannot reset multi-factor authentication",
        |config| async move {
            let (pool, migration, _) = prepare_database(&config).await?;
            migration.close().await;
            let user_id = find_user_for_cli(&pool, &email).await?;
            let mut tx = pool.begin().await?;
            auth::mfa::disable(&mut tx, user_id).await?;
            auth::session::revoke_all(&mut *tx, user_id).await?;
            audit::record(
                &mut *tx,
                None,
                None,
                audit::action::MFA_RESET_COMPLETED,
                &[user_id],
            )
            .await?;
            tx.commit().await?;
            println!(
                "Multi-factor authentication was removed for {email}. All sessions were revoked."
            );
            println!("Sign in with the password and set it up again.");
            Ok(())
        },
    )
}

/// 取り消せない（全員に影響する）操作の確認。`--yes` がなければ説明して止める。
fn confirmed(yes: bool, what: &str) -> bool {
    if !yes {
        eprintln!("error: this will {what}. Run again with --yes to confirm.");
    }
    yes
}

fn revoke_all_sessions_main(config_path: &Path, yes: bool) -> ExitCode {
    if !confirmed(
        yes,
        "sign out every user and revoke every personal access token",
    ) {
        return ExitCode::FAILURE;
    }
    with_checked_config(config_path, "cannot revoke sessions", |config| async move {
        let (pool, migration, _) = prepare_database(&config).await?;
        migration.close().await;
        let revoked = cms_server::emergency::revoke_all_sessions_and_tokens(&pool).await?;
        println!(
            "Revoked {} sessions and {} personal access tokens. Everyone has to sign in again.",
            revoked.sessions, revoked.personal_tokens
        );
        Ok(())
    })
}

fn rotate_api_keys_main(config_path: &Path, workspace: Option<Uuid>, yes: bool) -> ExitCode {
    if !confirmed(
        yes,
        "replace every active delivery API key; the old values stop working immediately",
    ) {
        return ExitCode::FAILURE;
    }
    with_checked_config(config_path, "cannot rotate API keys", |config| async move {
        let (pool, migration, _) = prepare_database(&config).await?;
        migration.close().await;
        let rotated = cms_server::emergency::rotate_api_keys(&pool, workspace).await?;
        eprintln!(
            "Rotated {} delivery API keys. The new values are shown only now:",
            rotated.len()
        );
        let mut out = std::io::stdout().lock();
        for k in &rotated {
            // 名前に区切りの文字があっても列がずれないようにする
            let name: String = k
                .name
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            writeln!(
                out,
                "{}\t{}\t{}\t{}",
                k.workspace_id, k.key_id, name, k.secret
            )?;
        }
        Ok(())
    })
}

fn key_file_of(config: &Config) -> Result<PathBuf> {
    config
        .encryption_key_file
        .clone()
        .context("`encryption_key_file` is not set")
}

fn describe_versions(in_use: &std::collections::BTreeMap<u32, u64>) -> String {
    if in_use.is_empty() {
        return "none".to_owned();
    }
    in_use
        .iter()
        .map(|(v, n)| format!("v{v}: {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn add_encryption_key_main(config_path: &Path) -> ExitCode {
    with_checked_config(
        config_path,
        "cannot add an encryption key",
        |config| async move {
            let path = key_file_of(&config)?;
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            let (updated, version) = keys::with_new_key(&text)?;
            keys::replace_key_file(&path, &updated)?;
            let (pool, migration, _) = prepare_database(&config).await?;
            migration.close().await;
            audit::record(&pool, None, None, audit::action::ENCRYPTION_KEY_ADDED, &[]).await?;
            println!("Added encryption key v{version} to {}.", path.display());
            println!(
                "Restart every server process, then run `cms-server reencrypt-secrets --yes`."
            );
            Ok(())
        },
    )
}

fn reencrypt_secrets_main(config_path: &Path, yes: bool) -> ExitCode {
    if !confirmed(
        yes,
        "re-encrypt every stored secret with the newest encryption key",
    ) {
        return ExitCode::FAILURE;
    }
    with_checked_config(
        config_path,
        "cannot re-encrypt secrets",
        |config| async move {
            let ring = keys::KeyRing::load(&key_file_of(&config)?)?;
            let (pool, migration, _) = prepare_database(&config).await?;
            migration.close().await;
            let outcome = cms_server::rekey::reencrypt_all(&pool, &ring).await?;
            audit::record(&pool, None, None, audit::action::SECRETS_REENCRYPTED, &[]).await?;
            let in_use = cms_server::rekey::versions_in_use(&pool).await?;
            println!(
                "Re-encrypted {} secrets with key v{}.",
                outcome.reencrypted,
                ring.active_version()
            );
            println!("Key versions in use: {}.", describe_versions(&in_use));
            if outcome.failed > 0 {
                bail!(
                    "{} secrets could not be decrypted and were left unchanged",
                    outcome.failed
                );
            }
            Ok(())
        },
    )
}

fn remove_encryption_key_main(config_path: &Path, version: u32, yes: bool) -> ExitCode {
    if !confirmed(
        yes,
        &format!(
            "remove encryption key v{version}; secrets encrypted with it can no longer be read"
        ),
    ) {
        return ExitCode::FAILURE;
    }
    with_checked_config(
        config_path,
        "cannot remove the encryption key",
        |config| async move {
            let path = key_file_of(&config)?;
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            let updated = keys::without_version(&text, version)?;
            let (pool, migration, _) = prepare_database(&config).await?;
            migration.close().await;
            let in_use = cms_server::rekey::versions_in_use(&pool).await?;
            if let Some(n) = in_use.get(&version) {
                bail!(
                    "{n} stored secrets still use key v{version}; run `cms-server reencrypt-secrets --yes` first"
                );
            }
            keys::replace_key_file(&path, &updated)?;
            audit::record(
                &pool,
                None,
                None,
                audit::action::ENCRYPTION_KEY_REMOVED,
                &[],
            )
            .await?;
            println!("Removed encryption key v{version} from {}.", path.display());
            println!("Restart every server process.");
            Ok(())
        },
    )
}

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
    cms_server::workspace_db::upgrade_all(&migration_pool).await?;
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

    cms_server::media::store::prepare_root(&config.media.root)?;
    let (app_pool, migration_pool, app_role) = prepare_database(&config).await?;
    auth::password::warm_up();
    let listen = config.listen.clone();
    let state = http::AppState::new(app_pool, migration_pool, app_role, config, ring);
    // 変換結果のない・古い公開中の版を作り直す（Q58）
    if let Err(e) = cms_server::content::rerender::schedule_all(&state.pool, &state.keys).await {
        tracing::error!("cannot queue re-rendering: {e:#}");
    }
    cms_server::http::realtime::spawn_listener(state.clone());
    worker::spawn(state.clone());
    worker::spawn_sweeper(state.clone());
    cms_server::audit_export::spawn(state.clone());
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
