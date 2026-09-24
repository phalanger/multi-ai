//! JSON Lines encoding helpers.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Serialize `msg` as one JSON line terminated by `\n`.
///
/// serde_json escapes newlines inside strings, so the output always
/// contains exactly one `\n`.
pub fn encode_line<T: Serialize>(msg: &T) -> Result<String, serde_json::Error> {
    let mut s = serde_json::to_string(msg)?;
    s.push('\n');
    Ok(s)
}

/// Parse one line; a trailing `\n` or `\r\n` is ignored.
pub fn decode_line<T: DeserializeOwned>(
    line: &str,
) -> Result<T, serde_json::Error> {
    serde_json::from_str(line.trim_end_matches(['\r', '\n']))
}
