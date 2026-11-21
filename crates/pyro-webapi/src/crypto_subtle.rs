//! The native half of `crypto.subtle` beyond `digest`: byte-in, byte-out
//! primitives that subtle.js (the WebIDL and Web Crypto algorithm steps, made
//! on a realm's first SubtleCrypto call or `CryptoKey` read) calls through a
//! `natives` object. Nothing here keeps state: key material lives in the
//! script's CryptoKey objects as bytes (raw secret bytes; PKCS #8 / SPKI DER
//! for EC and RSA keys; the 32 raw bytes for Ed25519 and X25519), so a key is
//! structured-cloned by copying those bytes.
//!
//! Errors carry the Web Crypto exception names: DataError for key data that
//! does not parse, OperationError for a failed operation.

use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use base64::Engine as _;
use ghash::universal_hash::UniversalHash;
use hmac::{Hmac, Mac};
use pyro_js::{JsError, JsResult, Object, Scope, Value};
use rsa::{PrivateKey as RsaPrivateKey, PublicKey as RsaPublicKey};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha384, Sha512};

#[cfg(feature = "experimental-mlkem-openssl")]
mod mlkem;
mod rsa;

fn data_error(message: impl Into<String>) -> JsError {
    JsError::dom("DataError", message)
}

fn operation_error(message: impl Into<String>) -> JsError {
    JsError::dom("OperationError", message)
}

fn bytes_arg(s: &Scope<'_>, v: Value<'_>) -> JsResult<Vec<u8>> {
    crate::encoding::buffer_source_bytes(s, v)?.ok_or_else(|| JsError::Type("expected a BufferSource".into()))
}

fn buffer<'s>(s: &Scope<'s>, bytes: &[u8]) -> JsResult<Value<'s>> {
    crate::encoding::array_buffer(s, bytes)
}

