use crate::loco_rs::{
    app::{AppContext, Hooks, Initializer},
    boot::{create_app, BootResult, StartMode},
    controller::AppRoutes,
    db::truncate_table,
    environment::Environment,
    storage,
    task::Tasks,
    worker::{AppWorker, Processor},
    Result,
};
use async_trait::async_trait;
use axum::Extension;
use axum_server::tls_rustls::RustlsConfig;
use migration::Migrator;
use reqwest::Client;
use sea_orm::DatabaseConnection;
use std::{
    env,
    path::Path,
    sync::Arc,
    {net::SocketAddr, path::PathBuf},
};
use tokio::time;
use tower_http::normalize_path::NormalizePathLayer;
use tower_layer::Layer;

use crate::{
    common::upload_tracker::UploadTracker,
    controllers,
    controllers::ws::ConnectionManager,
    initializers,
    models::_entities::{devices, users},
    tasks,
    workers::log_helpers::{persist_device_params, persist_param_value_counts},
};

pub struct App {}
#[async_trait]
impl Hooks for App {
    fn app_name() -> &'static str {
        env!("CARGO_CRATE_NAME")
    }

    fn app_version() -> String {
        format!(
            "{} ({})",
            env!("CARGO_PKG_VERSION"),
            option_env!("BUILD_SHA")
                .or(option_env!("GITHUB_SHA"))
                .unwrap_or("dev")
        )
    }

    async fn boot(mode: StartMode, environment: &Environment) -> Result<BootResult> {
        create_app::<Self, Migrator>(mode, environment).await
    }

    async fn initializers(_ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
        Ok(vec![Box::new(
            initializers::view_engine::ViewEngineInitializer,
        )])
    }

    fn routes(ctx: &AppContext) -> AppRoutes {
        if ctx.environment == crate::loco_rs::environment::Environment::Any("connect".to_string()) {
            return AppRoutes::with_default_routes();
        }
        AppRoutes::with_default_routes()
            .add_route(controllers::ws::routes())
            .add_route(controllers::v2::routes())
            .add_route(controllers::useradmin::routes())
            .add_route(controllers::connectincomming::routes())
            .add_route(controllers::connectdata::routes())
            .add_route(controllers::v1::routes())
            .add_route(controllers::maps::routes())
            .add_route(controllers::params::routes())
            .add_route(controllers::stats::routes())
    }

    fn connect_workers<'a>(p: &'a mut Processor, ctx: &'a AppContext) {
        if ctx.environment == crate::loco_rs::environment::Environment::Any("connect".to_string()) {
            return;
        }
        p.register(crate::workers::bootlog_parser::BootlogParserWorker::build(
            ctx,
        ));
        p.register(crate::workers::jpg_extractor::JpgExtractorWorker::build(
            ctx,
        ));
        p.register(crate::workers::log_parser::LogSegmentWorker::build(ctx));
    }

    fn register_tasks(tasks: &mut Tasks) {
        tasks.register(tasks::storage_count::StorageCount);
        tasks.register(tasks::huggingface::Huggingface);
        tasks.register(tasks::deleter::Deleter);
        tasks.register(tasks::cleaner::Cleaner);
        tasks.register(tasks::collect_data::CollectData);
        tasks.register(tasks::seed_from_mkv::SeedFromMkv);
        tasks.register(tasks::seed::SeedData);
    }

    async fn truncate(db: &DatabaseConnection) -> Result<()> {
        truncate_table(db, users::Entity).await?;
        Ok(())
    }

    async fn seed(_db: &DatabaseConnection, _base: &Path) -> Result<()> {
        //db::seed::<users::ActiveModel>(db, &base.join("users.yaml").display().to_string()).await?;
        //db::seed::<notes::ActiveModel>(db, &base.join("notes.yaml").display().to_string()).await?;
        //db::seed::<devices::ActiveModel>(db, &base.join("devices.yaml").display().to_string()).await?;
        //db::seed::<routes::ActiveModel>(db, &base.join("routes.yaml").display().to_string()).await?;
        //db::seed::<segments::ActiveModel>(db, &base.join("segments.yaml").display().to_string()).await?;
        Ok(())
    }

    async fn after_context(ctx: AppContext) -> Result<AppContext> {
        Ok(AppContext {
            storage: {
                storage::drivers::local::new();
                storage::Storage::single(())
            }
            .into(),
            ..ctx
        })
    }

    async fn after_routes(router: axum::Router, ctx: &AppContext) -> Result<axum::Router> {
        let router = router.layer(axum::middleware::from_fn(
            crate::common::metrics::http_metrics_middleware,
        ));
        let router = NormalizePathLayer::trim_trailing_slash().layer(router);
        let router = axum::Router::new().fallback_service(router);

        if ctx.environment == crate::loco_rs::environment::Environment::Any("connect".to_string()) {
            return Ok(router);
        }

        match devices::Model::reset_online(&ctx.db).await {
            Ok(_) => tracing::info!("Reset all devices to offline"),
            Err(e) => tracing::error!("Failed to Reset all devices to offline: {e}"),
        };

        let connection_manager: Arc<ConnectionManager> = ConnectionManager::new();
        let upload_tracker = UploadTracker::new();
        let ping_manager: Arc<ConnectionManager> = connection_manager.clone();
        let metrics_tracker: Arc<UploadTracker> = upload_tracker.clone();
        let db_clone: DatabaseConnection = ctx.db.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(time::Duration::from_secs(10)); // Ping every 10 seconds
            loop {
                interval.tick().await;
                crate::controllers::ws::send_ping_to_all_devices(
                    ping_manager.clone(),
                    &db_clone.clone(),
                )
                .await;
                let online_devices = ping_manager.online_device_count().await;
                let (active_uploads, active_upload_bytes) = metrics_tracker.active_stats();
                crate::common::metrics::record_runtime_gauges(
                    online_devices,
                    active_uploads,
                    active_upload_bytes,
                );
            }
        });

        tokio::spawn({
            let storage = ctx.storage.clone();
            async move {
                let mut interval = time::interval(time::Duration::from_secs(60)); // Save every 60 seconds
                loop {
                    interval.tick().await;
                    persist_param_value_counts(&storage).await.unwrap();
                    persist_device_params(&storage).await.unwrap();
                    tracing::info!("Persisted param value counts");
                }
            }
        });

        //let client = Client::new();
        let client = Client::builder()
            .pool_max_idle_per_host(500)
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();

        let router = router
            .layer(Extension(client))
            .layer(Extension(connection_manager))
            .layer(Extension(upload_tracker));

        Ok(router)
    }

    async fn serve(
        app: axum::Router,
        server_config: crate::loco_rs::boot::ServeParams,
    ) -> Result<()> {
        let metrics_port: u16 = env::var("METRICS_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(9090);
        if metrics_port > 0 {
            let handle = crate::common::metrics::init_prometheus_handle();
            let metrics_addr = SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, metrics_port));
            tracing::info!("Binding Prometheus metrics on {}/metrics", metrics_addr);
            tokio::spawn(async move {
                let metrics_app = crate::common::metrics::metrics_router(handle);
                if let Err(e) = axum_server::bind(metrics_addr)
                    .serve(metrics_app.into_make_service())
                    .await
                {
                    tracing::error!("Metrics server failed: {e}");
                }
            });
        }

        let my_server_config = MyServerConfig {
            http: server_config.port as u16,
            https: (server_config.port + 111) as u16,
            binding: server_config.binding,
        };

        let enable_tls = env::var("ENABLE_INTERNAL_TLS")
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes"))
            .unwrap_or(false);

        let http_addr = SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, my_server_config.http));

        if enable_tls {
            let cert_dir = env::var("TLS_CERT_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("self_signed_certs")
                });
            let config = RustlsConfig::from_pem_file(
                cert_dir.join("cert.pem"),
                cert_dir.join("key.pem"),
            )
            .await
            .expect("Failed to load internal TLS certs (set ENABLE_INTERNAL_TLS=false to disable)");

            let app_clone = app.clone();
            let https_addr =
                SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, my_server_config.https));

            tracing::info!("Binding HTTP on {} and HTTPS on {}", http_addr, https_addr);
            let https_server = tokio::spawn(async move {
                axum_server::bind_rustls(https_addr, config)
                    .serve(app.into_make_service())
                    .await
            });
            let http_server = tokio::spawn(async move {
                axum_server::bind(http_addr)
                    .serve(app_clone.into_make_service())
                    .await
            });

            if let Err(e) = http_server.await {
                eprintln!("HTTP server failed: {}", e);
            }
            if let Err(e) = https_server.await {
                eprintln!("HTTPS server failed: {}", e);
            }
        } else {
            tracing::info!("Binding HTTP on {} (internal TLS disabled)", http_addr);
            if let Err(e) = axum_server::bind(http_addr)
                .serve(app.into_make_service())
                .await
            {
                eprintln!("HTTP server failed: {}", e);
            }
        }

        Ok(())
    }
}

