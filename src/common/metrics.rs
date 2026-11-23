use std::sync::OnceLock;
use std::time::Instant;

use axum::{
    extract::{MatchedPath, Request},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

static PROMETHEUS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

const HISTOGRAM_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

pub fn init_prometheus_handle() -> PrometheusHandle {
    PROMETHEUS_HANDLE
        .get_or_init(|| {
            PrometheusBuilder::new()
                .set_buckets(HISTOGRAM_BUCKETS)
                .expect("valid Prometheus histogram buckets")
                .install_recorder()
                .expect("failed to install Prometheus metrics recorder")
        })
        .clone()
}

pub fn metrics_router(handle: PrometheusHandle) -> Router {
    Router::new().route(
        "/metrics",
        get(move || {
            let handle = handle.clone();
            async move {
                (
                    StatusCode::OK,
                    [(
                        header::CONTENT_TYPE,
                        "text/plain; version=0.0.4; charset=utf-8",
                    )],
                    handle.render(),
                )
            }
        }),
    )
}

pub async fn http_metrics_middleware(req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().as_str().to_owned();
    let path = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".to_owned());

    metrics::gauge!("http_requests_in_flight").increment(1.0);
    let response = next.run(req).await;
    metrics::gauge!("http_requests_in_flight").decrement(1.0);

    let status = response.status().as_u16().to_string();
    let elapsed = start.elapsed().as_secs_f64();

    metrics::counter!(
        "http_requests_total",
        "method" => method.clone(),
        "path" => path.clone(),
        "status" => status,
    )
    .increment(1);
    metrics::histogram!(
        "http_request_duration_seconds",
        "method" => method,
        "path" => path,
    )
    .record(elapsed);

    response.into_response()
}

pub fn record_runtime_gauges(
    online_devices: usize,
    active_uploads: usize,
    active_upload_bytes: u64,
) {
    metrics::gauge!("athena_connected_devices").set(online_devices as f64);
    metrics::gauge!("active_uploads").set(active_uploads as f64);
    metrics::gauge!("active_upload_bytes").set(active_upload_bytes as f64);
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request as HttpRequest, middleware};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn metrics_middleware_and_endpoint_render_prometheus_text() {
        let handle = init_prometheus_handle();
        let app = Router::new()
            .route("/v1/ping", get(|| async { "pong" }))
            .layer(middleware::from_fn(http_metrics_middleware));

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/v1/ping")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        record_runtime_gauges(3, 1, 2048);

        let metrics_resp = metrics_router(handle)
            .oneshot(
                HttpRequest::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(metrics_resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(metrics_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("http_requests_total"));
        assert!(text.contains("http_request_duration_seconds_bucket"));
        assert!(text.contains("/v1/ping"));
        assert!(text.contains("athena_connected_devices 3"));
        assert!(text.contains("active_uploads 1"));
        assert!(text.contains("active_upload_bytes 2048"));
    }
}