fn str_arg(v: Value<'_>) -> JsResult<String> {
    v.as_string().ok_or_else(|| JsError::Type("expected a string".into()))
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A JWK member: base64url without padding (RFC 7518).
fn unb64(text: &str) -> JsResult<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| data_error("The JWK member is not valid base64url"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hash {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    fn parse(v: Value<'_>) -> JsResult<Hash> {
        Ok(match str_arg(v)?.as_str() {
            "SHA-1" => Hash::Sha1,
            "SHA-256" => Hash::Sha256,
            "SHA-384" => Hash::Sha384,
            "SHA-512" => Hash::Sha512,
            other => return Err(JsError::dom("NotSupportedError", format!("Unrecognized hash '{other}'"))),
        })
    }

    fn digest(self, data: &[u8]) -> Vec<u8> {
        match self {
            Hash::Sha1 => Sha1::digest(data).to_vec(),
            Hash::Sha256 => Sha256::digest(data).to_vec(),
            Hash::Sha384 => Sha384::digest(data).to_vec(),
            Hash::Sha512 => Sha512::digest(data).to_vec(),
        }
    }
}

/// Runs `$body` with `$d` naming the digest type of `$hash`.
macro_rules! with_hash {
    ($hash:expr, $d:ident => $body:expr) => {
        match $hash {
            Hash::Sha1 => {
                type $d = Sha1;
                $body
            }
            Hash::Sha256 => {
                type $d = Sha256;
                $body
            }
            Hash::Sha384 => {
                type $d = Sha384;
                $body
            }
            Hash::Sha512 => {
                type $d = Sha512;
                $body
            }
        }
    };
}

// ----- AES ---------------------------------------------------------------

enum Aes {
    A128(aes::Aes128),
    A192(aes::Aes192),
    A256(aes::Aes256),
}

impl Aes {
    fn new(key: &[u8]) -> JsResult<Aes> {
        Ok(match key.len() {
            16 => Aes::A128(aes::Aes128::new_from_slice(key).expect("16-byte key")),
            24 => Aes::A192(aes::Aes192::new_from_slice(key).expect("24-byte key")),
            32 => Aes::A256(aes::Aes256::new_from_slice(key).expect("32-byte key")),
            _ => return Err(operation_error("AES key data must be 128, 192 or 256 bits")),
        })
    }

    fn encrypt(&self, block: &mut [u8; 16]) {
        let b = aes::Block::from_mut_slice(block);
        match self {
            Aes::A128(c) => c.encrypt_block(b),
            Aes::A192(c) => c.encrypt_block(b),
            Aes::A256(c) => c.encrypt_block(b),
        }
    }

    fn decrypt(&self, block: &mut [u8; 16]) {
        let b = aes::Block::from_mut_slice(block);
        match self {
            Aes::A128(c) => c.decrypt_block(b),
            Aes::A192(c) => c.decrypt_block(b),
            Aes::A256(c) => c.decrypt_block(b),
        }
    }
}

fn xor_into(out: &mut [u8], keystream: &[u8]) {
    for (o, k) in out.iter_mut().zip(keystream) {
        *o ^= k;
    }
}

/// AES-CBC with PKCS #7 padding.
fn aes_cbc(encrypt: bool, key: &[u8], iv: &[u8], data: &[u8]) -> JsResult<Vec<u8>> {
    let cipher = Aes::new(key)?;
    let mut chain: [u8; 16] = iv.try_into().map_err(|_| operation_error("The AES-CBC iv must be 16 bytes"))?;
    if encrypt {
        let pad = 16 - data.len() % 16;
        let mut out = Vec::with_capacity(data.len() + pad);
        out.extend_from_slice(data);
        out.resize(data.len() + pad, pad as u8);
        for block in out.chunks_exact_mut(16) {
            xor_into(block, &chain);
            let b: &mut [u8; 16] = block.try_into().expect("16-byte chunk");
            cipher.encrypt(b);
            chain = *b;
        }
        Ok(out)
    } else {
        if data.is_empty() || !data.len().is_multiple_of(16) {
            return Err(operation_error("The AES-CBC ciphertext is not a multiple of the block size"));
        }
        let mut out = data.to_vec();
        for block in out.chunks_exact_mut(16) {
            let b: &mut [u8; 16] = block.try_into().expect("16-byte chunk");
            let next = *b;
            cipher.decrypt(b);
            xor_into(b, &chain);
            chain = next;
        }
        let pad = *out.last().expect("non-empty") as usize;
        if pad == 0 || pad > 16 || out[out.len() - pad..].iter().any(|&p| p as usize != pad) {
            return Err(operation_error("The AES-CBC padding is invalid"));
        }
        out.truncate(out.len() - pad);
        Ok(out)
    }
}

/// AES-CTR: the rightmost `length` bits of `counter` count blocks, wrapping
/// within those bits; a counter that would repeat is an OperationError.
fn aes_ctr(key: &[u8], counter: &[u8], length: u32, data: &[u8]) -> JsResult<Vec<u8>> {
    let cipher = Aes::new(key)?;
    let counter: [u8; 16] = counter.try_into().map_err(|_| operation_error("The AES-CTR counter must be 16 bytes"))?;
    if length == 0 || length > 128 {
        return Err(operation_error("The AES-CTR length must be between 1 and 128"));
    }
    let blocks = data.len().div_ceil(16) as u128;
    if length < 128 && blocks > (1u128 << length) {
        return Err(operation_error("The AES-CTR counter would repeat"));
    }
    let mask = if length == 128 { u128::MAX } else { (1u128 << length) - 1 };
    let mut value = u128::from_be_bytes(counter);
    let mut out = data.to_vec();
    for chunk in out.chunks_mut(16) {
        let mut block = value.to_be_bytes();
        cipher.encrypt(&mut block);
        xor_into(chunk, &block);
        value = (value & !mask) | (value.wrapping_add(1) & mask);
    }
    Ok(out)
}

fn gcm_ghash(h: &[u8; 16], aad: &[u8], data: &[u8]) -> [u8; 16] {
    let mut g = ghash::GHash::new(h.into());
    g.update_padded(aad);
    g.update_padded(data);
    let mut lengths = [0u8; 16];
    lengths[..8].copy_from_slice(&((aad.len() as u64) * 8).to_be_bytes());
    lengths[8..].copy_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    g.update(&[lengths.into()]);
    g.finalize().into()
}

/// GCTR from counter block `j`, incrementing its last 32 bits.
fn gcm_ctr(cipher: &Aes, mut j: [u8; 16], data: &mut [u8]) {
    for chunk in data.chunks_mut(16) {
        let n = u32::from_be_bytes(j[12..].try_into().expect("4 bytes")).wrapping_add(1);
        j[12..].copy_from_slice(&n.to_be_bytes());
        let mut block = j;
        cipher.encrypt(&mut block);
        xor_into(chunk, &block);
    }
}

/// AES-GCM (NIST SP 800-38D) for any IV length and the tag lengths Web
/// Crypto allows; the tag follows the ciphertext.
fn aes_gcm(encrypt: bool, key: &[u8], iv: &[u8], data: &[u8], aad: &[u8], tag_bytes: usize) -> JsResult<Vec<u8>> {
    let cipher = Aes::new(key)?;
    if iv.is_empty() {
        return Err(operation_error("The AES-GCM iv must not be empty"));
    }
    let mut h = [0u8; 16];
    cipher.encrypt(&mut h);
    let j0: [u8; 16] = if iv.len() == 12 {
        let mut j = [0u8; 16];
        j[..12].copy_from_slice(iv);
        j[15] = 1;
        j
    } else {
        gcm_ghash(&h, &[], iv)
    };
    let mut tag_mask = j0;
    cipher.encrypt(&mut tag_mask);
    if encrypt {
        let mut out = data.to_vec();
        gcm_ctr(&cipher, j0, &mut out);
        let mut tag = gcm_ghash(&h, aad, &out);
        xor_into(&mut tag, &tag_mask);
        out.extend_from_slice(&tag[..tag_bytes]);
        Ok(out)
    } else {
        if data.len() < tag_bytes {
            return Err(operation_error("The AES-GCM ciphertext is shorter than its tag"));
        }
        let (body, given) = data.split_at(data.len() - tag_bytes);
        let mut tag = gcm_ghash(&h, aad, body);
        xor_into(&mut tag, &tag_mask);
        let diff = tag[..tag_bytes].iter().zip(given).fold(0u8, |acc, (a, b)| acc | (a ^ b));
        if diff != 0 {
            return Err(operation_error("The AES-GCM authentication tag does not match"));
        }
        let mut out = body.to_vec();
        gcm_ctr(&cipher, j0, &mut out);
        Ok(out)
    }
}

fn aes_kw(wrap: bool, key: &[u8], data: &[u8]) -> JsResult<Vec<u8>> {
    macro_rules! kw {
        ($kek:ty) => {{
            let kek = <$kek>::try_from(key).map_err(|_| operation_error("Invalid AES-KW key"))?;
            if wrap {
                kek.wrap_vec(data).map_err(|_| operation_error("The data cannot be wrapped with AES-KW"))
            } else {
                kek.unwrap_vec(data).map_err(|_| operation_error("The data cannot be unwrapped with AES-KW"))
            }
        }};
    }
    if wrap && (data.len() < 16 || !data.len().is_multiple_of(8)) {
        return Err(operation_error("AES-KW data must be a multiple of 8 bytes and at least 16 bytes"));
    }
    match key.len() {
        16 => kw!(aes_kw::KekAes128),
        24 => kw!(aes_kw::KekAes192),
        32 => kw!(aes_kw::KekAes256),
        _ => Err(operation_error("AES key data must be 128, 192 or 256 bits")),
    }
}

// ----- HMAC and key derivation ---------------------------------------------

fn hmac(hash: Hash, key: &[u8], data: &[u8]) -> Vec<u8> {
    with_hash!(hash, D => {
        let mut mac = <Hmac<D> as Mac>::new_from_slice(key).expect("HMAC takes keys of any length");
        mac.update(data);
        mac.finalize().into_bytes().to_vec()
    })
}

fn hmac_verify(hash: Hash, key: &[u8], data: &[u8], signature: &[u8]) -> bool {
    with_hash!(hash, D => {
        let mut mac = <Hmac<D> as Mac>::new_from_slice(key).expect("HMAC takes keys of any length");
        mac.update(data);
        mac.verify_slice(signature).is_ok()
    })
}

fn pbkdf2(hash: Hash, password: &[u8], salt: &[u8], iterations: u32, bytes: usize) -> Vec<u8> {
    let mut out = vec![0u8; bytes];
    with_hash!(hash, D => pbkdf2::pbkdf2_hmac::<D>(password, salt, iterations, &mut out));
    out
}

fn hkdf(hash: Hash, ikm: &[u8], salt: &[u8], info: &[u8], bytes: usize) -> JsResult<Vec<u8>> {
    let mut out = vec![0u8; bytes];
    with_hash!(hash, D => hkdf::Hkdf::<D>::new(Some(salt), ikm)
        .expand(info, &mut out)
        .map_err(|_| operation_error("The HKDF length is too large for the hash"))?);
    Ok(out)
}

// ----- Elliptic curves (NIST) ---------------------------------------------

/// The NIST curve operations over SPKI (public) and PKCS #8 (private) DER.
macro_rules! nist_curve {
    ($m:ident, $c:ident) => {
        mod $m {
            use super::{data_error, operation_error};
            use pyro_js::{JsError, JsResult};
            use $c::ecdsa::signature::hazmat::{PrehashSigner, PrehashVerifier};
            use $c::ecdsa::{Signature, SigningKey, VerifyingKey};
            use $c::elliptic_curve::sec1::{FromEncodedPoint, ToEncodedPoint};
            use $c::pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey};
            use $c::{AffinePoint, EncodedPoint, FieldBytes, PublicKey, SecretKey};

            pub(super) const FIELD: usize = std::mem::size_of::<FieldBytes>();

            fn spki(key: &PublicKey) -> JsResult<Vec<u8>> {
                Ok(key.to_public_key_der().map_err(|_| operation_error("cannot encode the key"))?.as_bytes().to_vec())
            }
            fn pkcs8(key: &SecretKey) -> JsResult<Vec<u8>> {
                Ok(key.to_pkcs8_der().map_err(|_| operation_error("cannot encode the key"))?.as_bytes().to_vec())
            }
            fn secret(der: &[u8]) -> JsResult<SecretKey> {
                SecretKey::from_pkcs8_der(der).map_err(|_| data_error("Invalid EC private key"))
            }
            fn public(der: &[u8]) -> JsResult<PublicKey> {
                PublicKey::from_public_key_der(der).map_err(|_| data_error("Invalid EC public key"))
            }
            pub(super) fn import_pkcs8(der: &[u8]) -> JsResult<Vec<u8>> {
                pkcs8(&secret(der)?)
            }
            pub(super) fn import_spki(der: &[u8]) -> JsResult<Vec<u8>> {
                spki(&public(der)?)
            }
            pub(super) fn import_raw(point: &[u8]) -> JsResult<Vec<u8>> {
                spki(&PublicKey::from_sec1_bytes(point).map_err(|_| data_error("Invalid EC point"))?)
            }
            fn from_xy(x: &[u8], y: &[u8]) -> JsResult<PublicKey> {
                if x.len() != FIELD || y.len() != FIELD {
                    return Err(data_error("The JWK x and y members have the wrong length for the curve"));
                }
                let point = EncodedPoint::from_affine_coordinates(FieldBytes::from_slice(x), FieldBytes::from_slice(y), false);
                Option::<AffinePoint>::from(AffinePoint::from_encoded_point(&point))
                    .and_then(|p| PublicKey::from_affine(p).ok())
                    .ok_or_else(|| data_error("The JWK point is not on the curve"))
            }
            pub(super) fn import_jwk_public(x: &[u8], y: &[u8]) -> JsResult<Vec<u8>> {
                spki(&from_xy(x, y)?)
            }
            pub(super) fn import_jwk_private(d: &[u8], x: &[u8], y: &[u8]) -> JsResult<Vec<u8>> {
                if d.len() != FIELD {
                    return Err(data_error("The JWK d member has the wrong length for the curve"));
                }
                let key = SecretKey::from_slice(d).map_err(|_| data_error("Invalid EC private key"))?;
                if key.public_key() != from_xy(x, y)? {
                    return Err(data_error("The JWK d member does not match x and y"));
                }
                pkcs8(&key)
            }
            pub(super) fn raw(spki: &[u8]) -> JsResult<Vec<u8>> {
                Ok(public(spki)?.to_encoded_point(false).as_bytes().to_vec())
            }
            fn xy(key: &PublicKey) -> (Vec<u8>, Vec<u8>) {
                let point = key.to_encoded_point(false);
                (point.x().expect("affine").to_vec(), point.y().expect("uncompressed").to_vec())
            }
            pub(super) fn jwk_public(spki: &[u8]) -> JsResult<(Vec<u8>, Vec<u8>)> {
                Ok(xy(&public(spki)?))
            }
            pub(super) fn jwk_private(der: &[u8]) -> JsResult<(Vec<u8>, Vec<u8>, Vec<u8>)> {
                let key = secret(der)?;
                let (x, y) = xy(&key.public_key());
                Ok((key.to_bytes().to_vec(), x, y))
            }
            pub(super) fn public_of(der: &[u8]) -> JsResult<Vec<u8>> {
                spki(&secret(der)?.public_key())
            }
            pub(super) fn generate() -> JsResult<(Vec<u8>, Vec<u8>)> {
                let key = SecretKey::random(&mut rand_core::OsRng);
                Ok((pkcs8(&key)?, spki(&key.public_key())?))
            }
            /// `prehash` is the message digest, left-padded to the field size
            /// when shorter (its integer value is what ECDSA signs).
            pub(super) fn sign(der: &[u8], prehash: &[u8]) -> JsResult<Vec<u8>> {
                let key = SigningKey::from_bytes(&secret(der)?.to_bytes())
                    .map_err(|_| data_error("Invalid EC private key"))?;
                let signature: Signature =
                    key.sign_prehash(prehash).map_err(|e: $c::ecdsa::Error| JsError::dom("OperationError", e.to_string()))?;
                Ok(signature.to_bytes().to_vec())
            }
            pub(super) fn verify(spki: &[u8], prehash: &[u8], signature: &[u8]) -> JsResult<bool> {
                let key = VerifyingKey::from_sec1_bytes(public(spki)?.to_encoded_point(false).as_bytes())
                    .map_err(|_| data_error("Invalid EC public key"))?;
                let Ok(signature) = Signature::from_slice(signature) else { return Ok(false) };
                Ok(key.verify_prehash(prehash, &signature).is_ok())
            }
            pub(super) fn derive(der: &[u8], spki: &[u8]) -> JsResult<Vec<u8>> {
                let shared = $c::ecdh::diffie_hellman(secret(der)?.to_nonzero_scalar(), public(spki)?.as_affine());
                Ok(shared.raw_secret_bytes().to_vec())
            }
        }
    };
}

nist_curve!(p256_ops, p256);
nist_curve!(p384_ops, p384);
nist_curve!(p521_ops, p521);

/// Dispatches `$f` to the curve named by `$curve`.
macro_rules! on_curve {
    ($curve:expr, $f:ident($($a:expr),*)) => {
        match $curve {
            "P-256" => p256_ops::$f($($a),*),
            "P-384" => p384_ops::$f($($a),*),
            "P-521" => p521_ops::$f($($a),*),
            other => Err(JsError::dom("NotSupportedError", format!("Unsupported curve '{other}'"))),
        }
    };
}

fn field_size(curve: &str) -> usize {
    match curve {
        "P-384" => p384_ops::FIELD,
        "P-521" => p521_ops::FIELD,
        _ => p256_ops::FIELD,
    }
}

/// The ECDSA input for `data`: its digest, left-padded with zeros to the
/// field size when shorter (the same integer, which is what ECDSA signs).
fn ecdsa_prehash(curve: &str, hash: Hash, data: &[u8]) -> Vec<u8> {
    let digest = hash.digest(data);
    let field = field_size(curve);
    if digest.len() >= field {
        return digest;
    }
    let mut padded = vec![0u8; field - digest.len()];
    padded.extend_from_slice(&digest);
    padded
}

// ----- Ed25519 / X25519 ----------------------------------------------------

use p256::pkcs8::der::Decode as _;
use p256::pkcs8::ObjectIdentifier;

const OID_ED25519: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.101.112");
const OID_X25519: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.101.110");

fn okp_oid(kind: &str) -> ObjectIdentifier {
    if kind == "Ed25519" {
        OID_ED25519
    } else {
        OID_X25519
    }
}

/// RFC 8410 SPKI of a 32-byte public key.
fn okp_spki(kind: &str, public: &[u8]) -> Vec<u8> {
    let last = if kind == "Ed25519" { 0x70 } else { 0x6e };
    let mut out = vec![0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, last, 0x03, 0x21, 0x00];
    out.extend_from_slice(public);
    out
}

/// RFC 8410 PKCS #8 (version 1, no public key) of a 32-byte private key.
fn okp_pkcs8(kind: &str, private: &[u8]) -> Vec<u8> {
    let last = if kind == "Ed25519" { 0x70 } else { 0x6e };
    let mut out = vec![0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, last, 0x04, 0x22, 0x04, 0x20];
    out.extend_from_slice(private);
    out
}

fn okp_import_pkcs8(kind: &str, der: &[u8]) -> JsResult<Vec<u8>> {
    let info = p256::pkcs8::PrivateKeyInfo::from_der(der).map_err(|_| data_error("Invalid PKCS #8 key data"))?;
    if info.algorithm.oid != okp_oid(kind) || info.algorithm.parameters.is_some() {
        return Err(data_error(format!("The key is not an {kind} key")));
    }
    // CurvePrivateKey ::= OCTET STRING
    let key = p256::pkcs8::der::asn1::OctetStringRef::from_der(info.private_key)
        .map_err(|_| data_error("Invalid PKCS #8 key data"))?;
    let key = key.as_bytes();
    if key.len() != 32 {
        return Err(data_error("Invalid private key length"));
    }
    Ok(key.to_vec())
}

fn okp_import_spki(kind: &str, der: &[u8]) -> JsResult<Vec<u8>> {
    let info =
        p256::pkcs8::spki::SubjectPublicKeyInfoRef::from_der(der).map_err(|_| data_error("Invalid SPKI key data"))?;
    if info.algorithm.oid != okp_oid(kind) || info.algorithm.parameters.is_some() {
        return Err(data_error(format!("The key is not an {kind} key")));
    }
    let key = info.subject_public_key.as_bytes().ok_or_else(|| data_error("Invalid SPKI key data"))?;
    okp_check_public(kind, key)?;
    Ok(key.to_vec())
}

/// Only the length: as in Chromium, any 32 bytes import, and a point that does
/// not decode (or has small order) fails at verification instead.
fn okp_check_public(_kind: &str, key: &[u8]) -> JsResult<()> {
    if key.len() != 32 {
        return Err(data_error("Invalid public key length"));
    }
    Ok(())
}

fn okp_public_of(kind: &str, private: &[u8]) -> JsResult<Vec<u8>> {
    let bytes: [u8; 32] = private.try_into().map_err(|_| data_error("Invalid private key length"))?;
    Ok(if kind == "Ed25519" {
        ed25519_dalek::SigningKey::from_bytes(&bytes).verifying_key().to_bytes().to_vec()
    } else {
        x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(bytes)).to_bytes().to_vec()
    })
}

