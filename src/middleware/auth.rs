use std::collections::HashMap;

use axum::{
    extract::{FromRef, FromRequestParts, Query},
    http::{request::Parts, HeaderMap},
    response::{IntoResponse, Redirect},
};
use axum_extra::extract::cookie;
//use eyre::Error;
//use eyre::ErrReport;
use crate::loco_rs::{app::AppContext, prelude::*};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::jwt;
use crate::models::{devices::DM, users::UM};
// Define constants for token prefix and authorization header
const QUERY_TOKEN_PREFIX: &str = "sig";
const QUERY_TOKEN_PREFIX_DIRECTIONS: &str = "access_token";
const TOKEN_PREFIX: &str = "JWT ";
const AUTH_HEADER: &str = "Authorization";
const AUTH_COOKIE_NAME: &str = "jwt";

// Define a struct to represent user authentication information serialized
// to/from JSON
#[derive(Debug, Deserialize, Serialize)]
pub struct MyJWT {
    pub claims: jwt::UserClaims,
    pub device_model: Option<DM>,
    pub user_model: Option<UM>,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct UnverifiedJWT {
    pub claims: jwt::UserClaims,
    pub device_model: Option<DM>,
    pub user_model: Option<UM>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SuperUserJWT {
    pub claims: jwt::UserClaims,
    pub device_model: Option<DM>,
    pub user_model: Option<UM>,
}

use jsonwebtoken::{errors::ErrorKind, Algorithm};

#[derive(Serialize)]
struct ErrorMessage {
    code: u16,
    message: String,
    error_kind: Option<String>, // Include the ErrorKind as an optional field
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("unauthorized")]
    Unauthorized(String),
    #[error("redirect to login")]
    RedirectToLogin,
    #[error("reset dongleId")]
    ResetDone,
    #[error("server error")]
    InternalError,
    #[error("Error verifying jwt")]
    JWTError(ErrorKind),
}

impl IntoResponse for AuthError {
    fn into_response(self) -> axum::response::Response {
        match self {
            AuthError::Unauthorized(msg) => (http::StatusCode::UNAUTHORIZED, msg).into_response(),
            AuthError::RedirectToLogin => Redirect::to("/login").into_response(),
            AuthError::ResetDone => {
                (http::StatusCode::ACCEPTED, "Reset your DongleId").into_response()
            }
            //AuthError::FormatError => (http::StatusCode::BAD_REQUEST, "Unauthorized").into_response(),
            AuthError::InternalError => (
                http::StatusCode::INTERNAL_SERVER_ERROR,
                "Internal Server Error",
            )
                .into_response(),
            AuthError::JWTError(error_kind) => {
                let error_message = ErrorMessage {
                    code: http::StatusCode::UNAUTHORIZED.as_u16(),
                    message: "JWT validation failed".to_string(),
                    error_kind: Some(format!("{:?}", error_kind)), // Convert ErrorKind to a string
                };
                (http::StatusCode::UNAUTHORIZED, Json(error_message)).into_response()
            }
        }
    }
}

impl<S> FromRequestParts<S> for MyJWT
where
    AppContext: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let ctx: AppContext = AppContext::from_ref(state);

        let token = extract_token(parts)?;
        //.map_err(|e| handle_unauth(parts, e))?;

        let jwt_secret = ctx
            .config
            .get_jwt_config()
            .map_err(|_| AuthError::InternalError)?;

        let mut jwt_processor = jwt::JWT::new(&jwt_secret.secret); // algo defaults to HS512 which we use for users
                                                                   // extract the token data to get the algo
        let token_data = jwt_processor
            .parse_unverified(&token)
            .map_err(|e| handle_jwt_err(parts, e.kind()))?;

