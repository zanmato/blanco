use anyhow::{Context, Result};
use encoding_rs::Encoding;
use std::fs::File;
use std::io::Read;
use std::path::Path;

const SAMPLE_BYTES: usize = 64 * 1024;
const SAMPLE_ROWS: usize = 5;
const CANDIDATE_DELIMITERS: &[u8] = b",;\t|";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportFormat {
    /// Delimited text; `delimiter` and the header toggle apply.
    Csv,
    /// A JSON array of objects, or newline delimited JSON objects. Keys become
    /// the columns, nested values are kept as JSON text.
    Json,
}

#[derive(Debug, Clone)]
pub struct DetectedFile {
    pub format: ImportFormat,
    pub encoding: &'static Encoding,
    pub delimiter: u8,
    pub headers: Vec<String>,
    pub sample_rows: Vec<Vec<String>>,
}

pub fn detect_file(path: &Path) -> Result<DetectedFile> {
    let raw = read_sample_bytes(path)?;
    let encoding = detect_encoding(&raw);
    let decoded = decode_sample(&raw, encoding);
    if looks_like_json(path, &decoded) {
        let mut detected = read_json_sample(&decoded).context("failed to read JSON sample")?;
        detected.encoding = encoding;
        return Ok(detected);
    }
    let delimiter = detect_delimiter(&decoded);
    read_sample(&decoded, delimiter, true)
        .map(|mut detected| {
            detected.encoding = encoding;
            detected.delimiter = delimiter;
            detected
        })
        .context("failed to read CSV sample")
}

fn looks_like_json(path: &Path, decoded: &str) -> bool {
    let by_extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("json")
                || ext.eq_ignore_ascii_case("ndjson")
                || ext.eq_ignore_ascii_case("jsonl")
        });
    by_extension || matches!(decoded.trim_start().chars().next(), Some('[') | Some('{'))
}

/// Column names are the union of object keys in order of first appearance, so
/// sparse objects still map every field they use.
pub fn json_headers(objects: &[serde_json::Map<String, serde_json::Value>]) -> Vec<String> {
    let mut headers: Vec<String> = Vec::new();
    for object in objects {
        for key in object.keys() {
            if !headers.iter().any(|existing| existing == key) {
                headers.push(key.clone());
            }
        }
    }
    headers
}

/// Text form of a JSON value as an import cell. `null` is `None`, strings
/// are unwrapped, everything else keeps its JSON serialisation.
pub fn json_cell(value: Option<&serde_json::Value>) -> Option<String> {
    match value {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(text)) => Some(text.clone()),
        Some(other) => Some(other.to_string()),
    }
}

/// Parse JSON objects from `text`: either one top level array or one object
/// per line (NDJSON). A truncated sample is tolerated by stopping at the first
/// object that does not parse.
pub fn parse_json_objects(
    text: &str,
    limit: Option<usize>,
) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
    let trimmed = text.trim_start();
    let mut objects = Vec::new();
    if let Some(array_body) = trimmed.strip_prefix('[') {
        // Iterate the array elements without materialising the whole array,
        // skipping the commas between them.
        let mut rest = array_body;
        loop {
            rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
            if rest.is_empty() || rest.starts_with(']') {
                break;
            }
            let mut element =
                serde_json::Deserializer::from_str(rest).into_iter::<serde_json::Value>();
            match element.next() {
                Some(Ok(serde_json::Value::Object(object))) => {
                    objects.push(object);
                    rest = &rest[element.byte_offset()..];
                }
                Some(Ok(_)) => anyhow::bail!("JSON array elements must be objects"),
                Some(Err(_)) | None => break,
            }
            if limit.is_some_and(|limit| objects.len() >= limit) {
                break;
            }
        }
    } else {
        for line in trimmed.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<serde_json::Value>(line) {
                Ok(serde_json::Value::Object(object)) => objects.push(object),
                Ok(_) => anyhow::bail!("each JSON line must be an object"),
                Err(_) => break,
            }
            if limit.is_some_and(|limit| objects.len() >= limit) {
                break;
            }
        }
    }
    Ok(objects)
}

fn read_json_sample(decoded: &str) -> Result<DetectedFile> {
    let objects = parse_json_objects(decoded, Some(SAMPLE_ROWS))?;
    if objects.is_empty() {
        anyhow::bail!("no JSON objects found");
    }
    let headers = json_headers(&objects);
    let sample_rows = objects
        .iter()
        .map(|object| {
            headers
                .iter()
                .map(|header| json_cell(object.get(header)).unwrap_or_default())
                .collect()
        })
        .collect();
    Ok(DetectedFile {
        format: ImportFormat::Json,
        encoding: encoding_rs::UTF_8,
        delimiter: b',',
        headers,
        sample_rows,
    })
}

pub fn read_sample_with(
    path: &Path,
    encoding: &'static Encoding,
    delimiter: u8,
    has_header: bool,
) -> Result<DetectedFile> {
    let raw = read_sample_bytes(path)?;
    let decoded = decode_sample(&raw, encoding);
    let mut detected = read_sample(&decoded, delimiter, has_header)?;
    detected.encoding = encoding;
    detected.delimiter = delimiter;
    Ok(detected)
}

