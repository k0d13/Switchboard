//! Xbox Live over HTTP: Microsoft account device-code sign-in, Xbox user token, XSTS, then the
//! presence and titlehub APIs.

use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Value, json};
use ureq::Agent;
use ureq::tls::{TlsConfig, TlsProvider};

const PUBLIC_CLIENT_ID: &str = "000000004C12AE6F";
const DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// Refresh the XSTS token when it has less than this left.
const XSTS_MARGIN_SECS: u64 = 5 * 60;
/// Used when the token's expiry can't be parsed.
const XSTS_FALLBACK_SECS: u64 = 30 * 60;

pub fn agent() -> Agent {
    agent_with_timeout(Duration::from_secs(20))
}

fn agent_with_timeout(timeout: Duration) -> Agent {
    Agent::config_builder()
        .tls_config(TlsConfig::builder().provider(TlsProvider::NativeTls).build())
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .build()
        .new_agent()
}

#[derive(Debug)]
pub enum XboxError {
    /// Microsoft rejected the saved refresh token, so the user has to sign in again.
    SignInExpired,
    Failed(String),
}

impl fmt::Display for XboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SignInExpired => write!(f, "Xbox sign-in expired"),
            Self::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for XboxError {}

fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split('/').next().unwrap_or(rest)
}

fn failed(url: &str, e: impl fmt::Display) -> XboxError {
    XboxError::Failed(format!("{}: {e}", host(url)))
}

/// The two ways of signing in with a Microsoft account: the Xbox app's own public client on the
/// legacy endpoints, or the user's Azure app registration on the v2 endpoints.
#[derive(Clone, Debug, PartialEq)]
pub struct Msa {
    client_id: String,
    own_client: bool,
}

impl Msa {
    pub fn new(ms_client_id: &str) -> Self {
        let id = ms_client_id.trim();
        if id.is_empty() {
            Self { client_id: PUBLIC_CLIENT_ID.into(), own_client: false }
        } else {
            Self { client_id: id.into(), own_client: true }
        }
    }

    fn device_code_url(&self) -> &'static str {
        if self.own_client {
            "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode"
        } else {
            "https://login.live.com/oauth20_connect.srf"
        }
    }

    fn token_url(&self) -> &'static str {
        if self.own_client {
            "https://login.microsoftonline.com/consumers/oauth2/v2.0/token"
        } else {
            "https://login.live.com/oauth20_token.srf"
        }
    }

    fn scope(&self) -> &'static str {
        if self.own_client { "XboxLive.signin offline_access" } else { "service::user.auth.xboxlive.com::MBI_SSL" }
    }

    fn ticket(&self, access_token: &str) -> String {
        if self.own_client { format!("d={access_token}") } else { access_token.to_string() }
    }
}

/// Posts a form and returns the status and JSON body. MSA reports errors as JSON with a 400, so
/// the body matters more than the status.
fn form(agent: &Agent, msa: &Msa, url: &str, params: &[(&str, &str)]) -> Result<(u16, Value), String> {
    let mut fields = vec![("client_id", msa.client_id.as_str())];
    fields.extend_from_slice(params);
    let mut resp = agent.post(url).send_form(fields).map_err(|e| format!("{}: {e}", host(url)))?;
    let status = resp.status().as_u16();
    let body = resp.body_mut().read_to_string().unwrap_or_default();
    Ok((status, serde_json::from_str(&body).unwrap_or(Value::Null)))
}

#[derive(Clone, Debug, Deserialize)]
pub struct DeviceCode {
    #[serde(default)]
    pub device_code: String,
    #[serde(default)]
    pub user_code: String,
    #[serde(default)]
    pub verification_uri: String,
    pub expires_in: Option<u64>,
    pub interval: Option<u64>,
}

