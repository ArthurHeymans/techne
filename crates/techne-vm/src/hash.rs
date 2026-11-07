//! A hash map whose iteration order is reproducible across processes.
//!
//! Native emission writes some blocks in map iteration order, and those
//! blocks decide machine-code layout, so a per-process seed made two runs of
//! one binary compile the same function differently. `DefaultHasher` is
//! SipHash-1-3 with zero keys, so the order is fixed.
//!
//! Reproducible does not mean canonical: the order also depends on capacity
//! and insertion/removal history and on this toolchain's `HashMap`, and it is
//! not hardened against adversarial keys the way a randomized map is. Use it
//! only for the VM's internal program-counter, global-index and code-id maps.

use std::collections::HashMap as Std;
use std::collections::hash_map::DefaultHasher;
use std::hash::BuildHasherDefault;

/// `std::collections::HashMap` with a fixed hasher: reproducible iteration.
pub(crate) type Map<K, V> = Std<K, V, BuildHasherDefault<DefaultHasher>>;
