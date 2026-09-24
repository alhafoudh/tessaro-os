//! Just enough URL handling for the questions this program asks:
//! "where does a redirect point" and "is the browser still on our site".
//!
//! Deliberately not a URL crate. Both callers work on absolute http(s) URLs
//! that a server or Chromium produced, and the alternative is a dependency
//! (and a line in the recipe's crate list) to compare two strings.

/// `scheme://host[:port]` of an absolute http(s) URL, or `None` for anything
/// else - `data:`, `file:`, `about:blank`, or a relative reference.
pub fn origin(url: &str) -> Option<&str> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }

    let scheme_end = url.find("://")? + 3;
    let authority_end = url[scheme_end..]
        .find(['/', '?', '#'])
        .map(|index| scheme_end + index)
        .unwrap_or(url.len());

    let origin = &url[..authority_end];

    // "https://" with nothing after it is not an origin.
    if origin.len() == scheme_end {
        None
    } else {
        Some(origin)
    }
}

/// Resolve a `Location` header against the URL it came from.
///
/// Absolute, root-relative and path-relative are what real servers send;
/// anything stranger is passed through and fails on the next hop with a
/// message that says so.
pub fn join(base: &str, location: &str) -> String {
    if location.starts_with("http://") || location.starts_with("https://") {
        return location.to_string();
    }

    let origin = match origin(base) {
        Some(origin) => origin,
        None => return location.to_string(),
    };

    if location.starts_with('/') {
        return format!("{origin}{location}");
    }

    let path = &base[origin.len()..];
    let path = path.split(['?', '#']).next().unwrap_or("");
    let directory = match path.rfind('/') {
        Some(index) => &path[..=index],
        None => "/",
    };

    format!("{origin}{directory}{location}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_of_an_http_url() {
        assert_eq!(origin("https://a.test/x/y?q=1"), Some("https://a.test"));
        assert_eq!(origin("http://a.test:8080/x"), Some("http://a.test:8080"));
        assert_eq!(origin("https://a.test"), Some("https://a.test"));
    }

    #[test]
    fn origin_distinguishes_scheme_host_and_port() {
        assert_ne!(origin("https://a.test/"), origin("http://a.test/"));
        assert_ne!(origin("https://a.test/"), origin("https://b.test/"));
        assert_ne!(origin("https://a.test/"), origin("https://a.test:8443/"));
    }

    #[test]
    fn non_http_urls_have_no_origin() {
        assert_eq!(origin("about:blank"), None);
        assert_eq!(origin("file:///run/tessaro-kiosk/index.html"), None);
        assert_eq!(origin("data:text/html,<h1>hi</h1>"), None);
        assert_eq!(origin("https://"), None);
        assert_eq!(origin(""), None);
    }

    #[test]
    fn join_handles_the_three_shapes() {
        assert_eq!(
            join("http://a.test/x/y", "https://b.test/z"),
            "https://b.test/z"
        );
        assert_eq!(join("http://a.test/x/y", "/z"), "http://a.test/z");
        assert_eq!(join("http://a.test/x/y", "z"), "http://a.test/x/z");
        assert_eq!(join("http://a.test", "z"), "http://a.test/z");
        assert_eq!(join("http://a.test/x/y?q=1", "z"), "http://a.test/x/z");
    }
}
