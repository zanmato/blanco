use anyhow::{Context, Result};
use encoding_rs::Encoding;
use std::fs::File;
use std::io::Read;
use std::path::Path;

const SAMPLE_BYTES: usize = 64 * 1024;
const SAMPLE_ROWS: usize = 5;
const CANDIDATE_DELIMITERS: &[u8] = b",;\t|";

#[derive(Debug, Clone)]
pub struct DetectedFile {
    pub encoding: &'static Encoding,
    pub delimiter: u8,
    pub has_header_guess: bool,
    pub headers: Vec<String>,
    pub sample_rows: Vec<Vec<String>>,
}

pub fn detect_file(path: &Path) -> Result<DetectedFile> {
    let raw = read_sample_bytes(path)?;
    let encoding = detect_encoding(&raw);
    let decoded = decode_sample(&raw, encoding);
    let delimiter = detect_delimiter(&decoded);
    read_sample(&decoded, delimiter, true)
        .map(|mut detected| {
            detected.encoding = encoding;
            detected.delimiter = delimiter;
            detected
        })
        .context("failed to read CSV sample")
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

    let has_header_guess = has_header && guess_header_is_header(&headers, &sample_rows);
    Ok(DetectedFile {
        encoding: encoding_rs::UTF_8,
        delimiter,
        has_header_guess,
        headers,
        sample_rows,
    })
}

fn guess_header_is_header(headers: &[String], sample_rows: &[Vec<String>]) -> bool {
    if headers.is_empty() || sample_rows.is_empty() {
        return !headers.is_empty();
    }
    let header_numeric = headers.iter().filter(|h| looks_numeric(h)).count();
    let first_row_numeric = sample_rows
        .first()
        .map(|row| row.iter().filter(|v| looks_numeric(v)).count())
        .unwrap_or(0);
    header_numeric <= first_row_numeric
}

fn looks_numeric(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }
    trimmed.parse::<f64>().is_ok()
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
}
