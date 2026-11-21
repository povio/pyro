//! Optional system OpenSSL KEM provider. The private representation is the
//! standardized 64-byte seed. DER and raw public keys are converted by EVP.
use super::{buffer, bytes_arg, data_error, operation_error, str_arg};
use pyro_js::{JsResult, Object, Persistent, Scope, Value};

unsafe extern "C" {
    fn pyro_mlkem_generate(level: i32, seed: *mut u8, sl: usize, public: *mut u8, pl: usize) -> i32;
    fn pyro_mlkem_convert(
        level: i32,
        from: i32,
        to: i32,
        input: *const u8,
        len: usize,
        out: *mut u8,
        cap: usize,
        written: *mut usize,
    ) -> i32;
    fn pyro_mlkem_encapsulate(
        level: i32,
        public: *const u8,
        pl: usize,
        ct: *mut u8,
        cl: usize,
        secret: *mut u8,
        sl: usize,
    ) -> i32;
    fn pyro_mlkem_decapsulate(
        level: i32,
        seed: *const u8,
        sl: usize,
        ct: *const u8,
        cl: usize,
        secret: *mut u8,
        shared_len: usize,
    ) -> i32;
    fn pyro_mlkem_cleanse(data: *mut u8, len: usize);
}

struct Sensitive(Vec<u8>);
impl Drop for Sensitive {
    fn drop(&mut self) {
        // SAFETY: the allocation stays uniquely borrowed until this call returns.
        unsafe { pyro_mlkem_cleanse(self.0.as_mut_ptr(), self.0.len()) }
    }
}

fn parameters(v: Value<'_>) -> JsResult<(i32, usize, usize)> {
    match str_arg(v)?.as_str() {
        "ML-KEM-512" => Ok((512, 800, 768)),
        "ML-KEM-768" => Ok((768, 1184, 1088)),
        "ML-KEM-1024" => Ok((1024, 1568, 1568)),
        _ => Err(data_error("Unknown ML-KEM parameter set")),
    }
}
fn format(v: Value<'_>) -> JsResult<i32> {
    match str_arg(v)?.as_str() {
        "raw-public" => Ok(1),
        "raw-seed" => Ok(2),
        "spki" => Ok(3),
        "pkcs8" => Ok(4),
        _ => Err(data_error("Unsupported ML-KEM encoding")),
    }
}
pub(super) fn install(s: &Scope<'_>, n: Object<'_>) -> JsResult<()> {
    n.set_atom(
        "mlKemGenerate",
        s.function("mlKemGenerate", |s, _, a| {
            let (level, pl, _) = parameters(a.get(0))?;
            let mut seed = Sensitive(vec![0; 64]);
            let mut public = vec![0; pl];
            // SAFETY: C receives the actual capacities of the two disjoint allocations.
            if unsafe {
                pyro_mlkem_generate(level, seed.0.as_mut_ptr(), seed.0.len(), public.as_mut_ptr(), public.len())
            } != 1
            {
                return Err(operation_error("OpenSSL could not generate the ML-KEM key"));
            }
            let key = Persistent::new(s, buffer(s, &seed.0)?);
            Ok(s.array(&[key.get(s), buffer(s, &public)?])?.value())
        })?
        .value(),
    )?;
    n.set_atom(
        "mlKemConvert",
        s.function("mlKemConvert", |s, _, a| {
            let (level, _, _) = parameters(a.get(0))?;
            let from = format(a.get(1))?;
            let to = format(a.get(2))?;
            let bytes = Sensitive(bytes_arg(s, a.get(3))?);
            // Largest supported output is an ML-KEM-1024 SPKI, 1590 bytes.
            let mut output = Sensitive(vec![0; 2048]);
            let mut used = 0;
            // SAFETY: lengths match the native allocations, no JS runs inside EVP.
            let converted = unsafe {
                pyro_mlkem_convert(
                    level,
                    from,
                    to,
                    bytes.0.as_ptr(),
                    bytes.0.len(),
                    output.0.as_mut_ptr(),
                    output.0.len(),
                    &mut used,
                )
            };
            if converted == -1 {
                return Err(pyro_js::JsError::dom(
                    "NotSupportedError",
                    "Only seed-only ML-KEM PKCS8 keys are supported.",
                ));
            }
            if converted != 1 || used > output.0.len() {
                return Err(data_error("The ML-KEM key encoding is invalid or unsupported"));
            }
            buffer(s, &output.0[..used])
        })?
        .value(),
    )?;
    n.set_atom(
        "mlKemEncapsulate",
        s.function("mlKemEncapsulate", |s, _, a| {
            let (level, _, cl) = parameters(a.get(0))?;
            let public = bytes_arg(s, a.get(1))?;
            let mut ct = vec![0; cl];
            let mut shared = Sensitive(vec![0; 32]);
            // SAFETY: input and output allocations are disjoint and lengths are checked by the adapter.
            if unsafe {
                pyro_mlkem_encapsulate(
                    level,
                    public.as_ptr(),
                    public.len(),
                    ct.as_mut_ptr(),
                    ct.len(),
                    shared.0.as_mut_ptr(),
                    shared.0.len(),
                )
            } != 1
            {
                return Err(operation_error("OpenSSL ML-KEM encapsulation failed"));
            }
            let bits = Persistent::new(s, buffer(s, &shared.0)?);
            Ok(s.array(&[bits.get(s), buffer(s, &ct)?])?.value())
        })?
        .value(),
    )?;
    n.set_atom(
        "mlKemDecapsulate",
        s.function("mlKemDecapsulate", |s, _, a| {
            let (level, _, _) = parameters(a.get(0))?;
            let seed = Sensitive(bytes_arg(s, a.get(1))?);
            let ct = bytes_arg(s, a.get(2))?;
            let mut shared = Sensitive(vec![0; 32]);
            // SAFETY: same ownership and capacity rules as encapsulation.
            if unsafe {
                pyro_mlkem_decapsulate(
                    level,
                    seed.0.as_ptr(),
                    seed.0.len(),
                    ct.as_ptr(),
                    ct.len(),
                    shared.0.as_mut_ptr(),
                    shared.0.len(),
                )
            } != 1
            {
                return Err(operation_error("OpenSSL ML-KEM decapsulation failed"));
            }
            buffer(s, &shared.0)
        })?
        .value(),
    )?;
    Ok(())
}
