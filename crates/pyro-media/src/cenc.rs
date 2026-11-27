//! Common Encryption (ISO/IEC 23001-7) sample decryption for fragmented MP4:
//! the four protection schemes `cenc` (AES-CTR), `cens` (AES-CTR, pattern),
//! `cbc1` (AES-CBC) and `cbcs` (AES-CBC, pattern), with whole-sample and
//! subsample encryption. The block cipher is the caller's (pyro-webapi already
//! depends on the `aes` crate), so this crate takes no new dependency: the
//! modes of operation are built here from single-block operations.

/// One AES-128 key schedule, applied to single 16-byte blocks.
pub trait BlockCipher {
    fn encrypt_block(&self, block: &mut [u8; 16]);
    fn decrypt_block(&self, block: &mut [u8; 16]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Cenc,
    Cens,
    Cbc1,
    Cbcs,
}

impl Scheme {
    pub fn from_fourcc(code: &[u8]) -> Option<Self> {
        match code {
            b"cenc" => Some(Self::Cenc),
            b"cens" => Some(Self::Cens),
            b"cbc1" => Some(Self::Cbc1),
            b"cbcs" => Some(Self::Cbcs),
            _ => None,
        }
    }

    fn ctr(self) -> bool {
        matches!(self, Self::Cenc | Self::Cens)
    }

    fn patterned(self) -> bool {
        matches!(self, Self::Cens | Self::Cbcs)
    }
}

/// A `seig` sample group entry (CENCSampleEncryptionGroupEntry): the
/// protection of the samples it is assigned to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeigEntry {
    pub protected: bool,
    pub iv_size: u8,
    pub kid: [u8; 16],
    pub constant_iv: Option<Vec<u8>>,
    pub crypt_blocks: u8,
    pub skip_blocks: u8,
}

/// A protected track's `sinf`: the scheme and `tenc` defaults, and the
/// `seig` entries of its sample table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Protection {
    pub scheme: Scheme,
    pub default: SeigEntry,
    pub groups: Vec<SeigEntry>,
}

/// How one sample is encrypted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleEnc {
    pub scheme: Scheme,
    pub kid: [u8; 16],
    /// The initialization vector, 8 or 16 bytes.
    pub iv: Vec<u8>,
    /// `(clear bytes, protected bytes)` runs; empty means the whole sample.
    pub subsamples: Vec<(u16, u32)>,
    pub crypt_blocks: u8,
    pub skip_blocks: u8,
}

impl SampleEnc {
    /// Every subsample is clear: nothing of the sample is encrypted (a protected track may carry
    /// clear samples), so no key is needed.
    pub fn is_clear(&self) -> bool {
        !self.subsamples.is_empty() && self.subsamples.iter().all(|&(_, protected)| protected == 0)
    }
}

fn full_iv(iv: &[u8]) -> Option<[u8; 16]> {
    if iv.len() != 8 && iv.len() != 16 {
        return None;
    }
    let mut out = [0u8; 16];
    out[..iv.len()].copy_from_slice(iv);
    Some(out)
}

/// The counter block incremented by one: an 8-byte IV leaves the low 64 bits
/// as the counter (ISO/IEC 23001-7 9.2), a 16-byte IV counts over all 128.
fn increment(counter: &mut [u8; 16], iv_len: usize) {
    let low = if iv_len == 8 { 8 } else { 0 };
    for byte in counter[low..].iter_mut().rev() {
        *byte = byte.wrapping_add(1);
        if *byte != 0 {
            break;
        }
    }
}

struct Ctr<'a> {
    cipher: &'a dyn BlockCipher,
    counter: [u8; 16],
    iv_len: usize,
    stream: [u8; 16],
    used: usize,
}

impl Ctr<'_> {
    fn apply(&mut self, data: &mut [u8]) {
        for byte in data {
            if self.used == 16 {
                self.stream = self.counter;
                self.cipher.encrypt_block(&mut self.stream);
                increment(&mut self.counter, self.iv_len);
                self.used = 0;
            }
            *byte ^= self.stream[self.used];
            self.used += 1;
        }
    }

    /// The next whole keystream block starts a fresh 16-byte unit: the
    /// pattern schemes encrypt only complete blocks.
    fn align(&mut self) {
        self.used = 16;
    }
}

fn cbc_decrypt(cipher: &dyn BlockCipher, chain: &mut [u8; 16], data: &mut [u8]) {
    for chunk in data.chunks_exact_mut(16) {
        let mut cipher_block = [0u8; 16];
        cipher_block.copy_from_slice(chunk);
        let mut plain = cipher_block;
        cipher.decrypt_block(&mut plain);
        for (p, c) in plain.iter_mut().zip(chain.iter()) {
            *p ^= c;
        }
        chunk.copy_from_slice(&plain);
        *chain = cipher_block;
    }
}

/// Decrypts the protected `range` of `data` with the pattern of 16-byte
/// blocks `crypt` encrypted then `skip` clear (no pattern: all whole blocks).
fn for_each_encrypted_run(range: &mut [u8], crypt: usize, skip: usize, mut f: impl FnMut(&mut [u8])) {
    if crypt == 0 {
        let whole = range.len() / 16 * 16;
        f(&mut range[..whole]);
        return;
    }
    let mut at = 0;
    while at + 16 <= range.len() {
        let end = (at + crypt * 16).min(range.len() / 16 * 16);
        if end > at {
            f(&mut range[at..end]);
        }
        at = end + skip * 16;
    }
}

