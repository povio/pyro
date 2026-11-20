//! Bounded float32 CPU kernels for the opt-in WebNN graph experiment.
use pyro_js::{JsError, JsResult, Object, Scope, Value};

const MAX_ELEMENTS: usize = 262_144;
const MAX_MACS: usize = 16_777_216;

fn dimension(v: Value<'_>) -> JsResult<usize> {
    let n = v.to_number()?;
    if n.fract() != 0.0 || !(1.0..=MAX_ELEMENTS as f64).contains(&n) {
        return Err(JsError::Type("Invalid CPU tensor dimension".into()));
    }
    Ok(n as usize)
}

fn floats(v: Value<'_>, len: usize) -> JsResult<Vec<f32>> {
    let a = v.as_object().ok_or_else(|| JsError::Type("Expected tensor storage".into()))?;
    if len > MAX_ELEMENTS || a.get_atom("length")?.to_number()? != len as f64 {
        return Err(JsError::Type("CPU tensor size mismatch".into()));
    }
    (0..len).map(|i| a.get_index(i as u32)?.to_number().map(|n| n as f32)).collect()
}

pub(crate) fn install(s: &Scope<'_>) -> JsResult<()> {
    let multiply = s.function("multiply", |s, _, a| {
        let (m, n, k) = (dimension(a.get(2))?, dimension(a.get(3))?, dimension(a.get(4))?);
        let size = m.checked_mul(n).ok_or_else(|| JsError::Type("Invalid matrix size".into()))?;
        if size > MAX_ELEMENTS || size.checked_mul(k).is_none_or(|count| count > MAX_MACS) {
            return Err(JsError::dom("OperationError", "CPU graph exceeds its computation budget."));
        }
        let count = |x: usize, y: usize| x.checked_mul(y).ok_or_else(|| JsError::Type("Invalid matrix size".into()));
        let left = floats(a.get(0), count(m, k)?)?;
        let right = floats(a.get(1), count(k, n)?)?;
        let (at, bt) = (a.get(5).to_bool(), a.get(6).to_bool());
        let out: Object<'_> = s.array(&[])?;
        for i in 0..m {
            for j in 0..n {
                let mut sum = 0.0f32;
                for h in 0..k {
                    let x = left[if at { h * m + i } else { i * k + h }];
                    let y = right[if bt { j * k + h } else { h * n + j }];
                    sum = x.mul_add(y, sum);
                }
                out.set_index((i * n + j) as u32, s.number(f64::from(sum)))?;
            }
        }
        Ok(out.value())
    })?;
    let install = s
        .eval(include_str!("webnn_cpu.js"), Some("pyro://experimental/webnn-cpu.js"), 1)?
        .as_object()
        .ok_or_else(|| JsError::Type("Missing CPU graph installer".into()))?;
    install.call(None, &[multiply.value(), s.number(MAX_ELEMENTS as f64), s.number(MAX_MACS as f64)])?;
    Ok(())
}