        let alg = token_data.header.alg;
        jwt_processor = jwt_processor.algorithm(token_data.header.alg);
        let identity = &token_data.claims.identity; // this is the dongle_id if its a device or identity if user
        let error_message = format!("identity: {} not registered", identity);
        match alg {
            Algorithm::RS256
            | Algorithm::RS384
            | Algorithm::RS512
            | Algorithm::ES256
            | Algorithm::ES384 => {
                // device can use these algos
                let device = DM::find_device(&ctx.db, identity)
                    .await
                    .map_err(|_| handle_unauth(parts, &error_message))?; // return here if not registered

                let valid_token_data = jwt_processor
                    .validate_pem(&token, device.public_key.as_bytes())
                    .map_err(|e| handle_unauth(parts, &format!("Got invalid token: {}", e)))?;

                Ok(Self {
                    claims: valid_token_data.claims,
                    device_model: Some(device),
                    user_model: None,
                })
            }
            Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512 => {
                // the server issues these to devices and users
                let valid_token_data = jwt_processor
                    .validate(&token)
                    .map_err(|e| handle_unauth(parts, &format!("Got invalid token: {}", e)))?;

                let (user_model, device_model) = tokio::join!(
                    UM::find_by_identity(&ctx.db, identity),
                    DM::find_device(&ctx.db, identity)
                );
                if user_model.is_err() && device_model.is_err() {
                    return Err(handle_unauth(parts, &error_message));
                } else if user_model.is_ok() && device_model.is_ok() {
                    panic!(); // should never get here
                }

                Ok(Self {
                    claims: valid_token_data.claims,
                    user_model: user_model.ok(),
                    device_model: device_model.ok(),
                })
            }
            _ => Err(handle_unauth(
                parts,
                "Must use RS or HS or ES jwt algorithm",
            )),
        }
    }
}

fn handle_jwt_err(parts: &mut Parts, error_kind: &ErrorKind) -> AuthError {
    let host_header = parts
        .headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();

    let parts: Vec<&str> = host_header.split('.').collect();

    if parts[0] == "useradmin" {
        // Redirect for useradmin subdomain
        AuthError::RedirectToLogin
    } else {
        // Return JWTError with the ErrorKind
        AuthError::JWTError(error_kind.clone())
    }
}

fn handle_unauth(parts: &mut Parts, msg: &str) -> AuthError {
    let host_header = parts
        .headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();

    let parts: Vec<&str> = host_header.split('.').collect();

    if parts[0] == "useradmin" {
        // Redirect for useradmin subdomain
        AuthError::RedirectToLogin
    } else {
        // Return JWTError with the ErrorKind
        AuthError::Unauthorized(msg.to_string())
    }
}

#[derive(Debug, Deserialize)]
pub struct DeviceClaims {
    pub identity: String,
    pub nbf: usize,
    pub iat: usize,
    pub exp: usize,
}