fn read_sample_bytes(path: &Path) -> Result<Vec<u8>> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut buffer = vec![0u8; SAMPLE_BYTES];
    let n = file.read(&mut buffer)?;
    buffer.truncate(n);
    Ok(buffer)
}

fn detect_encoding(raw: &[u8]) -> &'static Encoding {
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
    detector.feed(raw, true);
    detector.guess(None, chardetng::Utf8Detection::Allow)
}

fn decode_sample(raw: &[u8], encoding: &'static Encoding) -> String {
    let (decoded, _, _) = encoding.decode(raw);
    decoded.into_owned()
}

fn detect_delimiter(decoded: &str) -> u8 {
    let mut best = b',';
    let mut best_score = -1i64;
    for &candidate in CANDIDATE_DELIMITERS {
        let score = score_delimiter(decoded, candidate);
        if score > best_score {
            best_score = score;
            best = candidate;
        }
    }
    best
}

fn score_delimiter(decoded: &str, delimiter: u8) -> i64 {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(decoded.as_bytes());
    let mut counts: Vec<usize> = Vec::new();
    for record in reader.records().take(SAMPLE_ROWS + 1) {
        match record {
            Ok(r) => counts.push(r.len()),
            Err(_) => return i64::MIN,
        }
    }
    if counts.is_empty() {
        return i64::MIN;
    }
    let first = counts[0];
    if first < 2 {
        return i64::MIN;
    }
    let consistent = counts.iter().filter(|&&c| c == first).count() as i64;
    consistent * 100 + first as i64
}

fn read_sample(decoded: &str, delimiter: u8, has_header: bool) -> Result<DetectedFile> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(has_header)
        .flexible(true)
        .from_reader(decoded.as_bytes());

    let headers: Vec<String> = if has_header {
        reader
            .headers()
            .map(|h| h.iter().map(|s| s.to_string()).collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut sample_rows = Vec::new();
    for record in reader.records().take(SAMPLE_ROWS) {
        match record {
            Ok(r) => sample_rows.push(r.iter().map(|s| s.to_string()).collect()),
            Err(_) => break,
        }
    }

    Ok(DetectedFile {
        format: ImportFormat::Csv,
        encoding: encoding_rs::UTF_8,
        delimiter,
        headers,
        sample_rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_comma_delimited_utf8() {
        let text = "id,name,email\n1,Ada,ada@example.com\n2,Bob,bob@example.com\n";
        let encoding = detect_encoding(text.as_bytes());
        assert_eq!(encoding, encoding_rs::UTF_8);
        assert_eq!(detect_delimiter(text), b',');
        let detected = read_sample(text, b',', true).unwrap();
        assert_eq!(detected.headers, vec!["id", "name", "email"]);
        assert_eq!(detected.sample_rows.len(), 2);
    }

    #[test]
    fn detects_semicolon_windows_1252() {
        let source = "id;name\n1;Ålborg\n2;Øslo\n";
        let raw = encoding_rs::WINDOWS_1252.encode(source).0.into_owned();
        let encoding = detect_encoding(&raw);
        assert_eq!(encoding, encoding_rs::WINDOWS_1252);
        let decoded = decode_sample(&raw, encoding);
        assert_eq!(detect_delimiter(&decoded), b';');
    }

    #[test]
    fn detects_tab_delimited() {
        let text = "a\tb\tc\n1\t2\t3\n4\t5\t6\n";
        assert_eq!(detect_delimiter(text), b'\t');
    }

    #[test]
    fn json_array_sample_uses_union_of_keys() {
        let text = r#"[{"id": 1, "name": "Ada"}, {"id": 2, "email": "bob@example.com", "tags": ["x", "y"]}]"#;
        let detected = read_json_sample(text).unwrap();
        assert_eq!(detected.format, ImportFormat::Json);
        assert_eq!(detected.headers, vec!["id", "name", "email", "tags"]);
        assert_eq!(detected.sample_rows[0], vec!["1", "Ada", "", ""]);
        assert_eq!(detected.sample_rows[1][3], "[\"x\",\"y\"]");
    }

    #[test]
    fn ndjson_and_truncated_samples() {
        let text = "{\"id\": 1}\n{\"id\": 2, \"ok\": true}\n{\"id\": 3, \"trunc";
        let objects = parse_json_objects(text, None).unwrap();
        assert_eq!(objects.len(), 2, "the truncated trailing object is skipped");
        assert_eq!(json_cell(objects[1].get("ok")).as_deref(), Some("true"));
        assert_eq!(json_cell(objects[0].get("missing")), None);

        let truncated_array = r#"[{"id": 1}, {"id": 2}, {"id": 3, "x"#;
        assert_eq!(parse_json_objects(truncated_array, None).unwrap().len(), 2);
        assert_eq!(
            parse_json_objects(truncated_array, Some(1)).unwrap().len(),
            1
        );
    }

    #[test]
    fn json_is_detected_by_extension_or_content() {
        assert!(looks_like_json(Path::new("data.JSON"), "garbage"));
        assert!(looks_like_json(Path::new("data.txt"), "  [{\"a\": 1}]"));
        assert!(!looks_like_json(Path::new("data.csv"), "a,b\n1,2"));
        assert!(parse_json_objects("[1, 2]", None).is_err());
    }
}
