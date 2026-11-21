//! RSA for Web Crypto: RSASSA-PKCS1-v1_5, RSA-PSS and RSA-OAEP (RFC 8017)
//! over crypto-bigint's constant-time big integers, with PKCS #1 / PKCS #8 /
//! SPKI DER from the `pkcs1` and `pkcs8` crates.
//!
//! This replaces the `rsa` crate, whose private-key operation is not constant
//! time (RUSTSEC-2023-0071, the Marvin attack; no fixed release exists). Here
//! the private operation is CRT exponentiation with crypto-bigint's
//! fixed-window Montgomery `pow` (its time depends on the operand sizes only),
//! the input is blinded by a fresh random `r^e` on every call, and the result
//! is checked against the public operation before it is used, so a fault
//! cannot leak a prime. OAEP decoding examines every byte and fails with one
//! error whatever went wrong. Web Crypto has no RSAES-PKCS1-v1_5, the padding
//! the Marvin attack targets.
//!
//! Key material stays in the DER CryptoKeys carry (PKCS #8 for private keys,
//! SPKI for public keys); every operation parses and checks it again.

use super::{data_error, operation_error, Hash};
use core::convert::Infallible;
use crypto_bigint::modular::{BoxedMontyForm, BoxedMontyParams};
use crypto_bigint::rand_core::{TryCryptoRng, TryRng};
use crypto_bigint::zeroize::Zeroize;
use crypto_bigint::{BoxedUint, Choice, ConcatenatingMul, CtEq, Integer, Lcm, Limb, NonZero, Odd, RandomMod, Resize};
use crypto_primes::hazmat::{SetBits, SmallFactorsSieveFactory};
use crypto_primes::{is_prime, sieve_and_find, Flavor};
use pyro_js::JsResult;
use pkcs8::der::asn1::{BitStringRef, Null, UintRef};
use pkcs8::der::{Decode, Encode};

/// The largest modulus generated or imported (Chromium's limit).
pub(super) const MAX_BITS: u32 = 16384;

/// The largest public exponent, in bits (BoringSSL's limit).
const MAX_EXPONENT_BITS: u32 = 33;

/// The OS CSPRNG as a rand_core 0.10 RNG (what crypto-bigint and
/// crypto-primes take). Like rand_core's own `OsRng`, it panics if the OS
/// cannot produce randomness.
struct OsRng;

impl TryRng for OsRng {
    type Error = Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        Ok(getrandom::u32().expect("the OS random number generator failed"))
    }

    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        Ok(getrandom::u64().expect("the OS random number generator failed"))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Infallible> {
        getrandom::fill(dst).expect("the OS random number generator failed");
        Ok(())
    }
}

impl TryCryptoRng for OsRng {}

fn random_bytes(len: usize) -> JsResult<Vec<u8>> {
    let mut bytes = vec![0u8; len];
    getrandom::fill(&mut bytes).map_err(|e| operation_error(e.to_string()))?;
    Ok(bytes)
}

/// Big-endian bytes without leading zeros.
fn trim(bytes: &[u8]) -> &[u8] {
    let zeros = bytes.iter().take_while(|&&b| b == 0).count();
    &bytes[zeros..]
}

/// The bit length of big-endian bytes.
fn bit_length(bytes: &[u8]) -> u32 {
    let bytes = trim(bytes);
    match bytes.first() {
        Some(&top) => {
            let bits = (bytes.len() as u64 - 1) * 8 + u64::from(8 - top.leading_zeros());
            u32::try_from(bits).unwrap_or(u32::MAX)
        }
        None => 0,
    }
}

/// `bytes` as an integer of `precision` bits, or None when it does not fit.
/// A zero precision is None too: a limbless BoxedUint panics in `Odd::new`
/// and `NonZero::new`.
fn uint(bytes: &[u8], precision: u32) -> Option<BoxedUint> {
    let bytes = trim(bytes);
    if precision == 0 || bit_length(bytes) > precision {
        return None;
    }
    BoxedUint::from_be_slice(bytes, precision).ok()
}

/// The minimal big-endian bytes of `x` (JWK members, DER integers).
fn be_bytes(x: &BoxedUint) -> Vec<u8> {
    trim(&x.to_be_bytes()).to_vec()
}

/// I2OSP: `x` as exactly `len` big-endian bytes; `x` is below a modulus of
/// at most `len` bytes, so nothing is cut.
fn i2osp(x: &BoxedUint, len: usize) -> Vec<u8> {
    let bytes = x.to_be_bytes();
    if bytes.len() >= len {
        bytes[bytes.len() - len..].to_vec()
    } else {
        let mut out = vec![0u8; len - bytes.len()];
        out.extend_from_slice(&bytes);
        out
    }
}

/// `x` at `precision` bits; callers know the value fits (only the
/// precisions, which are public, decide the time this takes).
fn resize(x: BoxedUint, precision: u32) -> BoxedUint {
    x.resize_unchecked(precision)
}

fn same(a: &BoxedUint, b: &BoxedUint) -> Choice {
    let precision = a.bits_precision().max(b.bits_precision());
    resize(a.clone(), precision).ct_eq(&resize(b.clone(), precision))
}

fn minus_one(x: &BoxedUint) -> BoxedUint {
    x.wrapping_sub(Limb::ONE)
}

// ----- Keys ------------------------------------------------------------------

pub(super) struct PublicKey {
    n: Odd<BoxedUint>,
    /// At 64 bits of precision (the exponent has at most 33).
    e: BoxedUint,
    params: BoxedMontyParams,
}