// ----- RSA (rsa.rs) -------------------------------------------------------

fn rsa_private(der: &[u8]) -> JsResult<RsaPrivateKey> {
    RsaPrivateKey::from_pkcs8(der)
}

fn rsa_public(der: &[u8]) -> JsResult<RsaPublicKey> {
    RsaPublicKey::from_spki(der)
}

// ----- The natives object --------------------------------------------------

/// An `importKey` result for subtle.js: `[type, material, modulusLength,
/// publicExponent]`, the last two for RSA only.
fn imported<'s>(s: &Scope<'s>, private: bool, material: &[u8], rsa: Option<&RsaPublicKey>) -> JsResult<Value<'s>> {
    let mut items = vec![s.string(if private { "private" } else { "public" }), buffer(s, material)?];
    if let Some(key) = rsa {
        items.push(s.number(key.bits() as f64));
        items.push(buffer(s, &key.e_bytes())?);
    }
    Ok(s.array(&items)?.value())
}

fn strings<'s>(s: &Scope<'s>, parts: &[&[u8]]) -> JsResult<Value<'s>> {
    let items: Vec<Value<'s>> = parts.iter().map(|p| s.string(&b64(p))).collect();
    Ok(s.array(&items)?.value())
}

/// JWK members as subtle.js passes them: an array of strings or undefined.
fn jwk_parts(v: Value<'_>, count: u32) -> JsResult<Vec<Option<Vec<u8>>>> {
    let array = v.as_object().ok_or_else(|| JsError::Type("expected JWK members".into()))?;
    (0..count)
        .map(|i| {
            let item = array.get_index(i)?;
            if item.is_undefined() {
                Ok(None)
            } else {
                unb64(&str_arg(item)?).map(Some)
            }
        })
        .collect()
}

