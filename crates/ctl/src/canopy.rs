//! The operator-facing side of the Canopy relay.
//!
//! Registering and withdrawing offers is deliberately absent: those are made by
//! the client that carries the requests, presenting the connection it will carry
//! them over, which an operator has nothing to present.

use clap::Subcommand;
use seedling_protocol::client::OiClient;
use serde_json::{Map, Value, json};

use super::print_result;

#[derive(Subcommand)]
pub(super) enum CanopyCommand {
    /// Show whether Canopy access is on, which client is carrying requests, and
    /// how the last status report went
    Status,
    /// Turn Canopy access on
    Enable,
    /// Turn Canopy access off.
    ///
    /// Refuses new offers and revokes any live one immediately, so it takes
    /// effect without waiting for the carrying client to reconnect.
    Disable,
    /// Send a status report now, in addition to the scheduled ones
    Report,
    /// Relay one request to Canopy and print the response.
    ///
    /// For checking the relay end to end. The path is Canopy's, in origin form
    /// (`/servers/self`); the carrying client resolves it against its own base
    /// URL and supplies the authentication.
    Request {
        /// HTTP method
        method: String,
        /// Request path, e.g. `/servers/self`
        path: String,
        /// Request body
        #[arg(long)]
        body: Option<String>,
        /// Request header as `name: value`. May be repeated.
        #[arg(long = "header", short = 'H')]
        headers: Vec<String>,
    },
}

// i[impl ctl.canopy]
pub(super) async fn dispatch(client: &OiClient, cmd: CanopyCommand) {
    match cmd {
        CanopyCommand::Status => {
            print_result(client.request("/canopy/status", json!({})).await);
        }
        CanopyCommand::Enable => {
            print_result(
                client
                    .request("/canopy/settings/set", json!({ "enabled": true }))
                    .await,
            );
        }
        CanopyCommand::Disable => {
            print_result(
                client
                    .request("/canopy/settings/set", json!({ "enabled": false }))
                    .await,
            );
        }
        CanopyCommand::Report => {
            print_result(client.request("/canopy/report", json!({})).await);
        }
        CanopyCommand::Request {
            method,
            path,
            body,
            headers,
        } => {
            let headers = match parse_headers(&headers) {
                Ok(headers) => headers,
                Err(e) => {
                    tracing::error!("{e}");
                    std::process::exit(1);
                }
            };
            let mut params = json!({
                "method": method.to_uppercase(),
                "path": path,
                "headers": headers,
            });
            if let (Some(map), Some(body)) = (params.as_object_mut(), body) {
                map.insert("body".into(), Value::String(body));
            }
            print_result(client.request("/canopy/request", params).await);
        }
    }
}

/// Parse `name: value` header arguments into the wire's header map.
///
/// Names are lower-cased to match the wire form. A repeated name has its values
/// combined with a comma and a space, as HTTP itself does, rather than the last
/// one silently winning.
fn parse_headers(raw: &[String]) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    for header in raw {
        let (name, value) = header
            .split_once(':')
            .ok_or_else(|| format!("header {header:?} is not in `name: value` form"))?;
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() {
            return Err(format!("header {header:?} has an empty name"));
        }
        let value = value.trim();
        match out.get(&name) {
            Some(Value::String(existing)) => {
                let combined = format!("{existing}, {value}");
                out.insert(name, Value::String(combined));
            }
            _ => {
                out.insert(name, Value::String(value.to_owned()));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(raw: &[&str]) -> Result<Map<String, Value>, String> {
        parse_headers(&raw.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
    }

    // i[verify ctl.canopy]
    #[test]
    fn a_header_is_split_on_the_first_colon_only() {
        // A value may itself contain a colon, as a URL does.
        let out = headers(&["x-origin: https://example.invalid:8443/x"]).unwrap();
        assert_eq!(
            out["x-origin"],
            Value::String("https://example.invalid:8443/x".into())
        );
    }

    // i[verify ctl.canopy]
    #[test]
    fn header_names_are_lower_cased() {
        let out = headers(&["Content-Type: application/json"]).unwrap();
        assert!(out.contains_key("content-type"));
        assert!(!out.contains_key("Content-Type"));
    }

    // i[verify ctl.canopy]
    #[test]
    fn a_repeated_header_combines_rather_than_losing_a_value() {
        let out = headers(&["accept: text/plain", "Accept: application/json"]).unwrap();
        assert_eq!(
            out["accept"],
            Value::String("text/plain, application/json".into())
        );
    }

    // i[verify ctl.canopy]
    #[test]
    fn a_header_without_a_colon_is_rejected() {
        let err = headers(&["not-a-header"]).expect_err("no colon");
        assert!(err.contains("name: value"), "{err}");
    }

    // i[verify ctl.canopy]
    #[test]
    fn a_header_with_an_empty_name_is_rejected() {
        assert!(headers(&[": value"]).is_err());
    }

    // i[verify ctl.canopy]
    #[test]
    fn an_empty_header_value_is_allowed() {
        // A present-but-empty header is meaningful in HTTP, so it is not an error.
        let out = headers(&["x-empty:"]).unwrap();
        assert_eq!(out["x-empty"], Value::String(String::new()));
    }
}
