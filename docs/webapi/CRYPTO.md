# Web Crypto subset

`window.crypto` (one `Crypto` object per realm) provides:

- `getRandomValues(view)`: integer typed arrays only (Int8/Uint8/
  Uint8Clamped/Int16/Uint16/Int32/Uint32/BigInt64/BigUint64); other views
  throw `TypeMismatchError`, more than 65536 bytes throw
  `QuotaExceededError`. Fills the view in place (respecting its byteOffset)
  from the OS CSPRNG (`getrandom`) and returns the same object.
- `randomUUID()`: RFC 4122 version 4 UUID string.
- `crypto.subtle.digest(algorithm, data)`: SHA-1, SHA-256, SHA-384 and
  SHA-512 (algorithm as a case-insensitive string or `{name}`), BufferSource
  data, resolving to an `ArrayBuffer`. Argument errors reject the promise.

All other `SubtleCrypto` operations (`encrypt`, `decrypt`, `sign`, `verify`,
`generateKey`, `deriveKey`, `deriveBits`, `importKey`, `exportKey`,
`wrapKey`, `unwrapKey`) reject with `NotSupportedError`; there is no
`CryptoKey`. Upstream Lightpanda implements more of SubtleCrypto
(src/browser/webapi/SubtleCrypto.zig); that is not ported. `crypto` and
`crypto.subtle` are exposed regardless of secure context.
