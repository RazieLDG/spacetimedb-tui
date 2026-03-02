//! HTTP client for the SpacetimeDB REST API.
//!
//! [`SpacetimeClient`] wraps a [`reqwest::Client`] and exposes typed methods
//! for every endpoint used by the TUI.  All methods are `async` and return
//! `anyhow::Result<T>` so that callers can use the `?` operator freely.

use anyhow::{anyhow, bail, Context, Result};
use reqwest::{header, Client, StatusCode};
use serde_json::Value;
use tracing::{debug, instrument, warn};

use super::types::{LogEntry, QueryResult, Schema, SchemaElement, SchemaResponse};

// ---------------------------------------------------------------------------
// Client struct
// ---------------------------------------------------------------------------

/// A thin, cheaply-cloneable HTTP client for SpacetimeDB.
///
/// All methods accept a `database` name and hit the appropriate endpoint on
/// the configured `base_url`.
#[derive(Debug, Clone)]
pub struct SpacetimeClient {
    /// Base URL, e.g. `http://localhost:3000`.
    base_url: String,
    /// Underlying HTTP client (connection-pooled, cheaply cloned).
    http: Client,
    /// Optional authentication token.
    auth_token: Option<String>,
}

impl SpacetimeClient {
    /// Create a new client pointing at `base_url`.
    ///
    /// # Errors
    /// Returns an error if `reqwest::Client` cannot be built (e.g. invalid
    /// TLS configuration).
    pub fn new(base_url: impl Into<String>, auth_token: Option<String>) -> Result<Self> {
        let base_url = base_url.into();
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/json"),
        );

        let http = Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to build HTTP client")?;

        Ok(Self {
            base_url,
            http,
            auth_token,
        })
    }

    /// Convenience constructor using host + port.
    pub fn from_host_port(host: &str, port: u16, auth_token: Option<String>) -> Result<Self> {
        let base_url = format!("http://{}:{}", host, port);
        Self::new(base_url, auth_token)
    }

    /// The WebSocket base URL derived from the HTTP base URL.
    pub fn ws_base_url(&self) -> String {
        self.base_url
            .replacen("http://", "ws://", 1)
            .replacen("https://", "wss://", 1)
    }

    /// Attach (or replace) the bearer auth token.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Build a `GET` request, attaching the auth token when present.
    fn get(&self, url: &str) -> reqwest::RequestBuilder {
        let req = self.http.get(url);
        self.maybe_auth(req)
    }

    /// Build a `POST` request, attaching the auth token when present.
    fn post(&self, url: &str) -> reqwest::RequestBuilder {
        let req = self.http.post(url);
        self.maybe_auth(req)
    }

    fn maybe_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth_token {
            Some(token) => req.bearer_auth(token),
            None => req,
        }
    }

    /// Send a request and deserialise the JSON body into `T`.
    async fn send_json<T>(&self, req: reqwest::RequestBuilder) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let resp = req.send().await.context("HTTP request failed")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("HTTP {status}: {body}");
        }
        resp.json::<T>().await.context("Failed to decode JSON response")
    }

    // ------------------------------------------------------------------
    // Public API methods
    // ------------------------------------------------------------------

    /// Execute a SQL statement against `database` and return the result set.
    ///
    /// SpacetimeDB endpoint: `POST /v1/sql/<database>`
    #[instrument(skip(self, sql), fields(db = %database))]
    pub async fn query_sql(&self, database: &str, sql: &str) -> Result<QueryResult> {
        let url = format!("{}/v1/database/{}/sql", self.base_url, database);
        debug!("SQL query: {}", sql);

        let resp = self
            .post(&url)
            .body(sql.to_owned())
            .header(header::CONTENT_TYPE, "text/plain")
            .send()
            .await
            .context("SQL query request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("SQL query HTTP {status}: {body}");
        }

        // SpacetimeDB returns an array of result sets; we take the first.
        let raw: Value = resp.json().await.context("Failed to decode SQL response")?;
        parse_query_result(raw)
    }

    /// Fetch the full schema (tables, reducers, typespace) for `database`.
    ///
    /// SpacetimeDB endpoint: `GET /v1/database/<database>/schema`
    #[instrument(skip(self), fields(db = %database))]
    pub async fn get_schema(&self, database: &str) -> Result<Schema> {
        let url = format!("{}/v1/database/{}/schema", self.base_url, database);
        debug!("Fetching schema");

        let resp = self
            .get(&url)
            .query(&[("version", "9")])
            .send()
            .await
            .context("Schema request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("Schema HTTP {status}: {body}");
        }

        let raw: Value = resp.json().await.context("Failed to decode schema response")?;
        parse_schema_response(raw)
    }

    /// Retrieve the last `num_lines` log lines for `database`.
    ///
    /// SpacetimeDB endpoint: `GET /v1/database/<database>/logs`
    ///
    /// When `follow` is `true` the server streams logs; this method collects
    /// the whole stream until EOF and returns all lines.  For live streaming
    /// use [`crate::api::ws::WsClient`] instead.
    #[instrument(skip(self), fields(db = %database))]
    pub async fn get_logs(
        &self,
        database: &str,
        num_lines: u32,
        follow: bool,
    ) -> Result<Vec<LogEntry>> {
        let url = format!("{}/v1/database/{}/logs", self.base_url, database);
        debug!(num_lines, follow, "Fetching logs");

        let resp = self
            .get(&url)
            .query(&[
                ("num_lines", num_lines.to_string()),
                ("follow", follow.to_string()),
            ])
            .send()
            .await
            .context("Logs request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("Logs HTTP {status}: {body}");
        }

        // The server returns newline-delimited JSON (one object per line).
        let text = resp.text().await.context("Failed to read log body")?;
        parse_ndjson_logs(&text)
    }

    /// List all databases visible to the authenticated identity.
    ///
    /// SpacetimeDB 2.0 endpoint: `GET /v1/identity/{hex_identity}/databases`
    ///
    /// The identity is extracted directly from the JWT bearer token without
    /// making an additional network request.  The response contains database
    /// *identities* (hex strings), which are valid database references for all
    /// other API endpoints.
    #[instrument(skip(self))]
    pub async fn list_databases(&self) -> Result<Vec<String>> {
        // Extract the caller's identity from the JWT payload.
        let identity = self
            .auth_token
            .as_deref()
            .and_then(extract_identity_from_jwt)
            .ok_or_else(|| {
                anyhow!(
                    "No auth token configured or cannot parse identity from JWT.\n\
                     Run `spacetime server login` or pass --token."
                )
            })?;

        let url = format!("{}/v1/identity/{}/databases", self.base_url, identity);
        debug!(
            "Listing databases for identity {}…",
            &identity[..identity.len().min(12)]
        );

        let resp = self
            .get(&url)
            .send()
            .await
            .context("List databases request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("List databases HTTP {status}: {body}");
        }

        let raw: Value = resp
            .json()
            .await
            .context("Failed to decode databases response")?;

        // Response: {"identities": ["hex1", "hex2", ...]}
        let identities = raw
            .get("identities")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();

        Ok(identities)
    }

    /// Ping the server and return `true` if it responds.
    pub async fn ping(&self) -> bool {
        let url = format!("{}/v1/ping", self.base_url);
        self.get(&url).send().await.map(|r| r.status().is_success()).unwrap_or(false)
    }

    /// Fetch server metrics (Prometheus format).
    pub async fn get_metrics(&self) -> Result<String> {
        let url = format!("{}/metrics", self.base_url);
        let resp = self.get(&url).send().await.context("Metrics request failed")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("Metrics HTTP {status}: {body}");
        }
        resp.text().await.context("Failed to read metrics body")
    }

    /// Return the configured base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

