#![allow(clippy::unused_async)]
use crate::common::upload_tracker::UploadTracker;
use crate::loco_rs::prelude::*;
use crate::models::{devices::DM, routes::RM};
use crate::views;
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    Extension,
};
use chrono::TimeZone;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use sysinfo::Disks;
use tokio::time::{self, Duration};
#[derive(Serialize, Deserialize)]
pub struct TimeSeriesPoint {
    pub date: String,
    pub miles: i64,
}
#[derive(Serialize, Deserialize)]
pub struct CpuUsage {
    pub core: u8,
    pub usage: f32,
}
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct DiskSpace {
    pub name: String,
    pub total: u64,
    pub used: u64,
    pub free: u64,
}
#[derive(Serialize, Deserialize)]
pub struct Active {
    pub daily: u64,
    pub weekly: u64,
    pub monthly: u64,
    pub quarterly: u64,
}

#[derive(Serialize, Deserialize)]
pub struct Devices {
    pub online: u64,
    pub total: u64,
    pub active: Active,
}
#[derive(Serialize, Deserialize)]
pub struct Network {
    pub current_upload: f32,
    pub current_download: f32,
    pub total_upload: f32,
    pub total_download: f32,
}
#[derive(Serialize, Deserialize)]
pub struct DriveStats {
    pub total_miles: i32,
    pub total_drives: u64,
}

#[derive(Serialize, Deserialize)]
pub struct ServerUsage {
    pub time: String,
    pub distance_unit: String,
    pub disk_usage: Vec<DiskSpace>,
    //users: u64,
    pub devices: Devices,
    pub drive_stats: DriveStats,
    // Charts data expected by the template
    pub miles_over_time: Vec<TimeSeriesPoint>,
    pub daily_miles_over_time: Vec<TimeSeriesPoint>,
    pub devices_over_time: Vec<TimeSeriesPoint>,
    pub daily_devices_over_time: Vec<TimeSeriesPoint>,
}

/// Returns `"mi"` when `STATS_UNIT` is `"imperial"`, `"mi"`, or `"miles"`, otherwise `"km"` (metric default).
pub fn configured_distance_unit() -> &'static str {
    match std::env::var("STATS_UNIT")
        .ok()
        .as_deref()
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("imperial" | "mi" | "miles") => "mi",
        _ => "km",
    }
}

#[derive(Deserialize)]
struct PromQueryResponse {
    status: String,
    data: PromQueryData,
}

#[derive(Deserialize)]
struct PromQueryData {
    result: Vec<PromVectorSample>,
}

#[derive(Deserialize)]
struct PromVectorSample {
    metric: BTreeMap<String, String>,
    value: (f64, String),
}

fn resolve_prometheus_namespace() -> Option<String> {
    std::env::var("PROMETHEUS_NAMESPACE")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/namespace")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
}

async fn query_prometheus_vector(
    client: &reqwest::Client,
    prom_url: &str,
    query: &str,
) -> Option<Vec<PromVectorSample>> {
    let base = format!("{}/api/v1/query", prom_url.trim_end_matches('/'));
    let url = url::Url::parse_with_params(&base, &[("query", query)]).ok()?;
    let resp = client
        .get(url)
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let parsed: PromQueryResponse = resp.json().await.ok()?;
    if parsed.status != "success" {
        return None;
    }
    Some(parsed.data.result)
}

fn parse_sample_u64(sample: &PromVectorSample) -> Option<u64> {
    let val: f64 = sample.value.1.parse().ok()?;
    if !val.is_finite() || val < 0.0 {
        return None;
    }
    Some(val.round() as u64)
}

fn parse_pvc_disk_usage(
    capacity: &[PromVectorSample],
    used: &[PromVectorSample],
    available: &[PromVectorSample],
) -> Vec<DiskSpace> {
    let mut map: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();

    for s in capacity {
        if let (Some(pvc), Some(val)) = (s.metric.get("persistentvolumeclaim"), parse_sample_u64(s))
        {
            map.entry(pvc.clone()).or_default().0 = val;
        }
    }
    for s in used {
        if let (Some(pvc), Some(val)) = (s.metric.get("persistentvolumeclaim"), parse_sample_u64(s))
        {
            map.entry(pvc.clone()).or_default().1 = val;
        }
    }
    for s in available {
        if let (Some(pvc), Some(val)) = (s.metric.get("persistentvolumeclaim"), parse_sample_u64(s))
        {
            map.entry(pvc.clone()).or_default().2 = val;
        }
    }

    map.into_iter()
        .filter(|(_, (total, _, _))| *total > 0)
        .map(|(name, (total, used, free))| {
            let free = if free > 0 {
                free
            } else {
                total.saturating_sub(used)
            };
            DiskSpace {
                name,
                total,
                used,
                free,
            }
        })
        .collect()
}

