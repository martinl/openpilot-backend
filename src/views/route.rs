use crate::loco_rs::prelude::*;

use crate::controllers::{
    stats::ServerUsage,
    useradmin::{CloudlogsTemplate, MasterTemplate, UlogText},
};

pub fn admin_route(v: impl ViewRenderer, template: MasterTemplate) -> Result<impl IntoResponse> {
    // Render the view with the template
    format::render().view(&v, "useradmin/template.html", template)
}

pub fn admin_cloudlogs(
    v: impl ViewRenderer,
    template: CloudlogsTemplate,
) -> Result<impl IntoResponse> {
    // Render the cloudlog.html view with an empty context
    format::render().view(&v, "useradmin/cloudlog.html", template)
}

pub fn admin_segment_ulog(v: impl ViewRenderer, data: UlogText) -> Result<impl IntoResponse> {
    // Render the view with the template
    format::render().view(&v, "useradmin/ulog.html", data)
}

pub fn server_usage(v: impl ViewRenderer, data: ServerUsage) -> Result<impl IntoResponse> {
    // Render the server usage view with the provided data
    format::render().view(&v, "stats/server_usage.html", data)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn route_views_render_real_tera_templates() {
        let tera = TeraView::build().expect("assets/views should load");

        let route_resp = admin_route(tera.clone(), MasterTemplate::default())
            .unwrap()
            .into_response();
        assert_eq!(route_resp.status(), StatusCode::OK);

        let cloud_resp = admin_cloudlogs(
            tera.clone(),
            CloudlogsTemplate {
                defined: true,
                cloudlogs: HashMap::new(),
            },
        )
        .unwrap()
        .into_response();
        assert_eq!(cloud_resp.status(), StatusCode::OK);

        let ulog_resp = admin_segment_ulog(
            tera,
            UlogText {
                text: "sample log".into(),
                events: vec!["initData".into()],
                selected_event: None,
            },
        )
        .unwrap()
        .into_response();
        assert_eq!(ulog_resp.status(), StatusCode::OK);
    }
}
