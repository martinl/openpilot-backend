#![allow(dead_code)]

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, RwLock},
};

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Extension, Json, Router,
};
use connect::{
    app::App,
    common::upload_tracker::UploadTracker,
    controllers::ws::ConnectionManager,
    loco_rs::{
        app::{AppContext, Hooks},
        config::{Config, JWTConfig},
        controller::views::{engines::TeraView, ViewEngine},
        environment::Environment,
        storage::Storage,
    },
};
use migration::{Migrator, MigratorTrait};
use sea_orm::{Database, DatabaseConnection};
use serde_json::json;
use tokio::net::TcpListener;
use tower_http::normalize_path::NormalizePathLayer;
use tower_layer::Layer;

pub const TEST_JWT_SECRET: &str = "AAAAAAAAAAAAAAAAAAAA";

/// Connects to `TEST_DATABASE_URL` and resets the schema via SeaORM migrations.
/// Returns `None` when `TEST_DATABASE_URL` is unset so offline `cargo test` stays green.
pub async fn connect_fresh_test_db() -> Option<DatabaseConnection> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Database::connect(&url)
        .await
        .expect("failed to connect to TEST_DATABASE_URL");
    Migrator::fresh(&db)
        .await
        .expect("failed to run migrations on TEST_DATABASE_URL");
    Some(db)
}

#[derive(Clone, Default)]
pub struct MockMkvStore {
    pub data: Arc<RwLock<HashMap<String, Vec<u8>>>>,
}

pub struct MockMkvServer {
    pub base_url: String,
    pub store: MockMkvStore,
}

async fn mkv_get_or_list(
    State(store): State<MockMkvStore>,
    Path(key): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let map = store.data.read().unwrap();
    if query.contains_key("list") {
        let mut keys: Vec<String> = map
            .keys()
            .filter(|k| k.starts_with(&key))
            .map(|k| format!("/{k}"))
            .collect();
        keys.sort();
        return (StatusCode::OK, Json(json!({ "keys": keys }))).into_response();
    }
    match map.get(&key) {
        Some(bytes) => (StatusCode::OK, bytes.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn mkv_put(
    State(store): State<MockMkvStore>,
    Path(key): Path<String>,
    body: Bytes,
) -> StatusCode {
    store.data.write().unwrap().insert(key, body.to_vec());
    StatusCode::CREATED
}

async fn mkv_delete(State(store): State<MockMkvStore>, Path(key): Path<String>) -> StatusCode {
    if store.data.write().unwrap().remove(&key).is_some() {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

impl MockMkvServer {
    pub async fn start() -> Self {
        let store = MockMkvStore::default();
        let app = Router::new()
            .route(
                "/{*key}",
                get(mkv_get_or_list).put(mkv_put).delete(mkv_delete),
            )
            .with_state(store.clone());

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock mkv listener");
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Self {
            base_url: format!("http://{addr}"),
            store,
        }
    }
}

pub fn build_test_app(db: DatabaseConnection) -> (AppContext, Router) {
    let ctx = AppContext {
        environment: Environment::Any("useradmin".to_string()),
        db,
        config: Config {
            jwt: JWTConfig {
                secret: TEST_JWT_SECRET.to_string(),
                expiration: 3600,
            },
            port: 3112,
            binding: "127.0.0.1".to_string(),
        },
        storage: Arc::new(Storage::single(())),
    };

    let router = App::routes(&ctx)
        .into_router(ctx.clone())
        .layer(Extension(ViewEngine::from(
            TeraView::build().expect("TeraView::build"),
        )))
        .layer(Extension(reqwest::Client::new()))
        .layer(Extension(ConnectionManager::new()))
        .layer(Extension(UploadTracker::new()));

    let router = NormalizePathLayer::trim_trailing_slash().layer(router);
    let router = Router::new().fallback_service(router);

    (ctx, router)
}