fn required(part: &Option<Vec<u8>>, name: &str) -> JsResult<Vec<u8>> {
    part.clone().ok_or_else(|| data_error(format!("The JWK '{name}' member is missing")))
}

/// `importAsym(kind, curve, format, data)`: kind is "EC", "RSA", "Ed25519"
/// or "X25519"; for "jwk", `data` is the member strings ([d, x, y] for EC,
/// [d, x] for OKP, [n, e, d, p, q, dp, dq, qi] for RSA).
fn import_asym<'s>(s: &Scope<'s>, kind: &str, curve: &str, format: &str, data: Value<'s>) -> JsResult<Value<'s>> {
    match (kind, format) {
        ("EC", "pkcs8") => imported(s, true, &on_curve!(curve, import_pkcs8(&bytes_arg(s, data)?))?, None),
        ("EC", "spki") => imported(s, false, &on_curve!(curve, import_spki(&bytes_arg(s, data)?))?, None),
        ("EC", "raw") => imported(s, false, &on_curve!(curve, import_raw(&bytes_arg(s, data)?))?, None),
        ("EC", "jwk") => {
            let parts = jwk_parts(data, 3)?;
            let x = required(&parts[1], "x")?;
            let y = required(&parts[2], "y")?;
            match &parts[0] {
                Some(d) => imported(s, true, &on_curve!(curve, import_jwk_private(d, &x, &y))?, None),
                None => imported(s, false, &on_curve!(curve, import_jwk_public(&x, &y))?, None),
            }
        }
        ("Ed25519" | "X25519", "pkcs8") => imported(s, true, &okp_import_pkcs8(kind, &bytes_arg(s, data)?)?, None),
        ("Ed25519" | "X25519", "spki") => imported(s, false, &okp_import_spki(kind, &bytes_arg(s, data)?)?, None),
        ("Ed25519" | "X25519", "raw") => {
            let key = bytes_arg(s, data)?;
            okp_check_public(kind, &key)?;
            imported(s, false, &key, None)
        }
        ("Ed25519" | "X25519", "jwk") => {
            let parts = jwk_parts(data, 2)?;
            let x = required(&parts[1], "x")?;
            okp_check_public(kind, &x)?;
            match &parts[0] {
                Some(d) => {
                    if d.len() != 32 || okp_public_of(kind, d)? != x {
                        return Err(data_error("The JWK d member does not match x"));
                    }
                    imported(s, true, d, None)
                }
                None => imported(s, false, &x, None),
            }
        }
        ("RSA", "pkcs8") => {
            let key = rsa_private(&bytes_arg(s, data)?)?;
            imported(s, true, &key.to_pkcs8()?, Some(key.public()))
        }
        ("RSA", "spki") => {
            let key = rsa_public(&bytes_arg(s, data)?)?;
            imported(s, false, &key.to_spki()?, Some(&key))
        }
        ("RSA", "jwk") => {
            let p = jwk_parts(data, 8)?;
            let n = required(&p[0], "n")?;
            let e = required(&p[1], "e")?;
            match &p[2] {
                None => {
                    let key = RsaPublicKey::from_components(&n, &e)?;
                    imported(s, false, &key.to_spki()?, Some(&key))
                }
                Some(d) => {
                    let names = ["p", "q", "dp", "dq", "qi"];
                    let crt: Vec<Vec<u8>> = (0..5).map(|i| required(&p[3 + i], names[i])).collect::<JsResult<_>>()?;
                    let key =
                        RsaPrivateKey::from_components(&n, &e, d, &crt[0], &crt[1], Some([&crt[2], &crt[3], &crt[4]]))?;
                    imported(s, true, &key.to_pkcs8()?, Some(key.public()))
                }
            }
        }
        _ => Err(JsError::dom("NotSupportedError", format!("Unsupported key format '{format}'"))),
    }
}