#[derive(Clone)]
#[allow(dead_code)]
struct MyServerConfig {
    http: u16,
    https: u16,
    binding: String,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
        Router,
    };
    use tower::ServiceExt;

    use super::*;
    use crate::loco_rs::{
        config::{Config, JWTConfig},
        environment::Environment,
        storage::Storage,
    };

    const TEST_SECRET: &str = "AAAAAAAAAAAAAAAAAAAA";

    #[test]
    fn all_controller_routes_register_cleanly() {
        let mut router = axum::Router::new();
        for routes in [
            controllers::ws::routes(),
            controllers::v2::routes(),
            controllers::useradmin::routes(),
            controllers::connectincomming::routes(),
            controllers::connectdata::routes(),
            controllers::v1::routes(),
            controllers::maps::routes(),
            controllers::params::routes(),
            controllers::stats::routes(),
        ] {
            router = router.merge(routes.into_router());
        }
        let _ = router;
    }

    fn test_router() -> Router {
        let ctx = AppContext {
            environment: Environment::Any("useradmin".to_string()),
            db: DatabaseConnection::Disconnected,
            config: Config {
                jwt: JWTConfig {
                    secret: TEST_SECRET.to_string(),
                    expiration: 3600,
                },
                port: 3112,
                binding: "127.0.0.1".to_string(),
            },
            storage: Arc::new(Storage::single(())),
        };
        let router = App::routes(&ctx).into_router(ctx);
        let router = NormalizePathLayer::trim_trailing_slash().layer(router);
        axum::Router::new().fallback_service(router)
    }

    #[tokio::test]
    async fn http_router_rejects_unauthenticated_api_and_redirects_useradmin() {
        let app = test_router();

        let api_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/me")
                    .header(header::HOST, "api.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(api_resp.status(), StatusCode::UNAUTHORIZED);

        let admin_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::HOST, "useradmin.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(admin_resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            admin_resp.headers().get(header::LOCATION).unwrap(),
            "/login"
        );

        let logout_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/auth/logout")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(logout_resp.status(), StatusCode::FOUND);
        assert_eq!(
            logout_resp.headers().get(header::LOCATION).unwrap(),
            "/login"
        );

        let bad_jwt_resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/me")
                    .header(header::HOST, "api.example.com")
                    .header(header::AUTHORIZATION, "JWT not.a.valid.jwt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad_jwt_resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn http_router_returns_404_for_unknown_paths() {
        let app = test_router();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Root-level POST /{dongle_id} (Athena JSON-RPC) is registered and requires auth
        let athena_resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/1d3dc3e03047b0c7")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(athena_resp.status(), StatusCode::UNAUTHORIZED);
    }
}

