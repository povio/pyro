# Web Crypto

`crypto` (one `Crypto` object per realm, in windows and dedicated workers)
provides:

- `getRandomValues(view)`: integer typed arrays only (Int8/Uint8/
  Uint8Clamped/Int16/Uint16/Int32/Uint32/BigInt64/BigUint64); other views
  throw `TypeMismatchError`, more than 65536 bytes throw
  `QuotaExceededError`. Fills the view in place (respecting its byteOffset)
  from the OS CSPRNG (`getrandom`) and returns the same object.
- `randomUUID()`: RFC 4122 version 4 UUID string.
- `crypto.subtle` (`SubtleCrypto`) and `CryptoKey`, below.

`crypto` and `crypto.subtle` are exposed regardless of secure context.

## SubtleCrypto

`digest` runs natively (crypto.rs). The other operations run in
`crates/lp-webapi/src/subtle.js` (WebIDL conversions, algorithm
normalization and the per-algorithm steps of the Web Crypto API) over the
byte primitives of `crypto_subtle.rs` (RustCrypto: `aes`, `ghash`, `aes-kw`,
`hmac`, `pbkdf2`, `hkdf`, `p256`/`p384`/`p521`, `ed25519-dalek`,
`x25519-dalek`) and, for RSA, `crypto_subtle/rsa.rs` (below). The script is made on a realm's first SubtleCrypto
call other than `digest`, or its first read of `CryptoKey`; a realm that never
uses keys pays one global accessor.

| Algorithm | Operations | Formats |
|---|---|---|
| SHA-1/256/384/512 | digest | |
| HMAC | sign, verify, generateKey, importKey, exportKey, deriveKey target | raw, jwk |
| AES-GCM | encrypt, decrypt (any IV length; tag 32-128 bits), generateKey, import/export, wrap/unwrap | raw, jwk |
| AES-CBC, AES-CTR | encrypt, decrypt, generateKey, import/export, wrap/unwrap | raw, jwk |
| AES-KW | wrapKey, unwrapKey, generateKey, import/export | raw, jwk |
| PBKDF2, HKDF | deriveBits, deriveKey, importKey (non-extractable) | raw |
| ECDSA (P-256/384/521) | sign, verify (IEEE P1363 signatures), generateKey, import/export | raw (public, compressed or not), spki, pkcs8, jwk |
| ECDH (P-256/384/521) | deriveBits, deriveKey, generateKey, import/export | raw, spki, pkcs8, jwk |
| Ed25519 | sign, verify (strict: small-order keys and R fail, as in Chrome), generateKey, import/export | raw, spki, pkcs8, jwk |
| X25519 | deriveBits (an all-zero secret is an OperationError), deriveKey, generateKey, import/export | raw, spki, pkcs8, jwk |
| RSASSA-PKCS1-v1_5, RSA-PSS | sign, verify, generateKey, import/export | spki, pkcs8, jwk |
| RSA-OAEP | encrypt, decrypt, wrap/unwrap, generateKey, import/export | spki, pkcs8, jwk |

Errors carry the spec names and follow the spec's step order: WebIDL
argument errors (missing arguments, a non-CryptoKey, an invalid enum, an
[EnforceRange] value out of range, a missing required dictionary member) reject
with `TypeError`; an unknown algorithm name (matched ASCII case-insensitively,
so U+212A KELVIN SIGN is not `K`) or curve with `NotSupportedError`; bad usages
or empty usages for a secret/private key with `SyntaxError`; key data that does
not parse or match the algorithm with `DataError`; a wrong key, usage or key
type with `InvalidAccessError`; a failed operation (bad tag, bad padding, an
invalid length) with `OperationError`. Key data is copied after the algorithm
is normalized. Operations run synchronously and settle the returned promise.