pub fn request_device_code(agent: &Agent, msa: &Msa) -> Result<DeviceCode, String> {
    let mut params = vec![("scope", msa.scope())];
    if !msa.own_client {
        params.push(("response_type", "device_code"));
    }
    let (_, body) = form(agent, msa, msa.device_code_url(), &params)?;
    serde_json::from_value::<DeviceCode>(body.clone())
        .ok()
        .filter(|d| !d.device_code.is_empty() && !d.user_code.is_empty())
        .ok_or_else(|| format!("Couldn't start sign-in: {body}"))
}

pub struct Tokens {
    pub refresh_token: Option<String>,
}

pub enum Poll {
    Pending,
    SlowDown,
    Done(Tokens),
    /// Network trouble or an odd response: worth trying again a few times.
    Transient(String),
}

/// Err means Microsoft refused the sign-in (declined, expired, bad client).
pub fn poll_device_code(agent: &Agent, msa: &Msa, dc: &DeviceCode) -> Result<Poll, String> {
    let params = [("grant_type", DEVICE_CODE_GRANT_TYPE), ("device_code", dc.device_code.as_str())];
    let (status, body) = match form(agent, msa, msa.token_url(), &params) {
        Ok(r) => r,
        Err(e) => return Ok(Poll::Transient(e)),
    };
    if body["access_token"].as_str().is_some() {
        return Ok(Poll::Done(Tokens {
            refresh_token: body["refresh_token"].as_str().map(str::to_string),
        }));
    }
    match body["error"].as_str() {
        Some("authorization_pending") => Ok(Poll::Pending),
        Some("slow_down") => Ok(Poll::SlowDown),
        Some(error) => {
            let why = body["error_description"].as_str().unwrap_or(error);
            Err(format!("Sign-in failed: {}", why.lines().next().unwrap_or(why)))
        }
        None => Ok(Poll::Transient(format!("{}: unexpected response ({status})", host(msa.token_url())))),
    }
}


#[derive(Clone, Debug)]
pub struct Xsts {
    pub token: String,
    pub uhs: String,
    pub xuid: String,
    pub gamertag: String,
    expires_at: u64,
}

pub struct Xbox {
    agent: Agent,
    msa: Msa,
    refresh_token: Option<String>,
    rotated: bool,
    xsts: Option<Xsts>,
    images: HashMap<String, Option<String>>,
}

impl Xbox {
    pub fn new(ms_client_id: &str) -> Self {
        Self {
            agent: agent(),
            msa: Msa::new(ms_client_id),
            refresh_token: None,
            rotated: false,
            xsts: None,
            images: HashMap::new(),
        }
    }

    pub fn signed_in(&self) -> bool {
        self.refresh_token.is_some()
    }

    /// Also drops the cached XSTS token, since it belonged to whoever was signed in before.
    pub fn set_refresh_token(&mut self, token: Option<String>) {
        self.refresh_token = token;
        self.rotated = false;
        self.xsts = None;
    }

    /// A refresh token Microsoft issued in place of the one we had, to be saved by the caller.
    pub fn take_rotated(&mut self) -> Option<String> {
        if std::mem::take(&mut self.rotated) { self.refresh_token.clone() } else { None }
    }

    fn msa_token(&mut self) -> Result<String, XboxError> {
        let Some(refresh) = self.refresh_token.clone() else { return Err(XboxError::SignInExpired) };
        let params =
            [("grant_type", "refresh_token"), ("refresh_token", refresh.as_str()), ("scope", self.msa.scope())];
        let url = self.msa.token_url();
        let (status, body) = form(&self.agent, &self.msa, url, &params).map_err(XboxError::Failed)?;
        if let Some(access) = body["access_token"].as_str() {
            if let Some(new) = body["refresh_token"].as_str().filter(|t| *t != refresh) {
                self.refresh_token = Some(new.to_string());
                self.rotated = true;
            }
            return Ok(access.to_string());
        }
        match body["error"].as_str() {
            Some("temporarily_unavailable" | "server_error") | None => {
                Err(XboxError::Failed(format!("{}: refreshing sign-in failed ({status})", host(url))))
            }
            Some(_) => Err(XboxError::SignInExpired),
        }
    }