impl PublicKey {
    /// A public key from its big-endian modulus and exponent, checked as
    /// BoringSSL checks one: an odd modulus of at most MAX_BITS bits, an odd
    /// exponent from 3 to 2^33 - 1 below the modulus.
    pub(super) fn from_components(n: &[u8], e: &[u8]) -> JsResult<PublicKey> {
        let invalid = || data_error("Invalid RSA public key");
        let bits = bit_length(n);
        let e_bits = bit_length(e);
        if bits == 0 || bits > MAX_BITS || !(2..=MAX_EXPONENT_BITS).contains(&e_bits) {
            return Err(invalid());
        }
        let n = uint(n, bits).ok_or_else(invalid)?;
        let e = uint(e, 64).ok_or_else(invalid)?;
        if !e.is_odd().to_bool() || e_bits >= bits {
            return Err(invalid());
        }
        let n = Odd::new(n).into_option().ok_or_else(invalid)?;
        // The modulus is public: variable-time setup is fine.
        let params = BoxedMontyParams::new_vartime(n.clone());
        Ok(PublicKey { n, e, params })
    }

    /// A SubjectPublicKeyInfo with the rsaEncryption OID and NULL parameters.
    pub(super) fn from_spki(der: &[u8]) -> JsResult<PublicKey> {
        let invalid = || data_error("Invalid RSA public key");
        let spki = pkcs8::SubjectPublicKeyInfoRef::from_der(der).map_err(|_| invalid())?;
        if spki.algorithm.oid != pkcs1::ALGORITHM_OID || spki.algorithm.parameters_any().ok() != Some(Null.into()) {
            return Err(invalid());
        }
        let key = pkcs1::RsaPublicKey::from_der(spki.subject_public_key.as_bytes().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        PublicKey::from_components(key.modulus.as_bytes(), key.public_exponent.as_bytes())
    }

    pub(super) fn to_spki(&self) -> JsResult<Vec<u8>> {
        let fail = || operation_error("cannot encode the key");
        let (n, e) = (self.n_bytes(), self.e_bytes());
        let key = pkcs1::RsaPublicKey {
            modulus: UintRef::new(&n).map_err(|_| fail())?,
            public_exponent: UintRef::new(&e).map_err(|_| fail())?,
        };
        let key = key.to_der().map_err(|_| fail())?;
        pkcs8::SubjectPublicKeyInfoRef {
            algorithm: pkcs1::ALGORITHM_ID,
            subject_public_key: BitStringRef::from_bytes(&key).map_err(|_| fail())?,
        }
        .to_der()
        .map_err(|_| fail())
    }

    /// The modulus length in bits (CryptoKey.algorithm.modulusLength).
    pub(super) fn bits(&self) -> u32 {
        self.n.as_ref().bits_vartime()
    }

    /// The modulus length in bytes (k in RFC 8017).
    fn size(&self) -> usize {
        self.bits().div_ceil(8) as usize
    }

    pub(super) fn n_bytes(&self) -> Vec<u8> {
        be_bytes(self.n.as_ref())
    }

    pub(super) fn e_bytes(&self) -> Vec<u8> {
        be_bytes(&self.e)
    }

    /// OS2IP of `bytes` (exactly k bytes) as a representative below n.
    fn representative(&self, bytes: &[u8]) -> Option<BoxedUint> {
        if bytes.len() != self.size() {
            return None;
        }
        let x = uint(bytes, self.n.bits_precision())?;
        (x < *self.n.as_ref()).then_some(x)
    }

    /// RSAEP / RSAVP1: `x^e mod n` (public, so variable time in e).
    fn public_op(&self, x: &BoxedUint) -> BoxedUint {
        BoxedMontyForm::new(x.clone(), &self.params).pow_bounded_exp(&self.e, self.e.bits_vartime()).retrieve()
    }
}

pub(super) struct PrivateKey {
    public: PublicKey,
    /// At the modulus's precision.
    d: BoxedUint,
    /// The primes and CRT values share one precision.
    p: Odd<BoxedUint>,
    q: Odd<BoxedUint>,
    dp: BoxedUint,
    dq: BoxedUint,
    qinv: BoxedUint,
}

impl Drop for PrivateKey {
    fn drop(&mut self) {
        self.d.zeroize();
        self.p.zeroize();
        self.q.zeroize();
        self.dp.zeroize();
        self.dq.zeroize();
        self.qinv.zeroize();
    }
}

impl PrivateKey {
    /// A two-prime private key from its big-endian components, checked as
    /// BoringSSL's RSA_check_key does: n = p q, d e = 1 mod (p - 1) and
    /// mod (q - 1), and, when given, the CRT values must be the ones those
    /// imply. Primality is not tested (nor is it by BoringSSL); a key that
    /// is not RSA fails the per-operation consistency check instead.
    pub(super) fn from_components(
        n: &[u8],
        e: &[u8],
        d: &[u8],
        p: &[u8],
        q: &[u8],
        crt: Option<[&[u8]; 3]>,
    ) -> JsResult<PrivateKey> {
        let invalid = || data_error("Invalid RSA private key");
        let public = PublicKey::from_components(n, e).map_err(|_| invalid())?;
        let precision = public.n.bits_precision();
        let n_value = public.n.as_ref();
        let d = uint(d, precision).ok_or_else(invalid)?;
        if d.is_zero().to_bool() || d >= *n_value {
            return Err(invalid());
        }
        // Neither prime may be longer than the modulus (this also bounds the
        // work a hostile key can cause), and each must be at least 3.
        let (p_bits, q_bits) = (bit_length(p), bit_length(q));
        let prime_precision = p_bits.max(q_bits);
        if p_bits < 2 || q_bits < 2 || prime_precision > public.bits() {
            return Err(invalid());
        }
        let p = uint(p, prime_precision).ok_or_else(invalid)?;
        let q = uint(q, prime_precision).ok_or_else(invalid)?;
        let p = Odd::new(p).into_option().ok_or_else(invalid)?;
        let q = Odd::new(q).into_option().ok_or_else(invalid)?;
        let one = BoxedUint::one_with_precision(p.bits_precision());
        if *p.as_ref() == one || *q.as_ref() == one || p == q {
            return Err(invalid());
        }
        // The modulus is public, so this comparison may be variable time.
        let product = p.as_ref().concatenating_mul(q.as_ref());
        if !same(&product, n_value).to_bool() {
            return Err(invalid());
        }
        let p1 = NonZero::new(minus_one(p.as_ref())).into_option().ok_or_else(invalid)?;
        let q1 = NonZero::new(minus_one(q.as_ref())).into_option().ok_or_else(invalid)?;
        let dp = d.rem(&p1);
        let dq = d.rem(&q1);
        let inverts = |dx: &BoxedUint, x1: &NonZero<BoxedUint>| {
            let product = public.e.concatenating_mul(dx).rem(x1);
            product.ct_eq(&BoxedUint::one_with_precision(product.bits_precision()))
        };
        let qinv = q.as_ref().rem(p.as_nz_ref()).invert_odd_mod(&p).into_option().ok_or_else(invalid)?;
        let mut valid = inverts(&dp, &p1) & inverts(&dq, &q1);
        if let Some([given_dp, given_dq, given_qi]) = crt {
            let given = |bytes: &[u8], expected: &BoxedUint| match uint(bytes, expected.bits_precision()) {
                Some(value) => value.ct_eq(expected),
                None => Choice::FALSE,
            };
            let matches = given(given_dp, &dp) & given(given_dq, &dq) & given(given_qi, &qinv);
            if valid.to_bool() && !matches.to_bool() {
                return Err(data_error("The RSA key's CRT values do not match the key"));
            }
            valid &= matches;
        }
        if !valid.to_bool() {
            return Err(invalid());
        }
        Ok(PrivateKey { public, d, p, q, dp, dq, qinv })
    }

