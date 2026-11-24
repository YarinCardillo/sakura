use super::model::{parse_version, CheckError, Release};
use serde_json::Value;
use url::Url;

pub struct Repository {
    owner: String,
    name: String,
}

impl Repository {
    /// Only public GitHub HTTPS metadata is accepted; credentials are never used.
    pub fn parse(url: &str) -> Option<Self> {
        let url = Url::parse(url).ok()?;
        if !is_public_github_url(&url) {
            return None;
        }
        let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
        let [owner, name] = parts[..] else {
            return None;
        };
        let name = name.strip_suffix(".git").unwrap_or(name);
        if ![owner, name].into_iter().all(is_component) {
            return None;
        }
        Some(Self {
            owner: owner.into(),
            name: name.into(),
        })
    }

    /// The unauthenticated endpoint; no per-user query parameters or headers.
    pub fn api_url(&self) -> String {
        format!(
            "https://api.github.com/repos/{}/{}/releases/latest",
            self.owner, self.name
        )
    }

    /// A published stable release with a valid version and a link belonging to this repository.
    pub fn release(&self, body: &str) -> Result<Release, CheckError> {
        let value: Value = serde_json::from_str(body).map_err(|_| CheckError)?;
        let text = |key| value.get(key).and_then(Value::as_str).ok_or(CheckError);
        if value.get("draft").and_then(Value::as_bool) != Some(false)
            || value.get("prerelease").and_then(Value::as_bool) != Some(false)
        {
            return Err(CheckError);
        }
        let tag = text("tag_name")?;
        let version = parse_version(tag)?;
        if !version.pre.is_empty() {
            return Err(CheckError);
        }
        let url = self.release_url(text("html_url")?, tag)?;
        Ok(Release {
            version: version.to_string(),
            url,
        })
    }

    fn release_url(&self, text: &str, tag: &str) -> Result<String, CheckError> {
        let url = Url::parse(text).map_err(|_| CheckError)?;
        if !is_public_github_url(&url) {
            return Err(CheckError);
        }
        let parts: Vec<_> = url.path_segments().ok_or(CheckError)?.collect();
        let [owner, name, "releases", "tag", encoded_tag] = parts.as_slice() else {
            return Err(CheckError);
        };
        let decoded = percent_encoding::percent_decode_str(encoded_tag)
            .decode_utf8()
            .map_err(|_| CheckError)?;
        if !owner.eq_ignore_ascii_case(&self.owner)
            || !name.eq_ignore_ascii_case(&self.name)
            || decoded != tag
        {
            return Err(CheckError);
        }
        Ok(url.to_string())
    }
}

fn is_public_github_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

fn is_component(text: &str) -> bool {
    !text.is_empty()
        && text != "."
        && text != ".."
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

impl super::model::Checker for Repository {
    fn check(&self) -> Result<Release, CheckError> {
        const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(0)
            .proxy(None)
            .user_agent("Sakura")
            .timeout_global(Some(std::time::Duration::from_secs(20)))
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let mut response = agent
            .get(self.api_url())
            .header(ureq::http::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .call()
            .map_err(|_| CheckError)?;
        if response.status() != 200 {
            return Err(CheckError);
        }
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_string()
            .map_err(|_| CheckError)?;
        self.release(&body)
    }
}