    fn post_json(&self, url: &str, body: &Value) -> Result<Value, XboxError> {
        let mut resp = self
            .agent
            .post(url)
            .header("Accept", "application/json")
            .header("x-xbl-contract-version", "1")
            .send_json(body)
            .map_err(|e| failed(url, e))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let header = resp.headers().get("x-err").and_then(|v| v.to_str().ok()).map(str::to_string);
            let err = header.or_else(|| {
                let v: Value = resp.body_mut().read_json().ok()?;
                v["XErr"].as_u64().map(|n| n.to_string()).or_else(|| v["XErr"].as_str().map(str::to_string))
            });
            let suffix = err.map(|e| format!(" (XErr {e})")).unwrap_or_default();
            return Err(XboxError::Failed(format!("{} → {status}{suffix}", host(url))));
        }
        resp.body_mut().read_json().map_err(|e| failed(url, e))
    }

    pub fn xsts(&mut self) -> Result<Xsts, XboxError> {
        if let Some(x) = &self.xsts
            && x.expires_at.saturating_sub(unix_now()) > XSTS_MARGIN_SECS
        {
            return Ok(x.clone());
        }

        let access = self.msa_token()?;
        let ticket = self.msa.ticket(&access);
        let user_url = "https://user.auth.xboxlive.com/user/authenticate";
        let user = self.post_json(
            user_url,
            &json!({
                "RelyingParty": "http://auth.xboxlive.com",
                "TokenType": "JWT",
                "Properties": { "AuthMethod": "RPS", "SiteName": "user.auth.xboxlive.com", "RpsTicket": ticket },
            }),
        )?;
        let user_token = user["Token"].as_str().ok_or_else(|| failed(user_url, "no user token in response"))?;

        let xsts_url = "https://xsts.auth.xboxlive.com/xsts/authorize";
        let x = self.post_json(
            xsts_url,
            &json!({
                "RelyingParty": "http://xboxlive.com",
                "TokenType": "JWT",
                "Properties": { "UserTokens": [user_token], "SandboxId": "RETAIL" },
            }),
        )?;
        let claims = &x["DisplayClaims"]["xui"][0];
        let field = |v: &Value| v.as_str().map(str::to_string);
        let (Some(token), Some(uhs), Some(xuid), Some(gamertag)) =
            (field(&x["Token"]), field(&claims["uhs"]), field(&claims["xid"]), field(&claims["gtg"]))
        else {
            return Err(failed(xsts_url, "incomplete token in response"));
        };
        let expires_at = x["NotAfter"].as_str().and_then(parse_iso8601).unwrap_or(unix_now() + XSTS_FALLBACK_SECS);
        let xsts = Xsts { token, uhs, xuid, gamertag, expires_at };
        self.xsts = Some(xsts.clone());
        Ok(xsts)
    }

    /// GET with the XBL3.0 header.
    fn xbl(&mut self, url: &str, contract: &str) -> Result<Value, XboxError> {
        self.xbl_request(url, contract, None)
    }

    /// GET, or POST when there's a body, with the XBL3.0 header. One 401 means the token was
    /// revoked early, so get a fresh one.
    fn xbl_request(&mut self, url: &str, contract: &str, body: Option<&Value>) -> Result<Value, XboxError> {
        for attempt in 0..2 {
            let auth = self.xsts()?;
            let authorization = format!("XBL3.0 x={};{}", auth.uhs, auth.token);
            // Remote control of a console wants this as well
            let skill = if url.starts_with(SMARTGLASS) { "RemoteManagement" } else { "" };
            let resp = match body {
                Some(b) => self
                    .agent
                    .post(url)
                    .header("Authorization", authorization)
                    .header("x-xbl-contract-version", contract)
                    .header("skillplatform", skill)
                    .header("Accept", "application/json")
                    .send_json(b),
                None => self
                    .agent
                    .get(url)
                    .header("Authorization", authorization)
                    .header("x-xbl-contract-version", contract)
                    .header("skillplatform", skill)
                    .header("Accept-Language", "en-GB")
                    .header("Accept", "application/json")
                    .call(),
            };
            let mut resp = resp.map_err(|e| failed(url, e))?;
            let status = resp.status().as_u16();
            if status == 401 && attempt == 0 {
                self.xsts = None;
                continue;
            }
            if !(200..300).contains(&status) {
                return Err(XboxError::Failed(format!("{} → {status}", host(url))));
            }
            return resp.body_mut().read_json().map_err(|e| failed(url, e));
        }
        Err(XboxError::Failed(format!("{} → 401", host(url))))
    }

    /// The consoles on this account, with their power state and whether remote features are on.
    pub fn consoles(&mut self) -> Result<Vec<CloudConsole>, XboxError> {
        let url = format!("{SMARTGLASS}/lists/devices?queryCurrentDevice=false&includeStorageDevices=true");
        let v = self.xbl(&url, "4")?;
        let list = v["result"].as_array().cloned().unwrap_or_default();
        Ok(list.iter().filter_map(CloudConsole::from_json).collect())
    }

    /// Turns a console on or off through Xbox Live, the way the Xbox app does. `live_id` is the
    /// same ID the console gives on the local network.
    pub fn power(&mut self, live_id: &str, on: bool) -> Result<(), XboxError> {
        let url = format!("{SMARTGLASS}/commands");
        let body = json!({
            "destination": "Xbox",
            "type": "Power",
            "command": if on { "WakeUp" } else { "TurnOff" },
            "sessionId": session_id(),
            "sourceId": "com.microsoft.smartglass",
            "parameters": [{}],
            "linkedXboxId": live_id,
        });
        let v = self.xbl_request(&url, "4", Some(&body))?;
        match v["status"]["errorCode"].as_str() {
            Some("OK") | None => Ok(()),
            Some(code) => {
                let message = v["status"]["errorMessage"].as_str().unwrap_or(code);
                Err(XboxError::Failed(format!("Xbox Live wouldn't send the command: {message}")))
            }
        }
    }

    pub fn presence(&mut self) -> Result<Value, XboxError> {
        self.xbl("https://userpresence.xboxlive.com/users/me?level=all", "3")
    }

    /// The newest captures of one kind, newest upload first. Only what the search returns: no
    /// paging. `Err(Failed)` carries the status, or a snippet when the response has no `values`.
    pub fn captures(&mut self, kind: CaptureKind) -> Result<Vec<Capture>, XboxError> {
        let xuid = self.xsts()?.xuid;
        let body = json!({ "max": CAPTURES_PER_SEARCH, "skip": 0, "query": format!("OwnerXuid eq {xuid}") });
        let url = kind.search_url();
        let v = self.xbl_request(url, MEDIAHUB_CONTRACT, Some(&body))?;
        let mut list = parse_captures(&v, kind)
            .ok_or_else(|| XboxError::Failed(format!("{} had no captures list: {}", host(url), snippet(&v))))?;
        list.sort_by(|a, b| b.uploaded.cmp(&a.uploaded));
        Ok(list)
    }

    /// Cover art for a title, cached per title id (including misses). The second value is a
    /// warning to log when the lookup failed.
    pub fn game_image(&mut self, title_id: &str) -> (Option<String>, Option<String>) {
        if let Some(cached) = self.images.get(title_id) {
            return (cached.clone(), None);
        }
        let lookup = self.xsts().and_then(|auth| {
            let url = format!(
                "https://titlehub.xboxlive.com/users/xuid({})/titles/titleid({title_id})/decoration/image",
                auth.xuid
            );
            self.xbl(&url, "2")
        });
        let (image, warning) = match lookup {
            Ok(data) => (data["titles"][0]["displayImage"].as_str().map(str::to_string), None),
            Err(e) => (None, Some(format!("Couldn't fetch game art: {e}"))),
        };
        self.images.insert(title_id.to_string(), image.clone());
        (image, warning)
    }
}

