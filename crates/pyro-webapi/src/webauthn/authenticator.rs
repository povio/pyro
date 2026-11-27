//! Virtual authenticators: software CTAP authenticators a test driver adds to a browser context
//! (WebDriver's Virtual Authenticators, Chrome's `WebAuthn.*` CDP domain). Each holds credentials
//! (ES256 keys, optionally discoverable), a signature counter per credential, and the capabilities
//! of its configuration: protocol (U2F, CTAP 2.0, CTAP 2.1), transport, resident keys, user
//! verification, large blobs, credBlob, minPinLength and PRF (hmac-secret). `make_credential` and
//! `get_assertion` are the authenticatorMakeCredential and authenticatorGetAssertion operations as a
//! client sees them: authenticator data with the right flags, a `none` attestation object, and
//! assertion signatures that verify with the credential public key.

use base64::Engine as _;
use hmac::{Hmac, Mac};
use p256::ecdsa::signature::hazmat::PrehashSigner;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use p256::SecretKey;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::cbor::Cbor;

const FLAG_UP: u8 = 0x01;
const FLAG_UV: u8 = 0x04;
const FLAG_BE: u8 = 0x08;
const FLAG_BS: u8 = 0x10;
const FLAG_AT: u8 = 0x40;
const FLAG_ED: u8 = 0x80;
/// The COSE algorithm of ES256, the only one a virtual authenticator signs with.
const ES256: i64 = -7;
/// The longest credBlob an authenticator stores (CTAP 2.1 `maxCredBlobLength` minimum).
const MAX_CRED_BLOB: usize = 32;
const MIN_PIN_LENGTH: i64 = 4;

