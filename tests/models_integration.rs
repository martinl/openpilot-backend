mod common;

use connect::{
    controllers::{
        v2::DeviceRegistrationParams,
        ws::{Id, JsonRpcRequest},
    },
    models::{
        _entities::anonlogs::Model as AnonlogModel,
        authorized_users::{AuthorizeParams, Model as AuthUserModel},
        bootlogs::BM,
        device_msg_queues::DMQM,
        devices::DM,
        routes::RM,
        segments::SM,
        users::{OAuthUserParams, UM},
    },
};
use sea_orm::{ActiveModelTrait, ActiveValue};
use serial_test::serial;

#[tokio::test]
async fn mock_mkv_server_put_get_list_delete_roundtrip() {
    let mkv = common::MockMkvServer::start().await;
    let client = reqwest::Client::new();

    let key = "1d3dc3e03047b0c7_2024-02-05--16-22-28--0--qlog.zst";
    let put_resp = client
        .put(format!("{}/{key}", mkv.base_url))
        .body("hello-mkv")
        .send()
        .await
        .unwrap();
    assert_eq!(put_resp.status(), 201);

    let get_resp = client
        .get(format!("{}/{key}", mkv.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(get_resp.status(), 200);
    assert_eq!(get_resp.text().await.unwrap(), "hello-mkv");

    let list_resp: serde_json::Value = client
        .get(format!("{}/1d3dc3e03047b0c7?list", mkv.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list_resp["keys"], serde_json::json!([format!("/{key}")]));

    let del_resp = client
        .delete(format!("{}/{key}", mkv.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(del_resp.status(), 204);

    let missing = client
        .get(format!("{}/{key}", mkv.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}

#[tokio::test]
#[serial]
async fn users_devices_and_authorized_users_crud() {
    let Some(db) = common::connect_fresh_test_db().await else {
        return;
    };

    // Create user via OAuth without email, then backfill email on second call
    let user = UM::with_oauth(
        &db,
        &OAuthUserParams {
            name: "github_100".into(),
            email: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(user.email, None);

    let updated = UM::with_oauth(
        &db,
        &OAuthUserParams {
            name: "github_100".into(),
            email: Some("driver@example.com".into()),
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.id, user.id);
    assert_eq!(updated.email.as_deref(), Some("driver@example.com"));

    assert_eq!(
        UM::find_by_identity(&db, &user.identity.to_string())
            .await
            .unwrap()
            .id,
        user.id
    );
    assert_eq!(
        UM::find_by_name(&db, "github_100").await.unwrap().id,
        user.id
    );
    assert_eq!(
        UM::find_by_email(&db, "driver@example.com")
            .await
            .unwrap()
            .id,
        user.id
    );
    assert_eq!(UM::find_by_id(&db, user.id).await.unwrap().id, user.id);
    assert_eq!(UM::find_all_users(&db).await.len(), 1);

    let guest = UM::with_oauth(
        &db,
        &OAuthUserParams {
            name: "github_200".into(),
            email: Some("guest@example.com".into()),
        },
    )
    .await
    .unwrap();

    // Register device & assign owner
    let dongle_id = "1d3dc3e03047b0c7".to_string();
    let reg_params = DeviceRegistrationParams {
        imei: "111111111111111".into(),
        imei2: "222222222222222".into(),
        serial: "serial-123".into(),
        public_key: "-----BEGIN PUBLIC KEY-----\nTEST\n-----END PUBLIC KEY-----".into(),
        register_token: "tok".into(),
    };
    DM::register_device(&db, reg_params, &dongle_id)
        .await
        .unwrap();

    let device = DM::find_device(&db, &dongle_id).await.unwrap();
    let mut active_dev: connect::models::devices::ActiveModel = device.into();
    active_dev.owner_id = ActiveValue::Set(Some(user.id));
    active_dev.online = ActiveValue::Set(true);
    active_dev.last_athena_ping = ActiveValue::Set(1_700_000_000_000);
    active_dev.update(&db).await.unwrap();

    assert_eq!(DM::find_all_devices(&db).await.len(), 1);
    assert_eq!(DM::find_user_devices(&db, user.id).await.len(), 1);
    assert!(DM::ensure_user_device(&db, user.id, &dongle_id)
        .await
        .is_ok());
    assert!(DM::ensure_user_device(&db, guest.id, &dongle_id)
        .await
        .is_err());

    // Authorize guest user on device
    let auth_params = AuthorizeParams {
        user_id: guest.id,
        device_dongle_id: dongle_id.clone(),
    };
    assert!(!AuthUserModel::is_user_authorized(&db, &auth_params).await);
    AuthUserModel::add_authorization(&db, &auth_params)
        .await
        .unwrap();
    assert!(AuthUserModel::is_user_authorized(&db, &auth_params).await);
    assert!(DM::ensure_user_device(&db, guest.id, &dongle_id)
        .await
        .is_ok());
    assert_eq!(DM::find_user_devices(&db, guest.id).await.len(), 1);

    AuthUserModel::remove_authorization(&db, &auth_params)
        .await
        .unwrap();
    assert!(!AuthUserModel::is_user_authorized(&db, &auth_params).await);

    // Analytics & reset_online
    assert_eq!(
        DM::get_registered_devices(&db, None, None, None)
            .await
            .unwrap(),
        1
    );
    let (regs, actives) = DM::daily_devices_all_time(&db).await.unwrap();
    assert_eq!(regs.len(), 1);
    assert_eq!(actives.len(), 1);
    let (recent_regs, _) = DM::daily_devices_since(&db, 0).await.unwrap();
    assert_eq!(recent_regs.len(), 1);

    DM::reset_online(&db).await.unwrap();
    assert!(!DM::find_device(&db, &dongle_id).await.unwrap().online);
}

#[tokio::test]
#[serial]
async fn routes_segments_bootlogs_anonlogs_and_msg_queues_crud() {
    let Some(db) = common::connect_fresh_test_db().await else {
        return;
    };

    let dongle_id = "aaaaaaaaaaaaaaaa".to_string();
    let reg = DeviceRegistrationParams {
        imei: "333".into(),
        imei2: "444".into(),
        serial: "ser".into(),
        public_key: "pub".into(),
        register_token: "tok".into(),
    };
    DM::register_device(&db, reg, &dongle_id).await.unwrap();

    // Add route & segment
    let fullname = format!("{dongle_id}|2024-02-05--16-22-28");
    let route = RM {
        fullname: fullname.clone(),
        device_dongle_id: dongle_id.clone(),
        url: format!("https://api.example.com/connectdata/{dongle_id}/2024-02-05--16-22-28"),
        length: 12.5,
        is_public: true,
        is_preserved: true,
        start_time_utc_millis: 1_707_150_148_000,
        end_time_utc_millis: 1_707_150_208_000,
        end_lat: 59.437,
        end_lng: 24.7536,
        maxqlog: 0,
        ..Default::default()
    };
    route.add_route_self(&db).await.unwrap();

    assert!(RM::is_public(&db, &fullname).await.unwrap());
    assert_eq!(RM::find_route(&db, &fullname).await.unwrap().length, 12.5);
    assert_eq!(
        RM::find_device_routes(&db, &dongle_id).await.unwrap().len(),
        1
    );
    assert_eq!(
        RM::find_recent_device_routes(&db, &dongle_id, Some(5))
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        RM::find_time_filtered_device_routes(
            &db,
            &dongle_id,
            Some(0),
            Some(i64::MAX),
            Some(10),
            None
        )
        .await
        .unwrap()
        .len(),
        1
    );
    let (total_len, count) = RM::total_length_and_count(&db, &dongle_id).await.unwrap();
    assert_eq!(count, 1);
    assert!((total_len - 12.5).abs() < 1e-4);
    assert_eq!(RM::get_drive_count(&db).await.unwrap(), 1);
    assert!((RM::get_miles(&db).await.unwrap() - 12.5).abs() < 1e-4);
    assert!(!RM::daily_miles_all_time(&db).await.unwrap().is_empty());

    let seg_name = format!("{fullname}--0");
    let seg = SM {
        canonical_name: seg_name.clone(),
        canonical_route_name: fullname.clone(),
        number: 0,
        ..Default::default()
    };
    seg.add_segment_self(&db).await.unwrap();
    assert_eq!(SM::find_one(&db, &seg_name).await.unwrap().number, 0);
    assert_eq!(
        SM::find_segments_by_route(&db, &fullname)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(SM::find_all_segments(&db).await.unwrap().len(), 1);

    SM::delete_segment(&db, &seg_name).await.unwrap();
    RM::delete_route(&db, &fullname).await.unwrap();
    assert_eq!(RM::get_drive_count(&db).await.unwrap(), 0);

    // Bootlogs, anonlogs, device_msg_queues
    BM::add_bootlog(
        &db,
        &dongle_id,
        &"https://api/boot.bz2".to_string(),
        &"https://api/boot.unlog".to_string(),
        &"2024-02-05--16-22-28".to_string(),
    )
    .await
    .unwrap();
    assert_eq!(
        BM::find_device_bootlogs(&db, &dongle_id)
            .await
            .unwrap()
            .len(),
        1
    );

    let anon = AnonlogModel::add_anonlog(&db, "https://api/anon.bz2")
        .await
        .unwrap();
    assert_eq!(anon.url, "https://api/anon.bz2");

    DMQM::insert_msg(
        &db,
        &dongle_id,
        JsonRpcRequest {
            method: "getVersion".into(),
            params: None,
            jsonrpc: "2.0".into(),
            id: Id::Int(1),
        },
    )
    .await
    .unwrap();
    assert!(DMQM::find_latest_msg(&db, &dongle_id)
        .await
        .unwrap()
        .is_some());
    DMQM::delete_all_msgs(&db, &dongle_id).await.unwrap();
    assert!(DMQM::find_latest_msg(&db, &dongle_id)
        .await
        .unwrap()
        .is_none());
}
