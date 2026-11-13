//! Public suffix list lookups behind non-inline functions.
//!
//! The `psl` crate marks its lookups `#[inline]`, so the generated list
//! (about 400k lines of LLVM IR) was code-generated inside pyro-net on every
//! pyro-net rebuild, at -O0 in dev. Here it is generated once, in a crate that
//! rarely changes.

/// The public suffix of `name`, e.g. `co.uk` for `www.example.co.uk`.
#[inline(never)]
pub fn suffix_str(name: &str) -> Option<&str> {
    psl::suffix_str(name)
}

/// The registrable domain of `name`, e.g. `example.co.uk` for
/// `www.example.co.uk`.
#[inline(never)]
pub fn domain_str(name: &str) -> Option<&str> {
    psl::domain_str(name)
}