    /// A PKCS #8 PrivateKeyInfo with the rsaEncryption OID and NULL
    /// parameters around a two-prime PKCS #1 RSAPrivateKey.
    pub(super) fn from_pkcs8(der: &[u8]) -> JsResult<PrivateKey> {
        let invalid = || data_error("Invalid RSA private key");
        let info = pkcs8::PrivateKeyInfo::from_der(der).map_err(|_| invalid())?;
        if info.algorithm.oid != pkcs1::ALGORITHM_OID || info.algorithm.parameters_any().ok() != Some(Null.into()) {
            return Err(invalid());
        }
        let key = pkcs1::RsaPrivateKey::from_der(info.private_key).map_err(|_| invalid())?;
        if key.other_prime_infos.is_some() {
            return Err(data_error("Multi-prime RSA keys are not supported"));
        }
        PrivateKey::from_components(
            key.modulus.as_bytes(),
            key.public_exponent.as_bytes(),
            key.private_exponent.as_bytes(),
            key.prime1.as_bytes(),
            key.prime2.as_bytes(),
            Some([key.exponent1.as_bytes(), key.exponent2.as_bytes(), key.coefficient.as_bytes()]),
        )
    }

    pub(super) fn to_pkcs8(&self) -> JsResult<Vec<u8>> {
        let fail = || operation_error("cannot encode the key");
        let parts = self.components();
        fn int(bytes: &[u8]) -> JsResult<UintRef<'_>> {
            UintRef::new(bytes).map_err(|_| operation_error("cannot encode the key"))
        }
        let key = pkcs1::RsaPrivateKey {
            modulus: int(&parts[0])?,
            public_exponent: int(&parts[1])?,
            private_exponent: int(&parts[2])?,
            prime1: int(&parts[3])?,
            prime2: int(&parts[4])?,
            exponent1: int(&parts[5])?,
            exponent2: int(&parts[6])?,
            coefficient: int(&parts[7])?,
            other_prime_infos: None,
        };
        let mut key = key.to_der().map_err(|_| fail())?;
        let info = pkcs8::PrivateKeyInfo::new(pkcs1::ALGORITHM_ID, &key).to_der().map_err(|_| fail());
        key.zeroize();
        info
    }

    /// `[n, e, d, p, q, dp, dq, qi]`, minimal big-endian (the JWK members).
    pub(super) fn components(&self) -> [Vec<u8>; 8] {
        [
            self.public.n_bytes(),
            self.public.e_bytes(),
            be_bytes(&self.d),
            be_bytes(self.p.as_ref()),
            be_bytes(self.q.as_ref()),
            be_bytes(&self.dp),
            be_bytes(&self.dq),
            be_bytes(&self.qinv),
        ]
    }

    pub(super) fn public(&self) -> &PublicKey {
        &self.public
    }

    /// Validate key generation options without generating any prime numbers.
    pub(super) fn generation_exponent(bits: u32, exponent: &[u8]) -> JsResult<u32> {
        let e = match trim(exponent) {
            [3] => 3,
            [1, 0, 1] => 65537,
            _ => return Err(operation_error("The RSA public exponent must be 3 or 65537")),
        };
        if !(256..=MAX_BITS).contains(&bits) || !bits.is_multiple_of(8) {
            return Err(operation_error("The RSA modulus length must be a multiple of 8 between 256 and 16384"));
        }
        Ok(e)
    }