pub(crate) fn b64u(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Base64, standard or URL alphabet, with or without padding.
pub(crate) fn unb64(text: &str) -> Option<Vec<u8>> {
    let normalized: String = text.trim_end_matches('=').chars().map(|c| match c { '+' => '-', '/' => '_', c => c }).collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(normalized).ok()
}

fn b64_std(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn random(len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    // Credential ids and PRF secrets come from here: without a random source there is no authenticator.
    getrandom::fill(&mut bytes).expect("system random source");
    bytes
}

fn sha256(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC takes keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// One ASN.1 INTEGER from an unsigned big-endian value.
fn der_integer(raw: &[u8]) -> Vec<u8> {
    let mut value = raw.to_vec();
    while value.len() > 1 && value[0] == 0 {
        value.remove(0);
    }
    if value[0] & 0x80 != 0 {
        value.insert(0, 0);
    }
    let mut out = vec![0x02, value.len() as u8];
    out.extend(value);
    out
}

/// An ECDSA signature as the DER `Ecdsa-Sig-Value` WebAuthn carries (from `r || s`).
fn der_signature(raw: &[u8]) -> Vec<u8> {
    let r = der_integer(&raw[..32]);
    let s = der_integer(&raw[32..]);
    let mut out = vec![0x30, (r.len() + s.len()) as u8];
    out.extend(r);
    out.extend(s);
    out
}

/// ES256 signature (DER) of `message` with the PKCS #8 private key `pkcs8`.
fn sign_es256(pkcs8: &[u8], message: &[u8]) -> Option<Vec<u8>> {
    let secret = SecretKey::from_pkcs8_der(pkcs8).ok()?;
    let key = SigningKey::from_bytes(&secret.to_bytes()).ok()?;
    let digest = Sha256::digest(message);
    let signature: Signature = key.sign_prehash(digest.as_slice()).ok()?;
    Some(der_signature(&signature.to_bytes().to_vec()))
}

/// The uncompressed public point (x, y) of a PKCS #8 P-256 private key.
fn public_point(pkcs8: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let secret = SecretKey::from_pkcs8_der(pkcs8).ok()?;
    let point = secret.public_key().to_encoded_point(false);
    Some((point.x()?.to_vec(), point.y()?.to_vec()))
}

/// SubjectPublicKeyInfo of an uncompressed P-256 point.
fn spki(x: &[u8], y: &[u8]) -> Vec<u8> {
    let mut out = vec![
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01,
        0x07, 0x03, 0x42, 0x00, 0x04,
    ];
    out.extend_from_slice(x);
    out.extend_from_slice(y);
    out
}

/// The COSE_Key of an ES256 public key.
fn cose_key(x: &[u8], y: &[u8]) -> Vec<u8> {
    Cbor::Map(vec![
        (Cbor::Int(1), Cbor::Int(2)),
        (Cbor::Int(3), Cbor::Int(ES256)),
        (Cbor::Int(-1), Cbor::Int(1)),
        (Cbor::Int(-2), Cbor::Bytes(x.to_vec())),
        (Cbor::Int(-3), Cbor::Bytes(y.to_vec())),
    ])
    .encode()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Protocol {
    U2f,
    Ctap2,
    Ctap21,
}

impl Protocol {
    fn ctap2(self) -> bool {
        self != Protocol::U2f
    }
}

#[derive(Clone)]
pub(crate) struct Credential {
    pub id: Vec<u8>,
    pub rp_id: String,
    /// PKCS #8 private key (ES256).
    pub private_key: Vec<u8>,
    pub user_handle: Option<Vec<u8>>,
    pub resident: bool,
    pub sign_count: u32,
    pub user_name: Option<String>,
    pub user_display_name: Option<String>,
    pub large_blob: Option<Vec<u8>>,
    pub cred_blob: Option<Vec<u8>>,
    pub backup_eligibility: bool,
    pub backup_state: bool,
    /// The credential's hmac-secret (CredRandom): what PRF evaluations key.
    pub secret: Vec<u8>,
}

impl Credential {
    fn flags(&self) -> u8 {
        (if self.backup_eligibility { FLAG_BE } else { 0 }) | (if self.backup_state { FLAG_BS } else { 0 })
    }

    /// The credential as CDP's `WebAuthn.Credential` has it.
    fn to_cdp(&self) -> Value {
        let mut out = json!({
            "credentialId": b64_std(&self.id),
            "isResidentCredential": self.resident,
            "rpId": self.rp_id,
            "privateKey": b64_std(&self.private_key),
            "signCount": self.sign_count,
            "backupEligibility": self.backup_eligibility,
            "backupState": self.backup_state,
        });
        let object = out.as_object_mut().expect("an object");
        if let Some(handle) = &self.user_handle {
            object.insert("userHandle".into(), json!(b64_std(handle)));
        }
        if let Some(blob) = &self.large_blob {
            object.insert("largeBlob".into(), json!(b64_std(blob)));
        }
        if let Some(name) = &self.user_name {
            object.insert("userName".into(), json!(name));
        }
        if let Some(name) = &self.user_display_name {
            object.insert("userDisplayName".into(), json!(name));
        }
        out
    }
}

pub(crate) struct Authenticator {
    pub id: String,
    pub protocol: Protocol,
    pub transport: String,
    pub has_resident_key: bool,
    pub has_user_verification: bool,
    pub has_large_blob: bool,
    pub has_cred_blob: bool,
    pub has_min_pin_length: bool,
    pub has_prf: bool,
    /// Whether a request is answered at once (a user touches the authenticator): CDP's
    /// `automaticPresenceSimulation`, WebDriver's `isUserConsenting`. Off, no request completes.
    pub presence: bool,
    pub user_verified: bool,
    pub default_backup_eligibility: bool,
    pub default_backup_state: bool,
    pub credentials: Vec<Credential>,
}

impl Authenticator {
    fn platform(&self) -> bool {
        self.transport == "internal"
    }

    fn transports(&self) -> Vec<&'static str> {
        vec![match self.transport.as_str() {
            "usb" => "usb",
            "nfc" => "nfc",
            "ble" => "ble",
            "cable" => "hybrid",
            _ => "internal",
        }]
    }

    fn attachment(&self) -> &'static str {
        if self.platform() { "platform" } else { "cross-platform" }
    }
}

/// How a CDP / WebDriver command failed: an invalid argument, or a state it cannot act on.
pub(crate) enum CommandError {
    Invalid(String),
    Failed(String),
}

fn invalid<T>(message: &str) -> Result<T, CommandError> {
    Err(CommandError::Invalid(message.to_owned()))
}

fn failed<T>(message: &str) -> Result<T, CommandError> {
    Err(CommandError::Failed(message.to_owned()))
}

/// The virtual authenticators of a browser context.
#[derive(Default)]
pub(crate) struct State {
    pub enabled: bool,
    pub authenticators: Vec<Authenticator>,
}

fn error(name: &str, message: &str) -> Value {
    json!({ "status": "error", "name": name, "message": message })
}

fn bytes_of(value: &Value) -> Option<Vec<u8>> {
    value.as_str().and_then(unb64)
}

fn list_of(value: &Value) -> Vec<Vec<u8>> {
    value.as_array().map(|items| items.iter().filter_map(bytes_of).collect()).unwrap_or_default()
}

/// A PRF evaluation point: HMAC-SHA-256 of the credential's secret over SHA-256("WebAuthn PRF" 0x00 input).
fn prf_output(secret: &[u8], input: &[u8]) -> Vec<u8> {
    let mut salt = b"WebAuthn PRF\0".to_vec();
    salt.extend_from_slice(input);
    hmac_sha256(secret, &sha256(&salt))
}

/// The `first` (and `second`) outputs of the PRF values `values` ({first, second?}, base64url).
fn prf_results(secret: &[u8], values: &Value) -> Option<Value> {
    let first = bytes_of(&values["first"])?;
    let mut results = json!({ "first": b64u(&prf_output(secret, &first)) });
    if let Some(second) = bytes_of(&values["second"]) {
        results["second"] = json!(b64u(&prf_output(secret, &second)));
    }
    Some(results)
}

impl State {
    fn find(&mut self, id: &str) -> Result<&mut Authenticator, CommandError> {
        self.authenticators
            .iter_mut()
            .find(|a| a.id == id)
            .map_or_else(|| failed("Could not find a Virtual Authenticator matching the ID"), Ok)
    }

    /// A user-verifying platform authenticator exists (`uvpaa`), and one that holds discoverable
    /// credentials for conditional mediation.
    pub(crate) fn platform(&self) -> Value {
        let uvpaa = self.authenticators.iter().any(|a| a.platform() && a.protocol.ctap2() && a.has_user_verification);
        let conditional = uvpaa && self.authenticators.iter().any(|a| a.platform() && a.protocol.ctap2() && a.has_resident_key && a.has_user_verification);
        json!({ "uvpaa": uvpaa, "conditional": conditional })
    }

    /// authenticatorMakeCredential on the first authenticator of the context that can answer the
    /// request. `{status: 'pending'}`: none can (the request stays open, as a browser with no key
    /// to touch); an `error` is the authenticator's refusal.
    pub(crate) fn make_credential(&mut self, req: &Value) -> Value {
        let rp_id = req["rpId"].as_str().unwrap_or_default().to_owned();
        let algs: Vec<i64> = req["algs"].as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
        let exclude = list_of(&req["exclude"]);
        let resident_key = req["residentKey"].as_str().unwrap_or("discouraged");
        let user_verification = req["userVerification"].as_str().unwrap_or("preferred");
        let attachment = req["attachment"].as_str();
        let ext = &req["extensions"];
        let large_blob_required = ext["largeBlob"]["support"].as_str() == Some("required");
        let mut pending = false;
        let mut denied = false;
        for index in 0..self.authenticators.len() {
            let a = &self.authenticators[index];
            if attachment.is_some_and(|wanted| (wanted == "platform") != a.platform()) {
                continue;
            }
            if !algs.contains(&ES256) {
                continue;
            }
            let ctap2 = a.protocol.ctap2();
            let uv_required = user_verification == "required";
            if (resident_key == "required" && !(ctap2 && a.has_resident_key))
                || (uv_required && !(ctap2 && a.has_user_verification))
                || (large_blob_required && !(ctap2 && a.has_large_blob))
            {
                denied = true;
                continue;
            }
            if !a.presence {
                pending = true;
                continue;
            }
            if a.credentials.iter().any(|c| c.rp_id == rp_id && exclude.contains(&c.id)) {
                return error("InvalidStateError", "The user attempted to register an authenticator that contains one of the credentials already registered with the relying party.");
            }
            let verified = uv_required || (user_verification == "preferred" && ctap2 && a.has_user_verification);
            if verified && !a.user_verified {
                return error("NotAllowedError", "The operation either timed out or was not allowed.");
            }
            return self.authenticators[index].make(req, &rp_id, resident_key, verified);
        }
        if pending || !denied {
            json!({ "status": "pending" })
        } else {
            error("NotAllowedError", "The operation either timed out or was not allowed.")
        }
    }

    /// authenticatorGetAssertion: the first authenticator that holds a credential the request names
    /// (`allow`: its list, or null for the discoverable credentials of the RP) answers it.
    pub(crate) fn get_assertion(&mut self, req: &Value) -> Value {
        // Conditional mediation lists the passkeys in a form-field popup nobody can pick from here.
        if req["conditional"].as_bool() == Some(true) {
            return json!({ "status": "pending" });
        }
        let rp_id = req["rpId"].as_str().unwrap_or_default().to_owned();
        let allow = req["allow"].is_array().then(|| list_of(&req["allow"]));
        let user_verification = req["userVerification"].as_str().unwrap_or("preferred");
        let mut pending = false;
        let mut denied = false;
        let mut empty = false;
        for index in 0..self.authenticators.len() {
            let a = &self.authenticators[index];
            let ctap2 = a.protocol.ctap2();
            let uv_required = user_verification == "required";
            if uv_required && !(ctap2 && a.has_user_verification) {
                denied = true;
                continue;
            }
            let candidate = a.credentials.iter().position(|c| {
                c.rp_id == rp_id
                    && match &allow {
                        Some(ids) => ids.contains(&c.id),
                        None => c.resident,
                    }
            });
            if !a.presence {
                pending = true;
                continue;
            }
            let Some(credential) = candidate else {
                // The authenticator reports "no credentials" after the user's touch.
                empty = true;
                continue;
            };
            let verified = uv_required || (user_verification == "preferred" && ctap2 && a.has_user_verification);
            if verified && !a.user_verified {
                return error("NotAllowedError", "The operation either timed out or was not allowed.");
            }
            return self.authenticators[index].assert(req, &rp_id, credential, verified);
        }
        if pending || !(denied || empty) {
            json!({ "status": "pending" })
        } else {
            error("NotAllowedError", "The operation either timed out or was not allowed.")
        }
    }

    /// The Signal API: `unknown` drops the discoverable credential with that id, `all` drops the
    /// user's discoverable credentials that are not in the accepted list, `details` renames them.
    pub(crate) fn signal(&mut self, req: &Value) -> Value {
        let rp_id = req["rpId"].as_str().unwrap_or_default();
        let user_id = bytes_of(&req["userId"]);
        let matches_user = |c: &Credential| c.resident && c.rp_id == rp_id && c.user_handle.is_some() && c.user_handle == user_id;
        match req["kind"].as_str() {
            Some("unknown") => {
                let id = bytes_of(&req["credentialId"]);
                for a in &mut self.authenticators {
                    a.credentials.retain(|c| !(c.resident && c.rp_id == rp_id && Some(&c.id) == id.as_ref()));
                }
            }
            Some("all") => {
                let accepted = list_of(&req["accepted"]);
                for a in &mut self.authenticators {
                    a.credentials.retain(|c| !(matches_user(c) && !accepted.contains(&c.id)));
                }
            }
            Some("details") => {
                let name = req["name"].as_str().unwrap_or_default();
                let display = req["displayName"].as_str().unwrap_or_default();
                for a in &mut self.authenticators {
                    for c in a.credentials.iter_mut().filter(|c| matches_user(c)) {
                        c.user_name = Some(name.to_owned());
                        c.user_display_name = Some(display.to_owned());
                    }
                }
            }
            _ => {}
        }
        json!({})
    }

    /// A CDP `WebAuthn.*` command (the same shapes as Chrome's).
    pub(crate) fn command(&mut self, method: &str, params: &Value) -> Result<Value, CommandError> {
        match method {
            "enable" => {
                self.enabled = true;
                return Ok(json!({}));
            }
            "disable" => {
                self.enabled = false;
                self.authenticators.clear();
                return Ok(json!({}));
            }
            _ => {}
        }
        if !self.enabled {
            return failed("The Virtual Authenticator Environment has not been enabled for this session");
        }
        match method {
            "addVirtualAuthenticator" => self.add_authenticator(&params["options"]),
            "removeVirtualAuthenticator" => {
                let id = params["authenticatorId"].as_str().unwrap_or_default();
                let before = self.authenticators.len();
                self.authenticators.retain(|a| a.id != id);
                if self.authenticators.len() == before {
                    return failed("Could not find a Virtual Authenticator matching the ID");
                }
                Ok(json!({}))
            }
            "setUserVerified" => {
                let verified = params["isUserVerified"].as_bool().unwrap_or(false);
                self.find(params["authenticatorId"].as_str().unwrap_or_default())?.user_verified = verified;
                Ok(json!({}))
            }
            "setAutomaticPresenceSimulation" => {
                let enabled = params["enabled"].as_bool().unwrap_or(false);
                self.find(params["authenticatorId"].as_str().unwrap_or_default())?.presence = enabled;
                Ok(json!({}))
            }
            "addCredential" => {
                let a = self.find(params["authenticatorId"].as_str().unwrap_or_default())?;
                a.add_credential(&params["credential"])?;
                Ok(json!({}))
            }
            "getCredential" => {
                let a = self.find(params["authenticatorId"].as_str().unwrap_or_default())?;
                let id = bytes_of(&params["credentialId"]);
                match a.credentials.iter().find(|c| Some(&c.id) == id.as_ref()) {
                    Some(c) => Ok(json!({ "credential": c.to_cdp() })),
                    None => failed("Could not find a credential matching the ID"),
                }
            }
            "getCredentials" => {
                let a = self.find(params["authenticatorId"].as_str().unwrap_or_default())?;
                Ok(json!({ "credentials": a.credentials.iter().map(Credential::to_cdp).collect::<Vec<_>>() }))
            }
            "removeCredential" => {
                let a = self.find(params["authenticatorId"].as_str().unwrap_or_default())?;
                let id = bytes_of(&params["credentialId"]);
                let before = a.credentials.len();
                a.credentials.retain(|c| Some(&c.id) != id.as_ref());
                if a.credentials.len() == before {
                    return failed("Could not find a credential matching the ID");
                }
                Ok(json!({}))
            }
            "clearCredentials" => {
                self.find(params["authenticatorId"].as_str().unwrap_or_default())?.credentials.clear();
                Ok(json!({}))
            }
            "setCredentialProperties" => {
                let a = self.find(params["authenticatorId"].as_str().unwrap_or_default())?;
                let id = bytes_of(&params["credentialId"]);
                let Some(c) = a.credentials.iter_mut().find(|c| Some(&c.id) == id.as_ref()) else {
                    return failed("Could not find a credential matching the ID");
                };
                if let Some(v) = params["backupEligibility"].as_bool() {
                    c.backup_eligibility = v;
                }
                if let Some(v) = params["backupState"].as_bool() {
                    c.backup_state = v;
                }
                if let Some(v) = params["signCount"].as_u64() {
                    c.sign_count = v as u32;
                }
                Ok(json!({}))
            }
            _ => invalid(&format!("'WebAuthn.{method}' wasn't found")),
        }
    }

    fn add_authenticator(&mut self, options: &Value) -> Result<Value, CommandError> {
        let protocol = match options["protocol"].as_str() {
            Some("u2f") => Protocol::U2f,
            Some("ctap2") => match options["ctap2Version"].as_str() {
                None | Some("ctap2_0") => Protocol::Ctap2,
                Some("ctap2_1") => Protocol::Ctap21,
                Some(_) => return invalid("Invalid ctap2Version"),
            },
            _ => return invalid("Invalid protocol"),
        };
        let transport = match options["transport"].as_str() {
            Some(t @ ("usb" | "nfc" | "ble" | "cable" | "internal")) => t.to_owned(),
            _ => return invalid("Invalid transport"),
        };
        let flag = |name: &str, default: bool| options[name].as_bool().unwrap_or(default);
        let id = b64u(&random(16));
        self.authenticators.push(Authenticator {
            id: id.clone(),
            protocol,
            transport,
            has_resident_key: flag("hasResidentKey", false),
            has_user_verification: flag("hasUserVerification", false),
            has_large_blob: flag("hasLargeBlob", false),
            has_cred_blob: flag("hasCredBlob", false),
            has_min_pin_length: flag("hasMinPinLength", false),
            has_prf: flag("hasPrf", false),
            presence: flag("automaticPresenceSimulation", true),
            user_verified: flag("isUserVerified", false),
            default_backup_eligibility: flag("defaultBackupEligibility", false),
            default_backup_state: flag("defaultBackupState", false),
            credentials: Vec::new(),
        });
        Ok(json!({ "authenticatorId": id }))
    }
}

impl Authenticator {
    /// CDP `WebAuthn.addCredential`.
    fn add_credential(&mut self, credential: &Value) -> Result<(), CommandError> {
        let Some(id) = bytes_of(&credential["credentialId"]).filter(|id| !id.is_empty()) else {
            return invalid("Invalid credentialId");
        };
        let Some(private_key) = bytes_of(&credential["privateKey"]).filter(|k| public_point(k).is_some()) else {
            return invalid("Invalid privateKey: not a PKCS #8 EC P-256 private key");
        };
        let resident = credential["isResidentCredential"].as_bool().unwrap_or(false);
        let Some(rp_id) = credential["rpId"].as_str().filter(|r| !r.is_empty()) else {
            return invalid("Invalid rpId");
        };
        if resident && !self.protocol.ctap2() {
            return invalid("Resident credentials need a CTAP2 authenticator");
        }
        let user_handle = bytes_of(&credential["userHandle"]);
        if resident && user_handle.is_none() {
            return invalid("A resident credential needs a userHandle");
        }
        if self.credentials.iter().any(|c| c.id == id) {
            return failed("A credential with this ID already exists");
        }
        self.credentials.push(Credential {
            id,
            rp_id: rp_id.to_owned(),
            private_key,
            user_handle,
            resident,
            sign_count: credential["signCount"].as_u64().unwrap_or(0) as u32,
            user_name: credential["userName"].as_str().map(str::to_owned),
            user_display_name: credential["userDisplayName"].as_str().map(str::to_owned),
            large_blob: bytes_of(&credential["largeBlob"]),
            cred_blob: None,
            backup_eligibility: credential["backupEligibility"].as_bool().unwrap_or(self.default_backup_eligibility),
            backup_state: credential["backupState"].as_bool().unwrap_or(self.default_backup_state),
            secret: random(32),
        });
        Ok(())
    }

    /// The attestation response of a new credential (`none` attestation).
    fn make(&mut self, req: &Value, rp_id: &str, resident_key: &str, user_verified: bool) -> Value {
        let ctap2 = self.protocol.ctap2();
        let discoverable = ctap2 && self.has_resident_key && resident_key != "discouraged";
        let secret_key = SecretKey::random(&mut rand_core::OsRng);
        let Ok(pkcs8) = secret_key.to_pkcs8_der().map(|d| d.as_bytes().to_vec()) else {
            return error("UnknownError", "The authenticator could not generate a key.");
        };
        let Some((x, y)) = public_point(&pkcs8) else {
            return error("UnknownError", "The authenticator could not generate a key.");
        };
        let ext = &req["extensions"];
        let mut credential = Credential {
            id: random(32),
            rp_id: rp_id.to_owned(),
            private_key: pkcs8,
            user_handle: ctap2.then(|| bytes_of(&req["user"]["id"])).flatten(),
            resident: discoverable,
            sign_count: 0,
            user_name: ctap2.then(|| req["user"]["name"].as_str().map(str::to_owned)).flatten(),
            user_display_name: ctap2.then(|| req["user"]["displayName"].as_str().map(str::to_owned)).flatten(),
            large_blob: None,
            cred_blob: None,
            backup_eligibility: self.default_backup_eligibility,
            backup_state: self.default_backup_state,
            secret: random(32),
        };
        // Extensions: what the authenticator data carries, and what the client reports to the page.
        let mut authenticator_extensions: Vec<(Cbor, Cbor)> = Vec::new();
        let mut outputs = json!({});
        if ext["credProps"].as_bool() == Some(true) {
            // CTAP 2.0 authenticators with resident key support may or may not have made a
            // discoverable credential when it was not asked for: unknown, so `rk` is left out.
            let rk = if !ctap2 || !self.has_resident_key {
                Some(false)
            } else if discoverable {
                Some(true)
            } else if self.protocol == Protocol::Ctap21 {
                Some(false)
            } else {
                None
            };
            outputs["credProps"] = rk.map_or_else(|| json!({}), |rk| json!({ "rk": rk }));
        }
        if ext["minPinLength"].as_bool() == Some(true) && ctap2 && self.has_min_pin_length {
            authenticator_extensions.push((Cbor::Text("minPinLength".into()), Cbor::Int(MIN_PIN_LENGTH)));
        }
        if let Some(blob) = bytes_of(&ext["credBlob"]) {
            let stored = ctap2 && self.has_cred_blob && blob.len() <= MAX_CRED_BLOB;
            if stored {
                credential.cred_blob = Some(blob);
                authenticator_extensions.push((Cbor::Text("credBlob".into()), Cbor::Bool(true)));
            }
            outputs["credBlob"] = json!(stored);
        }
        if ext["largeBlob"].is_object() {
            outputs["largeBlob"] = json!({ "supported": ctap2 && self.has_large_blob });
        }
        if ext["prf"].is_object() || ext["hmacCreateSecret"].as_bool() == Some(true) {
            let enabled = ctap2 && self.has_prf;
            if enabled {
                authenticator_extensions.push((Cbor::Text("hmac-secret".into()), Cbor::Bool(true)));
            }
            if ext["prf"].is_object() {
                outputs["prf"] = json!({ "enabled": enabled });
                if enabled {
                    if let Some(results) = prf_results(&credential.secret, &ext["prf"]["eval"]) {
                        outputs["prf"]["results"] = results;
                    }
                }
            }
        }
        let mut flags = FLAG_UP | FLAG_AT | credential.flags();
        if user_verified {
            flags |= FLAG_UV;
        }
        let mut data = sha256(rp_id.as_bytes());
        if !authenticator_extensions.is_empty() {
            flags |= FLAG_ED;
        }
        data.push(flags);
        data.extend_from_slice(&0u32.to_be_bytes());
        // Attested credential data: the AAGUID (zeroes, as `none` attestation asks), the credential ID, the public key.
        data.extend_from_slice(&[0u8; 16]);
        data.extend_from_slice(&(credential.id.len() as u16).to_be_bytes());
        data.extend_from_slice(&credential.id);
        data.extend_from_slice(&cose_key(&x, &y));
        if !authenticator_extensions.is_empty() {
            data.extend_from_slice(&Cbor::Map(authenticator_extensions).encode());
        }
        let attestation = Cbor::Map(vec![
            (Cbor::Text("fmt".into()), Cbor::Text("none".into())),
            (Cbor::Text("attStmt".into()), Cbor::Map(Vec::new())),
            (Cbor::Text("authData".into()), Cbor::Bytes(data.clone())),
        ])
        .encode();
        let response = json!({
            "status": "ok",
            "id": b64u(&credential.id),
            "clientDataJSON": b64u(req["clientData"].as_str().unwrap_or_default().as_bytes()),
            "attestationObject": b64u(&attestation),
            "authenticatorData": b64u(&data),
            "publicKey": b64u(&spki(&x, &y)),
            "publicKeyAlgorithm": ES256,
            "transports": self.transports(),
            "attachment": self.attachment(),
            "extensions": outputs,
        });
        self.credentials.push(credential);
        response
    }

    /// The assertion of credential `index`: authenticator data, and the signature over it and the
    /// hash of the client data.
    fn assert(&mut self, req: &Value, rp_id: &str, index: usize, user_verified: bool) -> Value {
        let ctap2 = self.protocol.ctap2();
        let (has_cred_blob, has_large_blob, has_prf) = (self.has_cred_blob, self.has_large_blob, self.has_prf);
        let attachment = self.attachment();
        let credential = &mut self.credentials[index];
        let ext = &req["extensions"];
        credential.sign_count = credential.sign_count.wrapping_add(1);
        let mut authenticator_extensions: Vec<(Cbor, Cbor)> = Vec::new();
        let mut outputs = json!({});
        if ext["getCredBlob"].as_bool() == Some(true) && ctap2 && has_cred_blob {
            let blob = credential.cred_blob.clone().unwrap_or_default();
            authenticator_extensions.push((Cbor::Text("credBlob".into()), Cbor::Bytes(blob.clone())));
            outputs["getCredBlob"] = json!(b64u(&blob));
        }
        if ext["largeBlob"].is_object() {
            let supported = ctap2 && has_large_blob;
            let mut large_blob = json!({});
            if ext["largeBlob"]["read"].as_bool() == Some(true) && supported {
                if let Some(blob) = credential.large_blob.as_ref().filter(|b| !b.is_empty()) {
                    large_blob["blob"] = json!(b64u(blob));
                }
            }
            if let Some(blob) = bytes_of(&ext["largeBlob"]["write"]) {
                if supported {
                    credential.large_blob = Some(blob);
                }
                large_blob["written"] = json!(supported);
            }
            outputs["largeBlob"] = large_blob;
        }
        if ext["prf"].is_object() {
            let mut prf = json!({});
            if ctap2 && has_prf {
                // The evaluation for this credential's ID if the page gave one, else the default.
                let values = ext["prf"]["evalByCredential"].get(b64u(&credential.id)).filter(|v| v.is_object()).unwrap_or(&ext["prf"]["eval"]);
                if let Some(results) = prf_results(&credential.secret, values) {
                    prf["results"] = results;
                }
            }
            outputs["prf"] = prf;
        }
        let mut flags = FLAG_UP | credential.flags();
        if user_verified {
            flags |= FLAG_UV;
        }
        if !authenticator_extensions.is_empty() {
            flags |= FLAG_ED;
        }
        let mut data = sha256(rp_id.as_bytes());
        data.push(flags);
        data.extend_from_slice(&credential.sign_count.to_be_bytes());
        if !authenticator_extensions.is_empty() {
            data.extend_from_slice(&Cbor::Map(authenticator_extensions).encode());
        }
        let client_data = req["clientData"].as_str().unwrap_or_default().as_bytes();
        let mut signed = data.clone();
        signed.extend_from_slice(&sha256(client_data));
        let Some(signature) = sign_es256(&credential.private_key, &signed) else {
            return error("NotAllowedError", "The authenticator could not sign the assertion.");
        };
        json!({
            "status": "ok",
            "id": b64u(&credential.id),
            "clientDataJSON": b64u(client_data),
            "authenticatorData": b64u(&data),
            "signature": b64u(&signature),
            // Only a discoverable credential names its user in an assertion.
            "userHandle": if credential.resident { credential.user_handle.as_ref().map(|h| json!(b64u(h))).unwrap_or(Value::Null) } else { Value::Null },
            "attachment": attachment,
            "extensions": outputs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(options: Value) -> (State, String) {
        let mut state = State::default();
        state.command("enable", &json!({})).ok().unwrap();
        let id = match state.command("addVirtualAuthenticator", &json!({ "options": options })) {
            Ok(v) => v["authenticatorId"].as_str().unwrap().to_owned(),
            Err(_) => panic!("add failed"),
        };
        (state, id)
    }

    fn create_request(extra: Value) -> Value {
        let mut req = json!({
            "op": "create", "rpId": "example.com", "clientData": "{\"type\":\"webauthn.create\"}",
            "user": { "id": b64u(&[1, 2, 3, 4]), "name": "reimu", "displayName": "Reimu" },
            "algs": [-7], "exclude": [], "residentKey": "discouraged", "userVerification": "preferred",
            "attachment": null, "attestation": "none", "extensions": {},
        });
        for (k, v) in extra.as_object().into_iter().flatten() {
            req[k] = v.clone();
        }
        req
    }

    fn get_request(allow: Value, extra: Value) -> Value {
        let mut req = json!({
            "op": "get", "rpId": "example.com", "clientData": "{\"type\":\"webauthn.get\"}",
            "allow": allow, "userVerification": "preferred", "extensions": {}, "immediate": false, "conditional": false,
        });
        for (k, v) in extra.as_object().into_iter().flatten() {
            req[k] = v.clone();
        }
        req
    }

    fn verify(spki_der: &[u8], message: &[u8], der_signature: &[u8]) -> bool {
        use p256::ecdsa::signature::hazmat::PrehashVerifier;
        use p256::ecdsa::{Signature, VerifyingKey};
        use p256::pkcs8::DecodePublicKey;
        let public = p256::PublicKey::from_public_key_der(spki_der).unwrap();
        let key = VerifyingKey::from_sec1_bytes(public.to_encoded_point(false).as_bytes()).unwrap();
        // 30 len 02 rlen r 02 slen s, each integer as the 32 bytes of a fixed-size signature.
        assert_eq!(der_signature[0], 0x30);
        assert_eq!(der_signature[1] as usize, der_signature.len() - 2);
        let mut raw = Vec::new();
        let mut rest = &der_signature[2..];
        for _ in 0..2 {
            assert_eq!(rest[0], 0x02);
            let len = rest[1] as usize;
            let mut int = rest[2..2 + len].to_vec();
            while int.len() > 32 {
                assert_eq!(int.remove(0), 0);
            }
            while int.len() < 32 {
                int.insert(0, 0);
            }
            raw.extend(int);
            rest = &rest[2 + len..];
        }
        let signature = Signature::from_slice(&raw).unwrap();
        key.verify_prehash(&sha256(message), &signature).is_ok()
    }

    #[test]
    fn a_created_credential_asserts_with_a_signature_that_verifies() {
        let (mut s, _) = state(json!({ "protocol": "ctap2", "ctap2Version": "ctap2_1", "transport": "usb" }));
        let created = s.make_credential(&create_request(json!({})));
        assert_eq!(created["status"], "ok", "{created}");
        assert_eq!(created["attachment"], "cross-platform");
        let data = unb64(created["authenticatorData"].as_str().unwrap()).unwrap();
        assert_eq!(&data[..32], sha256(b"example.com").as_slice());
        assert_eq!(data[32] & (FLAG_UP | FLAG_AT | FLAG_UV), FLAG_UP | FLAG_AT);
        let id = created["id"].as_str().unwrap().to_owned();
        let assertion = s.get_assertion(&get_request(json!([id]), json!({})));
        assert_eq!(assertion["status"], "ok", "{assertion}");
        assert_eq!(assertion["userHandle"], Value::Null);
        let auth = unb64(assertion["authenticatorData"].as_str().unwrap()).unwrap();
        assert_eq!(u32::from_be_bytes(auth[33..37].try_into().unwrap()), 1);
        let mut signed = auth.clone();
        signed.extend(sha256(b"{\"type\":\"webauthn.get\"}"));
        let public = unb64(created["publicKey"].as_str().unwrap()).unwrap();
        assert!(verify(&public, &signed, &unb64(assertion["signature"].as_str().unwrap()).unwrap()));
        // The counter goes up per assertion.
        let again = s.get_assertion(&get_request(json!([id]), json!({})));
        let auth = unb64(again["authenticatorData"].as_str().unwrap()).unwrap();
        assert_eq!(u32::from_be_bytes(auth[33..37].try_into().unwrap()), 2);
    }

    #[test]
    fn requests_nobody_can_answer_stay_pending_or_are_refused() {
        // No authenticator at all, a platform request on a roaming key, no consenting user.
        let mut empty = State::default();
        assert_eq!(empty.make_credential(&create_request(json!({})))["status"], "pending");
        let (mut s, id) = state(json!({ "protocol": "u2f", "transport": "usb" }));
        assert_eq!(s.make_credential(&create_request(json!({ "attachment": "platform" })))["status"], "pending");
        assert_eq!(s.make_credential(&create_request(json!({ "algs": [-257] })))["status"], "pending");
        // A capability it lacks is refused at once.
        let refused = s.make_credential(&create_request(json!({ "residentKey": "required" })));
        assert_eq!(refused["name"], "NotAllowedError");
        let refused = s.get_assertion(&get_request(json!(null), json!({ "userVerification": "required" })));
        assert_eq!(refused["name"], "NotAllowedError");
        // No credential: refused after the touch; without a touch it waits.
        assert_eq!(s.get_assertion(&get_request(json!(null), json!({})))["name"], "NotAllowedError");
        s.command("setAutomaticPresenceSimulation", &json!({ "authenticatorId": id, "enabled": false })).ok().unwrap();
        assert_eq!(s.make_credential(&create_request(json!({})))["status"], "pending");
        assert_eq!(s.get_assertion(&get_request(json!(null), json!({})))["status"], "pending");
    }

    #[test]
    fn excluded_credentials_and_resident_keys() {
        let (mut s, _) = state(json!({ "protocol": "ctap2", "transport": "internal", "hasResidentKey": true, "hasUserVerification": true, "isUserVerified": true }));
        let created = s.make_credential(&create_request(json!({ "residentKey": "required", "extensions": { "credProps": true } })));
        assert_eq!(created["status"], "ok", "{created}");
        assert_eq!(created["attachment"], "platform");
        assert_eq!(created["extensions"]["credProps"]["rk"], true);
        let data = unb64(created["authenticatorData"].as_str().unwrap()).unwrap();
        assert_eq!(data[32] & FLAG_UV, FLAG_UV);
        let id = created["id"].as_str().unwrap().to_owned();
        let excluded = s.make_credential(&create_request(json!({ "exclude": [id] })));
        assert_eq!(excluded["name"], "InvalidStateError");
        // A discoverable credential answers an empty allow list, with its user handle.
        let assertion = s.get_assertion(&get_request(json!(null), json!({})));
        assert_eq!(assertion["status"], "ok", "{assertion}");
        assert_eq!(unb64(assertion["userHandle"].as_str().unwrap()).unwrap(), vec![1, 2, 3, 4]);
        // The Signal API drops it.
        s.signal(&json!({ "kind": "all", "rpId": "example.com", "userId": b64u(&[1, 2, 3, 4]), "accepted": [] }));
        assert_eq!(s.get_assertion(&get_request(json!(null), json!({})))["name"], "NotAllowedError");
    }

    #[test]
    fn extensions_prf_cred_blob_and_large_blob() {
        let (mut s, _) = state(json!({ "protocol": "ctap2", "ctap2Version": "ctap2_1", "transport": "usb", "hasResidentKey": true, "hasUserVerification": true, "isUserVerified": true,
            "hasPrf": true, "hasCredBlob": true, "hasLargeBlob": true, "hasMinPinLength": true }));
        let created = s.make_credential(&create_request(json!({ "residentKey": "required", "extensions": {
            "prf": { "eval": { "first": b64u(&[1, 2, 3, 4]) } }, "credBlob": b64u(&[9; 16]), "largeBlob": { "support": "required" }, "minPinLength": true } })));
        assert_eq!(created["status"], "ok", "{created}");
        assert_eq!(created["extensions"]["prf"]["enabled"], true);
        assert_eq!(unb64(created["extensions"]["prf"]["results"]["first"].as_str().unwrap()).unwrap().len(), 32);
        assert_eq!(created["extensions"]["credBlob"], true);
        assert_eq!(created["extensions"]["largeBlob"]["supported"], true);
        let data = unb64(created["authenticatorData"].as_str().unwrap()).unwrap();
        assert!(data.windows(12).any(|w| w == b"minPinLength"));
        let id = created["id"].as_str().unwrap().to_owned();
        let two = |a: u8, b: u8| json!({ "prf": { "eval": { "first": b64u(&[a]), "second": b64u(&[b]) } }, "getCredBlob": true, "largeBlob": { "write": b64u(b"blob") } });
        let assertion = s.get_assertion(&get_request(json!([id]), json!({ "extensions": two(1, 1) })));
        assert_eq!(assertion["extensions"]["prf"]["results"]["first"], assertion["extensions"]["prf"]["results"]["second"]);
        assert_eq!(unb64(assertion["extensions"]["getCredBlob"].as_str().unwrap()).unwrap(), vec![9; 16]);
        assert_eq!(assertion["extensions"]["largeBlob"]["written"], true);
        let other = s.get_assertion(&get_request(json!([id]), json!({ "extensions": two(1, 2) })));
        assert_ne!(other["extensions"]["prf"]["results"]["first"], other["extensions"]["prf"]["results"]["second"]);
        let read = s.get_assertion(&get_request(json!([id]), json!({ "extensions": { "largeBlob": { "read": true } } })));
        assert_eq!(unb64(read["extensions"]["largeBlob"]["blob"].as_str().unwrap()).unwrap(), b"blob");
    }

    #[test]
    fn credentials_added_by_command_round_trip() {
        let (mut s, id) = state(json!({ "protocol": "u2f", "transport": "usb" }));
        let private_key = "MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg8_zMDQDYAxlU-Qhk1Dwkf0v18GZca1DMF3SaJ9HPdmShRANCAASNYX5lyVCOZLzFZzrIKmeZ2jwURmgsJYxGP__fWN_S-j5sN4tT15XEpN_7QZnt14YvI6uvAgO0uJEboFaZlOEB";
        let credential = json!({ "credentialId": b64u(b"cred-1"), "rpId": "example.com", "privateKey": private_key, "signCount": 0, "isResidentCredential": false });
        assert!(s.command("addCredential", &json!({ "authenticatorId": id, "credential": credential })).is_ok());
        let listed = s.command("getCredentials", &json!({ "authenticatorId": id })).ok().unwrap();
        assert_eq!(listed["credentials"].as_array().unwrap().len(), 1);
        let got = &listed["credentials"][0];
        assert_eq!(unb64(got["privateKey"].as_str().unwrap()).unwrap(), unb64(private_key).unwrap());
        assert_eq!(unb64(got["credentialId"].as_str().unwrap()).unwrap(), b"cred-1");
        // The credential signs.
        let assertion = s.get_assertion(&get_request(json!([b64u(b"cred-1")]), json!({})));
        assert_eq!(assertion["status"], "ok", "{assertion}");
        s.command("removeCredential", &json!({ "authenticatorId": id, "credentialId": b64u(b"cred-1") })).ok().unwrap();
        assert!(s.command("getCredential", &json!({ "authenticatorId": id, "credentialId": b64u(b"cred-1") })).is_err());
        assert!(s.command("addCredential", &json!({ "authenticatorId": id, "credential": { "credentialId": "AA", "rpId": "a", "privateKey": "AAAA" } })).is_err());
    }
}
