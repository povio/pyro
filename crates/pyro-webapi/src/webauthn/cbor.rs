//! A CBOR encoder (RFC 8949) for what WebAuthn puts on the wire: integers, byte and text strings,
//! arrays, maps and booleans. Maps are written in CTAP2 canonical order (keys sorted by their
//! encoding, shorter first), which is how authenticators emit the attestation object and the COSE key.

pub(crate) enum Cbor {
    Int(i64),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Cbor>),
    Map(Vec<(Cbor, Cbor)>),
    Bool(bool),
}

impl Cbor {
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Cbor::Int(n) if *n >= 0 => head(out, 0, *n as u64),
            Cbor::Int(n) => head(out, 1, (-1 - *n) as u64),
            Cbor::Bytes(bytes) => {
                head(out, 2, bytes.len() as u64);
                out.extend_from_slice(bytes);
            }
            Cbor::Text(text) => {
                head(out, 3, text.len() as u64);
                out.extend_from_slice(text.as_bytes());
            }
            Cbor::Array(items) => {
                head(out, 4, items.len() as u64);
                for item in items {
                    item.write(out);
                }
            }
            Cbor::Map(entries) => {
                let mut encoded: Vec<(Vec<u8>, Vec<u8>)> = entries.iter().map(|(k, v)| (k.encode(), v.encode())).collect();
                encoded.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then_with(|| a.0.cmp(&b.0)));
                head(out, 5, encoded.len() as u64);
                for (key, value) in encoded {
                    out.extend_from_slice(&key);
                    out.extend_from_slice(&value);
                }
            }
            Cbor::Bool(value) => out.push(if *value { 0xf5 } else { 0xf4 }),
        }
    }
}

/// The initial byte (major type) and the argument in the shortest form.
fn head(out: &mut Vec<u8>, major: u8, n: u64) {
    let major = major << 5;
    if n < 24 {
        out.push(major | n as u8);
    } else if n <= 0xff {
        out.push(major | 24);
        out.push(n as u8);
    } else if n <= 0xffff {
        out.push(major | 25);
        out.extend_from_slice(&(n as u16).to_be_bytes());
    } else if n <= 0xffff_ffff {
        out.push(major | 26);
        out.extend_from_slice(&(n as u32).to_be_bytes());
    } else {
        out.push(major | 27);
        out.extend_from_slice(&n.to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::Cbor;

    #[test]
    fn integers_use_the_shortest_form() {
        let enc = |n| Cbor::Int(n).encode();
        assert_eq!(enc(0), [0x00]);
        assert_eq!(enc(23), [0x17]);
        assert_eq!(enc(24), [0x18, 0x18]);
        assert_eq!(enc(256), [0x19, 0x01, 0x00]);
        assert_eq!(enc(-1), [0x20]);
        assert_eq!(enc(-7), [0x26]);
        assert_eq!(enc(-25), [0x38, 0x18]);
    }

    #[test]
    fn strings_arrays_and_booleans() {
        assert_eq!(Cbor::Text("fmt".into()).encode(), [0x63, b'f', b'm', b't']);
        assert_eq!(Cbor::Bytes(vec![1, 2]).encode(), [0x42, 1, 2]);
        assert_eq!(Cbor::Array(vec![Cbor::Int(1), Cbor::Bool(true)]).encode(), [0x82, 0x01, 0xf5]);
        assert_eq!(Cbor::Bool(false).encode(), [0xf4]);
    }

    #[test]
    fn maps_are_in_canonical_key_order() {
        // The attestation object: "fmt" (3 bytes) before "attStmt" before "authData".
        let map = Cbor::Map(vec![
            (Cbor::Text("authData".into()), Cbor::Bytes(vec![])),
            (Cbor::Text("fmt".into()), Cbor::Text("none".into())),
            (Cbor::Text("attStmt".into()), Cbor::Map(vec![])),
        ]);
        let mut expected = vec![0xa3];
        expected.extend([0x63, b'f', b'm', b't', 0x64, b'n', b'o', b'n', b'e']);
        expected.extend([0x67]);
        expected.extend(b"attStmt");
        expected.push(0xa0);
        expected.extend([0x68]);
        expected.extend(b"authData");
        expected.push(0x40);
        assert_eq!(map.encode(), expected);
        // The COSE key of ES256: 1, 3, -1, -2, -3.
        let key = Cbor::Map(vec![
            (Cbor::Int(-3), Cbor::Bytes(vec![3])),
            (Cbor::Int(-1), Cbor::Int(1)),
            (Cbor::Int(3), Cbor::Int(-7)),
            (Cbor::Int(1), Cbor::Int(2)),
            (Cbor::Int(-2), Cbor::Bytes(vec![2])),
        ]);
        assert_eq!(key.encode(), [0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21, 0x41, 0x02, 0x22, 0x41, 0x03]);
    }
}