    /// A new key of `bits` bits with the supported public exponent.
    pub(super) fn generate(bits: u32, exponent: &[u8]) -> JsResult<PrivateKey> {
        let e = Self::generation_exponent(bits, exponent)?;
        loop {
            // Two top bits set in each prime: the product has exactly `bits` bits.
            let p = generate_prime(bits / 2, e)?;
            let q = generate_prime(bits / 2, e)?;
            if p != q {
                return PrivateKey::from_primes(&p, &q, e);
            }
        }
    }

    /// The key with primes `p` and `q` and public exponent `e` (prime, and
    /// coprime to p - 1 and q - 1), with d = e^-1 mod lcm(p - 1, q - 1) as
    /// BoringSSL makes it.
    fn from_primes(p: &BoxedUint, q: &BoxedUint, e: u32) -> JsResult<PrivateKey> {
        let failed = || operation_error("RSA key generation failed");
        let precision = p.bits_precision().max(q.bits_precision());
        let (p, q) = (resize(p.clone(), precision), resize(q.clone(), precision));
        let lambda = minus_one(&p).lcm(&minus_one(&q));
        let d = invert_small(e, &lambda).ok_or_else(failed)?;
        let n = p.concatenating_mul(&q);
        PrivateKey::from_components(
            &be_bytes(&n),
            &be_bytes(&BoxedUint::from(e)),
            &be_bytes(&d),
            &be_bytes(&p),
            &be_bytes(&q),
            None,
        )
        .map_err(|_| failed())
    }

    /// RSADP / RSASP1: `c^d mod n` for `c < n`, by CRT on a blinded input,
    /// checked against the public operation.
    fn private_op(&self, c: &BoxedUint) -> JsResult<BoxedUint> {
        let public = &self.public;
        let n = &public.n;
        // Blinding: work on c r^e for a fresh random unit r, then multiply
        // the result by r^-1.
        let (r, r_inv) = loop {
            let r = BoxedUint::random_mod_vartime(&mut OsRng, n.as_nz_ref());
            if let Some(r_inv) = r.invert_odd_mod(n).into_option() {
                break (r, r_inv);
            }
        };
        let blind = BoxedMontyForm::new(r, &public.params).pow_bounded_exp(&public.e, public.e.bits_vartime());
        let blinded = BoxedMontyForm::new(c.clone(), &public.params).mul(&blind).retrieve();

        // m1 = c^dp mod p, m2 = c^dq mod q, h = qinv (m1 - m2) mod p,
        // m = m2 + h q.
        let p_params = BoxedMontyParams::new(self.p.clone());
        let q_params = BoxedMontyParams::new(self.q.clone());
        let m1 = BoxedMontyForm::new(blinded.rem(self.p.as_nz_ref()), &p_params).pow(&self.dp).retrieve();
        let m2 = BoxedMontyForm::new(blinded.rem(self.q.as_nz_ref()), &q_params).pow(&self.dq).retrieve();
        let difference = m1.sub_mod(&m2.rem(self.p.as_nz_ref()), self.p.as_nz_ref());
        let h = BoxedMontyForm::new(difference, &p_params).mul(&BoxedMontyForm::new(self.qinv.clone(), &p_params));
        let hq = h.retrieve().concatenating_mul(self.q.as_ref());
        let m = hq.wrapping_add(resize(m2, hq.bits_precision()));
        let m = resize(m, n.bits_precision());

        let m = BoxedMontyForm::new(m, &public.params).mul(&BoxedMontyForm::new(r_inv, &public.params)).retrieve();
        if !public.public_op(&m).ct_eq(c).to_bool() {
            return Err(operation_error("The RSA private key is inconsistent"));
        }
        Ok(m)
    }
}

/// A prime of exactly `bits` bits with its two top bits set and `p - 1`
/// coprime to the (prime) public exponent `e`.
fn generate_prime(bits: u32, e: u32) -> JsResult<BoxedUint> {
    let e = NonZero::new(Limb::from(e)).into_option().ok_or_else(|| operation_error("Invalid RSA public exponent"))?;
    loop {
        let factory = SmallFactorsSieveFactory::<BoxedUint>::new(Flavor::Any, bits, SetBits::TwoMsb)
            .map_err(|e| operation_error(e.to_string()))?;
        let prime = sieve_and_find(&mut OsRng, factory, |_, candidate| is_prime(Flavor::Any, candidate))
            .map_err(|e| operation_error(e.to_string()))?
            .ok_or_else(|| operation_error("RSA key generation failed"))?;
        if minus_one(&prime).rem_limb(e) != Limb::ZERO {
            return Ok(prime);
        }
    }
}

/// `e^-1 mod m` for a small prime `e` that does not divide `m`:
/// `(1 + k m) / e` where `k = -m^-1 mod e`.
fn invert_small(e: u32, m: &BoxedUint) -> Option<BoxedUint> {
    let e64 = u64::from(e);
    #[allow(clippy::useless_conversion)] // A limb is 32 bits on 32-bit targets.
    let t = u64::from(m.rem_limb(NonZero::new(Limb::from(e)).into_option()?).0);
    if t == 0 {
        return None;
    }
    // t^(e - 2) mod e = t^-1 mod e (e is prime); the loop's shape depends on
    // the public e only.
    let (mut inverse, mut base, mut power) = (1u64, t, e64 - 2);
    while power > 0 {
        let factor = if power & 1 == 1 { base } else { 1 };
        inverse = inverse * factor % e64;
        base = base * base % e64;
        power >>= 1;
    }
    let k = BoxedUint::from(e64 - inverse);
    let numerator = m.concatenating_mul(&k).wrapping_add(Limb::ONE);
    let (d, rest) = numerator.div_rem_limb(NonZero::new(Limb::from(e)).into_option()?);
    (rest == Limb::ZERO).then_some(d)
}

