# Page-local user-agent identity

`UserAgentOverride { user_agent, accept_language, platform }` is independent of
the shared context NetClient. `PageDom::set_user_agent_override(Option<...>)`
validates both HTTP header values before updating any field and returns
`http::header::InvalidHeaderValue` on invalid input. `None` clears all fields;
an empty `user_agent` retains the client's normal UA while optional language
and platform overrides still apply. Platform affects navigator only.

Existing navigator wrappers and isolated worlds read the current page values.
Future fetch/XHR requests snapshot prevalidated `pyro_net::RequestDefaults` at
send time. Requests already started retain their snapshot through redirects.
Explicit author headers retain precedence over defaults; existing context extra
headers retain their precedence. Defaults are applied before Network observation
and request-stage interception, but are separate from author headers used for
CORS preflight decisions. A page UA override alone does not trigger preflight.

The browser embedder owns persistence across navigation: retain the override
and `UserAgentOverride::request_defaults()` result per Page, apply the override
to each new PageDom before realm installation, and copy defaults into
`pyro_net::Request.defaults` for document/script/stylesheet loads. The integrated
[browser setter](../browser/USER_AGENT.md) and CDP mapper now provide this wiring.
Never change the shared NetClient's settings to implement a page override. CDP `userAgentMetadata` becomes `UserAgentOverride::metadata`, which `navigator.userAgentData` reports (an override without it reports empty hints, as Chrome); its low-entropy hints become `RequestDefaults::client_hints` (Sec-CH-UA, -Mobile, -Platform), which pyro-net sends only to potentially trustworthy origins and never over request-authored headers.

The unchanged TOS smoke after IndexedDB discovery (browser86ff4ee, TOSf9fde4aa0d)
progressed past storage-state setup but failed both tests at newPage because
Emulation.setUserAgentOverride was unsupported. The exact locale-only request
from Playwright1.59.1 is `{userAgent:"",acceptLanguage:"en-US"}`; platform and
metadata are omitted. This primitive is an integration dependency for that
failure, not evidence that the complete TOS flow passes.

Tests use two pages sharing one network client, real JavaScriptCore and local
HTTP: navigator/header isolation, atomic invalid-header rejection, empty-UA
locale behavior, reset, redirect and in-flight snapshots, observed headers and
a simple cross-origin request that must not issue OPTIONS.

The later [exact integrated TOS smoke](../validation/TOS_IDENTITY_SMOKE.md) passed
page creation and reached routes, then failed on missing MutationObserver and
URLSearchParams; the complete flow still does not pass.
