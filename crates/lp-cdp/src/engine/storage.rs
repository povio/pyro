//! Context cookie commands. Native validity and retention limits belong to
//! lp-net; CDP owns parameter normalization and wire representation.

use lp_net::cookies::{CookieJar, SameSite, MAX_TOTAL};
use lp_net::Url;
use serde::Deserialize;
use serde_json::{json, Value};

use super::EngineHost;
use crate::msg::{CdpError, CdpResult, Command};
use crate::target::ContextKey;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CookieParam {
    name: String,
    value: String,
    url: Option<String>,
    domain: Option<String>,
    path: Option<String>,
    expires: Option<f64>,
    #[serde(default)]
    secure: bool,
    #[serde(default)]
    http_only: bool,
    same_site: Option<String>,
    priority: Option<String>,
    source_scheme: Option<String>,
    source_port: Option<i32>,
    partition_key: Option<Value>,
    #[serde(default)]
    same_party: bool,
}

struct NormalizedCookie {
    name: String,
    value: String,
    domain: String,
    path: String,
    expires: Option<f64>,
    secure: bool,
    http_only: bool,
    same_site: SameSite,
}

fn invalid_cookie() -> CdpError {
    CdpError::invalid_params("Invalid cookie fields")
}

impl CookieParam {
    fn normalize(self) -> CdpResult<NormalizedCookie> {
        if self.partition_key.is_some() || self.same_party {
            return Err(CdpError::server("Partitioned and SameParty cookies are not supported"));
        }
        if self.priority.as_deref().is_some_and(|p| p != "Medium")
            || self.source_scheme.as_deref().is_some_and(|s| s != "Unset")
            || self.source_port.is_some_and(|p| p != -1)
        {
            return Err(CdpError::server("Cookie priority and source overrides are not supported"));
        }
        let url =
            self.url.filter(|s| !s.is_empty()).map(|s| Url::parse(&s).map_err(|_| invalid_cookie())).transpose()?;
        if url.as_ref().is_some_and(|u| !matches!(u.scheme(), "http" | "https") || u.host_str().is_none()) {
            return Err(invalid_cookie());
        }
        // As upstream/CDP, explicit domain overrides URL host; only a leading
        // dot requests a domain cookie. An HTTPS URL always implies Secure.
        let domain = self
            .domain
            .filter(|s| !s.is_empty())
            .or_else(|| url.as_ref().and_then(|u| u.host_str().map(str::to_owned)))
            .ok_or_else(invalid_cookie)?;
        let same_site = match self.same_site.as_deref() {
            None => SameSite::Unspecified,
            Some("Strict") => SameSite::Strict,
            Some("Lax") => SameSite::Lax,
            Some("None") => SameSite::None,
            _ => return Err(invalid_cookie()),
        };
        Ok(NormalizedCookie {
            name: self.name,
            value: self.value,
            domain,
            path: self.path.unwrap_or_else(|| "/".into()),
            expires: self.expires.filter(|v| *v >= 0.0),
            secure: self.secure || url.as_ref().is_some_and(|u| u.scheme() == "https"),
            http_only: self.http_only,
            same_site,
        })
    }
}

impl NormalizedCookie {
    fn apply(&self, jar: &mut CookieJar) -> bool {
        jar.set_cookie(
            &self.name,
            &self.value,
            &self.domain,
            &self.path,
            self.expires,
            self.secure,
            self.http_only,
            self.same_site,
        )
    }
}

impl EngineHost {
    pub(super) fn storage_command(&self, command: &Command<'_>, context: Option<ContextKey>) -> CdpResult<Value> {
        let context = context
            .and_then(|key| self.contexts.get(&key))
            .ok_or_else(|| CdpError::server("Browser context is gone"))?;
        let net =
            self.thread.browser.context_net(context.id).ok_or_else(|| CdpError::server("Browser context is gone"))?;
        match command.method.as_ref() {
            "Storage.getCookies" => {
                let cookies: Vec<_> = net.cookies().lock().unwrap().all().into_iter().map(|c| {
                    let mut wire = json!({
                        "name":c.name, "value":c.value,
                        "domain":if c.host_only { c.domain.clone() } else { format!(".{}", c.domain) },
                        "path":c.path, "expires":c.expires.unwrap_or(-1.0), "size":c.size(),
                        "httpOnly":c.http_only, "secure":c.secure, "session":c.expires.is_none(), "priority":"Medium",
                    });
                    if c.same_site != SameSite::Unspecified {
                        wire["sameSite"] = json!(match c.same_site {
                            SameSite::Strict => "Strict", SameSite::Lax => "Lax", SameSite::None => "None",
                            SameSite::Unspecified => unreachable!(),
                        });
                    }
                    wire
                }).collect();
                Ok(json!({"cookies":cookies}))
            }
            "Storage.clearCookies" => {
                net.cookies().lock().unwrap().clear();
                Ok(json!({}))
            }
            "Storage.setCookies" => {
                #[derive(Deserialize)]
                struct Params {
                    cookies: Vec<CookieParam>,
                }
                let params: Params = command.params()?;
                if params.cookies.len() > MAX_TOTAL {
                    return Err(CdpError::invalid_params("Too many cookies in one batch"));
                }
                let cookies: Vec<_> =
                    params.cookies.into_iter().map(CookieParam::normalize).collect::<CdpResult<_>>()?;
                // Validate every entry with the native rules before mutating
                // the real context, so malformed later entries leave it intact.
                let mut validation = CookieJar::new();
                for cookie in &cookies {
                    if !cookie.apply(&mut validation) {
                        return Err(invalid_cookie());
                    }
                }
                let mut jar = net.cookies().lock().unwrap();
                for cookie in cookies {
                    if !cookie.apply(&mut jar) {
                        return Err(invalid_cookie());
                    }
                }
                Ok(json!({}))
            }
            _ => Err(CdpError::unsupported(&command.method)),
        }
    }
}