// ---------------------------------------------------------------------------
// JWT helpers
// ---------------------------------------------------------------------------

/// Extract the `hex_identity` field from a JWT bearer token.
///
/// Splits the token on `.`, base64url-decodes the payload (middle part), and
/// parses the resulting JSON without requiring any external JWT crate.
pub fn extract_identity_from_jwt(token: &str) -> Option<String> {
    let mut parts = token.splitn(3, '.');
    let _header = parts.next()?;
    let payload_b64 = parts.next()?;
    let payload_bytes = base64url_decode(payload_b64)?;
    let json: Value = serde_json::from_slice(&payload_bytes).ok()?;
    json.get("hex_identity")?.as_str().map(String::from)
}

/// Decode a base64url-encoded string (no padding required) into raw bytes.
fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    // Translate base64url alphabet → standard base64.
    let mut s = input.replace('-', "+").replace('_', "/");
    // Restore padding.
    match s.len() % 4 {
        2 => s.push_str("=="),
        3 => s.push('='),
        _ => {}
    }
    base64_decode(s.as_bytes())
}

/// Minimal, allocation-efficient standard base64 decoder.
fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    let decode_byte = |c: u8| -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            b'=' => Some(0), // padding — value is discarded
            _ => None,
        }
    };

    let mut result = Vec::with_capacity(input.len() * 3 / 4);
    let mut i = 0;

    while i + 4 <= input.len() {
        let a = decode_byte(input[i])?;
        let b = decode_byte(input[i + 1])?;
        let c = decode_byte(input[i + 2])?;
        let d = decode_byte(input[i + 3])?;

        result.push((a << 2) | (b >> 4));
        if input[i + 2] != b'=' {
            result.push((b << 4) | (c >> 2));
        }
        if input[i + 3] != b'=' {
            result.push((c << 6) | d);
        }
        i += 4;
    }

    Some(result)
}

// ---------------------------------------------------------------------------
// Response parsers
// ---------------------------------------------------------------------------

