mod common;

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
};
use connect::models::{
    devices::DM,
    routes::RM,
    segments::SM,
    users::{OAuthUserParams, UM},
};
use sea_orm::{ActiveModelTrait, ActiveValue};
use serde_json::json;
use serial_test::serial;
use tower::ServiceExt;

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
#[serial]
async fn v1_and_useradmin_endpoints_end_to_end() {
    let Some(db) = common::connect_fresh_test_db().await else {
        return;
    };

    let mkv = common::MockMkvServer::start().await;
    std::env::set_var("MKV_ENDPOINT", &mkv.base_url);
    std::env::set_var("API_ENDPOINT", "https://api.example.com");
    std::env::set_var("ATHENA_ENDPOINT", "https://athena.example.com");
    std::env::set_var("WS_ENDPOINT", "wss://athena.example.com");
    std::env::set_var("FRONTEND_URL", "https://connect.example.com");
    std::env::set_var("USERADMIN_HOST", "useradmin.example.com");
    std::env::set_var("GITHUB_CLIENT", "test-gh-client");

    // Seed owner (superuser for admin view test) + guest user
    let owner = UM::with_oauth(
        &db,
        &OAuthUserParams {
            name: "github_owner".into(),
            email: Some("owner@example.com".into()),
        },
    )
    .await
    .unwrap();
    let mut active_owner: connect::models::users::ActiveModel = owner.into();
    active_owner.superuser = ActiveValue::Set(true);
    let owner = active_owner.update(&db).await.unwrap();

    let guest = UM::with_oauth(
        &db,
        &OAuthUserParams {
            name: "github_guest".into(),
            email: Some("guest@example.com".into()),
        },
    )
    .await
    .unwrap();

    let dongle_id = "1d3dc3e03047b0c7".to_string();
    let device = DM {
        dongle_id: dongle_id.clone(),
        owner_id: Some(owner.id),
        public_key: "pub".into(),
        serial: "ser-1".into(),
        alias: "My C3X".into(),
        uploads_allowed: true,
        prime: true,
        prime_type: 2,
        online: true,
        ..Default::default()
    };
    connect::models::devices::ActiveModel::from(device)
        .insert(&db)
        .await
        .unwrap();

    let fullname = format!("{dongle_id}|2024-02-05--16-22-28");
    let route = RM {
        fullname: fullname.clone(),
        device_dongle_id: dongle_id.clone(),
        url: format!("https://api.example.com/connectdata/{dongle_id}/2024-02-05--16-22-28"),
        length: 5.5,
        is_public: false,
        is_preserved: true,
        start_time_utc_millis: 1_707_150_148_000,
        end_time_utc_millis: 1_707_150_208_000,
        end_lat: 59.437,
        end_lng: 24.7536,
        maxqlog: 0,
        maxqcamera: 0,
        ..Default::default()
    };
    route.add_route_self(&db).await.unwrap();

    let seg = SM {
        canonical_name: format!("{fullname}--0"),
        canonical_route_name: fullname.clone(),
        number: 0,
        qcam_url: format!(
            "{}/{dongle_id}_2024-02-05--16-22-28--0--qcamera.ts",
            mkv.base_url
        ),
        qcam_duration: 60.0,
        ..Default::default()
    };
    seg.add_segment_self(&db).await.unwrap();

    // Put a file in mock MKV so /files lists it
    mkv.store.data.write().unwrap().insert(
        format!("{dongle_id}_2024-02-05--16-22-28--0--qlog.zst"),
        b"qlog-bytes".to_vec(),
    );

    let token = owner.generate_jwt(common::TEST_JWT_SECRET, &3600).unwrap();
    let auth_header = format!("JWT {token}");
    let (_ctx, app) = common::build_test_app(db);

    // GET /v1/me
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/me")
                .header(header::AUTHORIZATION, &auth_header)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let me = body_json(resp).await;
    assert_eq!(me["email"], "owner@example.com");

    // GET /v1/me/devices & /v1/me/jwt & /v1/iceservers
    for uri in ["/v1/me/devices", "/v1/me/jwt", "/v1/iceservers"] {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::AUTHORIZATION, &auth_header)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "failed on {uri}");
    }

    // Device info, stats, location, routes, preserved
    for uri in [
        format!("/v1.1/devices/{dongle_id}"),
        format!("/v1.1/devices/{dongle_id}/stats"),
        format!("/v1/devices/{dongle_id}/location"),
        format!("/v1/devices/{dongle_id}/routes"),
        format!("/v1/devices/{dongle_id}/routes_segments"),
        format!("/v1/devices/{dongle_id}/routes/preserved"),
    ] {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&uri)
                    .header(header::AUTHORIZATION, &auth_header)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "failed on {uri}");
    }

    // PATCH /v1/devices/{dongle_id} (alias)
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/v1/devices/{dongle_id}"))
                .header(header::AUTHORIZATION, &auth_header)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "alias": "Konik A1M" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // POST /v1/devices/{dongle_id}/firehose
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/v1/devices/{dongle_id}/firehose"))
                .header(header::AUTHORIZATION, &auth_header)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "firehose": false }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // Device sharing: add_user -> users -> del_user
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/v1/devices/{dongle_id}/add_user"))
                .header(header::AUTHORIZATION, &auth_header)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "email": guest.email.as_deref().unwrap() }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/devices/{dongle_id}/users"))
                .header(header::AUTHORIZATION, &auth_header)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/v1/devices/{dongle_id}/del_user"))
                .header(header::AUTHORIZATION, &auth_header)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "email": guest.email.as_deref().unwrap() }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // Route info, files, qcamera.m3u8, share_signature
    for uri in [
        format!("/v1/route/{fullname}"),
        format!("/v1/route/{fullname}/files"),
        format!("/v1/route/{fullname}/qcamera.m3u8"),
        format!("/v1/route/{fullname}/share_signature"),
        format!("/v1.4/{dongle_id}/upload_url/?path=2024-02-05--16-22-28--0/qlog.zst"),
    ] {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&uri)
                    .header(header::AUTHORIZATION, &auth_header)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "failed on {uri}");
    }

    // POST /v1/{dongle_id}/upload_urls/
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/v1/{dongle_id}/upload_urls/"))
                .header(header::AUTHORIZATION, &auth_header)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "paths": ["2024-02-05--16-22-28--0/qlog.zst"],
                        "expiry_days": 1
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // PATCH /v1/route/{fullname}
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/v1/route/{fullname}"))
                .header(header::AUTHORIZATION, &auth_header)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "is_public": true }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // Navigation lifecycle: set_destination -> next -> put/get/delete locations -> favorite -> clear next
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/v1/navigation/{dongle_id}/set_destination"))
                .header(header::AUTHORIZATION, &auth_header)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "latitude": 59.437,
                        "longitude": 24.7536,
                        "place_name": "Tallinn",
                        "place_details": "Estonia"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/navigation/{dongle_id}/next"))
                .header(header::AUTHORIZATION, &auth_header)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/navigation/{dongle_id}/locations"))
                .header(header::AUTHORIZATION, &auth_header)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "place_name": "Home",
                        "place_details": "123 Main St",
                        "latitude": 59.437,
                        "longitude": 24.7536,
                        "save_type": "home",
                        "label": "Home"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/navigation/{dongle_id}/locations"))
                .header(header::AUTHORIZATION, &auth_header)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let locs = body_json(r).await;
    let saved_id = locs
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/v1/navigation/{dongle_id}/locations"))
                .header(header::AUTHORIZATION, &auth_header)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "id": saved_id }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/v1/navigation/{dongle_id}/favorite"))
                .header(header::AUTHORIZATION, &auth_header)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "address": "123 Main St",
                        "save_type": "home",
                        "label": "Home"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/v1/navigation/{dongle_id}/next"))
                .header(header::AUTHORIZATION, &auth_header)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // v2 endpoints: pilotauth, user/token, auth/h/redirect, pilotpair error path
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v2/pilotauth?imei=111111111111111&imei2=222222222222222&serial=ser-new&public_key=pub-pilotauth-2&register_token=not-a-jwt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let pilotauth_body = body_json(r).await;
    assert_eq!(pilotauth_body["dongle_id"].as_str().unwrap().len(), 16);

    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v2/user/token")
                .header(header::AUTHORIZATION, &auth_header)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let tok_body = body_json(r).await;
    assert!(!tok_body["access_token"].as_str().unwrap().is_empty());

    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v2/auth/h/redirect?code=abc123&state=service,connect.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FOUND);
    assert_eq!(
        r.headers().get(header::LOCATION).unwrap(),
        "https://connect.example.com/auth/?provider=h&code=abc123"
    );

    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v2/auth/h/redirect?code=abc123&state=service,localhost:3000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FOUND);
    assert_eq!(
        r.headers().get(header::LOCATION).unwrap(),
        "http://localhost:3000/auth/?provider=h&code=abc123"
    );

    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v2/auth/h/redirect?code=abc123")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);

    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v2/pilotpair")
                .header(header::AUTHORIZATION, &auth_header)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("pair_token=not-a-jwt"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);

    // Params endpoints
    for uri in ["/params/?params=NonExistentParam", "/params/metrics"] {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::AUTHORIZATION, &auth_header)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "failed on {uri}");
    }

    // Useradmin & stats HTML views
    for uri in ["/login", "/", "/cloudlogs", "/stats/usage"] {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::AUTHORIZATION, &auth_header)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "failed on {uri}");
    }

    // POST /v1/devices/{dongle_id}/unpair
    let r = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/v1/devices/{dongle_id}/unpair"))
                .header(header::AUTHORIZATION, &auth_header)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}