// ----- Encodings (RFC 8017) --------------------------------------------------

impl Hash {
    fn size(self) -> usize {
        match self {
            Hash::Sha1 => 20,
            Hash::Sha256 => 32,
            Hash::Sha384 => 48,
            Hash::Sha512 => 64,
        }
    }

    /// The DER DigestInfo prefix EMSA-PKCS1-v1_5 puts before the digest.
    fn digest_info(self) -> &'static [u8] {
        match self {
            Hash::Sha1 => &[0x30, 0x21, 0x30, 0x09, 0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a, 0x05, 0x00, 0x04, 0x14],
            Hash::Sha256 => &[
                0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00,
                0x04, 0x20,
            ],
            Hash::Sha384 => &[
                0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02, 0x05, 0x00,
                0x04, 0x30,
            ],
            Hash::Sha512 => &[
                0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05, 0x00,
                0x04, 0x40,
            ],
        }
    }

    fn digest_parts(self, parts: &[&[u8]]) -> Vec<u8> {
        self.digest(&parts.concat())
    }
}

/// MGF1 (RFC 8017 B.2.1): `len` bytes from `seed`.
fn mgf1(hash: Hash, seed: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + hash.size());
    let mut counter = 0u32;
    while out.len() < len {
        out.extend_from_slice(&hash.digest_parts(&[seed, &counter.to_be_bytes()]));
        counter += 1;
    }
    out.truncate(len);
    out
}

fn xor(into: &mut [u8], mask: &[u8]) {
    for (byte, m) in into.iter_mut().zip(mask) {
        *byte ^= m;
    }
}

/// EMSA-PKCS1-v1_5 (RFC 8017 9.2) for a `len`-byte encoded message.
fn emsa_pkcs1(hash: Hash, data: &[u8], len: usize) -> Option<Vec<u8>> {
    let prefix = hash.digest_info();
    let t_len = prefix.len() + hash.size();
    if len < t_len + 11 {
        return None;
    }
    let mut em = vec![0xffu8; len];
    em[0] = 0;
    em[1] = 1;
    em[len - t_len - 1] = 0;
    em[len - t_len..len - hash.size()].copy_from_slice(prefix);
    em[len - hash.size()..].copy_from_slice(&hash.digest(data));
    Some(em)
}

/// EMSA-PSS-ENCODE (RFC 8017 9.1.1) for a modulus of `mod_bits` bits.
fn emsa_pss_encode(hash: Hash, data: &[u8], salt_len: usize, mod_bits: u32) -> JsResult<Vec<u8>> {
    let em_bits = mod_bits as usize - 1;
    let em_len = em_bits.div_ceil(8);
    let h_len = hash.size();
    if em_len < h_len.saturating_add(salt_len).saturating_add(2) {
        return Err(operation_error("The RSA-PSS salt is too long for the key"));
    }
    let salt = random_bytes(salt_len)?;
    let m_hash = hash.digest(data);
    let h = hash.digest_parts(&[&[0u8; 8][..], &m_hash, &salt]);
    let db_len = em_len - h_len - 1;
    let mut db = vec![0u8; db_len];
    db[db_len - salt_len - 1] = 1;
    db[db_len - salt_len..].copy_from_slice(&salt);
    xor(&mut db, &mgf1(hash, &h, db_len));
    db[0] &= 0xff >> (8 * em_len - em_bits);
    db.extend_from_slice(&h);
    db.push(0xbc);
    Ok(db)
}

/// EMSA-PSS-VERIFY (RFC 8017 9.1.2) on the `em_len`-byte tail of `em`.
fn emsa_pss_verify(hash: Hash, data: &[u8], salt_len: usize, em: &[u8], mod_bits: u32) -> bool {
    let em_bits = mod_bits as usize - 1;
    let em_len = em_bits.div_ceil(8);
    let h_len = hash.size();
    // An em_len shorter than k (mod_bits = 1 mod 8) leaves a zero byte first.
    let (head, em) = em.split_at(em.len() - em_len);
    if head.iter().any(|&b| b != 0) || em_len < h_len.saturating_add(salt_len).saturating_add(2) {
        return false;
    }
    if em[em_len - 1] != 0xbc {
        return false;
    }
    let (masked_db, h) = em[..em_len - 1].split_at(em_len - h_len - 1);
    let top_mask = !(0xffu8 >> (8 * em_len - em_bits));
    if masked_db[0] & top_mask != 0 {
        return false;
    }
    let mut db = masked_db.to_vec();
    xor(&mut db, &mgf1(hash, h, masked_db.len()));
    db[0] &= !top_mask;
    let ps_len = em_len - h_len - salt_len - 2;
    if db[..ps_len].iter().any(|&b| b != 0) || db[ps_len] != 1 {
        return false;
    }
    let salt = &db[db.len() - salt_len..];
    let expected = hash.digest_parts(&[&[0u8; 8][..], &hash.digest(data), salt]);
    expected.as_slice().ct_eq(h).to_bool()
}

// ----- Operations ------------------------------------------------------------

