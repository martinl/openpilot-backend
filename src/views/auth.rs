use serde::{Deserialize, Serialize};

use crate::loco_rs::prelude::*;

#[derive(Debug, Deserialize, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub pid: String,
    pub name: String,
    pub is_verified: bool,
}

#[derive(Serialize)]
pub struct LoginTemplate {
    pub api_host: String,
    pub github_oauth_url: String,
}

pub fn login(v: impl ViewRenderer, template: LoginTemplate) -> Result<impl IntoResponse> {
    // Render the view with the template
    format::render().view(&v, "useradmin/login.html", template)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_view_renders_real_tera_template() {
        let tera = TeraView::build().expect("assets/views should load");
        let resp = login(
            tera,
            LoginTemplate {
                api_host: "https://api.example.com".into(),
                github_oauth_url: "https://github.com/login/oauth/authorize?client_id=test".into(),
            },
        )
        .unwrap()
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
