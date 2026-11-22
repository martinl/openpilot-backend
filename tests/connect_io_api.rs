mod common;

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
};
use connect::models::{
    devices::DM,
    routes::RM,
    users::{OAuthUserParams, UM},
};
use sea_orm::{ActiveModelTrait, ActiveValue};
use serial_test::serial;
use tower::ServiceExt;

#[tokio::test]
#[serial]
async fn connectdata_downloads_range_and_delete_route_end_to_end() {
    let Some(db) = common::connect_fresh_test_db().await else {
        return;
    };

    let mkv = common::MockMkvServer::start().await;
    std::env::set_var("MKV_ENDPOINT", &mkv.base_url);
    std::env::set_var("API_ENDPOINT", "https://api.example.com");

    let user = UM::with_oauth(
        &db,
        &OAuthUserParams {
            name: "github_io".into(),
            email: Some("io@example.com".into()),
        },
    )
    .await
    .unwrap();

    let dongle_id = "bbbbbbbbbbbbbbbb".to_string();
    let device = DM {
        dongle_id: dongle_id.clone(),
        owner_id: Some(user.id),
        public_key: "pub".into(),
        uploads_allowed: true,
        ..Default::default()
    };
    connect::models::devices::ActiveModel::from(device)
        .insert(&db)
        .await
        .unwrap();

    let ts = "2024-02-05--16-22-28";
    let fullname = format!("{dongle_id}|{ts}");
    let route = RM {
        fullname: fullname.clone(),
        device_dongle_id: dongle_id.clone(),
        url: format!("https://api.example.com/connectdata/{dongle_id}/{ts}"),
        is_public: true,
        ..Default::default()
    };
    route.add_route_self(&db).await.unwrap();

    // Seed mock MKV objects for coords, events, sprite, and qcamera.ts
    {
        let mut store = mkv.store.data.write().unwrap();
        store.insert(
            format!("{dongle_id}_{ts}--0--coords.json"),
            br#"[{"t":0,"lat":59.4,"lng":24.7}]"#.to_vec(),
        );
        store.insert(
            format!("{dongle_id}_{ts}--0--events.json"),
            br#"[]"#.to_vec(),
        );
        store.insert(
            format!("{dongle_id}_{ts}--0--sprite.jpg"),
            vec![0xFF, 0xD8, 0xFF, 0xD9],
        );
        store.insert(
            format!("{dongle_id}_{ts}--0--qcamera.ts"),
            b"0123456789abcdef".to_vec(),
        );
    }

    let token = user.generate_jwt(common::TEST_JWT_SECRET, &3600).unwrap();
    let auth_header = format!("JWT {token}");
    let (_ctx, app) = common::build_test_app(db.clone());

    // Public/derived downloads: coords, events, sprite
    for file in ["coords.json", "events.json", "sprite.jpg"] {
        let uri = format!("/connectdata/{dongle_id}/{ts}/0/{file}");
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

    // Authenticated file download with Range header -> 206 Partial Content
    let range_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/connectdata/{dongle_id}/{ts}/0/qcamera.ts"))
                .header(header::AUTHORIZATION, &auth_header)
                .header(header::RANGE, "bytes=0-3")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(range_resp.status(), StatusCode::PARTIAL_CONTENT);
    let body = to_bytes(range_resp.into_body(), usize::MAX).await.unwrap();
    assert!(!body.is_empty());

    // Cloudlog cache endpoints
    for uri in [
        format!("/connectdata/{dongle_id}/cloudlogs"),
        "/connectdata/cloudlogs/all".to_string(),
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

    // DELETE route -> removes route from DB
    let del_resp = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/connectdata/delete/{dongle_id}/{ts}"))
                .header(header::AUTHORIZATION, &auth_header)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(del_resp.status(), StatusCode::OK);
    assert!(RM::find_route(&db, &fullname).await.is_err());

    // Make sure user wasn't affected
    let mut active_u: connect::models::users::ActiveModel = user.into();
    active_u.points = ActiveValue::Set(1);
    active_u.update(&db).await.unwrap();
}