RSA generation accepts the public exponents 3 and 65537 and modulus lengths
that are multiples of 8 from 256 to 16384, as Chromium does, and public keys
of up to 16384 bits import (generation of large keys runs synchronously on the
page's thread). Hash members are normalized after their dictionary is
converted, so a missing required member is reported before an unknown hash.
JWK export orders
members alphabetically (a dictionary converted to JS); EC and X25519 JWKs carry
no `alg`, Ed25519 JWKs carry `alg: "Ed25519"`.

## RSA implementation

RSA is implemented in `crates/lp-webapi/src/crypto_subtle/rsa.rs` on
`crypto-bigint` 0.7 (constant-time big integers) and `crypto-primes` 0.7
(key generation), with the `pkcs1`/`pkcs8` crates for DER. We do not use the
`rsa` crate: every release, including the 0.10 release candidates, carries
RUSTSEC-2023-0071 (Marvin attack, a timing side channel in the private-key
operation), and the advisory lists no fixed version. The alternatives were
worse fits: `ring` cannot generate RSA keys or decrypt RSA-OAEP, and
`aws-lc-rs` adds a C/assembly build and supports neither arbitrary RSA-PSS
salt lengths nor modulus lengths other than 2048/3072/4096/8192 for
generation.

- Private-key operations (sign, decrypt) use CRT with crypto-bigint's
  fixed-window Montgomery exponentiation, whose time depends only on the
  operand sizes. The input is blinded with a fresh `r^e` each time, and the
  result is checked against the public operation before it is used (a
  faulty or inconsistent key fails with OperationError instead of leaking a
  prime).
- RSA-OAEP decoding examines every byte and fails with one OperationError
  whatever was wrong. Web Crypto has no RSAES-PKCS1-v1_5 decryption, the
  padding the Marvin attack targets.
- Imported private keys are checked as BoringSSL's `RSA_check_key` does:
  n = pq, de = 1 mod (p - 1) and mod (q - 1), and the given CRT values
  (`dp`, `dq`, `qi`, in PKCS #8 and JWK alike) must be the ones the key
  implies. Primality is not tested. Public keys need an odd modulus of at
  most 16384 bits and an odd exponent from 3 to 2^33 - 1 (BoringSSL's
  bounds). Multi-prime PKCS #8 keys reject with DataError, as in Chromium.
- Generated keys have exactly the requested modulus length (two top bits
  set in each prime) and d = e^-1 mod lcm(p - 1, q - 1). A 2048-bit
  generation takes about 60 ms and a 2048-bit signature about 1.5 ms
  (optimized build on an Apple Silicon laptop).
- Known-answer tests: `crypto_subtle/rsa/kat.rs` holds the WPT RSA vectors
  (PKCS #1 v1.5 and unsalted PSS signatures reproduced byte for byte, salted
  PSS signatures verified, OAEP ciphertexts with and without a label
  decrypted), used by the unit tests in `rsa.rs` and, through
  `crypto.subtle`, by `tests/crypto_js.rs`.

`cargo audit` still reports RUSTSEC-2026-0387 for `crypto-bigint` 0.5.5,
which reaches the tree through `elliptic-curve` 0.13 (`p256`/`p384`/`p521`,
ECDSA/ECDH); RSA uses crypto-bigint 0.7.10, which has the fix. Moving the EC
crates to their 0.14 releases removes that one.

## CryptoKey

`type`, `extractable`, `algorithm` and `usages` (the last two are the same
object on every read). Usages are kept de-duplicated in a fixed order. Key
material stays in a private field: the raw bytes of secret keys, PKCS #8 or
SPKI DER for EC and RSA keys, the 32 raw bytes for Ed25519 and X25519.

CryptoKeys are structured-cloneable within a realm (`structuredClone`,
`MessageChannel`): the clone script asks `crypto::clone_platform_object` for
an object it does not know, which copies a CryptoKey's slots. A realm that has
not made subtle.js has no keys, so this costs it nothing.

They also cross VMs: the worker wire format asks `crypto::serialize_key` for
a host object it does not know, which gives the key's slots as plain data
(type, extractable, algorithm, usages, key bytes), and `deserialize_key`
makes the CryptoKey in the receiving realm (making its subtle.js if needed;
the parts are shape-checked). So keys reach and leave dedicated workers by
`postMessage` and MessagePorts, cross BroadcastChannel, and are stored by
IndexedDB.

## Limits

- RSA JWKs need all CRT members (`p`, `q`, `dp`, `dq`, `qi`) for private
  keys; multi-prime keys (`oth` in JWK, other-prime infos in PKCS #8) are
  not supported. JWK members are decoded
  as unpadded base64url.
- Not implemented: the `raw-secret`/`raw-public`/`raw-private` formats, Ed448,
  X448, and the modern algorithms (ML-KEM, ML-DSA, AES-OCB, ChaCha20-Poly1305,
  Argon2, KMAC, `getPublicKey`, `supports`); they reject as unknown algorithms
  or invalid formats.
