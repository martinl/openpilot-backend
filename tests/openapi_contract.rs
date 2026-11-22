//! Contract tests validating `openpilot-backend` responses against the upstream
//! comma.ai OpenAPI 3.1 specification (`commaai/comma-api/openapi.yaml`,
//! converted to `tests/fixtures/comma-openapi.json`).
//!
//! To refresh the fixture:
//! ```bash
//! curl -fsSL https://raw.githubusercontent.com/commaai/comma-api/master/openapi.yaml \
//!   | ruby -ryaml -rjson -e 'puts JSON.pretty_generate(YAML.load(STDIN.read))' \
//!   > tests/fixtures/comma-openapi.json
//! ```

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
use sea_orm::ActiveModelTrait;
use serde_json::{json, Value};
use serial_test::serial;
use tower::ServiceExt;

const OPENAPI_JSON: &str = include_str!("fixtures/comma-openapi.json");

fn load_spec() -> Value {
    serde_json::from_str(OPENAPI_JSON).expect("comma-openapi.json must be valid JSON")
}

fn resolve_ref<'a>(spec: &'a Value, node: &'a Value) -> &'a Value {
    let mut current = node;
    for _ in 0..16 {
        let Some(ref_str) = current.get("$ref").and_then(Value::as_str) else {
            return current;
        };
        let pointer = ref_str
            .strip_prefix('#')
            .unwrap_or_else(|| panic!("only local JSON pointer $ref supported, got {ref_str}"));
        current = spec
            .pointer(pointer)
            .unwrap_or_else(|| panic!("unresolvable $ref: {ref_str}"));
    }
    current
}

fn matches_type(type_name: &str, value: &Value) -> bool {
    match type_name {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "string" => value.is_string(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true,
    }
}

fn validate_schema(
    spec: &Value,
    schema: &Value,
    value: &Value,
    path: &str,
    errors: &mut Vec<String>,
) {
    let schema = resolve_ref(spec, schema);

    if let Some(all_of) = schema.get("allOf").and_then(Value::as_array) {
        for sub in all_of {
            validate_schema(spec, sub, value, path, errors);
        }
    }

    if let Some(any_of) = schema.get("anyOf").and_then(Value::as_array) {
        let any_ok = any_of.iter().any(|sub| {
            let mut sub_errors = Vec::new();
            validate_schema(spec, sub, value, path, &mut sub_errors);
            sub_errors.is_empty()
        });
        if !any_ok {
            errors.push(format!("{path}: value {value} did not match anyOf schema"));
        }
    }

    if let Some(one_of) = schema.get("oneOf").and_then(Value::as_array) {
        let matches = one_of
            .iter()
            .filter(|sub| {
                let mut sub_errors = Vec::new();
                validate_schema(spec, sub, value, path, &mut sub_errors);
                sub_errors.is_empty()
            })
            .count();
        if matches != 1 {
            errors.push(format!(
                "{path}: value {value} matched {matches} oneOf branches (expected 1)"
            ));
        }
    }

    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            errors.push(format!("{path}: value {value} not in enum {allowed:?}"));
        }
    }

    let nullable = schema
        .get("nullable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if value.is_null() && nullable {
        return;
    }

    if let Some(type_val) = schema.get("type") {
        let ok = match type_val {
            Value::String(t) => matches_type(t, value),
            Value::Array(ts) => ts
                .iter()
                .filter_map(Value::as_str)
                .any(|t| matches_type(t, value)),
            _ => true,
        };
        if !ok {
            errors.push(format!(
                "{path}: expected type {type_val}, got value {value}"
            ));
            return;
        }
    }

    if let Some(obj) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for req in required.iter().filter_map(Value::as_str) {
                if !obj.contains_key(req) {
                    errors.push(format!("{path}: missing required property `{req}`"));
                }
            }
        }
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for (k, prop_schema) in props {
                if let Some(prop_val) = obj.get(k) {
                    validate_schema(spec, prop_schema, prop_val, &format!("{path}.{k}"), errors);
                }
            }
        }
    }

    if let Some(arr) = value.as_array() {
        if let Some(item_schema) = schema.get("items") {
            for (idx, item) in arr.iter().enumerate() {
                validate_schema(spec, item_schema, item, &format!("{path}[{idx}]"), errors);
            }
        }
    }
}

fn check_matches_openapi(
    spec: &Value,
    method: &str,
    openapi_path: &str,
    status: StatusCode,
    body: &Value,
) -> Vec<String> {
    let op = spec
        .get("paths")
        .and_then(|p| p.get(openapi_path))
        .and_then(|p| p.get(method))
        .unwrap_or_else(|| panic!("OpenAPI spec missing {method} {openapi_path}"));

    let responses = op
        .get("responses")
        .unwrap_or_else(|| panic!("OpenAPI spec missing responses for {method} {openapi_path}"));

    let status_key = status.as_u16().to_string();
    let resp_obj = responses
        .get(&status_key)
        .or_else(|| responses.get("default"))
        .unwrap_or_else(|| {
            panic!("OpenAPI spec missing status {status_key} for {method} {openapi_path}")
        });
    let resp_obj = resolve_ref(spec, resp_obj);

    let Some(schema) = resp_obj.pointer("/content/application~1json/schema") else {
        return Vec::new();
    };

    let mut errors = Vec::new();
    validate_schema(spec, schema, body, "$", &mut errors);
    errors
}