/// Contract version of the media hub's search. Unofficial, so this is the thing to change if the
/// searches start failing.
const MEDIAHUB_CONTRACT: &str = "3";
/// How many of the newest captures of each kind to ask for.
const CAPTURES_PER_SEARCH: u32 = 10;
/// A recording can be hundreds of megabytes, so downloads get far longer than API calls.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureKind {
    Screenshot,
    Recording,
}

impl CaptureKind {
    fn search_url(self) -> &'static str {
        match self {
            Self::Screenshot => "https://mediahub.xboxlive.com/screenshots/search",
            Self::Recording => "https://mediahub.xboxlive.com/gameclips/search",
        }
    }

    pub fn noun(self) -> &'static str {
        match self {
            Self::Screenshot => "Screenshot",
            Self::Recording => "Recording",
        }
    }
}

/// A screenshot or recording uploaded to Xbox network.
#[derive(Clone, Debug, PartialEq)]
pub struct Capture {
    pub id: String,
    pub kind: CaptureKind,
    pub title: String,
    /// Seconds since the epoch. Zero when the timestamp is missing or unreadable.
    pub uploaded: u64,
    pub captured: u64,
    /// Signed and short-lived: use it straight after searching.
    pub download: Option<String>,
    pub size: Option<u64>,
}

