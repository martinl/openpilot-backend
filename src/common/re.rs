pub const DONGLE_ID: &str = r"[0-9a-z]{16}";
///
/// const MONOTONIC_TIMESTAMP: &str = r"[0-9a-f]{8}--[0-9a-f]{10}";
///
/// const TIMESTAMP: &str = r"[0-9]{4}-[0-9]{2}-[0-9]{2}--[0-9]{2}-[0-9]{2}-[0-9]{2}";
///
/// MONOTONIC_TIMESTAMP or TIMESTAMP
pub const ROUTE_NAME: &str =
    r"(?:[0-9a-f]{8}--[0-9a-f]{10}|[0-9]{4}-[0-9]{2}-[0-9]{2}--[0-9]{2}-[0-9]{2}-[0-9]{2})";
/// Any number
pub const NUMBER: &str = r"[0-9]+";
pub const HEX: &str = r"[0-9a-f]+";
/// Any file name
pub const ANY_FILENAME: &str = r".+";
pub const ALLOWED_FILENAME: &str = r"(rlog\.(?:bz2|zst)|qlog\.(?:bz2|zst)|qcamera\.ts|fcamera\.hevc|dcamera\.hevc|ecamera\.hevc|qlog\.unlog|sprite\.jpg|coords\.json|events\.json)";

#[cfg(test)]
mod tests {
    use regex::Regex;

    use super::*;

    #[test]
    fn dongle_id_matches_16_lowercase_alnum_only() {
        let re = Regex::new(&format!("^{DONGLE_ID}$")).unwrap();
        assert!(re.is_match("1d3dc3e03047b0c7"));
        assert!(re.is_match("aaaaaaaaaaaaaaaa"));
        assert!(!re.is_match("1d3dc3e03047b0c")); // 15 chars
        assert!(!re.is_match("1d3dc3e03047b0c70")); // 17 chars
        assert!(!re.is_match("1D3DC3E03047B0C7")); // uppercase
        assert!(!re.is_match("1d3dc3e0-047b0c7")); // hyphen
    }

    #[test]
    fn route_name_matches_monotonic_and_wall_clock_timestamps() {
        let re = Regex::new(&format!("^(?:{ROUTE_NAME})$")).unwrap();
        assert!(re.is_match("0000001a--1234567890"));
        assert!(re.is_match("2024-02-05--16-22-28"));
        assert!(!re.is_match("2024-02-05_16-22-28"));
        assert!(!re.is_match("not-a-route"));
    }

    #[test]
    fn allowed_filename_accepts_known_media_and_rejects_others() {
        let re = Regex::new(&format!("^{ALLOWED_FILENAME}$")).unwrap();
        for valid in [
            "rlog.bz2",
            "rlog.zst",
            "qlog.bz2",
            "qlog.zst",
            "qcamera.ts",
            "fcamera.hevc",
            "dcamera.hevc",
            "ecamera.hevc",
            "qlog.unlog",
            "sprite.jpg",
            "coords.json",
            "events.json",
        ] {
            assert!(re.is_match(valid), "expected {valid} to match");
        }

        for invalid in ["rlog.tar", "evil.sh", "../rlog.zst", "qcamera.mp4", ""] {
            assert!(!re.is_match(invalid), "expected {invalid} to be rejected");
        }
    }
}