/// Parse the raw SQL response value into a [`QueryResult`].
///
/// SpacetimeDB wraps the result set in an array:
/// ```json
/// [{"schema": [...], "rows": [...], "total_duration_micros": 42}]
/// ```
fn parse_query_result(raw: Value) -> Result<QueryResult> {
    // The server may return either an array of result sets or a single object.
    let obj = match raw {
        Value::Array(mut arr) if !arr.is_empty() => arr.swap_remove(0),
        Value::Object(_) => raw,
        Value::Array(_) => {
            // Empty result set.
            return Ok(QueryResult {
                schema: Vec::new(),
                rows: Vec::new(),
                total_duration_micros: 0,
            });
        }
        other => bail!("Unexpected SQL response shape: {other}"),
    };

    // Schema is an array of {name, algebraic_type} objects.
    let schema = obj
        .get("schema")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("SQL response missing 'schema' array"))?;

    let schema_elements: Vec<SchemaElement> = schema
        .iter()
        .map(|col| {
            let name = col
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let algebraic_type = col.get("algebraic_type").cloned().unwrap_or(Value::Null);
            SchemaElement { name, algebraic_type }
        })
        .collect();

    let rows: Vec<Vec<Value>> = obj
        .get("rows")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|row| {
                    row.as_array().cloned().unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();

    let total_duration_micros = obj
        .get("total_duration_micros")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    Ok(QueryResult {
        schema: schema_elements,
        rows,
        total_duration_micros,
    })
}

/// Parse the raw schema response into a [`SchemaResponse`].
fn parse_schema_response(raw: Value) -> Result<SchemaResponse> {
    serde_json::from_value(raw).context("Failed to deserialise schema response")
}

/// Parse a newline-delimited JSON log stream into `Vec<LogEntry>`.
fn parse_ndjson_logs(text: &str) -> Result<Vec<LogEntry>> {
    let mut entries = Vec::new();
    for (line_num, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<LogEntry>(trimmed) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                warn!("Failed to parse log line {}: {} — {:?}", line_num + 1, e, trimmed);
            }
        }
    }
    Ok(entries)
}

/// Extract a flat list of database names from a JSON value.
///
/// Handles several shapes returned by different SpacetimeDB versions:
/// - `["name1", "name2"]`
/// - `{"databases": ["name1", ...]}`
/// - `{"databases": [{"database_identity": "...", "database_name": "name"}, ...]}`
fn extract_database_names(raw: Value) -> Result<Vec<String>> {
    let arr = match raw {
        Value::Array(a) => a,
        Value::Object(o) => {
            let inner = o
                .get("databases")
                .or_else(|| o.get("names"))
                .cloned()
                .unwrap_or(Value::Null);
            match inner {
                Value::Array(a) => a,
                _ => return Ok(Vec::new()),
            }
        }
        _ => return Ok(Vec::new()),
    };

    let names = arr
        .into_iter()
        .filter_map(|item| match item {
            Value::String(s) => Some(s),
            Value::Object(ref _map) => {
                // Try common field names.
                item.get("database_name")
                    .or_else(|| item.get("name"))
                    .or_else(|| item.get("database_identity"))
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            }
            _ => None,
        })
        .collect();

    Ok(names)
}

// ---------------------------------------------------------------------------
// Unit tests (no network required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_query_result_array_wrapper() {
        let raw = json!([{
            "schema": [{"name": "id", "algebraic_type": "U64"}],
            "rows": [[1], [2]],
            "total_duration_micros": 100
        }]);
        let result = parse_query_result(raw).unwrap();
        assert_eq!(result.schema.len(), 1);
        assert_eq!(result.schema[0].name, "id");
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.total_duration_micros, 100);
    }

    #[test]
    fn test_parse_query_result_empty_array() {
        let raw = json!([]);
        let result = parse_query_result(raw).unwrap();
        assert_eq!(result.row_count(), 0);
    }

    #[test]
    fn test_extract_database_names_flat_array() {
        let raw = json!(["alpha", "beta", "gamma"]);
        let names = extract_database_names(raw).unwrap();
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn test_extract_database_names_wrapped() {
        let raw = json!({"databases": ["alpha", "beta"]});
        let names = extract_database_names(raw).unwrap();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn test_extract_database_names_object_array() {
        let raw = json!({"databases": [
            {"database_name": "alpha"},
            {"name": "beta"}
        ]});
        let names = extract_database_names(raw).unwrap();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn test_parse_ndjson_logs_valid() {
        let text = r#"{"level":"info","message":"started"}
{"level":"error","message":"boom"}"#;
        let entries = parse_ndjson_logs(text).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message, "started");
        assert_eq!(entries[1].message, "boom");
    }

    #[test]
    fn test_parse_ndjson_logs_skips_bad_lines() {
        let text = "not json\n{\"level\":\"info\",\"message\":\"ok\"}";
        let entries = parse_ndjson_logs(text).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message, "ok");
    }
}