fn auth_cookie_name() -> String {
    std::env::var("COOKIE_NAME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| AUTH_COOKIE_NAME.to_string())
}

fn extract_token(parts: &mut Parts) -> Result<String, AuthError> {
    // Attempt to extract the token from the query string
    extract_token_from_query(QUERY_TOKEN_PREFIX, parts)
        .or_else(|_| {
            // If extracting from the query string fails, attempt to extract from cookies.
            // Try the configured COOKIE_NAME first (e.g. `jwt_test` for browsers in test),
            // then fall back to the default `jwt` cookie because openpilot's `athenad.py`
            // hardcodes `cookie="jwt=<token>"` on WebSocket connections.
            let cookie_name = auth_cookie_name();
            extract_token_from_cookie(&cookie_name, parts).or_else(|e| {
                if cookie_name != AUTH_COOKIE_NAME {
                    extract_token_from_cookie(AUTH_COOKIE_NAME, parts)
                } else {
                    Err(e)
                }
            })
        })
        .or_else(|_| {
            // If extracting from cookies fails, attempt to extract from the authorization header
            extract_token_from_header(&parts.headers)
        })
        .or_else(|_| {
            // If extracting from the authorization header fails, attempt to extract from the query string
            extract_token_from_query(QUERY_TOKEN_PREFIX_DIRECTIONS, parts)
                .map_err(|e| handle_unauth(parts, &e))
        })
}
/// Function to extract a token from the authorization header
///
/// # Errors
///
/// When token is not valid or out found
pub fn extract_token_from_header(headers: &HeaderMap) -> Result<String, String> {
    Ok(headers
        .get(AUTH_HEADER)
        .ok_or_else(|| format!("Header {AUTH_HEADER} token not found"))?
        .to_str()
        .map_err(|err| err.to_string())?
        .strip_prefix(TOKEN_PREFIX)
        .ok_or_else(|| format!("Error strip {AUTH_HEADER} value"))?
        .to_string())
}

/// Extract a token value from cookie
///
/// # Errors
/// when token value from cookie is not found
pub fn extract_token_from_cookie(name: &str, parts: &Parts) -> Result<String, String> {
    let jar: cookie::CookieJar = cookie::CookieJar::from_headers(&parts.headers);
    Ok(jar
        .get(name)
        .ok_or("Token is not found".to_string())?
        .to_string()
        .strip_prefix(&format!("{name}="))
        .ok_or_else(|| "Error strip value".to_string())?
        .to_string())
}
/// Extract a token value from query
///
/// # Errors
/// when token value from cookie is not found
pub fn extract_token_from_query(name: &str, parts: &Parts) -> Result<String, String> {
    // LogoResult
    let parameters: Query<HashMap<String, String>> =
        Query::try_from_uri(&parts.uri).map_err(|err| err.to_string())?;
    parameters
        .get(name)
        .cloned()
        .ok_or_else(|| format!("`{name}` query parameter not found"))
}

#[cfg(test)]
mod tests {
    use axum::http::{header, Request, StatusCode};
    use serial_test::serial;

    use super::*;

    fn make_parts(uri: &str, headers: &[(&str, &str)]) -> Parts {
        let mut builder = Request::builder().uri(uri);
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        builder.body(()).unwrap().into_parts().0
    }

    #[test]
    fn extract_token_from_header_validates_jwt_prefix() {
        let mut headers = HeaderMap::new();
        assert!(extract_token_from_header(&headers).is_err());

        headers.insert(header::AUTHORIZATION, "Bearer wrong".parse().unwrap());
        assert!(extract_token_from_header(&headers).is_err());

        headers.insert(header::AUTHORIZATION, "JWT my-token-123".parse().unwrap());
        assert_eq!(extract_token_from_header(&headers).unwrap(), "my-token-123");
    }

    #[test]
    fn extract_token_from_cookie_and_query() {
        let parts = make_parts(
            "/v1/me?sig=query-tok&access_token=dir-tok",
            &[("cookie", "jwt_test=cookie-tok")],
        );
        assert_eq!(
            extract_token_from_cookie("jwt_test", &parts).unwrap(),
            "cookie-tok"
        );
        assert!(extract_token_from_cookie("jwt", &parts).is_err());
        assert_eq!(
            extract_token_from_query("sig", &parts).unwrap(),
            "query-tok"
        );
        assert_eq!(
            extract_token_from_query("access_token", &parts).unwrap(),
            "dir-tok"
        );
        assert!(extract_token_from_query("missing", &parts).is_err());
    }

    #[test]
    #[serial]
    fn extract_token_respects_precedence_and_cookie_name() {
        let prev = std::env::var("COOKIE_NAME").ok();
        std::env::set_var("COOKIE_NAME", "jwt_test");

        let mut parts_query = make_parts(
            "/v1/me?sig=from-sig",
            &[
                ("cookie", "jwt_test=from-cookie"),
                ("authorization", "JWT from-header"),
            ],
        );
        assert_eq!(extract_token(&mut parts_query).unwrap(), "from-sig");

        let mut parts_cookie = make_parts(
            "/v1/me",
            &[
                ("cookie", "jwt_test=from-cookie; jwt=from-default-jwt"),
                ("authorization", "JWT from-header"),
            ],
        );
        assert_eq!(extract_token(&mut parts_cookie).unwrap(), "from-cookie");

        // openpilot athenad.py hardcodes `Cookie: jwt=<device_token>` even when COOKIE_NAME=jwt_test
        let mut parts_athena_cookie = make_parts(
            "/ws/v2/1d3dc3e03047b0c7",
            &[("cookie", "jwt=from-athena-device")],
        );
        assert_eq!(
            extract_token(&mut parts_athena_cookie).unwrap(),
            "from-athena-device"
        );

        let mut parts_header = make_parts("/v1/me", &[("authorization", "JWT from-header")]);
        assert_eq!(extract_token(&mut parts_header).unwrap(), "from-header");

        let mut parts_access = make_parts("/v1/me?access_token=from-access", &[]);
        assert_eq!(extract_token(&mut parts_access).unwrap(), "from-access");

        match prev {
            Some(v) => std::env::set_var("COOKIE_NAME", v),
            None => std::env::remove_var("COOKIE_NAME"),
        }
    }

    #[test]
    fn missing_token_redirects_on_useradmin_and_returns_401_on_api() {
        let mut api_parts = make_parts("/v1/me", &[("host", "api.example.com")]);
        let api_err = extract_token(&mut api_parts).unwrap_err();
        assert_eq!(api_err.into_response().status(), StatusCode::UNAUTHORIZED);

        let mut admin_parts = make_parts("/", &[("host", "useradmin.example.com")]);
        let admin_err = extract_token(&mut admin_parts).unwrap_err();
        let resp = admin_err.into_response();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/login");

        assert_eq!(
            AuthError::InternalError.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