/// `exportAsym(kind, curve, type, format, material)`: bytes, or for "jwk"
/// the base64url member strings in `import_asym`'s order.
fn export_asym<'s>(
    s: &Scope<'s>,
    kind: &str,
    curve: &str,
    private: bool,
    format: &str,
    material: &[u8],
) -> JsResult<Value<'s>> {
    match (kind, format, private) {
        (_, "spki", false) | (_, "pkcs8", true) if kind == "EC" || kind == "RSA" => buffer(s, material),
        ("EC", "raw", false) => buffer(s, &on_curve!(curve, raw(material))?),
        ("EC", "jwk", false) => {
            let (x, y) = on_curve!(curve, jwk_public(material))?;
            strings(s, &[&x, &y])
        }
        ("EC", "jwk", true) => {
            let (d, x, y) = on_curve!(curve, jwk_private(material))?;
            strings(s, &[&d, &x, &y])
        }
        ("Ed25519" | "X25519", "raw", false) => buffer(s, material),
        ("Ed25519" | "X25519", "spki", false) => buffer(s, &okp_spki(kind, material)),
        ("Ed25519" | "X25519", "pkcs8", true) => buffer(s, &okp_pkcs8(kind, material)),
        ("Ed25519" | "X25519", "jwk", false) => strings(s, &[material]),
        ("Ed25519" | "X25519", "jwk", true) => strings(s, &[material, &okp_public_of(kind, material)?]),
        ("RSA", "jwk", false) => {
            let key = rsa_public(material)?;
            strings(s, &[&key.n_bytes(), &key.e_bytes()])
        }
        ("RSA", "jwk", true) => {
            let parts = rsa_private(material)?.components();
            let parts: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
            strings(s, &parts)
        }
        _ => Err(JsError::dom("InvalidAccessError", format!("The key cannot be exported as '{format}'"))),
    }
}

