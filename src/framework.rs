use std::{env, sync::Arc, time::Duration};

use axum::{
    extract::DefaultBodyLimit,
    http::{header, HeaderName, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use sea_orm::{ConnectOptions, Database};
use sea_orm_migration::MigratorTrait;
use tower_http::{
    catch_panic::CatchPanicLayer,
    cors::{AllowOrigin, CorsLayer},
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Bad request: {0}")]
    BadRequest(String),
    #[error("Not found")]
    NotFound,
    #[error(transparent)]
    Model(#[from] model::ModelError),
    #[error(transparent)]
    DB(#[from] sea_orm::DbErr),
    #[error(transparent)]
    JSON(#[from] serde_json::Error),
    #[error(transparent)]
    IO(#[from] std::io::Error),
    #[error(transparent)]
    Tera(#[from] tera::Error),
    #[error(transparent)]
    Storage(#[from] storage::StorageError),
    #[error(transparent)]
    Eyre(#[from] eyre::Report),
    #[error("{0}")]
    Any(#[from] Box<dyn std::error::Error + Send + Sync>),
}

impl Error {
    pub fn string(s: &str) -> Self {
        Self::Message(s.to_string())
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self {
            Self::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg).into_response(),
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
            Self::NotFound | Self::Model(model::ModelError::EntityNotFound) => {
                StatusCode::NOT_FOUND.into_response()
            }
            Self::Model(model::ModelError::EntityAlreadyExists) => {
                StatusCode::CONFLICT.into_response()
            }
            other => {
                tracing::error!(error = %other, "Request error");
                (StatusCode::INTERNAL_SERVER_ERROR, other.to_string()).into_response()
            }
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub mod environment {
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Environment {
        Development,
        Production,
        Test,
        Any(String),
    }

    impl Environment {
        pub fn from_env() -> Self {
            match std::env::var("LOCO_ENV")
                .unwrap_or_else(|_| "useradmin".to_string())
                .as_str()
            {
                "development" => Self::Development,
                "production" => Self::Production,
                "test" => Self::Test,
                other => Self::Any(other.to_string()),
            }
        }
    }
}

pub mod config {
    #[derive(Clone, Debug)]
    pub struct JWTConfig {
        pub secret: String,
        pub expiration: u64,
    }

    #[derive(Clone, Debug)]
    pub struct Config {
        pub jwt: JWTConfig,
        pub port: u16,
        pub binding: String,
    }

    impl Config {
        pub fn from_env(env: &super::environment::Environment) -> Self {
            let default_port =
                if *env == super::environment::Environment::Any("connect".to_string()) {
                    std::env::var("CONNECT_PORT")
                        .ok()
                        .and_then(|p| p.parse().ok())
                        .unwrap_or(3111)
                } else {
                    std::env::var("USERADMIN_PORT")
                        .or_else(|_| std::env::var("PORT"))
                        .ok()
                        .and_then(|p| p.parse().ok())
                        .unwrap_or(3112)
                };

            let secret = std::env::var("SERVER_SECRET")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "4MqUawV3XQUNLNa575gC".to_string());

            Self {
                jwt: JWTConfig {
                    secret,
                    expiration: 7_776_000,
                },
                port: default_port,
                binding: "0.0.0.0".to_string(),
            }
        }

        pub fn get_jwt_config(&self) -> super::Result<&JWTConfig> {
            Ok(&self.jwt)
        }
    }
}

pub mod storage {
    use std::path::Path;

    use bytes::Bytes;

    #[derive(Debug, thiserror::Error)]
    pub enum StorageError {
        #[error(transparent)]
        Io(#[from] std::io::Error),
    }

    pub type StorageResult<T> = std::result::Result<T, StorageError>;

    #[derive(Clone, Default)]
    pub struct Storage;

    impl Storage {
        pub fn single(_driver: ()) -> Self {
            Self
        }

        pub async fn upload(&self, path: &Path, content: &Bytes) -> StorageResult<()> {
            let target = Self::resolve_path(path);
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&target, content).await?;
            Ok(())
        }

        pub async fn download<T: From<Vec<u8>>>(&self, path: &Path) -> StorageResult<T> {
            let target = Self::resolve_path(path);
            let data = tokio::fs::read(&target).await?;
            Ok(T::from(data))
        }

        fn resolve_path(path: &Path) -> std::path::PathBuf {
            if path.is_absolute() {
                path.to_path_buf()
            } else if std::path::Path::new("/params").exists() && path.starts_with("params") {
                std::path::Path::new("/").join(path)
            } else {
                path.to_path_buf()
            }
        }
    }

    pub mod drivers {
        pub mod local {
            pub fn new() {}
        }
    }
}

pub mod model {
    #[derive(Debug, thiserror::Error)]
    pub enum ModelError {
        #[error("Entity already exists")]
        EntityAlreadyExists,
        #[error("Entity not found")]
        EntityNotFound,
        #[error(transparent)]
        DbErr(#[from] sea_orm::DbErr),
        #[error(transparent)]
        Jwt(#[from] jsonwebtoken::errors::Error),
        #[error(transparent)]
        Any(#[from] Box<dyn std::error::Error + Send + Sync>),
    }

    pub type ModelResult<T, E = ModelError> = std::result::Result<T, E>;

    pub trait Validatable {
        fn validator(&self) -> Box<dyn validator::Validate>;
    }

    pub trait ModelValidator: Validatable {
        fn validate(&self) -> std::result::Result<(), sea_orm::DbErr> {
            self.validator()
                .validate()
                .map_err(|e| sea_orm::DbErr::Custom(e.to_string()))
        }
    }

    impl<T: Validatable> ModelValidator for T {}
}

pub mod app {
    use std::{path::Path, sync::Arc};

    use async_trait::async_trait;
    use axum::Router;
    use sea_orm::DatabaseConnection;

    use super::{
        boot::{BootResult, ServeParams, StartMode},
        config::Config,
        controller::AppRoutes,
        environment::Environment,
        storage::Storage,
        task::Tasks,
        worker::Processor,
        Result,
    };

    #[derive(Clone)]
    pub struct AppContext {
        pub environment: Environment,
        pub db: DatabaseConnection,
        pub config: Config,
        pub storage: Arc<Storage>,
    }

    #[async_trait]
    pub trait Initializer: Send + Sync {
        fn name(&self) -> String;

        async fn before_run(&self, _app_context: &AppContext) -> Result<()> {
            Ok(())
        }

        async fn after_routes(&self, router: Router, _ctx: &AppContext) -> Result<Router> {
            Ok(router)
        }
    }

    #[async_trait]
    pub trait Hooks: Send + Sync + Sized + 'static {
        fn app_name() -> &'static str;
        fn app_version() -> String;
        async fn boot(mode: StartMode, environment: &Environment) -> Result<BootResult>;

        async fn initializers(_ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
            Ok(vec![])
        }

        fn routes(ctx: &AppContext) -> AppRoutes;
        fn connect_workers<'a>(p: &'a mut Processor, ctx: &'a AppContext);
        fn register_tasks(tasks: &mut Tasks);
        async fn truncate(db: &DatabaseConnection) -> Result<()>;
        async fn seed(db: &DatabaseConnection, base: &Path) -> Result<()>;

        async fn after_context(ctx: AppContext) -> Result<AppContext> {
            Ok(ctx)
        }

        async fn after_routes(router: Router, _ctx: &AppContext) -> Result<Router> {
            Ok(router)
        }

        async fn serve(app: Router, server_config: ServeParams) -> Result<()>;
    }
}

pub mod controller {
    use std::sync::Arc;

    use axum::{
        extract::FromRequestParts,
        http::request::Parts,
        response::{Html, IntoResponse, Redirect, Response},
        routing::MethodRouter,
        Json, Router,
    };
    use serde::Serialize;

    use super::{app::AppContext, Error, Result};

    pub fn unauthorized<T>(msg: &str) -> Result<T> {
        Err(Error::Unauthorized(msg.to_string()))
    }

    pub fn bad_request<T>(msg: &str) -> Result<T> {
        Err(Error::BadRequest(msg.to_string()))
    }

    pub fn not_found<T>() -> Result<T> {
        Err(Error::NotFound)
    }

    pub mod format {
        use super::*;

        pub fn json<T: Serialize>(t: T) -> Result<Response> {
            Ok(Json(t).into_response())
        }

        pub fn html(content: &str) -> Result<Response> {
            Ok(Html(content.to_string()).into_response())
        }

        pub fn empty() -> Result<Response> {
            Ok(().into_response())
        }

        pub struct RenderBuilder;

        pub fn render() -> RenderBuilder {
            RenderBuilder
        }

        impl RenderBuilder {
            pub fn view<V: super::views::ViewRenderer, S: Serialize>(
                self,
                v: &V,
                key: &str,
                data: S,
            ) -> Result<Response> {
                let content = v.render(key, data)?;
                html(&content)
            }

            pub fn text(self, content: &str) -> Result<Response> {
                Ok(content.to_string().into_response())
            }

            pub fn redirect(self, to: &str) -> Result<Response> {
                Ok(Redirect::to(to).into_response())
            }
        }
    }

    pub mod views {
        use super::*;

        pub trait ViewRenderer {
            fn render<S: Serialize>(&self, key: &str, data: S) -> Result<String>;
        }

        impl<T: ViewRenderer> ViewRenderer for &T {
            fn render<S: Serialize>(&self, key: &str, data: S) -> Result<String> {
                (*self).render(key, data)
            }
        }

        #[derive(Clone)]
        pub struct ViewEngine<E>(pub E);

        impl<E> From<E> for ViewEngine<E> {
            fn from(e: E) -> Self {
                Self(e)
            }
        }

        impl<S, E> FromRequestParts<S> for ViewEngine<E>
        where
            S: Send + Sync,
            E: Clone + Send + Sync + 'static,
        {
            type Rejection = (axum::http::StatusCode, &'static str);

            async fn from_request_parts(
                parts: &mut Parts,
                _state: &S,
            ) -> std::result::Result<Self, Self::Rejection> {
                parts.extensions.get::<Self>().cloned().ok_or((
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "ViewEngine missing",
                ))
            }
        }

        pub mod engines {
            use super::*;

            #[derive(Clone)]
            pub struct TeraView {
                pub tera: Arc<tera::Tera>,
            }

            impl TeraView {
                pub fn build() -> Result<Self> {
                    let mut tera = if std::path::Path::new("assets/views").exists() {
                        tera::Tera::new("assets/views/**/*.html")?
                    } else {
                        tera::Tera::default()
                    };
                    tera.register_function(
                        "t",
                        |_: &std::collections::HashMap<String, tera::Value>| {
                            Ok(tera::Value::String(String::new()))
                        },
                    );
                    Ok(Self {
                        tera: Arc::new(tera),
                    })
                }
            }

            impl ViewRenderer for TeraView {
                fn render<S: Serialize>(&self, key: &str, data: S) -> Result<String> {
                    let context = tera::Context::from_serialize(data)?;
                    Ok(self.tera.render(key, &context)?)
                }
            }
        }
    }

    #[derive(Clone, Default)]
    pub struct Routes {
        prefix: Option<String>,
        router: Router<AppContext>,
    }

    impl Routes {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn at(prefix: &str) -> Self {
            Self {
                prefix: Some(prefix.to_string()),
                router: Router::new(),
            }
        }

        pub fn prefix(mut self, prefix: &str) -> Self {
            self.prefix = Some(prefix.to_string());
            self
        }

        pub fn add(mut self, uri: &str, method_router: MethodRouter<AppContext>) -> Self {
            let path = Self::join_path(self.prefix.as_deref(), uri);
            self.router = self.router.route(&path, method_router);
            self
        }

        pub(super) fn join_path(prefix: Option<&str>, uri: &str) -> String {
            let p = prefix.unwrap_or("").trim_matches('/');
            let u = uri.trim_end_matches('/');
            match (p.is_empty(), u.is_empty()) {
                (true, true) => "/".to_string(),
                (true, false) => {
                    if u.starts_with('/') {
                        u.to_string()
                    } else {
                        format!("/{u}")
                    }
                }
                (false, true) => format!("/{p}"),
                (false, false) => {
                    if u.starts_with('/') || u.starts_with('.') {
                        format!("/{p}{u}")
                    } else {
                        format!("/{p}/{u}")
                    }
                }
            }
        }

        pub fn into_router(self) -> Router<AppContext> {
            self.router
        }
    }

    #[derive(Clone, Default)]
    pub struct AppRoutes {
        routes: Vec<Routes>,
    }

    impl AppRoutes {
        pub fn with_default_routes() -> Self {
            Self::default()
        }

        pub fn empty_routes() -> Self {
            Self::default()
        }

        pub fn add_route(mut self, route: Routes) -> Self {
            self.routes.push(route);
            self
        }

        pub fn into_router(self, ctx: AppContext) -> Router {
            let mut router = Router::new();
            for route in self.routes {
                router = router.merge(route.into_router());
            }
            router.with_state(ctx)
        }
    }
}

pub mod sidekiq {
    #[derive(Debug, thiserror::Error)]
    pub enum Error {
        #[error("{0}")]
        Message(String),
        #[error(transparent)]
        Io(#[from] std::io::Error),
        #[error(transparent)]
        Reqwest(#[from] reqwest::Error),
        #[error(transparent)]
        Db(#[from] sea_orm::DbErr),
        #[error(transparent)]
        Json(#[from] serde_json::Error),
        #[error(transparent)]
        Capnp(#[from] capnp::Error),
        #[error(transparent)]
        Any(#[from] Box<dyn std::error::Error + Send + Sync>),
    }

    pub type Result<T> = std::result::Result<T, Error>;
}

pub mod worker {
    use async_trait::async_trait;
    use serde::{de::DeserializeOwned, Serialize};

    use super::app::AppContext;
    pub use super::sidekiq::{Error, Result};

    #[async_trait]
    pub trait Worker<A>: Send + Sync + 'static {
        async fn perform(&self, args: A) -> Result<()>;
    }

    #[async_trait]
    pub trait AppWorker<A>: Worker<A> + Sized
    where
        A: Send + Serialize + DeserializeOwned + 'static,
    {
        fn build(ctx: &AppContext) -> Self;

        async fn perform_later(ctx: &AppContext, args: A) -> super::Result<()> {
            let worker = Self::build(ctx);
            tokio::spawn(async move {
                if let Err(e) = worker.perform(args).await {
                    tracing::error!(error = %e, "Background worker task failed");
                }
            });
            Ok(())
        }
    }

    #[derive(Default)]
    pub struct Processor;

    impl Processor {
        pub fn register<A, W>(&mut self, _worker: W)
        where
            A: Send + Serialize + DeserializeOwned + 'static,
            W: AppWorker<A>,
        {
        }
    }
}

pub mod task {
    use std::collections::{BTreeMap, HashMap};

    use async_trait::async_trait;

    use super::{app::AppContext, Error, Result};

    pub struct TaskInfo {
        pub name: String,
        pub detail: String,
    }

    #[derive(Default)]
    pub struct Vars {
        pub cli: BTreeMap<String, String>,
    }

    impl Vars {
        pub fn from_args(args: &[String]) -> Self {
            let mut cli = BTreeMap::new();
            for arg in args {
                if let Some((k, v)) = arg.split_once(':').or_else(|| arg.split_once('=')) {
                    cli.insert(k.trim_start_matches('-').to_string(), v.to_string());
                }
            }
            Self { cli }
        }

        pub fn cli_arg(&self, key: &str) -> Result<&String> {
            self.cli
                .get(key)
                .ok_or_else(|| Error::Message(format!("CLI argument '{key}' not provided")))
        }
    }

    #[async_trait]
    pub trait Task: Send + Sync {
        fn task(&self) -> TaskInfo;
        async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()>;
    }

    #[derive(Default)]
    pub struct Tasks {
        tasks: HashMap<String, Box<dyn Task>>,
    }

    impl Tasks {
        pub fn register<T: Task + 'static>(&mut self, task: T) {
            let info = task.task();
            self.tasks.insert(info.name, Box::new(task));
        }

        pub async fn run(&self, name: &str, ctx: &AppContext, vars: &Vars) -> Result<()> {
            let task = self
                .tasks
                .get(name)
                .ok_or_else(|| Error::Message(format!("Task '{name}' not found")))?;
            task.run(ctx, vars).await
        }

        pub fn list(&self) -> Vec<TaskInfo> {
            self.tasks.values().map(|t| t.task()).collect()
        }
    }
}

pub mod db {
    use std::path::Path;

    use sea_orm::{DatabaseConnection, EntityTrait};
    use sea_orm_migration::MigratorTrait;

    use super::{app::Hooks, Result};

    pub async fn truncate_table<E>(db: &DatabaseConnection, entity: E) -> Result<()>
    where
        E: EntityTrait,
    {
        let _ = entity;
        E::delete_many().exec(db).await?;
        Ok(())
    }

    pub async fn reset<M: MigratorTrait>(db: &DatabaseConnection) -> Result<()> {
        M::fresh(db).await?;
        Ok(())
    }

    pub async fn run_app_seed<H: Hooks>(db: &DatabaseConnection, path: &Path) -> Result<()> {
        H::seed(db, path).await
    }
}

pub mod boot {
    use super::*;

    #[derive(Clone, Debug)]
    pub enum StartMode {
        ServerOnly,
        ServerAndWorker,
        WorkerOnly,
    }

    #[derive(Clone, Debug)]
    pub struct ServeParams {
        pub port: i32,
        pub binding: String,
    }

    pub struct BootResult {
        pub app_context: app::AppContext,
        pub router: Option<Router>,
        pub processor: Option<worker::Processor>,
    }

    fn build_cors_layer() -> CorsLayer {
        let mut origins: Vec<HeaderValue> = Vec::new();
        for var in ["FRONTEND_URL", "USERADMIN_URL", "API_ENDPOINT"] {
            if let Ok(val) = env::var(var) {
                let trimmed = val.trim().trim_end_matches('/');
                if let Ok(hv) = HeaderValue::from_str(trimmed) {
                    origins.push(hv);
                }
            }
        }

        let cors = CorsLayer::new()
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers([
                header::AUTHORIZATION,
                header::CONTENT_TYPE,
                HeaderName::from_static("x-requested-with"),
            ])
            .max_age(Duration::from_secs(86400));

        if origins.is_empty() {
            cors.allow_origin(AllowOrigin::mirror_request())
        } else {
            cors.allow_origin(origins)
        }
    }

    pub async fn create_context<H: app::Hooks, M: MigratorTrait>(
        environment: &environment::Environment,
    ) -> Result<app::AppContext> {
        let _ = dotenvy::dotenv();
        let config = config::Config::from_env(environment);
        let db_uri = env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgres://loco:loco@localhost:5432/connect_development".into());
        let max_conns: u32 = env::var("DB_MAX_CONNECTIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30);

        let mut opt = ConnectOptions::new(db_uri);
        opt.max_connections(max_conns)
            .min_connections(1)
            .connect_timeout(Duration::from_secs(10))
            .idle_timeout(Duration::from_secs(10))
            .sqlx_logging(false);

        let db = Database::connect(opt).await?;

        let auto_migrate = env::var("AUTO_MIGRATE")
            .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "false" | "0" | "no"))
            .unwrap_or(true);
        if auto_migrate {
            M::up(&db, None).await?;
        }

        let ctx = app::AppContext {
            environment: environment.clone(),
            db,
            config,
            storage: Arc::new(storage::Storage::single(())),
        };
        H::after_context(ctx).await
    }

    pub async fn create_app<H: app::Hooks, M: MigratorTrait>(
        mode: StartMode,
        environment: &environment::Environment,
    ) -> Result<BootResult> {
        let app_context = create_context::<H, M>(environment).await?;

        let mut processor = worker::Processor;
        H::connect_workers(&mut processor, &app_context);

        if matches!(mode, StartMode::WorkerOnly) {
            return Ok(BootResult {
                app_context,
                router: None,
                processor: Some(processor),
            });
        }

        let initializers = H::initializers(&app_context).await?;
        for init in &initializers {
            init.before_run(&app_context).await?;
        }

        let app_routes = H::routes(&app_context);
        let mut router = app_routes.into_router(app_context.clone());

        if *environment == environment::Environment::Any("connect".to_string()) {
            let static_service = ServeDir::new("frontend/dist")
                .not_found_service(ServeFile::new("frontend/dist/index.html"));
            router = router.fallback_service(static_service);
        }

        for init in &initializers {
            router = init.after_routes(router, &app_context).await?;
        }

        router = H::after_routes(router, &app_context).await?;
        router = router
            .layer(DefaultBodyLimit::max(500 * 1024 * 1024))
            .layer(build_cors_layer())
            .layer(CatchPanicLayer::new())
            .layer(TraceLayer::new_for_http());

        Ok(BootResult {
            app_context,
            router: Some(router),
            processor: Some(processor),
        })
    }
}

pub mod cli {
    use sea_orm_migration::MigratorTrait;
    use tracing_subscriber::{fmt, EnvFilter};

    use super::{
        app::Hooks,
        boot::{create_context, ServeParams, StartMode},
        environment::Environment,
        task::{Tasks, Vars},
        Result,
    };

    fn init_tracing() {
        let default_level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
        let filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
        let _ = fmt().with_env_filter(filter).compact().try_init();
    }

    fn parse_env_arg(args: &[String]) -> Environment {
        for i in 0..args.len() {
            if (args[i] == "--environment" || args[i] == "-e") && i + 1 < args.len() {
                std::env::set_var("LOCO_ENV", &args[i + 1]);
            }
        }
        Environment::from_env()
    }

    pub async fn main<H: Hooks, M: MigratorTrait>() -> Result<()> {
        let _ = dotenvy::dotenv();
        init_tracing();

        let args: Vec<String> = std::env::args().skip(1).collect();
        let environment = parse_env_arg(&args);
        let cmd = args.first().map(String::as_str).unwrap_or("start");

        match cmd {
            "start" => {
                let mode = if args.iter().any(|a| a == "--worker") {
                    StartMode::WorkerOnly
                } else if args.iter().any(|a| a == "--server-and-worker") {
                    StartMode::ServerAndWorker
                } else {
                    StartMode::ServerOnly
                };
                let boot = H::boot(mode, &environment).await?;
                if let Some(router) = boot.router {
                    let serve_params = ServeParams {
                        port: i32::from(boot.app_context.config.port),
                        binding: boot.app_context.config.binding.clone(),
                    };
                    H::serve(router, serve_params).await?;
                } else {
                    std::future::pending::<()>().await;
                }
            }
            "task" => {
                let ctx = create_context::<H, M>(&environment).await?;
                let mut tasks = Tasks::default();
                H::register_tasks(&mut tasks);
                let task_name = args
                    .get(1)
                    .filter(|s| !s.starts_with('-'))
                    .map(String::as_str);
                if let Some(name) = task_name {
                    let vars = Vars::from_args(&args[2..]);
                    tasks.run(name, &ctx, &vars).await?;
                } else {
                    for info in tasks.list() {
                        println!("{:<20} {}", info.name, info.detail);
                    }
                }
            }
            "db" => {
                let sub = args.get(1).map(String::as_str).unwrap_or("migrate");
                let ctx = create_context::<H, M>(&environment).await?;
                match sub {
                    "migrate" | "up" => M::up(&ctx.db, None).await?,
                    "down" => M::down(&ctx.db, Some(1)).await?,
                    "reset" => M::fresh(&ctx.db).await?,
                    "truncate" => H::truncate(&ctx.db).await?,
                    _ => M::up(&ctx.db, None).await?,
                }
            }
            "version" | "--version" | "-V" => {
                println!("{} {}", H::app_name(), H::app_version());
            }
            _ => {
                println!("Usage: connect-cli [start|task <name>|db <migrate|reset>|version]");
            }
        }
        Ok(())
    }
}

pub mod prelude {
    pub use async_trait::async_trait;
    pub use axum::{
        extract::{Form, Path, Query, State},
        http::StatusCode,
        response::{IntoResponse, Response},
        routing::{delete, get, patch, post, put},
        Json,
    };
    pub use axum_extra::extract::cookie;
    pub use sea_orm::{
        ActiveModelBehavior, ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait,
        DatabaseConnection, DbErr, EntityTrait, IntoActiveModel, ModelTrait, QueryFilter, Set,
        TransactionTrait,
    };
    pub use uuid::Uuid;
    pub use validator::Validate;

    pub use super::{
        app::{AppContext, Initializer},
        controller::{
            bad_request, format, not_found, unauthorized,
            views::{engines::TeraView, ViewEngine, ViewRenderer},
            AppRoutes, Routes,
        },
        model::{ModelError, ModelResult, ModelValidator, Validatable},
        sidekiq,
        task::{self, Task, TaskInfo, Tasks},
        worker::{self, AppWorker, Processor, Worker},
        Error, Result,
    };
}

#[cfg(test)]
mod tests {
    use std::env;

    use axum::http::{header, StatusCode};
    use bytes::Bytes;
    use serde::Serialize;
    use serial_test::serial;

    use super::*;

    #[test]
    fn error_into_response_maps_expected_status_codes() {
        assert_eq!(
            Error::Unauthorized("nope".into()).into_response().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            Error::BadRequest("bad".into()).into_response().status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            Error::NotFound.into_response().status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            Error::Model(model::ModelError::EntityNotFound)
                .into_response()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            Error::Model(model::ModelError::EntityAlreadyExists)
                .into_response()
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            Error::string("boom").into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn controller_error_helpers_return_expected_variants() {
        assert!(matches!(
            controller::unauthorized::<()>("denied"),
            Err(Error::Unauthorized(msg)) if msg == "denied"
        ));
        assert!(matches!(
            controller::bad_request::<()>("invalid"),
            Err(Error::BadRequest(msg)) if msg == "invalid"
        ));
        assert!(matches!(
            controller::not_found::<()>(),
            Err(Error::NotFound)
        ));
    }

    #[test]
    fn routes_join_path_normalizes_slashes_and_prefixes() {
        assert_eq!(controller::Routes::join_path(None, ""), "/");
        assert_eq!(controller::Routes::join_path(None, "/"), "/");
        assert_eq!(controller::Routes::join_path(None, "login"), "/login");
        assert_eq!(controller::Routes::join_path(None, "/login/"), "/login");
        assert_eq!(controller::Routes::join_path(Some("v1"), ""), "/v1");
        assert_eq!(controller::Routes::join_path(Some("/v1/"), "/"), "/v1");
        assert_eq!(
            controller::Routes::join_path(Some("v1"), "/me/devices"),
            "/v1/me/devices"
        );
        assert_eq!(
            controller::Routes::join_path(Some("/connectdata/"), "/{id}/coords.json/"),
            "/connectdata/{id}/coords.json"
        );
        assert_eq!(
            controller::Routes::join_path(Some("v1"), ".1/devices/{dongle_id}"),
            "/v1.1/devices/{dongle_id}"
        );
        assert_eq!(
            controller::Routes::join_path(Some("v1"), ".4/{dongleId}/upload_url/"),
            "/v1.4/{dongleId}/upload_url"
        );
    }

    #[test]
    fn format_helpers_produce_expected_responses() {
        let json_resp = controller::format::json(serde_json::json!({ "ok": true })).unwrap();
        assert_eq!(json_resp.status(), StatusCode::OK);
        assert_eq!(
            json_resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );

        let html_resp = controller::format::html("<h1>hi</h1>").unwrap();
        assert_eq!(html_resp.status(), StatusCode::OK);
        assert_eq!(
            html_resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );

        let empty_resp = controller::format::empty().unwrap();
        assert_eq!(empty_resp.status(), StatusCode::OK);

        let text_resp = controller::format::render().text("plain").unwrap();
        assert_eq!(text_resp.status(), StatusCode::OK);

        let redir_resp = controller::format::render().redirect("/login").unwrap();
        assert_eq!(redir_resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            redir_resp.headers().get(header::LOCATION).unwrap(),
            "/login"
        );
    }

    struct StubRenderer;
    impl controller::views::ViewRenderer for StubRenderer {
        fn render<S: Serialize>(&self, key: &str, data: S) -> Result<String> {
            let val = serde_json::to_string(&data)?;
            Ok(format!("{key}:{val}"))
        }
    }

    #[test]
    fn render_view_uses_view_renderer() {
        let resp = controller::format::render()
            .view(&StubRenderer, "login.html", serde_json::json!({ "a": 1 }))
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
    }

    #[test]
    fn task_vars_parses_colon_equals_and_dashed_cli_args() {
        let args = vec![
            "refresh:true".to_string(),
            "--dongle_id=abc123".to_string(),
            "throttle_ms=50".to_string(),
            "ignored_flag".to_string(),
        ];
        let vars = task::Vars::from_args(&args);
        assert_eq!(vars.cli_arg("refresh").unwrap(), "true");
        assert_eq!(vars.cli_arg("dongle_id").unwrap(), "abc123");
        assert_eq!(vars.cli_arg("throttle_ms").unwrap(), "50");
        assert!(vars.cli_arg("missing").is_err());
    }

    #[tokio::test]
    async fn storage_upload_and_download_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("nested/test.bin");
        let storage = storage::Storage::single(());
        let payload = Bytes::from_static(b"openpilot-data");

        storage.upload(&file_path, &payload).await.unwrap();
        let downloaded: Vec<u8> = storage.download(&file_path).await.unwrap();
        assert_eq!(downloaded, payload.as_ref());
    }

    #[test]
    #[serial]
    fn environment_from_env_parses_known_and_custom_values() {
        let prev = env::var("LOCO_ENV").ok();

        env::remove_var("LOCO_ENV");
        assert_eq!(
            environment::Environment::from_env(),
            environment::Environment::Any("useradmin".into())
        );

        env::set_var("LOCO_ENV", "development");
        assert_eq!(
            environment::Environment::from_env(),
            environment::Environment::Development
        );

        env::set_var("LOCO_ENV", "production");
        assert_eq!(
            environment::Environment::from_env(),
            environment::Environment::Production
        );

        env::set_var("LOCO_ENV", "test");
        assert_eq!(
            environment::Environment::from_env(),
            environment::Environment::Test
        );

        env::set_var("LOCO_ENV", "connect");
        assert_eq!(
            environment::Environment::from_env(),
            environment::Environment::Any("connect".into())
        );

        match prev {
            Some(v) => env::set_var("LOCO_ENV", v),
            None => env::remove_var("LOCO_ENV"),
        }
    }

    #[test]
    #[serial]
    fn config_from_env_uses_expected_ports_and_secret() {
        let prev_useradmin = env::var("USERADMIN_PORT").ok();
        let prev_connect = env::var("CONNECT_PORT").ok();
        let prev_port = env::var("PORT").ok();
        let prev_secret = env::var("SERVER_SECRET").ok();

        env::remove_var("USERADMIN_PORT");
        env::remove_var("CONNECT_PORT");
        env::remove_var("PORT");
        env::set_var("SERVER_SECRET", "unit-test-secret");

        let cfg = config::Config::from_env(&environment::Environment::Any("useradmin".into()));
        assert_eq!(cfg.port, 3112);
        assert_eq!(cfg.get_jwt_config().unwrap().secret, "unit-test-secret");

        let connect_cfg =
            config::Config::from_env(&environment::Environment::Any("connect".into()));
        assert_eq!(connect_cfg.port, 3111);

        env::set_var("USERADMIN_PORT", "4000");
        let custom_cfg =
            config::Config::from_env(&environment::Environment::Any("useradmin".into()));
        assert_eq!(custom_cfg.port, 4000);

        match prev_useradmin {
            Some(v) => env::set_var("USERADMIN_PORT", v),
            None => env::remove_var("USERADMIN_PORT"),
        }
        match prev_connect {
            Some(v) => env::set_var("CONNECT_PORT", v),
            None => env::remove_var("CONNECT_PORT"),
        }
        match prev_port {
            Some(v) => env::set_var("PORT", v),
            None => env::remove_var("PORT"),
        }
        match prev_secret {
            Some(v) => env::set_var("SERVER_SECRET", v),
            None => env::remove_var("SERVER_SECRET"),
        }
    }

    struct DummyTask;
    #[async_trait::async_trait]
    impl task::Task for DummyTask {
        fn task(&self) -> task::TaskInfo {
            task::TaskInfo {
                name: "dummy".into(),
                detail: "test task".into(),
            }
        }

        async fn run(&self, _ctx: &app::AppContext, vars: &task::Vars) -> Result<()> {
            let _ = vars.cli_arg("ok")?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn tasks_registry_lists_runs_and_reports_missing_tasks() {
        let mut tasks = task::Tasks::default();
        tasks.register(DummyTask);
        let list = tasks.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "dummy");

        let ctx = app::AppContext {
            environment: environment::Environment::Test,
            db: sea_orm::DatabaseConnection::Disconnected,
            config: config::Config {
                jwt: config::JWTConfig {
                    secret: "AAAAAAAAAAAAAAAAAAAA".into(),
                    expiration: 60,
                },
                port: 3112,
                binding: "127.0.0.1".into(),
            },
            storage: Arc::new(storage::Storage::single(())),
        };

        let ok_vars = task::Vars::from_args(&["ok:1".to_string()]);
        assert!(tasks.run("dummy", &ctx, &ok_vars).await.is_ok());

        let bad_vars = task::Vars::default();
        assert!(tasks.run("dummy", &ctx, &bad_vars).await.is_err());
        assert!(tasks.run("unknown", &ctx, &ok_vars).await.is_err());
    }
}