/// Reads a search response. `None` if it has no `values` list, which means the API changed or
/// the request was wrong rather than there being nothing to find. Entries without an ID are skipped.
pub fn parse_captures(v: &Value, kind: CaptureKind) -> Option<Vec<Capture>> {
    let values = v["values"].as_array()?;
    Some(values.iter().filter_map(|c| parse_capture(c, kind)).collect())
}

fn parse_capture(c: &Value, kind: CaptureKind) -> Option<Capture> {
    let id = c["contentId"].as_str().filter(|s| !s.is_empty())?.to_string();
    let captured = c["captureDate"].as_str().and_then(parse_iso8601).unwrap_or(0);
    let uploaded = c["uploadDate"].as_str().and_then(parse_iso8601).unwrap_or(captured);
    // Plain "Download" only: there may be HDR variants alongside it.
    let locator = c["contentLocators"]
        .as_array()
        .and_then(|l| l.iter().find(|l| l["locatorType"].as_str().is_some_and(|t| t.eq_ignore_ascii_case("download"))));
    Some(Capture {
        id,
        kind,
        title: c["titleName"].as_str().map(str::trim).filter(|t| !t.is_empty()).unwrap_or("Xbox").to_string(),
        uploaded,
        captured: if captured == 0 { uploaded } else { captured },
        download: locator.and_then(|l| l["uri"].as_str()).map(str::to_string),
        size: locator.and_then(|l| l["fileSize"].as_u64()),
    })
}

/// Downloads a signed URL to `to`. Signed URLs carry no auth header and must stay out of logs.
pub fn download(url: &str, to: &std::path::Path) -> Result<(), String> {
    let agent = agent_with_timeout(DOWNLOAD_TIMEOUT);
    let mut resp = agent.get(url).call().map_err(|e| format!("{}: {e}", host(url)))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(format!("{} → {status}", host(url)));
    }
    let mut reader = resp.body_mut().with_config().limit(u64::MAX).reader();
    let mut file = std::fs::File::create(to).map_err(|e| format!("{}: {e}", to.display()))?;
    std::io::copy(&mut reader, &mut file).map_err(|e| format!("{}: {e}", host(url)))?;
    Ok(())
}

/// A short, single-line piece of a response for the log, so a changed API can be diagnosed.
pub fn snippet(v: &Value) -> String {
    let text = v.to_string();
    match text.char_indices().nth(200) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text,
    }
}