fn get_local_disk_usage() -> Vec<DiskSpace> {
    let disks = Disks::new_with_refreshed_list();
    let mut by_mount: BTreeMap<String, DiskSpace> = BTreeMap::new();
    for disk in disks.iter() {
        let mount = disk.mount_point().to_string_lossy().to_string();
        if (mount == "/params" || mount == "/") && disk.total_space() > 0 {
            let total = disk.total_space();
            let free = disk.available_space();
            by_mount.insert(
                mount.clone(),
                DiskSpace {
                    name: mount,
                    total,
                    used: total.saturating_sub(free),
                    free,
                },
            );
        }
    }
    by_mount.into_values().collect()
}

async fn get_disk_usage(client: &reqwest::Client) -> Vec<DiskSpace> {
    let prom_url = std::env::var("PROMETHEUS_URL")
        .ok()
        .filter(|v| !v.trim().is_empty());

    if let Some(prom_url) = prom_url {
        let ns_filter = resolve_prometheus_namespace()
            .map(|ns| format!("namespace=\"{ns}\""))
            .unwrap_or_default();

        let cap_q = format!("kubelet_volume_stats_capacity_bytes{{{ns_filter}}}");
        let used_q = format!("kubelet_volume_stats_used_bytes{{{ns_filter}}}");
        let avail_q = format!("kubelet_volume_stats_available_bytes{{{ns_filter}}}");

        let (cap_res, used_res, avail_res) = tokio::join!(
            query_prometheus_vector(client, &prom_url, &cap_q),
            query_prometheus_vector(client, &prom_url, &used_q),
            query_prometheus_vector(client, &prom_url, &avail_q),
        );

        let disks = parse_pvc_disk_usage(
            cap_res.as_deref().unwrap_or(&[]),
            used_res.as_deref().unwrap_or(&[]),
            avail_res.as_deref().unwrap_or(&[]),
        );

        if !disks.is_empty() {
            return disks;
        }
    }

    get_local_disk_usage()
}

pub async fn get_server_usage(
    ViewEngine(view): ViewEngine<TeraView>,
    State(ctx): State<AppContext>,
    Extension(client): Extension<reqwest::Client>,
) -> Result<impl IntoResponse> {
    let utc_time_now_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let one_day_ago_millis =
        utc_time_now_millis - std::time::Duration::from_secs(24 * 60 * 60).as_secs();
    let one_week_ago_millis =
        utc_time_now_millis - std::time::Duration::from_secs(7 * 24 * 60 * 60).as_secs();
    let one_month_ago_millis =
        utc_time_now_millis - std::time::Duration::from_secs(30 * 24 * 60 * 60).as_secs();
    let three_months_ago_millis =
        utc_time_now_millis - std::time::Duration::from_secs(90 * 24 * 60 * 60).as_secs();

    // All-time daily miles (sum of route length per UTC day)
    let daily_miles_rows = RM::daily_miles_all_time(&ctx.db).await.unwrap_or_default();
    let daily_miles_points: Vec<TimeSeriesPoint> = daily_miles_rows
        .iter()
        .map(|(day_ms, miles)| {
            let date = chrono::Utc
                .timestamp_millis_opt(*day_ms)
                .single()
                .unwrap_or_else(|| chrono::Utc.timestamp_millis_opt(0).unwrap())
                .format("%Y-%m-%d")
                .to_string();
            TimeSeriesPoint {
                date,
                miles: *miles as i64,
            }
        })
        .collect();
    let mut cumulative_miles_points: Vec<TimeSeriesPoint> =
        Vec::with_capacity(daily_miles_points.len());
    let mut acc: i64 = 0;
    for p in &daily_miles_points {
        acc += p.miles;
        cumulative_miles_points.push(TimeSeriesPoint {
            date: p.date.clone(),
            miles: acc,
        });
    }

    // Fetch all-time device series
    // registrations by created_at and activity by last_athena_ping
    let (daily_reg_rows, daily_active_rows) = DM::daily_devices_all_time(&ctx.db)
        .await
        .unwrap_or_default();
    // We'll display "devices over time" as cumulative registrations, and daily_devices as daily actives
    let daily_devices_points: Vec<TimeSeriesPoint> = daily_active_rows
        .iter()
        .map(|(day_ms, cnt)| {
            let date = chrono::Utc
                .timestamp_millis_opt(*day_ms)
                .single()
                .unwrap_or_else(|| chrono::Utc.timestamp_millis_opt(0).unwrap())
                .format("%Y-%m-%d")
                .to_string();
            TimeSeriesPoint { date, miles: *cnt }
        })
        .collect();
    let mut cumulative_devices_points: Vec<TimeSeriesPoint> =
        Vec::with_capacity(daily_reg_rows.len());
    let mut acc_dev: i64 = 0;
    for (day_ms, cnt) in &daily_reg_rows {
        acc_dev += *cnt;
        let date = chrono::Utc
            .timestamp_millis_opt(*day_ms)
            .single()
            .unwrap_or_else(|| chrono::Utc.timestamp_millis_opt(0).unwrap())
            .format("%Y-%m-%d")
            .to_string();
        cumulative_devices_points.push(TimeSeriesPoint {
            date,
            miles: acc_dev,
        });
    }

    let disk_usage = get_disk_usage(&client).await;

    views::route::server_usage(
        view,
        ServerUsage {
            time: chrono::Utc::now().to_rfc3339(),
            distance_unit: configured_distance_unit().to_string(),
            disk_usage,
            devices: Devices {
                total: DM::get_registered_devices(&ctx.db, None, None, None).await?,
                online: DM::get_registered_devices(&ctx.db, Some(true), None, None).await?,
                active: Active {
                    daily: DM::get_registered_devices(
                        &ctx.db,
                        None,
                        None,
                        Some(one_day_ago_millis),
                    )
                    .await?,
                    weekly: DM::get_registered_devices(
                        &ctx.db,
                        None,
                        None,
                        Some(one_week_ago_millis),
                    )
                    .await?,
                    monthly: DM::get_registered_devices(
                        &ctx.db,
                        None,
                        None,
                        Some(one_month_ago_millis),
                    )
                    .await?,
                    quarterly: DM::get_registered_devices(
                        &ctx.db,
                        None,
                        None,
                        Some(three_months_ago_millis),
                    )
                    .await?,
                },
            },
            drive_stats: DriveStats {
                total_miles: RM::get_miles(&ctx.db).await? as i32,
                total_drives: RM::get_drive_count(&ctx.db).await?,
            },
            // Provide time series for charts
            miles_over_time: cumulative_miles_points,
            daily_miles_over_time: daily_miles_points,
            devices_over_time: cumulative_devices_points,
            daily_devices_over_time: daily_devices_points,
        },
    )
}