/// The clear sample for the encrypted `data`, or `None` when its subsample
/// map does not add up to the sample size or its IV has no valid size.
pub fn decrypt(enc: &SampleEnc, cipher: &dyn BlockCipher, data: &[u8]) -> Option<Vec<u8>> {
    let iv = full_iv(&enc.iv)?;
    let mut out = data.to_vec();
    let ranges: Vec<(usize, usize)> = if enc.subsamples.is_empty() {
        vec![(0, data.len())]
    } else {
        let mut at = 0usize;
        let mut ranges = Vec::with_capacity(enc.subsamples.len());
        for &(clear, protected) in &enc.subsamples {
            let start = at.checked_add(usize::from(clear))?;
            let end = start.checked_add(protected as usize)?;
            if end > data.len() {
                return None;
            }
            ranges.push((start, end));
            at = end;
        }
        if at != data.len() {
            return None;
        }
        ranges
    };
    let (crypt, skip) = if enc.scheme.patterned() {
        // A pattern of (0, 0) is "no pattern": every whole block.
        if enc.crypt_blocks == 0 && enc.skip_blocks == 0 {
            (0, 0)
        } else {
            (usize::from(enc.crypt_blocks), usize::from(enc.skip_blocks))
        }
    } else {
        (0, 0)
    };
    if enc.scheme.ctr() {
        // The keystream runs through the protected bytes of every subsample.
        let mut ctr = Ctr { cipher, counter: iv, iv_len: enc.iv.len(), stream: [0; 16], used: 16 };
        for (start, end) in ranges {
            let range = &mut out[start..end];
            if enc.scheme.patterned() {
                for_each_encrypted_run(range, crypt, skip, |run| {
                    ctr.align();
                    ctr.apply(run);
                });
            } else {
                ctr.apply(range);
            }
        }
    } else {
        // cbc1 chains across the protected ranges of the sample; cbcs starts
        // every subsample's protected range from the IV again.
        let mut chain = iv;
        for (start, end) in ranges {
            if enc.scheme == Scheme::Cbcs {
                chain = iv;
            }
            let range = &mut out[start..end];
            for_each_encrypted_run(range, crypt, skip, |run| cbc_decrypt(cipher, &mut chain, run));
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A permutation that is its own inverse: enough to check the modes.
    struct Xor;
    impl BlockCipher for Xor {
        fn encrypt_block(&self, block: &mut [u8; 16]) {
            for b in block.iter_mut() {
                *b ^= 0x5a;
            }
        }
        fn decrypt_block(&self, block: &mut [u8; 16]) {
            self.encrypt_block(block);
        }
    }

    fn enc(scheme: Scheme, iv: &[u8], subsamples: Vec<(u16, u32)>) -> SampleEnc {
        SampleEnc { scheme, kid: [0; 16], iv: iv.to_vec(), subsamples, crypt_blocks: 0, skip_blocks: 0 }
    }

    #[test]
    fn counter_mode_runs_through_subsamples_and_counts_the_low_half() {
        let iv = [0, 0, 0, 0, 0, 0, 0, 7];
        // Zero ciphertext decrypts to the keystream: block n is (iv || n) ^ 0x5a.
        let data = vec![0u8; 40];
        let out = decrypt(&enc(Scheme::Cenc, &iv, vec![(4, 20), (0, 16)]), &Xor, &data).unwrap();
        assert_eq!(&out[..4], &[0, 0, 0, 0]);
        // First protected byte: counter 0, byte 0 of the stream.
        assert_eq!(out[4], 0x5a ^ 0);
        assert_eq!(out[11], 0x5a ^ 7);
        // The second block of the stream begins at the sixteenth protected byte.
        assert_eq!(out[4 + 15], 0x5a);
        assert_eq!(out[4 + 16 + 7], 0x5a ^ 7 ^ 0 ^ 0);
        let mut again = out.clone();
        // Applying the same stream twice restores the input.
        let twice = decrypt(&enc(Scheme::Cenc, &iv, vec![(4, 20), (0, 16)]), &Xor, &again).unwrap();
        again.fill(0);
        assert_eq!(twice, again);
    }

    #[test]
    fn a_subsample_map_that_misses_the_sample_size_fails() {
        assert!(decrypt(&enc(Scheme::Cenc, &[0; 8], vec![(4, 20)]), &Xor, &[0u8; 30]).is_none());
        assert!(decrypt(&enc(Scheme::Cenc, &[0; 5], vec![]), &Xor, &[0u8; 30]).is_none());
    }

    #[test]
    fn cbcs_pattern_leaves_the_skip_blocks_and_the_partial_tail_clear() {
        let mut e = enc(Scheme::Cbcs, &[1; 16], vec![]);
        e.crypt_blocks = 1;
        e.skip_blocks = 1;
        let data: Vec<u8> = (0..70u8).collect();
        let out = decrypt(&e, &Xor, &data).unwrap();
        // Block 0 is decrypted (chain = IV), block 1 is skipped, block 2 decrypted
        // with the chain still the first block's ciphertext, the 6-byte tail is clear.
        assert_ne!(&out[..16], &data[..16]);
        assert_eq!(&out[16..32], &data[16..32]);
        assert_ne!(&out[32..48], &data[32..48]);
        assert_eq!(&out[48..], &data[48..]);
    }
}