/// Xbox Live's remote control service, used by the Xbox app to turn consoles on and off.
const SMARTGLASS: &str = "https://xccs.xboxlive.com";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudConsole {
    pub live_id: String,
    pub name: String,
    /// "On", "ConnectedStandby" (asleep, can be woken) or "Off"
    pub power_state: String,
    /// The console's "Remote features" setting, which waking and turning off need.
    pub remote_management: bool,
}

impl CloudConsole {
    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            live_id: v["id"].as_str()?.to_string(),
            name: v["name"].as_str().unwrap_or("Xbox").to_string(),
            power_state: v["powerState"].as_str().unwrap_or("Unknown").to_string(),
            remote_management: v["remoteManagementEnabled"].as_bool().unwrap_or(false),
        })
    }
}

/// A random-enough UUID for a command's session. Nothing checks it beyond its shape.
fn session_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = [0u8; 16];
    for half in bytes.chunks_mut(8) {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos());
        half.copy_from_slice(&h.finish().to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Seconds since the epoch for an `YYYY-MM-DDTHH:MM:SS[.fraction]Z` timestamp.
fn parse_iso8601(s: &str) -> Option<u64> {
    let (date, time) = s.trim_end_matches('Z').split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let min: i64 = t.next()?.parse().ok()?;
    let sec: i64 = t.next()?.split('.').next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from civil, after Howard Hinnant's algorithm.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + h * 3600 + min * 60 + sec).ok()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Game {
    pub id: String,
    pub name: String,
    pub detail: Option<String>,
    pub device: String,
}

/// What the user is playing right now, from a `userpresence` response. Dashboard apps (`ignored`),
/// background titles and anything not Active don't count; a Full-screen title beats the rest.
pub fn current_game(presence: &Value, ignored: &[String]) -> Option<Game> {
    if presence["state"].as_str() != Some("Online") {
        return None;
    }
    for device in presence["devices"].as_array()? {
        let titles: Vec<&Value> = device["titles"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| {
                !ignored.contains(&title_id(t))
                    && t["placement"].as_str() != Some("Background")
                    && t["state"].as_str() == Some("Active")
            })
            .collect();
        let chosen = titles.iter().find(|t| t["placement"].as_str() == Some("Full")).or(titles.first());
        if let Some(t) = chosen {
            return Some(Game {
                id: title_id(t),
                name: t["name"].as_str().unwrap_or("Unknown game").to_string(),
                detail: t["activity"]["richPresence"].as_str().map(str::to_string),
                device: device["type"].as_str().unwrap_or("Xbox").to_string(),
            });
        }
    }
    None
}

fn title_id(title: &Value) -> String {
    match &title["id"] {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub fn device_name(device: &str) -> &str {
    match device {
        "Scarlett" => "Xbox Series X|S",
        "XboxOne" => "Xbox One",
        "WindowsOneCore" => "PC",
        "Android" | "iOS" => "Mobile",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ignored() -> Vec<String> {
        ["750323071", "1022622766", "714681658"].map(String::from).to_vec()
    }

    fn title(id: &str, name: &str, placement: &str, state: &str) -> Value {
        json!({ "id": id, "name": name, "placement": placement, "state": state })
    }

    fn online(devices: Vec<Value>) -> Value {
        json!({ "xuid": "1", "state": "Online", "devices": devices })
    }

    #[test]
    fn dashboard_only_is_not_playing() {
        let p = online(vec![json!({ "type": "Scarlett", "titles": [
            title("750323071", "Home", "Full", "Active"),
            title("714681658", "Xbox", "Background", "Active"),
        ]})]);
        assert_eq!(current_game(&p, &ignored()), None);
    }

    #[test]
    fn game_in_full_wins_over_earlier_titles() {
        let p = online(vec![json!({ "type": "Scarlett", "titles": [
            title("750323071", "Home", "Full", "Active"),
            title("11", "Side app", "Fill", "Active"),
            title("22", "Forza", "Full", "Active"),
        ]})]);
        let g = current_game(&p, &ignored()).unwrap();
        assert_eq!((g.id.as_str(), g.name.as_str(), g.device.as_str()), ("22", "Forza", "Scarlett"));
        assert_eq!(g.detail, None);
    }

    #[test]
    fn first_active_title_is_used_without_a_full_one() {
        let p = online(vec![json!({ "type": "XboxOne", "titles": [title("11", "Snapped app", "Fill", "Active")] })]);
        assert_eq!(current_game(&p, &ignored()).unwrap().id, "11");
    }

    #[test]
    fn background_and_inactive_titles_are_skipped() {
        let p = online(vec![json!({ "type": "XboxOne", "titles": [
            title("11", "Music", "Background", "Active"),
            title("22", "Suspended game", "Full", "Inactive"),
        ]})]);
        assert_eq!(current_game(&p, &ignored()), None);
    }

    #[test]
    fn rich_presence_becomes_the_detail() {
        let mut t = title("22", "Forza", "Full", "Active");
        t["activity"] = json!({ "richPresence": "Racing in Tokyo" });
        let p = online(vec![json!({ "type": "WindowsOneCore", "titles": [t] })]);
        let g = current_game(&p, &ignored()).unwrap();
        assert_eq!(g.detail.as_deref(), Some("Racing in Tokyo"));
        assert_eq!(g.device, "WindowsOneCore");
    }

    #[test]
    fn offline_and_empty_presence_are_not_playing() {
        let titles = json!([title("22", "Forza", "Full", "Active")]);
        let offline = json!({ "state": "Offline", "devices": [{ "type": "Scarlett", "titles": titles }] });
        assert_eq!(current_game(&offline, &ignored()), None);
        assert_eq!(current_game(&json!({ "state": "Online" }), &ignored()), None);
        assert_eq!(current_game(&Value::Null, &ignored()), None);
    }

    #[test]
    fn later_devices_are_checked_and_numeric_ids_work() {
        let p = online(vec![
            json!({ "type": "Scarlett", "titles": [title("750323071", "Home", "Full", "Active")] }),
            json!({ "type": "Android", "titles": [{ "id": 33, "name": "Phone game", "state": "Active" }] }),
        ]);
        let g = current_game(&p, &ignored()).unwrap();
        assert_eq!((g.id.as_str(), g.device.as_str()), ("33", "Android"));
    }

    #[test]
    fn the_ignore_list_is_configurable() {
        let p = online(vec![json!({ "type": "Scarlett", "titles": [title("22", "Forza", "Full", "Active")] })]);
        assert_eq!(current_game(&p, &["22".to_string()]), None);
    }

    /// Uses the app's saved sign-in to list consoles, then wakes or turns off the one named in
    /// `XBOX_POWER` ("on:<live id>" or "off:<live id>"), if set:
    /// `cargo test cloud_consoles -- --ignored --nocapture`
    fn sample() -> Value {
        json!({
            "values": [
                {
                    "contentId": "aaaa-1111",
                    "localId": "1",
                    "titleName": "Halo Infinite",
                    "captureDate": "2026-11-17T20:15:30.1234567Z",
                    "uploadDate": "2026-11-17T20:16:10.5Z",
                    "contentLocators": [
                        { "locatorType": "Thumbnail_Small", "uri": "https://example.com/small.jpg" },
                        { "locatorType": "DownloadHDR", "uri": "https://example.com/hdr.png" },
                        { "locatorType": "Download", "uri": "https://example.com/full.png?sig=1", "fileSize": 2048 }
                    ]
                },
                {
                    "contentId": "bbbb-2222",
                    "titleName": "  ",
                    "uploadDate": "2026-11-17T21:00:00Z",
                    "durationInSeconds": 30,
                    "contentLocators": [{ "locatorType": "download", "uri": "https://example.com/clip.mp4" }]
                },
                { "titleName": "No id" },
                { "contentId": "cccc-3333", "uploadDate": "2026-11-16T09:00:00Z" }
            ]
        })
    }

    #[test]
    fn capture_search_responses_parse() {
        let list = parse_captures(&sample(), CaptureKind::Screenshot).unwrap();
        assert_eq!(list.len(), 3);
        let first = &list[0];
        assert_eq!(first.id, "aaaa-1111");
        assert_eq!(first.title, "Halo Infinite");
        assert_eq!(first.download.as_deref(), Some("https://example.com/full.png?sig=1"));
        assert_eq!(first.size, Some(2048));
        assert_eq!(first.captured, parse_iso8601("2026-11-17T20:15:30Z").unwrap());
        assert_eq!(first.uploaded, parse_iso8601("2026-11-17T20:16:10Z").unwrap());
        // Missing fields fall back instead of failing
        assert_eq!(list[1].title, "Xbox");
        assert_eq!(list[1].download.as_deref(), Some("https://example.com/clip.mp4"));
        assert_eq!(list[1].captured, list[1].uploaded);
        assert_eq!(list[2].download, None);
    }

    #[test]
    fn a_response_without_values_is_not_an_empty_list() {
        assert!(parse_captures(&json!({ "values": [] }), CaptureKind::Recording).unwrap().is_empty());
        assert!(parse_captures(&json!({ "error": "nope" }), CaptureKind::Recording).is_none());
    }

    #[test]
    fn snippets_are_short() {
        let long = json!({ "x": "y".repeat(500) });
        assert!(snippet(&long).chars().count() <= 201);
    }

    #[test]
    #[ignore]
    fn cloud_consoles() {
        let dir = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("Switchboard");
        let mut secrets = crate::platform::secrets::Secrets::load(dir.join("secrets.bin"));
        let mut xbox = Xbox::new("");
        xbox.set_refresh_token(secrets.get("xbox.refreshToken"));
        println!("{:#?}", xbox.consoles().unwrap());
        if let Ok(arg) = std::env::var("XBOX_POWER") {
            let (state, id) = arg.split_once(':').unwrap();
            println!("{:?}", xbox.power(id, state == "on"));
        }
        if let Some(t) = xbox.take_rotated() {
            secrets.set("xbox.refreshToken", Some(&t)).unwrap();
        }
    }

    #[test]
    fn session_ids_look_like_uuids() {
        let id = session_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id.as_bytes()[14], b'4');
        assert_ne!(id, session_id());
    }

    #[test]
    fn device_names() {
        assert_eq!(device_name("Scarlett"), "Xbox Series X|S");
        assert_eq!(device_name("XboxOne"), "Xbox One");
        assert_eq!(device_name("WindowsOneCore"), "PC");
        assert_eq!(device_name("iOS"), "Mobile");
        assert_eq!(device_name("Toaster"), "Toaster");
    }

    #[test]
    fn iso_timestamps() {
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601("2000-03-01T00:00:00.0000000Z"), Some(951_868_800));
        assert_eq!(parse_iso8601("2024-02-29T12:30:15.5Z"), Some(1_709_209_815));
        assert_eq!(parse_iso8601("nonsense"), None);
    }

    #[test]
    fn msa_endpoint_sets() {
        let public = Msa::new("  ");
        assert_eq!(public.client_id, "000000004C12AE6F");
        assert_eq!(public.token_url(), "https://login.live.com/oauth20_token.srf");
        assert_eq!(public.ticket("abc"), "abc");

        let own = Msa::new("my-client");
        assert_eq!(own.token_url(), "https://login.microsoftonline.com/consumers/oauth2/v2.0/token");
        assert_eq!(own.scope(), "XboxLive.signin offline_access");
        assert_eq!(own.ticket("abc"), "d=abc");
    }
}
