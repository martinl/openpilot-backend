use ::std::env;

pub fn list_keys_starting_with(str: &str) -> String {
    let mkv_endpoint = env::var("MKV_ENDPOINT").expect("MKV_ENDPOINT env variable not set");
    format!("{}/{}?list", mkv_endpoint, str)
}

pub fn get_mkv_file_url(file: &str) -> String {
    let mkv_endpoint = env::var("MKV_ENDPOINT").expect("MKV_ENDPOINT env variable not set");
    format!("{}/{}", mkv_endpoint, file)
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    #[test]
    #[serial]
    fn builds_mkv_list_and_file_urls_from_env() {
        let prev = env::var("MKV_ENDPOINT").ok();
        env::set_var("MKV_ENDPOINT", "http://minikeyvalue:3000");

        assert_eq!(
            list_keys_starting_with("aaaa_2024-02-05"),
            "http://minikeyvalue:3000/aaaa_2024-02-05?list"
        );
        assert_eq!(
            get_mkv_file_url("aaaa_2024-02-05--0--qlog.zst"),
            "http://minikeyvalue:3000/aaaa_2024-02-05--0--qlog.zst"
        );

        match prev {
            Some(v) => env::set_var("MKV_ENDPOINT", v),
            None => env::remove_var("MKV_ENDPOINT"),
        }
    }
}