async fn handle_uploads_socket(
    socket: WebSocket,
    upload_tracker: Arc<UploadTracker>,
    is_superuser: bool,
) {
    let (mut sender, mut receiver) = socket.split();

    // tokio::time::interval ticks immediately; discard the first one so we don't double-send.
    let mut interval = time::interval(Duration::from_secs(1));
    interval.tick().await;

    let initial = upload_tracker.snapshot_message(is_superuser);
    if let Ok(json) = serde_json::to_string(&initial) {
        if sender.send(Message::Text(json.into())).await.is_err() {
            return;
        }
    }

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let snapshot = upload_tracker.snapshot_message(is_superuser);
                let Ok(json) = serde_json::to_string(&snapshot) else { continue; };
                if sender.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
            incoming = receiver.next() => {
                match incoming {
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = sender.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        tracing::debug!("stats uploads ws recv error: {}", e);
                        break;
                    }
                }
            }
        }
    }
}

pub async fn ws_uploads(
    auth: crate::middleware::auth::MyJWT,
    ws: WebSocketUpgrade,
    Extension(upload_tracker): Extension<Arc<UploadTracker>>,
) -> impl IntoResponse {
    let is_superuser = auth
        .user_model
        .as_ref()
        .map(|u| u.superuser)
        .unwrap_or(false);

    ws.on_upgrade(move |socket| async move {
        handle_uploads_socket(socket, upload_tracker, is_superuser).await;
    })
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("stats")
        .add("/usage", get(get_server_usage))
        .add("/ws/uploads", get(ws_uploads))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(labels: &[(&str, &str)], value: &str) -> PromVectorSample {
        PromVectorSample {
            metric: labels
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            value: (1_700_000_000.0, value.to_string()),
        }
    }

    #[test]
    fn parse_pvc_disk_usage_joins_and_sorts_volumes() {
        let cap = vec![
            sample(
                &[("persistentvolumeclaim", "openpilot-db-1")],
                "21474836480",
            ),
            sample(
                &[("persistentvolumeclaim", "kvstore-minikeyvalue-0")],
                "107374182400",
            ),
            sample(&[("persistentvolumeclaim", "zero-pvc")], "0"),
        ];
        let used = vec![
            sample(&[("persistentvolumeclaim", "openpilot-db-1")], "1073741824"),
            sample(
                &[("persistentvolumeclaim", "kvstore-minikeyvalue-0")],
                "5368709120",
            ),
        ];
        let avail = vec![sample(
            &[("persistentvolumeclaim", "openpilot-db-1")],
            "20401094656",
        )];

        let disks = parse_pvc_disk_usage(&cap, &used, &avail);
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].name, "kvstore-minikeyvalue-0");
        assert_eq!(disks[0].total, 107_374_182_400);
        assert_eq!(disks[0].used, 5_368_709_120);
        assert_eq!(disks[0].free, 102_005_473_280);
        assert_eq!(disks[1].name, "openpilot-db-1");
        assert_eq!(disks[1].free, 20_401_094_656);
    }

    #[test]
    #[serial_test::serial]
    fn configured_distance_unit_defaults_to_km_and_supports_imperial() {
        let prev = std::env::var("STATS_UNIT").ok();
        std::env::remove_var("STATS_UNIT");
        assert_eq!(configured_distance_unit(), "km");

        std::env::set_var("STATS_UNIT", "metric");
        assert_eq!(configured_distance_unit(), "km");

        std::env::set_var("STATS_UNIT", "imperial");
        assert_eq!(configured_distance_unit(), "mi");

        std::env::set_var("STATS_UNIT", "MI");
        assert_eq!(configured_distance_unit(), "mi");

        match prev {
            Some(v) => std::env::set_var("STATS_UNIT", v),
            None => std::env::remove_var("STATS_UNIT"),
        }
    }
}