#[test]
fn openapi_fixture_covers_implemented_comma_endpoints() {
    let spec = load_spec();
    assert_eq!(spec["openapi"], "3.1.0");

    let implemented = [
        ("get", "/v1/me"),
        ("get", "/v1/me/devices"),
        ("get", "/v1.1/devices/{dongleId}"),
        ("patch", "/v1/devices/{dongleId}"),
        ("get", "/v1/devices/{dongleId}/location"),
        ("post", "/v1/devices/{dongleId}/unpair"),
        ("get", "/v1/devices/{dongleId}/users"),
        ("post", "/v1/devices/{dongleId}/add_user"),
        ("post", "/v1/devices/{dongleId}/del_user"),
        ("get", "/v1.1/devices/{dongleId}/stats"),
        ("get", "/v1/devices/{dongleId}/routes"),
        ("get", "/v1/devices/{dongleId}/routes/preserved"),
        ("get", "/v1/devices/{dongleId}/routes_segments"),
        ("get", "/v1.4/{dongleId}/upload_url"),
        ("post", "/v1/{dongleId}/upload_urls"),
        ("post", "/v2/pilotauth"),
        ("post", "/v2/pilotpair"),
        ("get", "/v1/route/{routeName}"),
        ("patch", "/v1/route/{routeName}"),
        ("get", "/v1/route/{routeName}/files"),
        ("get", "/v1/route/{routeName}/qcamera.m3u8"),
        ("get", "/v1/route/{routeName}/share_signature"),
        ("post", "/v1/navigation/{dongleId}/set_destination"),
        ("get", "/v1/navigation/{dongleId}/next"),
        ("delete", "/v1/navigation/{dongleId}/next"),
        ("get", "/v1/navigation/{dongleId}/locations"),
        ("put", "/v1/navigation/{dongleId}/locations"),
        ("delete", "/v1/navigation/{dongleId}/locations"),
    ];

    for (method, path) in implemented {
        assert!(
            spec["paths"][path][method].is_object(),
            "expected {method} {path} in comma-openapi.json"
        );
    }
}

#[tokio::test]
#[serial]
async fn comma_openapi_response_contracts() {
    let Some(db) = common::connect_fresh_test_db().await else {
        return;
    };

    let spec = load_spec();
    let mkv = common::MockMkvServer::start().await;
    std::env::set_var("MKV_ENDPOINT", &mkv.base_url);
    std::env::set_var("API_ENDPOINT", "https://api.example.com");
    std::env::set_var("ATHENA_ENDPOINT", "https://athena.example.com");

    let owner = UM::with_oauth(
        &db,
        &OAuthUserParams {
            name: "github_contract".into(),
            email: Some("contract@example.com".into()),
        },
    )
    .await
    .unwrap();

    let dongle_id = "c0ffee1234567890".to_string();
    let device = DM {
        dongle_id: dongle_id.clone(),
        owner_id: Some(owner.id),
        public_key: "pub-contract".into(),
        serial: "ser-contract".into(),
        alias: "Contract C3X".into(),
        device_type: "three".into(),
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

    mkv.store.data.write().unwrap().insert(
        format!("{dongle_id}_2024-02-05--16-22-28--0--qlog.zst"),
        b"qlog-bytes".to_vec(),
    );

    let token = owner.generate_jwt(common::TEST_JWT_SECRET, &3600).unwrap();
    let auth = format!("JWT {token}");
    let (_ctx, app) = common::build_test_app(db);

    let checks: Vec<(Method, String, &str, Option<Value>)> = vec![
        (Method::GET, "/v1/me".into(), "/v1/me", None),
        (Method::GET, "/v1/me/devices".into(), "/v1/me/devices", None),
        (
            Method::GET,
            format!("/v1.1/devices/{dongle_id}"),
            "/v1.1/devices/{dongleId}",
            None,
        ),
        (
            Method::GET,
            format!("/v1.1/devices/{dongle_id}/stats"),
            "/v1.1/devices/{dongleId}/stats",
            None,
        ),
        (
            Method::GET,
            format!("/v1/devices/{dongle_id}/location"),
            "/v1/devices/{dongleId}/location",
            None,
        ),
        (
            Method::GET,
            format!("/v1/devices/{dongle_id}/users"),
            "/v1/devices/{dongleId}/users",
            None,
        ),
        (
            Method::GET,
            format!("/v1/route/{fullname}/files"),
            "/v1/route/{routeName}/files",
            None,
        ),
        (
            Method::GET,
            format!("/v1/route/{fullname}/share_signature"),
            "/v1/route/{routeName}/share_signature",
            None,
        ),
        (
            Method::GET,
            format!("/v1.4/{dongle_id}/upload_url/?path=2024-02-05--16-22-28--0/qlog.zst"),
            "/v1.4/{dongleId}/upload_url",
            None,
        ),
        (
            Method::POST,
            format!("/v1/{dongle_id}/upload_urls/"),
            "/v1/{dongleId}/upload_urls",
            Some(json!({
                "paths": ["2024-02-05--16-22-28--0/qlog.zst"],
                "expiry_days": 1
            })),
        ),
    ];

    let mut all_violations = Vec::new();
    for (method, uri, openapi_path, payload) in checks {
        let mut req = Request::builder()
            .method(method.clone())
            .uri(&uri)
            .header(header::AUTHORIZATION, &auth);
        let body = if let Some(json_body) = payload {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(json_body.to_string())
        } else {
            Body::empty()
        };
        let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        assert_eq!(status, StatusCode::OK, "non-200 on {method} {uri}");
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("invalid JSON from {method} {uri}: {e}"));
        let errs = check_matches_openapi(
            &spec,
            method.as_str().to_ascii_lowercase().as_str(),
            openapi_path,
            status,
            &json,
        );
        if !errs.is_empty() {
            all_violations.push(format!(
                "{} {}:\n  - {}",
                method.as_str(),
                openapi_path,
                errs.join("\n  - ")
            ));
        }
    }

    assert!(
        all_violations.is_empty(),
        "OpenAPI contract violations:\n{}",
        all_violations.join("\n")
    );
}
