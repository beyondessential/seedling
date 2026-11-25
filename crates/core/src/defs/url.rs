//! The values the `url`, `url-origin` and `hostname` kinds' parsers return.

use ::url::Host;
use percent_encoding::percent_decode_str;

use super::{ingress::check_hostname, param_def::line_breaking_character};

/// The components of a URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub scheme: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub path: String,
    pub query: Option<String>,
    pub fragment: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

/// The scheme, host and port of an `http` or `https` URL that names nothing
/// more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlOrigin {
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

/// The hostname `text` names, as an ingress can be declared on it.
pub fn parse_hostname(text: &str) -> Result<String, String> {
    // spec: URL#hostnames
    let ascii = idna::domain_to_ascii_strict(text)
        .map_err(|_| format!("{text:?} is not a valid domain name"))?;
    // spec: URL#hostnames
    check_hostname(&ascii)?;
    // The URL parser reads a name ending in a number as an IPv4 address, and
    // gives a domain back only for one that does not.
    // spec: URL#hostnames
    if !matches!(Host::parse(&ascii), Ok(Host::Domain(_))) {
        return Err(format!(
            "{ascii:?} ends in a number, so a URL would read it as an IPv4 address"
        ));
    }
    Ok(ascii)
}

impl Url {
    pub fn parse(text: &str) -> Result<Self, String> {
        Self::from_parsed(&parse(text)?)
    }

    fn from_parsed(url: &::url::Url) -> Result<Self, String> {
        // spec: URL#hosts
        let host = match url.host() {
            None | Some(Host::Domain("")) => None,
            Some(Host::Domain(opaque)) if !url.is_special() => Some(
                Host::parse(opaque).map_or_else(|_| opaque.to_owned(), |host| host.to_string()),
            ),
            Some(host) => Some(host.to_string()),
        };

        // spec: URL#ports
        if url.port() == Some(0) {
            return Err("a URL's port is not 0".to_owned());
        }

        Ok(Self {
            scheme: url.scheme().to_owned(),
            host,
            // spec: URL#ports
            port: url.port_or_known_default(),
            path: url.path().to_owned(),
            // spec: URL#components
            query: non_empty(url.query()),
            fragment: non_empty(url.fragment()),
            username: userinfo("username", url.username())?,
            password: userinfo("password", url.password().unwrap_or_default())?,
        })
    }
}

impl UrlOrigin {
    pub fn parse(text: &str) -> Result<Self, String> {
        let parsed = parse(text)?;

        // spec: URL#url-origins
        let scheme = parsed.scheme();
        if !matches!(scheme, "http" | "https") {
            return Err(format!(
                "a URL origin's scheme is http or https, not {scheme}"
            ));
        }
        // spec: URL#url-origins
        if has_userinfo(text) {
            return Err("a URL origin has no `@` before its host".to_owned());
        }
        // spec: URL#url-origins
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err("a URL origin has no query or fragment".to_owned());
        }
        // spec: URL#url-origins
        if parsed.path() != "/" {
            return Err(format!(
                "a URL origin has no path beyond `/`, not {:?}",
                parsed.path()
            ));
        }

        let url = Url::from_parsed(&parsed)?;
        let (Some(host), Some(port)) = (url.host, url.port) else {
            return Err("a URL origin names a host".to_owned());
        };
        // spec: URL#url-origins
        let host = parse_hostname(&host).map_err(|e| format!("a URL origin's host: {e}"))?;
        Ok(Self {
            scheme: url.scheme,
            host,
            port,
        })
    }
}

// spec: URL#urls
fn parse(text: &str) -> Result<::url::Url, String> {
    ::url::Url::parse(text).map_err(|e| format!("not a URL: {e}"))
}

fn non_empty(part: Option<&str>) -> Option<String> {
    part.filter(|part| !part.is_empty()).map(str::to_owned)
}

/// A username or password, percent-decoded, or `None` when empty.
// spec: URL#components
// spec: URL#userinfo
fn userinfo(component: &str, encoded: &str) -> Result<Option<String>, String> {
    if encoded.is_empty() {
        return Ok(None);
    }
    let decoded = percent_decode_str(encoded)
        .decode_utf8()
        .map_err(|_| format!("the URL's {component} is not valid UTF-8 once percent-decoded"))?;
    if let Some(c) = line_breaking_character(&decoded) {
        return Err(format!(
            "the URL's {component} contains U+{:04X}, which a single-line value cannot hold",
            u32::from(c)
        ));
    }
    Ok(Some(decoded.into_owned()))
}

/// Whether the authority of `text`, an `http` or `https` URL the URL parser
/// has accepted, holds an `@`, read as the parser reads it: with every tab,
/// line feed and carriage return removed, after the scheme's `:` and any `/`
/// or `\`, up to the next `/`, `\`, `?` or `#`. The parser drops an empty
/// username and password, so this is the only way to see one.
fn has_userinfo(text: &str) -> bool {
    let text: String = text
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let Some((_, rest)) = text.split_once(':') else {
        return false;
    };
    rest.trim_start_matches(['/', '\\'])
        .split(['/', '\\', '?', '#'])
        .next()
        .is_some_and(|authority| authority.contains('@'))
}
