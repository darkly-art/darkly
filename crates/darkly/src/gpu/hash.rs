//! Integer hashing shared by anything that needs deterministic pseudo-randomness
//! from a coordinate, an index or a seed.
//!
//! Lives at `gpu` scope rather than inside any one of its callers: a hash is not
//! a property of noise voids, of film grain, or of preview backdrops, and naming
//! it after whichever of them was written first would make every later caller
//! reach across into a module it has nothing to do with.
//!
//! Credits:
//!
//! • `pcg_hash` is the `pcg` variant from Mark Jarzynski and Marc Olano, "Hash
//!   Functions for GPU Rendering", Journal of Computer Graphics Techniques 9(3),
//!   2020, <https://jcgt.org/published/0009/03/02/>.

/// Integer PCG hash: one multiply-add, an xorshift by a state-derived amount,
/// a second multiply, and a final xorshift. Fast, well-distributed across the
/// whole 32-bit range, and free of the visible axis-aligned structure the
/// cheaper `sin`-based hashes show once their output is used as a lattice value.
pub fn pcg_hash(n: u32) -> u32 {
    let mut h = n.wrapping_mul(747796405).wrapping_add(2891336453);
    h = ((h >> ((h >> 28) + 4)) ^ h).wrapping_mul(277803737);
    (h >> 22) ^ h
}