/*
async fn redirect_http_to_https(my_server_config: MyServerConfig) {
    let config_clone = my_server_config.clone(); // Clone the config for the closure

    fn make_https(host: String, uri: Uri, my_server_config: MyServerConfig) -> Result<Uri, Box<dyn std::error::Error>> {
        let mut parts = uri.into_parts();
        parts.scheme = Some(axum::http::uri::Scheme::HTTPS.try_into()?);

        if parts.path_and_query.is_none() {
            parts.path_and_query = Some("/".parse().unwrap());
        }

        let https_host = host.replace(&my_server_config.http.to_string(), &my_server_config.https.to_string());
        parts.authority = Some(https_host.parse()?);

        Ok(Uri::from_parts(parts)?)
    }

    let redirect = move |Host(host): Host, uri: Uri| async move {
        match make_https(host, uri, config_clone.clone()) {
            Ok(uri) => Ok(Redirect::permanent(&uri.to_string())),
            Err(error) => {
                tracing::warn!(%error, "failed to convert URI to HTTPS");
                Err(StatusCode::BAD_REQUEST)
            }
        }
    };


    let listener = tokio::net::TcpListener::bind(format!("{}:{}", my_server_config.binding, my_server_config.http)).await.unwrap();
    tracing::debug!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, redirect.into_make_service())
        .await
        .unwrap();
}
*/