/// `generateAsym(kind, curve, modulusLength, publicExponent)`:
/// `[private material, public material, modulusLength?, publicExponent?]`.
fn generate_asym<'s>(s: &Scope<'s>, kind: &str, curve: &str, bits: u32, exponent: &[u8]) -> JsResult<Value<'s>> {
    let (private, public) = match kind {
        "EC" => on_curve!(curve, generate())?,
        "Ed25519" | "X25519" => {
            let mut seed = [0u8; 32];
            getrandom::fill(&mut seed).map_err(|e| operation_error(e.to_string()))?;
            let public = okp_public_of(kind, &seed)?;
            (seed.to_vec(), public)
        }
        "RSA" => {
            let key = RsaPrivateKey::generate(bits, exponent)?;
            (key.to_pkcs8()?, key.public().to_spki()?)
        }
        _ => return Err(JsError::dom("NotSupportedError", "Unsupported algorithm")),
    };
    Ok(s.array(&[buffer(s, &private)?, buffer(s, &public)?])?.value())
}

/// RSA-OAEP: encrypt with an SPKI key or decrypt with a PKCS #8 key.
fn rsa_oaep(encrypt: bool, hash: Hash, label: &[u8], der: &[u8], data: &[u8]) -> JsResult<Vec<u8>> {
    if encrypt {
        rsa::oaep_encrypt(hash, label, &rsa_public(der)?, data)
    } else {
        rsa::oaep_decrypt(hash, label, &rsa_private(der)?, data)
    }
}

fn sign_asym(kind: &str, curve: &str, hash: Hash, salt: usize, der: &[u8], data: &[u8]) -> JsResult<Vec<u8>> {
    match kind {
        "EC" => on_curve!(curve, sign(der, &ecdsa_prehash(curve, hash, data))),
        "Ed25519" => {
            use ed25519_dalek::Signer;
            let seed: [u8; 32] = der.try_into().map_err(|_| data_error("Invalid Ed25519 private key"))?;
            Ok(ed25519_dalek::SigningKey::from_bytes(&seed).sign(data).to_bytes().to_vec())
        }
        "PKCS1" | "PSS" => rsa::sign(kind, hash, salt, &rsa_private(der)?, data),
        _ => Err(JsError::dom("NotSupportedError", "Unsupported algorithm")),
    }
}

fn verify_asym(
    kind: &str,
    curve: &str,
    hash: Hash,
    salt: usize,
    der: &[u8],
    signature: &[u8],
    data: &[u8],
) -> JsResult<bool> {
    match kind {
        "EC" => on_curve!(curve, verify(der, &ecdsa_prehash(curve, hash, data), signature)),
        "Ed25519" => {
            let public: [u8; 32] = der.try_into().map_err(|_| data_error("Invalid Ed25519 public key"))?;
            let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(&public) else { return Ok(false) };
            let Ok(signature) = ed25519_dalek::Signature::from_slice(signature) else { return Ok(false) };
            // Strict: small-order keys and R values do not verify, as in Chromium.
            Ok(key.verify_strict(data, &signature).is_ok())
        }
        "PKCS1" | "PSS" => Ok(rsa::verify(kind, hash, salt, &rsa_public(der)?, signature, data)),
        _ => Err(JsError::dom("NotSupportedError", "Unsupported algorithm")),
    }
}

fn derive_asym(kind: &str, curve: &str, private: &[u8], public: &[u8]) -> JsResult<Vec<u8>> {
    match kind {
        "EC" => on_curve!(curve, derive(private, public)),
        "X25519" => {
            let secret: [u8; 32] = private.try_into().map_err(|_| data_error("Invalid X25519 private key"))?;
            let peer: [u8; 32] = public.try_into().map_err(|_| data_error("Invalid X25519 public key"))?;
            let shared = x25519_dalek::StaticSecret::from(secret).diffie_hellman(&x25519_dalek::PublicKey::from(peer));
            if !shared.was_contributory() {
                return Err(operation_error("The X25519 shared secret is all zeros"));
            }
            Ok(shared.as_bytes().to_vec())
        }
        _ => Err(JsError::dom("NotSupportedError", "Unsupported algorithm")),
    }
}

fn u32_arg(v: Value<'_>) -> JsResult<u32> {
    let n = v.to_number()?;
    Ok(if n.is_finite() && n > 0.0 { n.min(u32::MAX as f64) as u32 } else { 0 })
}

