use loco_rs::prelude::*;

use futures_util::{SinkExt, StreamExt};
use http::Request;
use serde_json::{json, Value};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use url::Url;
use uuid::Uuid;

async fn websocket_client_task(server_url: &str, dongle_id: &str, jwt: &str) {
    let uri = format!("{}/{}", server_url, dongle_id);
    let request = Request::builder()
        .uri(uri)
        .header("Cookie", format!("jwt={}", jwt))
        .header("Sec-WebSocket-Key", "x3JJHMbDL1EzLkh9GBhXDw==")
        .header("Host", Url::parse(server_url).unwrap().host_str().unwrap())
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Version", "13")
        .body(())
        .expect("Failed to build request.");

    let (mut ws_stream, _) = match connect_async(request).await {
        Ok(connection) => connection,
        Err(e) => {
            eprintln!("Failed to connect: {:?}", e);
            return;
        }
    };
    let id: String = Uuid::new_v4().into();
    // Send listDataDirectory command
    let list_command = json!({
        "jsonrpc": "2.0",
        "method": "listDataDirectory",
        "params": {},
        "id": id
    })
    .to_string();
    if let Err(e) = ws_stream.send(Message::Text(list_command)).await {
        eprintln!("Failed to send message: {:?}", e);
        return;
    }

    // Regex to format the filename into the desired URL format
    let re_drive_log = regex::Regex::new(
        r"^([0-9]{4}-[0-9]{2}-[0-9]{2}--[0-9]{2}-[0-9]{2}-[0-9]{2})--([0-9]+)/rlog$",
    )
    .unwrap();

    // Listen for responses and handle them
    while let Some(message) = ws_stream.next().await {
        match message {
            Ok(Message::Text(text)) => {
                if let Ok(data) = serde_json::from_str::<Value>(&text) {
                    if let Some(files) = data["result"].as_array() {
                        let api_endpoint = std::env::var("API_ENDPOINT")
                            .expect("API_ENDPOINT env variable not set");
                        let api_endpoint = api_endpoint.trim_end_matches('/');
                        let upload_commands: Vec<Value> = files.iter()
                            .filter_map(|file| file.as_str()) // Ensure the file is a string
                            .filter(|file| file.contains("/rlog")) // Filter files containing "/rlog"
                            .filter_map(|file_name| { // Transform and filter in one step
                                re_drive_log.captures(file_name).map(|caps| {
                                    let url_file_format = format!("{}/{}/rlog",
                                        &caps[1], // Timestamp
                                        &caps[2], // Segment
                                    );
                                    json!({
                                        "fn": file_name,
                                        "url": format!("{api_endpoint}/connectincoming/{dongle_id}/{}.bz2", url_file_format),
                                        "headers": {}
                                    })
                                })
                            })
                            .collect();

                        let upload_message = json!({
                            "jsonrpc": "2.0",
                            "method": "uploadFilesToUrls",
                            "params": [upload_commands],
                            "id": id
                        })
                        .to_string();
                        ws_stream
                            .send(Message::Text(upload_message))
                            .await
                            .expect("Failed to send upload command");
                    }
                }
            }
            Ok(_) => {} // Handle other types of messages
            Err(e) => {
                eprintln!("Error in websocket stream: {:?}", e);
                break; // or attempt to reconnect
            }
        }
    }
}

pub struct CollectData;
#[async_trait]
impl Task for CollectData {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "collect_data".to_string(),
            detail: "Task generator".to_string(),
        }
    }
    async fn run(&self, _app_context: &AppContext, vars: &task::Vars) -> Result<()> {
        println!("Task CollectData generated");
        let ws_base = std::env::var("WS_ENDPOINT")
            .unwrap_or_else(|_| "ws://localhost:3111/ws".to_string());
        let server_url = format!("{}/v2", ws_base.trim_end_matches('/'));
        let dongle_id = vars
            .cli_arg("dongle_id")
            .map(|s| s.to_string())
            .or_else(|_| std::env::var("COLLECT_DATA_DONGLE_ID"))
            .expect("Provide dongle_id CLI arg or COLLECT_DATA_DONGLE_ID env var");
        let admin_jwt = std::env::var("ADMIN_JWT").expect("ADMIN_JWT env variable not set");
        websocket_client_task(&server_url, &dongle_id, &admin_jwt).await;
        Ok(())
    }
}