/// RSASSA-PKCS1-v1_5 ("PKCS1") or RSA-PSS ("PSS", with `salt_len`) signing.
pub(super) fn sign(scheme: &str, hash: Hash, salt_len: usize, key: &PrivateKey, data: &[u8]) -> JsResult<Vec<u8>> {
    let public = key.public();
    let em = if scheme == "PSS" {
        emsa_pss_encode(hash, data, salt_len, public.bits())?
    } else {
        emsa_pkcs1(hash, data, public.size()).ok_or_else(|| operation_error("The RSA key is too short for the hash"))?
    };
    let m = uint(&em, public.n.bits_precision()).ok_or_else(|| operation_error("RSA signing failed"))?;
    if m >= *public.n.as_ref() {
        return Err(operation_error("RSA signing failed"));
    }
    Ok(i2osp(&key.private_op(&m)?, public.size()))
}

/// RSASSA-PKCS1-v1_5 or RSA-PSS verification: false for any mismatch,
/// including a signature that is not exactly k bytes or not below n.
pub(super) fn verify(
    scheme: &str,
    hash: Hash,
    salt_len: usize,
    key: &PublicKey,
    signature: &[u8],
    data: &[u8],
) -> bool {
    let Some(s) = key.representative(signature) else { return false };
    let em = i2osp(&key.public_op(&s), key.size());
    if scheme == "PSS" {
        emsa_pss_verify(hash, data, salt_len, &em, key.bits())
    } else {
        emsa_pkcs1(hash, data, key.size()).is_some_and(|expected| expected.as_slice().ct_eq(em.as_slice()).to_bool())
    }
}

/// RSAES-OAEP-ENCRYPT (RFC 8017 7.1.1); any label bytes.
pub(super) fn oaep_encrypt(hash: Hash, label: &[u8], key: &PublicKey, message: &[u8]) -> JsResult<Vec<u8>> {
    let k = key.size();
    let h_len = hash.size();
    if k < 2 * h_len + 2 || message.len() > k - 2 * h_len - 2 {
        return Err(operation_error("The message is too long for RSA-OAEP with this key and hash"));
    }
    let db_len = k - h_len - 1;
    let mut db = vec![0u8; db_len];
    db[..h_len].copy_from_slice(&hash.digest(label));
    db[db_len - message.len() - 1] = 1;
    db[db_len - message.len()..].copy_from_slice(message);
    let mut seed = random_bytes(h_len)?;
    xor(&mut db, &mgf1(hash, &seed, db_len));
    xor(&mut seed, &mgf1(hash, &db, h_len));
    let mut em = Vec::with_capacity(k);
    em.push(0);
    em.extend_from_slice(&seed);
    em.extend_from_slice(&db);
    let m = uint(&em, key.n.bits_precision()).ok_or_else(|| operation_error("RSA-OAEP encryption failed"))?;
    Ok(i2osp(&key.public_op(&m), k))
}

/// RSAES-OAEP-DECRYPT (RFC 8017 7.1.2). The decoding reads every byte and
/// any failure is the same OperationError.
pub(super) fn oaep_decrypt(hash: Hash, label: &[u8], key: &PrivateKey, ciphertext: &[u8]) -> JsResult<Vec<u8>> {
    let failed = || operation_error("RSA-OAEP decryption failed");
    let public = key.public();
    let k = public.size();
    let h_len = hash.size();
    if k < 2 * h_len + 2 {
        return Err(failed());
    }
    let c = public.representative(ciphertext).ok_or_else(failed)?;
    let mut em = i2osp(&key.private_op(&c)?, k);
    let (y, rest) = em.split_at_mut(1);
    let (masked_seed, masked_db) = rest.split_at_mut(h_len);
    xor(masked_seed, &mgf1(hash, masked_db, h_len));
    xor(masked_db, &mgf1(hash, masked_seed, masked_db.len()));
    let db = &*masked_db;

    // Constant time: good = y == 0 and lHash matches and a 1 follows the
    // zero padding; `index` is where the message starts.
    let mut good = Choice::from_u8_eq(y[0], 0) & hash.digest(label).as_slice().ct_eq(&db[..h_len]);
    let mut looking = Choice::TRUE;
    let mut index = 0u32;
    for (i, &byte) in db.iter().enumerate().skip(h_len) {
        let is_one = Choice::from_u8_eq(byte, 1);
        let is_zero = Choice::from_u8_eq(byte, 0);
        index = (looking & is_one).select_u32(index, i as u32 + 1);
        // A byte that is neither 0 nor 1 before the separator is invalid.
        good &= !looking | is_one | is_zero;
        looking &= !is_one;
    }
    good &= !looking;
    let result = good.to_bool().then(|| db[index as usize..].to_vec());
    em.zeroize();
    result.ok_or_else(failed)
}