/// The object subtle.js receives: one function per primitive.
pub(crate) fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let n = s.object();
    #[cfg(feature = "experimental-mlkem-openssl")]
    if crate::secure_context::secure(s, crate::target::page_of(s)?.as_ref()) {
        mlkem::install(s, n)?;
    }
    macro_rules! native {
        ($name:literal, |$s:ident, $a:ident| $body:expr) => {
            n.set($name, s.function($name, |$s, _this, $a| $body)?.value())?;
        };
    }
    native!("random", |s, a| {
        let mut bytes = vec![0u8; u32_arg(a.get(0))? as usize];
        getrandom::fill(&mut bytes).map_err(|e| operation_error(e.to_string()))?;
        buffer(s, &bytes)
    });
    native!("digest", |s, a| buffer(s, &Hash::parse(a.get(0))?.digest(&bytes_arg(s, a.get(1))?)));
    native!("hmac", |s, a| {
        let hash = Hash::parse(a.get(0))?;
        let (key, data) = (bytes_arg(s, a.get(1))?, bytes_arg(s, a.get(2))?);
        if a.get(3).is_undefined() {
            buffer(s, &hmac(hash, &key, &data))
        } else {
            Ok(s.boolean(hmac_verify(hash, &key, &data, &bytes_arg(s, a.get(3))?)))
        }
    });
    native!("chacha20Poly1305", |s, a| {
        use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};
        let key = UnboundKey::new(&CHACHA20_POLY1305, &bytes_arg(s, a.get(1))?)
            .map_err(|_| operation_error("ChaCha20-Poly1305 requires a 256-bit key"))?;
        // Web Crypto supplies the caller's IV for each operation.
        let nonce = Nonce::try_assume_unique_for_key(&bytes_arg(s, a.get(2))?)
            .map_err(|_| operation_error("ChaCha20-Poly1305 requires a 96-bit IV"))?;
        let mut bytes = bytes_arg(s, a.get(3))?;
        let aad = Aad::from(bytes_arg(s, a.get(4))?);
        let key = LessSafeKey::new(key);
        if a.get(0).to_bool() {
            key.seal_in_place_append_tag(nonce, aad, &mut bytes)
                .map_err(|_| operation_error("ChaCha20-Poly1305 encryption failed"))?;
            buffer(s, &bytes)
        } else {
            let plaintext = key.open_in_place(nonce, aad, &mut bytes)
                .map_err(|_| operation_error("ChaCha20-Poly1305 decryption failed"))?;
            buffer(s, plaintext)
        }
    });
    native!("aesCbc", |s, a| {
        let out =
            aes_cbc(a.get(0).to_bool(), &bytes_arg(s, a.get(1))?, &bytes_arg(s, a.get(2))?, &bytes_arg(s, a.get(3))?)?;
        buffer(s, &out)
    });
    native!("aesCtr", |s, a| {
        let out =
            aes_ctr(&bytes_arg(s, a.get(0))?, &bytes_arg(s, a.get(1))?, u32_arg(a.get(2))?, &bytes_arg(s, a.get(3))?)?;
        buffer(s, &out)
    });
    native!("aesGcm", |s, a| {
        let out = aes_gcm(
            a.get(0).to_bool(),
            &bytes_arg(s, a.get(1))?,
            &bytes_arg(s, a.get(2))?,
            &bytes_arg(s, a.get(3))?,
            &bytes_arg(s, a.get(4))?,
            u32_arg(a.get(5))? as usize,
        )?;
        buffer(s, &out)
    });
    native!("aesKw", |s, a| buffer(
        s,
        &aes_kw(a.get(0).to_bool(), &bytes_arg(s, a.get(1))?, &bytes_arg(s, a.get(2))?)?
    ));
    native!("pbkdf2", |s, a| {
        let out = pbkdf2(
            Hash::parse(a.get(0))?,
            &bytes_arg(s, a.get(1))?,
            &bytes_arg(s, a.get(2))?,
            u32_arg(a.get(3))?,
            u32_arg(a.get(4))? as usize,
        );
        buffer(s, &out)
    });
    native!("hkdf", |s, a| {
        let out = hkdf(
            Hash::parse(a.get(0))?,
            &bytes_arg(s, a.get(1))?,
            &bytes_arg(s, a.get(2))?,
            &bytes_arg(s, a.get(3))?,
            u32_arg(a.get(4))? as usize,
        )?;
        buffer(s, &out)
    });
    native!("importAsym", |s, a| {
        import_asym(s, &str_arg(a.get(0))?, &str_arg(a.get(1))?, &str_arg(a.get(2))?, a.get(3))
    });
    native!("exportAsym", |s, a| {
        let private = str_arg(a.get(2))? == "private";
        export_asym(s, &str_arg(a.get(0))?, &str_arg(a.get(1))?, private, &str_arg(a.get(3))?, &bytes_arg(s, a.get(4))?)
    });
    native!("publicOf", |s, a| {
        let (kind, curve, material) = (str_arg(a.get(0))?, str_arg(a.get(1))?, bytes_arg(s, a.get(2))?);
        let public = match kind.as_str() {
            "EC" => on_curve!(curve.as_str(), public_of(&material))?,
            "RSA" => rsa_private(&material)?.public().to_spki()?,
            _ => okp_public_of(&kind, &material)?,
        };
        buffer(s, &public)
    });
    native!("generateAsym", |s, a| {
        let exponent = if a.get(3).is_undefined() { Vec::new() } else { bytes_arg(s, a.get(3))? };
        generate_asym(s, &str_arg(a.get(0))?, &str_arg(a.get(1))?, u32_arg(a.get(2))?, &exponent)
    });
    native!("signAsym", |s, a| {
        let hash = if a.get(2).is_undefined() { Hash::Sha256 } else { Hash::parse(a.get(2))? };
        let out = sign_asym(
            &str_arg(a.get(0))?,
            &str_arg(a.get(1))?,
            hash,
            u32_arg(a.get(3))? as usize,
            &bytes_arg(s, a.get(4))?,
            &bytes_arg(s, a.get(5))?,
        )?;
        buffer(s, &out)
    });
    native!("verifyAsym", |s, a| {
        let hash = if a.get(2).is_undefined() { Hash::Sha256 } else { Hash::parse(a.get(2))? };
        let ok = verify_asym(
            &str_arg(a.get(0))?,
            &str_arg(a.get(1))?,
            hash,
            u32_arg(a.get(3))? as usize,
            &bytes_arg(s, a.get(4))?,
            &bytes_arg(s, a.get(5))?,
            &bytes_arg(s, a.get(6))?,
        )?;
        Ok(s.boolean(ok))
    });
    native!("rsaOaep", |s, a| {
        let out = rsa_oaep(
            a.get(0).to_bool(),
            Hash::parse(a.get(1))?,
            &bytes_arg(s, a.get(2))?,
            &bytes_arg(s, a.get(3))?,
            &bytes_arg(s, a.get(4))?,
        )?;
        buffer(s, &out)
    });
    native!("deriveAsym", |s, a| {
        let out =
            derive_asym(&str_arg(a.get(0))?, &str_arg(a.get(1))?, &bytes_arg(s, a.get(2))?, &bytes_arg(s, a.get(3))?)?;
        buffer(s, &out)
    });
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn aes_gcm_matches_nist_test_case_4() {
        // McGrew & Viega GCM spec, test case 4 (96-bit IV, AAD).
        let key = hex("feffe9928665731c6d6a8f9467308308");
        let iv = hex("cafebabefacedbaddecaf888");
        let plain = hex("d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39");
        let aad = hex("feedfacedeadbeeffeedfacedeadbeefabaddad2");
        let out = aes_gcm(true, &key, &iv, &plain, &aad, 16).unwrap();
        assert_eq!(&out[plain.len()..], &hex("5bc94fbc3221a5db94fae95ae7121a47")[..]);
        assert_eq!(aes_gcm(false, &key, &iv, &out, &aad, 16).unwrap(), plain);
        let mut bad = out.clone();
        bad[0] ^= 1;
        assert!(aes_gcm(false, &key, &iv, &bad, &aad, 16).is_err());
    }

    #[test]
    fn aes_gcm_long_iv_matches_test_case_6() {
        let key = hex("feffe9928665731c6d6a8f9467308308");
        let iv = hex("9313225df88406e555909c5aff5269aa6a7a9538534f7da1e4c303d2a318a728c3c0c95156809539fcf0e2429a6b525416aedbf5a0de6a57a637b39b");
        let plain = hex("d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39");
        let aad = hex("feedfacedeadbeeffeedfacedeadbeefabaddad2");
        let out = aes_gcm(true, &key, &iv, &plain, &aad, 16).unwrap();
        assert_eq!(&out[plain.len()..], &hex("619cc5aefffe0bfa462af43c1699d050")[..]);
    }

    #[test]
    fn aes_cbc_round_trips_and_rejects_bad_padding() {
        let key = [7u8; 16];
        let iv = [1u8; 16];
        for len in [0, 1, 15, 16, 17, 40] {
            let data: Vec<u8> = (0..len as u8).collect();
            let c = aes_cbc(true, &key, &iv, &data).unwrap();
            assert_eq!(c.len(), (len / 16 + 1) * 16);
            assert_eq!(aes_cbc(false, &key, &iv, &c).unwrap(), data);
        }
        assert!(aes_cbc(false, &key, &iv, &[0u8; 15]).is_err());
    }

    #[test]
    fn aes_ctr_wraps_within_its_counter_bits() {
        let key = [3u8; 16];
        let mut counter = [0u8; 16];
        counter[15] = 0xff;
        let data = [0u8; 32];
        // Two blocks with a 1-bit counter: 0xff then 0xfe (the low bit wraps).
        let out = aes_ctr(&key, &counter, 1, &data).unwrap();
        let mut second = counter;
        second[15] = 0xfe;
        assert_eq!(&out[16..], &aes_ctr(&key, &second, 1, &[0u8; 16]).unwrap()[..]);
        assert!(aes_ctr(&key, &counter, 1, &[0u8; 48]).is_err());
    }

    #[test]
    fn hkdf_and_pbkdf2_match_rfc_vectors() {
        // RFC 5869 test case 1.
        let okm = hkdf(
            Hash::Sha256,
            &hex("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b"),
            &hex("000102030405060708090a0b0c"),
            &hex("f0f1f2f3f4f5f6f7f8f9"),
            42,
        )
        .unwrap();
        assert_eq!(okm, hex("3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"));
        // RFC 6070 test case 2.
        assert_eq!(pbkdf2(Hash::Sha1, b"password", b"salt", 2, 20), hex("ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957"));
    }

    #[test]
    fn ecdsa_signs_and_verifies_on_every_curve() {
        for curve in ["P-256", "P-384", "P-521"] {
            let (private, public) = on_curve!(curve, generate()).unwrap();
            for hash in [Hash::Sha1, Hash::Sha256, Hash::Sha384, Hash::Sha512] {
                let signature = sign_asym("EC", curve, hash, 0, &private, b"message").unwrap();
                assert_eq!(signature.len(), 2 * field_size(curve));
                assert!(verify_asym("EC", curve, hash, 0, &public, &signature, b"message").unwrap());
                assert!(!verify_asym("EC", curve, hash, 0, &public, &signature, b"other").unwrap());
            }
            let (other_private, other_public) = on_curve!(curve, generate()).unwrap();
            assert_eq!(
                derive_asym("EC", curve, &private, &other_public).unwrap(),
                derive_asym("EC", curve, &other_private, &public).unwrap()
            );
        }
    }

    #[test]
    fn okp_der_round_trips() {
        for kind in ["Ed25519", "X25519"] {
            let seed = [9u8; 32];
            let public = okp_public_of(kind, &seed).unwrap();
            assert_eq!(okp_import_pkcs8(kind, &okp_pkcs8(kind, &seed)).unwrap(), seed);
            assert_eq!(okp_import_spki(kind, &okp_spki(kind, &public)).unwrap(), public);
            let other = if kind == "Ed25519" { "X25519" } else { "Ed25519" };
            assert!(okp_import_pkcs8(other, &okp_pkcs8(kind, &seed)).is_err());
        }
    }

    #[test]
    fn rsa_signatures_and_oaep_round_trip() {
        let key = RsaPrivateKey::generate(1024, &[1, 0, 1]).unwrap();
        let private = key.to_pkcs8().unwrap();
        let public = key.public().to_spki().unwrap();
        for scheme in ["PKCS1", "PSS"] {
            let signature = sign_asym(scheme, "", Hash::Sha256, 32, &private, b"data").unwrap();
            assert!(verify_asym(scheme, "", Hash::Sha256, 32, &public, &signature, b"data").unwrap());
            assert!(!verify_asym(scheme, "", Hash::Sha256, 32, &public, &signature, b"datum").unwrap());
        }
        let c = rsa_oaep(true, Hash::Sha1, b"label", &public, b"secret").unwrap();
        assert_eq!(rsa_oaep(false, Hash::Sha1, b"label", &private, &c).unwrap(), b"secret");
        // A public key where a private key belongs (and the reverse) is a DataError.
        assert!(sign_asym("PKCS1", "", Hash::Sha256, 0, &public, b"data").is_err());
        assert!(rsa_oaep(true, Hash::Sha1, b"", &private, b"secret").is_err());
    }
}
