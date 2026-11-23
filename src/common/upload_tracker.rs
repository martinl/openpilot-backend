use dashmap::DashMap;
use serde::Serialize;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

fn unix_millis_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadKind {
    DrivingLog,
    Crash,
    Bootlog,
}

#[derive(Debug, Clone)]
pub struct UploadMeta {
    pub kind: UploadKind,
    pub dongle_id: String,
    pub file_name: String,
}

#[derive(Debug, Clone)]
struct ActiveUpload {
    meta: UploadMeta,
    started_at_ms: i64,
    last_update_ms: i64,
    bytes_received: u64,

    // Speed is a short moving estimate based on deltas, not full average.
    speed_bps: f64,
    speed_calc_at_ms: i64,
    speed_calc_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UploadSnapshot {
    pub id: String,
    pub kind: UploadKind,
    pub dongle_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    pub bytes_received: u64,
    pub speed_bps: f64,
    pub started_at_ms: i64,
    pub last_update_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UploadsSnapshotMessage {
    pub server_time_ms: i64,
    pub total_speed_bps: f64,
    pub uploads: Vec<UploadSnapshot>,
}

#[derive(Debug)]
pub struct UploadTracker {
    active: DashMap<String, ActiveUpload>,
}

impl UploadTracker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            active: DashMap::new(),
        })
    }

    pub fn start(&self, meta: UploadMeta) -> String {
        let id = Uuid::new_v4().to_string();
        let now_ms = unix_millis_now();
        self.active.insert(
            id.clone(),
            ActiveUpload {
                meta,
                started_at_ms: now_ms,
                last_update_ms: now_ms,
                bytes_received: 0,
                speed_bps: 0.0,
                speed_calc_at_ms: now_ms,
                speed_calc_bytes: 0,
            },
        );
        id
    }

    pub fn add_bytes(&self, id: &str, n: usize) {
        let now_ms = unix_millis_now();
        if let Some(mut entry) = self.active.get_mut(id) {
            entry.bytes_received = entry.bytes_received.saturating_add(n as u64);
            entry.last_update_ms = now_ms;

            let elapsed_ms = now_ms.saturating_sub(entry.speed_calc_at_ms);
            // Throttle speed recompute to keep it stable and avoid excessive churn.
            if elapsed_ms >= 200 {
                let bytes_delta = entry.bytes_received.saturating_sub(entry.speed_calc_bytes);
                let secs = (elapsed_ms as f64 / 1000.0).max(0.001);
                entry.speed_bps = bytes_delta as f64 / secs;
                entry.speed_calc_bytes = entry.bytes_received;
                entry.speed_calc_at_ms = now_ms;
            }
        }
    }

    pub fn finish(&self, id: &str) {
        self.active.remove(id);
    }

    pub fn active_stats(&self) -> (usize, u64) {
        let mut count = 0usize;
        let mut bytes = 0u64;
        for entry in self.active.iter() {
            count += 1;
            bytes = bytes.saturating_add(entry.value().bytes_received);
        }
        (count, bytes)
    }

    pub fn snapshot_message(&self, is_superuser: bool) -> UploadsSnapshotMessage {
        let now_ms = unix_millis_now();
        let mut total_speed_bps = 0.0;

        let mut uploads: Vec<UploadSnapshot> = self
            .active
            .iter()
            .map(|item| {
                let stale_ms = now_ms.saturating_sub(item.last_update_ms);
                let speed_bps = if stale_ms > 1500 {
                    0.0
                } else if item.speed_bps.is_finite() {
                    item.speed_bps
                } else {
                    0.0
                };

                total_speed_bps += speed_bps;
                UploadSnapshot {
                    id: item.key().clone(),
                    kind: item.meta.kind,
                    dongle_id: item.meta.dongle_id.clone(),
                    file_name: if is_superuser {
                        Some(item.meta.file_name.clone())
                    } else {
                        None
                    },
                    bytes_received: item.bytes_received,
                    speed_bps,
                    started_at_ms: item.started_at_ms,
                    last_update_ms: item.last_update_ms,
                }
            })
            .collect();

        uploads.sort_by(|a, b| b.started_at_ms.cmp(&a.started_at_ms));

        UploadsSnapshotMessage {
            server_time_ms: now_ms,
            total_speed_bps,
            uploads,
        }
    }
}

pub struct ActiveUploadGuard {
    tracker: Arc<UploadTracker>,
    id: String,
}

impl ActiveUploadGuard {
    pub fn new(tracker: Arc<UploadTracker>, meta: UploadMeta) -> Self {
        let id = tracker.start(meta);
        Self { tracker, id }
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

impl Drop for ActiveUploadGuard {
    fn drop(&mut self) {
        self.tracker.finish(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_bytes_and_redacts_file_name_for_non_superusers() {
        let tracker = UploadTracker::new();
        let id = tracker.start(UploadMeta {
            kind: UploadKind::DrivingLog,
            dongle_id: "1d3dc3e03047b0c7".into(),
            file_name: "rlog.zst".into(),
        });

        tracker.add_bytes(&id, 1024);
        tracker.add_bytes(&id, 2048);
        tracker.add_bytes("nonexistent-id", 512);

        let public_snap = tracker.snapshot_message(false);
        assert_eq!(public_snap.uploads.len(), 1);
        assert_eq!(public_snap.uploads[0].bytes_received, 3072);
        assert_eq!(public_snap.uploads[0].file_name, None);

        let admin_snap = tracker.snapshot_message(true);
        assert_eq!(admin_snap.uploads.len(), 1);
        assert_eq!(admin_snap.uploads[0].file_name.as_deref(), Some("rlog.zst"));

        tracker.finish(&id);
        assert!(tracker.snapshot_message(true).uploads.is_empty());
    }

    #[test]
    fn active_upload_guard_removes_entry_on_drop() {
        let tracker = UploadTracker::new();
        {
            let guard = ActiveUploadGuard::new(
                tracker.clone(),
                UploadMeta {
                    kind: UploadKind::Bootlog,
                    dongle_id: "aaaaaaaaaaaaaaaa".into(),
                    file_name: "boot.bz2".into(),
                },
            );
            tracker.add_bytes(guard.id(), 4096);
            assert_eq!(tracker.snapshot_message(false).uploads.len(), 1);
        }
        assert!(tracker.snapshot_message(false).uploads.is_empty());
    }
}