#[cfg(test)]
mod kat;

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
    }

    const HASHES: [Hash; 4] = [Hash::Sha1, Hash::Sha256, Hash::Sha384, Hash::Sha512];

    fn kat_keys() -> (PrivateKey, PublicKey) {
        (PrivateKey::from_pkcs8(&hex(kat::PKCS8)).unwrap(), PublicKey::from_spki(&hex(kat::SPKI)).unwrap())
    }

    #[test]
    fn kat_keys_round_trip_their_der() {
        let (private, public) = kat_keys();
        assert_eq!(private.to_pkcs8().unwrap(), hex(kat::PKCS8));
        assert_eq!(public.to_spki().unwrap(), hex(kat::SPKI));
        assert_eq!(private.public().to_spki().unwrap(), hex(kat::SPKI));
        assert_eq!(public.bits(), 2048);
        assert_eq!(public.e_bytes(), [1, 0, 1]);
    }

    #[test]
    fn pkcs1_v1_5_signatures_match_known_answers() {
        let (private, public) = kat_keys();
        let message = hex(kat::SIGNED);
        for (hash, expected) in HASHES.into_iter().zip(kat::PKCS1_SIGNATURES) {
            let expected = hex(expected);
            assert_eq!(sign("PKCS1", hash, 0, &private, &message).unwrap(), expected);
            assert!(verify("PKCS1", hash, 0, &public, &expected, &message));
            assert!(!verify("PKCS1", hash, 0, &public, &expected, &message[1..]));
            let mut bad = expected.clone();
            bad[100] ^= 1;
            assert!(!verify("PKCS1", hash, 0, &public, &bad, &message));
            // Not k bytes, with or without a leading zero: rejected.
            assert!(!verify("PKCS1", hash, 0, &public, &expected[1..], &message));
            assert!(!verify("PKCS1", hash, 0, &public, &[&[0u8][..], &expected].concat(), &message));
        }
    }

    #[test]
    fn pss_signatures_match_known_answers() {
        let (private, public) = kat_keys();
        let message = hex(kat::SIGNED);
        for (i, hash) in HASHES.into_iter().enumerate() {
            // saltLength 0 is deterministic.
            let unsalted = hex(kat::PSS_UNSALTED_SIGNATURES[i]);
            assert_eq!(sign("PSS", hash, 0, &private, &message).unwrap(), unsalted);
            assert!(verify("PSS", hash, 0, &public, &unsalted, &message));
            assert!(!verify("PSS", hash, 1, &public, &unsalted, &message));
            let salted = hex(kat::PSS_SALTED_SIGNATURES[i]);
            assert!(verify("PSS", hash, hash.size(), &public, &salted, &message));
            assert!(!verify("PSS", hash, hash.size() - 1, &public, &salted, &message));
            assert!(!verify("PSS", hash, hash.size(), &public, &salted, &message[1..]));
            // Fresh salted signatures verify; they differ from each other.
            let a = sign("PSS", hash, hash.size(), &private, &message).unwrap();
            let b = sign("PSS", hash, hash.size(), &private, &message).unwrap();
            assert_ne!(a, b);
            assert!(verify("PSS", hash, hash.size(), &public, &a, &message));
        }
        // The largest salt that fits a 2048-bit key with SHA-256 is
        // 256 - 32 - 2 bytes; one more is an OperationError (and fails to verify).
        let largest = sign("PSS", Hash::Sha256, 222, &private, &message).unwrap();
        assert!(verify("PSS", Hash::Sha256, 222, &public, &largest, &message));
        assert!(sign("PSS", Hash::Sha256, 223, &private, &message).is_err());
        assert!(!verify("PSS", Hash::Sha256, usize::MAX, &public, &largest, &message));
    }

    #[test]
    fn oaep_decrypts_known_answers() {
        let (private, public) = kat_keys();
        let label = hex(kat::OAEP_LABEL);
        for (i, hash) in HASHES.into_iter().enumerate() {
            let plain = hex(kat::OAEP_PLAINTEXTS[i]);
            let unlabeled = hex(kat::OAEP_UNLABELED_CIPHERTEXTS[i]);
            let labeled = hex(kat::OAEP_LABELED_CIPHERTEXTS[i]);
            assert_eq!(oaep_decrypt(hash, &[], &private, &unlabeled).unwrap(), plain);
            assert_eq!(oaep_decrypt(hash, &label, &private, &labeled).unwrap(), plain);
            assert!(oaep_decrypt(hash, &label, &private, &unlabeled).is_err());
            assert!(oaep_decrypt(hash, &[], &private, &labeled).is_err());
            let mut bad = unlabeled.clone();
            bad[0] ^= 1;
            assert!(oaep_decrypt(hash, &[], &private, &bad).is_err());
            assert!(oaep_decrypt(hash, &[], &private, &unlabeled[1..]).is_err());
            // The KAT plaintext is the longest that fits; one more byte does not.
            let c = oaep_encrypt(hash, &label, &public, &plain).unwrap();
            assert_eq!(c.len(), 256);
            assert_eq!(oaep_decrypt(hash, &label, &private, &c).unwrap(), plain);
            assert!(oaep_encrypt(hash, &label, &public, &[plain.as_slice(), &[0]].concat()).is_err());
            // Empty messages and non-UTF-8 labels work.
            let c = oaep_encrypt(hash, &[0xff, 0xfe], &public, &[]).unwrap();
            assert_eq!(oaep_decrypt(hash, &[0xff, 0xfe], &private, &c).unwrap(), b"");
        }
        // A ciphertext not below n is rejected before decryption.
        assert!(oaep_decrypt(Hash::Sha1, &[], &private, &[0xff; 256]).is_err());
    }

    #[test]
    fn private_keys_are_checked_on_import() {
        let (private, _) = kat_keys();
        let parts = private.components();
        let part = |i: usize| parts[i].as_slice();
        let import = |p: &[u8], q: &[u8], crt: Option<[&[u8]; 3]>| {
            PrivateKey::from_components(part(0), part(1), part(2), p, q, crt)
        };
        assert!(import(part(3), part(4), Some([part(5), part(6), part(7)])).is_ok());
        assert!(import(part(3), part(4), None).is_ok());
        // Wrong CRT values, swapped primes with the old CRT values, a wrong d.
        assert!(import(part(3), part(4), Some([part(5), part(6), part(6)])).is_err());
        assert!(import(part(4), part(3), Some([part(6), part(5), part(7)])).is_err());
        let swapped = import(part(4), part(3), None).unwrap();
        let message = b"swapped primes";
        let signature = sign("PKCS1", Hash::Sha256, 0, &swapped, message).unwrap();
        assert_eq!(signature, sign("PKCS1", Hash::Sha256, 0, &private, message).unwrap());
        let mut d = parts[2].clone();
        *d.last_mut().unwrap() ^= 2;
        assert!(PrivateKey::from_components(part(0), part(1), &d, part(3), part(4), None).is_err());
        assert!(import(part(3), part(3), None).is_err());
        assert!(import(&[1], part(0), None).is_err());
        // Empty or zero primes are a DataError, not a panic on a limbless integer.
        assert!(import(&[], &[], Some([part(5), part(6), part(7)])).is_err());
        assert!(import(&[0], &[0], None).is_err());
        assert!(import(&[], part(4), None).is_err());
        assert!(uint(&[], 0).is_none());
        // Truncated DER and a multi-prime-version key fail as DataError.
        let der = hex(kat::PKCS8);
        assert!(PrivateKey::from_pkcs8(&der[..der.len() - 1]).is_err());
    }

    #[test]
    fn public_keys_are_checked_on_import() {
        let n = vec![0xffu8; 1024];
        assert_eq!(PublicKey::from_components(&n, &[1, 0, 1]).unwrap().bits(), 8192);
        let large = PublicKey::from_components(&vec![0xffu8; 2048], &[1, 0, 1]).unwrap();
        assert_eq!(large.bits(), MAX_BITS);
        let spki = large.to_spki().unwrap();
        assert_eq!(PublicKey::from_spki(&spki).unwrap().bits(), MAX_BITS);
        assert!(PublicKey::from_spki(&spki[..spki.len() - 1]).is_err());
        assert!(PublicKey::from_components(&[0xff; 2049], &[1, 0, 1]).is_err());
        // Even moduli and even, tiny or huge exponents are refused.
        assert!(PublicKey::from_components(&[0xfe; 256], &[1, 0, 1]).is_err());
        assert!(PublicKey::from_components(&[0xff; 256], &[1, 0, 0]).is_err());
        assert!(PublicKey::from_components(&[0xff; 256], &[1]).is_err());
        assert!(PublicKey::from_components(&[0xff; 256], &[3, 0, 0, 0, 1]).is_err());
        assert!(PublicKey::from_components(&[0xff; 256], &[1, 0, 0, 0, 1]).is_ok());
        assert!(PublicKey::from_components(&[], &[3]).is_err());
        assert!(PublicKey::from_components(&[3], &[3]).is_err());
        // Leading zeros are not part of the modulus.
        assert_eq!(PublicKey::from_components(&[0, 0, 0xff, 0xff], &[3]).unwrap().n_bytes(), [0xff, 0xff]);
    }

    #[test]
    fn generated_keys_have_the_requested_size_and_work() {
        for (bits, e) in [(512u32, &[3u8][..]), (1024, &[1, 0, 1][..]), (264, &[1, 0, 1][..])] {
            let key = PrivateKey::generate(bits, e).unwrap();
            assert_eq!(key.public().bits(), bits);
            assert_eq!(key.public().e_bytes(), trim(e));
            let again = PrivateKey::from_pkcs8(&key.to_pkcs8().unwrap()).unwrap();
            assert_eq!(again.components(), key.components());
            let public = PublicKey::from_spki(&key.public().to_spki().unwrap()).unwrap();
            let signature = sign("PSS", Hash::Sha1, 0, &key, b"data").unwrap();
            assert!(verify("PSS", Hash::Sha1, 0, &public, &signature, b"data"));
            if bits >= 1024 {
                let c = oaep_encrypt(Hash::Sha256, b"", &public, b"secret").unwrap();
                assert_eq!(oaep_decrypt(Hash::Sha256, b"", &key, &c).unwrap(), b"secret");
            }
        }
        assert!(PrivateKey::generate(1024, &[1, 0, 0]).is_err());
        assert!(PrivateKey::generate(1028, &[1, 0, 1]).is_err());
        assert!(PrivateKey::generate(248, &[1, 0, 1]).is_err());
        assert!(PrivateKey::generate(16392, &[1, 0, 1]).is_err());
    }

    #[test]
    fn pss_handles_a_modulus_of_8k_plus_1_bits() {
        // modBits - 1 = 256: the encoded message is one byte shorter than k.
        let p = generate_prime(129, 3).unwrap();
        let q = generate_prime(128, 3).unwrap();
        let key = PrivateKey::from_primes(&p, &q, 3).unwrap();
        assert_eq!(key.public().bits(), 257);
        let public = PublicKey::from_spki(&key.public().to_spki().unwrap()).unwrap();
        for salt in [0, 10] {
            let signature = sign("PSS", Hash::Sha1, salt, &key, b"data").unwrap();
            assert_eq!(signature.len(), 33);
            assert!(verify("PSS", Hash::Sha1, salt, &public, &signature, b"data"));
            assert!(!verify("PSS", Hash::Sha1, salt, &public, &signature, b"date"));
        }
        // em_len 32 holds a 20-byte digest, the 0x01 and 0xbc, and 10 salt bytes.
        assert!(sign("PSS", Hash::Sha1, 11, &key, b"data").is_err());
        assert!(PrivateKey::from_pkcs8(&key.to_pkcs8().unwrap()).is_ok());
    }
}
