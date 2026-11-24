use base64::{engine::general_purpose::STANDARD, Engine as _};
use url::Url;

/// Accepts a public HTTPS feed and an encoded EdDSA public key, never credentials.
pub fn has_metadata(feed: Option<&str>, key: Option<&str>) -> bool {
    let (Some(feed), Some(key)) = (feed, key) else {
        return false;
    };
    let Ok(url) = Url::parse(feed) else {
        return false;
    };
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && key.len() == 44
        && STANDARD.decode(key).is_ok_and(|key| key.len() == 32)
}
