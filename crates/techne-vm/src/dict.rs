//! Persistent dicts and sets: hash tries in the heap (CHAMP, Steindorfer
//! and Vinju, "Optimizing Hash-Array Mapped Tries for Fast and Lean
//! Immutable JVM Collections", 2015).
//!
//! A dict or set object (`Kind::Dict`, `Kind::Set`) holds its entry count
//! and its root node. A node (`Kind::Node`) holds two bitmaps, then its
//! entries (a key and a value each in a dict, a key in a set), then its
//! subnodes, both in the order of their bits. Each level of the trie takes
//! five bits of the key's 32-bit hash (`key_hash`, consistent with
//! `equal?`); keys whose hashes are the same throughout share a collision
//! node, whose first field is minus the stride and second the hash.
//!
//! Deleting moves an entry left alone in a node up into its parent, so a
//! trie's shape depends only on its keys: equal dicts are made of equal
//! nodes, which `equal?` and `hash` compare and hash field by field (but
//! the entries of collision nodes, which are in no particular order).
//!
//! An update copies the nodes on the path to its key. They are allocated
//! after checking that the nursery has room for all of them, so no
//! collection moves the nodes being copied; paths too long for the nursery
//! (a collision node of very many keys) go to the old generation.

use crate::{
    builtins::{equal, equal_hash, list_values, type_error},
    heap::{Kind, field, header, is_kind, len_of, set_field},
    value::Value,
    vm::{Error, Vm},
};

type R = Result<Value, Error>;

/// Hash bits each level takes, and how many there are.
const BITS: u32 = 5;
const HASH_BITS: u32 = 32;

/// Where nodes are allocated.
#[derive(Clone, Copy)]
enum Space {
    /// The nursery, checked to have room beforehand.
    Young,
    /// The old generation, remembered (it may hold nursery values).
    Old,
    /// The old generation, for literals, which only hold old values.
    Const,
}

/// The 32-bit hash a key is placed by: `hash`'s, mixed (splitmix64's
/// finalizer), as `hash` is not meant to spread its low bits.
pub(crate) fn key_hash(vm: &mut Vm, k: Value) -> u32 {
    #[cfg(test)]
    if let Some(h) = tests::FAKE.with(std::cell::Cell::get) {
        return h(k);
    }
    let mut z = equal_hash(vm, k);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    ((z ^ (z >> 31)) >> 32) as u32
}

/// Fields per entry of a dict or set object: 2 (key and value) or 1.
pub(crate) fn stride_of(v: Value) -> usize {
    if is_kind(v, Kind::Dict) { 2 } else { 1 }
}

fn int(i: i64) -> Value {
    Value::int_unchecked(i)
}

/// Whether node `n` is a collision node.
#[inline]
pub(crate) unsafe fn is_collision(n: *mut u64) -> bool {
    unsafe { field(n, 0).as_int() < 0 }
}

/// The entry and subnode bitmaps of an ordinary node.
#[inline]
unsafe fn bitmaps(n: *mut u64) -> (u32, u32) {
    unsafe { (field(n, 0).as_int() as u32, field(n, 1).as_int() as u32) }
}

#[inline]
fn bit(hash: u32, shift: u32) -> u32 {
    1 << ((hash >> shift) & 31)
}

/// Position of `bit` among the bits of `map`.
#[inline]
fn index(map: u32, bit: u32) -> usize {
    (map & (bit - 1)).count_ones() as usize
}

unsafe fn fields(n: *mut u64) -> Vec<Value> {
    unsafe { (0..len_of(n)).map(|i| field(n, i)).collect() }
}

unsafe fn node(vm: &mut Vm, space: Space, fields: &[Value]) -> Value {
    let words = 1 + fields.len();
    let p = match space {
        Space::Young => vm.heap.bump(words),
        Space::Old => vm.heap.alloc_old(words),
        Space::Const => vm.heap.alloc_old_unremembered(words),
    };
    unsafe {
        *p = header(Kind::Node, fields.len(), 0);
        for (i, &v) in fields.iter().enumerate() {
            set_field(p, i, v);
        }
    }
    Value::ptr(p)
}

/// The node of a dict or set object.
fn root(d: Value) -> *mut u64 {
    unsafe { field(d.as_ptr(), 1).as_ptr() }
}

/// Whether `v` is a dict or a set.
fn is_dict_or_set(v: Value) -> bool {
    is_kind(v, Kind::Dict) || is_kind(v, Kind::Set)
}

/// The number of entries of dict or set `d`; 0 for any other value.
pub fn count(d: Value) -> usize {
    if !is_dict_or_set(d) {
        return 0;
    }
    unsafe { field(d.as_ptr(), 0).as_int() as usize }
}

/// The field of `key`'s entry in the trie at `n`, and its node.
unsafe fn find(mut n: *mut u64, key: Value, hash: u32, s: usize) -> Option<(*mut u64, usize)> {
    let mut shift = 0;
    unsafe {
        loop {
            if is_collision(n) {
                return (2..len_of(n)).step_by(s).find(|&i| equal(field(n, i), key)).map(|i| (n, i));
            }
            let (dm, nm) = bitmaps(n);
            let b = bit(hash, shift);
            if dm & b != 0 {
                let at = 2 + s * index(dm, b);
                return equal(field(n, at), key).then_some((n, at));
            }
            if nm & b == 0 {
                return None;
            }
            n = field(n, 2 + s * dm.count_ones() as usize + index(nm, b)).as_ptr();
            shift += BITS;
        }
    }
}

/// The words an update of `key` allocates at most: a copy of each node on
/// its path, an entry larger, and with `grow` the nodes that may separate
/// it from the key already where it goes.
unsafe fn path_words(mut n: *mut u64, hash: u32, s: usize, grow: bool) -> usize {
    let mut words = 0;
    let mut shift = 0;
    unsafe {
        loop {
            words += 1 + len_of(n) + s;
            if is_collision(n) {
                return words;
            }
            let (dm, nm) = bitmaps(n);
            let b = bit(hash, shift);
            if nm & b != 0 {
                n = field(n, 2 + s * dm.count_ones() as usize + index(nm, b)).as_ptr();
                shift += BITS;
                continue;
            }
            if grow && dm & b != 0 {
                words += ((HASH_BITS - shift) / BITS + 2) as usize * (3 + 2 * s);
            }
            return words;
        }
    }
}

enum Put {
    /// The key was there with this value already.
    Same,
    /// The new node, and whether the key is new.
    New(Value, bool),
}

/// The node of the trie at `n` with `key` mapped to `val`.
#[allow(clippy::too_many_arguments)]
unsafe fn put(vm: &mut Vm, sp: Space, n: *mut u64, key: Value, val: Value, hash: u32, shift: u32, s: usize) -> Put {
    unsafe {
        let len = len_of(n);
        if is_collision(n) {
            if let Some(i) = (2..len).step_by(s).find(|&i| equal(field(n, i), key)) {
                if s == 1 || field(n, i + 1) == val {
                    return Put::Same;
                }
                let mut fs = fields(n);
                fs[i + 1] = val;
                return Put::New(node(vm, sp, &fs), false);
            }
            let mut fs = fields(n);
            fs.extend([key, val].into_iter().take(s));
            return Put::New(node(vm, sp, &fs), true);
        }
        let (dm, nm) = bitmaps(n);
        let b = bit(hash, shift);
        if dm & b != 0 {
            let at = 2 + s * index(dm, b);
            let k0 = field(n, at);
            if equal(k0, key) {
                if s == 1 || field(n, at + 1) == val {
                    return Put::Same;
                }
                let mut fs = fields(n);
                fs[at + 1] = val;
                return Put::New(node(vm, sp, &fs), false);
            }
            // Both keys go one level down.
            let v0 = if s == 2 { field(n, at + 1) } else { Value::VOID };
            let h0 = key_hash(vm, k0);
            let sub = pair(vm, sp, (k0, v0, h0), (key, val, hash), shift + BITS, s);
            let mut fs = fields(n);
            fs.drain(at..at + s);
            fs.insert(2 + s * (dm.count_ones() as usize - 1) + index(nm, b), sub);
            fs[0] = int((dm ^ b) as i64);
            fs[1] = int((nm | b) as i64);
            return Put::New(node(vm, sp, &fs), true);
        }
        if nm & b != 0 {
            let at = 2 + s * dm.count_ones() as usize + index(nm, b);
            return match put(vm, sp, field(n, at).as_ptr(), key, val, hash, shift + BITS, s) {
                Put::Same => Put::Same,
                Put::New(c, added) => {
                    let mut fs = fields(n);
                    fs[at] = c;
                    Put::New(node(vm, sp, &fs), added)
                }
            };
        }
        let at = 2 + s * index(dm, b);
        let mut fs = fields(n);
        fs.splice(at..at, [key, val].into_iter().take(s));
        fs[0] = int((dm | b) as i64);
        Put::New(node(vm, sp, &fs), true)
    }
}

/// A node holding two entries whose hashes agree up to `shift`.
unsafe fn pair(vm: &mut Vm, sp: Space, a: (Value, Value, u32), b: (Value, Value, u32), shift: u32, s: usize) -> Value {
    let entry = |e: (Value, Value, u32)| [e.0, e.1].into_iter().take(s);
    unsafe {
        if shift >= HASH_BITS {
            let fs: Vec<Value> = [int(-(s as i64)), int(a.2 as i64)].into_iter().chain(entry(a)).chain(entry(b)).collect();
            return node(vm, sp, &fs);
        }
        let (bit_a, bit_b) = (bit(a.2, shift), bit(b.2, shift));
        if bit_a == bit_b {
            let sub = pair(vm, sp, a, b, shift + BITS, s);
            return node(vm, sp, &[int(0), int(bit_a as i64), sub]);
        }
        let (first, second) = if bit_a < bit_b { (a, b) } else { (b, a) };
        let fs: Vec<Value> = [int((bit_a | bit_b) as i64), int(0)].into_iter().chain(entry(first)).chain(entry(second)).collect();
        node(vm, sp, &fs)
    }
}

enum Cut {
    Absent,
    Node(Value),
    /// What is left is one entry, for the parent to hold.
    Single(Value, Value),
}

/// The node of the trie at `n` without `key`; `top` for the root, which
/// keeps its last entry.
#[allow(clippy::too_many_arguments)]
unsafe fn cut(vm: &mut Vm, sp: Space, n: *mut u64, key: Value, hash: u32, shift: u32, s: usize, top: bool) -> Cut {
    let entry = |n: *mut u64, at: usize| unsafe { (field(n, at), if s == 2 { field(n, at + 1) } else { Value::VOID }) };
    unsafe {
        let len = len_of(n);
        if is_collision(n) {
            let Some(i) = (2..len).step_by(s).find(|&i| equal(field(n, i), key)) else { return Cut::Absent };
            if len == 2 + 2 * s {
                let (k, v) = entry(n, if i == 2 { 2 + s } else { 2 });
                return Cut::Single(k, v);
            }
            let mut fs = fields(n);
            fs.drain(i..i + s);
            return Cut::Node(node(vm, sp, &fs));
        }
        let (dm, nm) = bitmaps(n);
        let b = bit(hash, shift);
        if dm & b != 0 {
            let at = 2 + s * index(dm, b);
            if !equal(field(n, at), key) {
                return Cut::Absent;
            }
            if !top && nm == 0 && dm.count_ones() == 2 {
                let (k, v) = entry(n, if at == 2 { 2 + s } else { 2 });
                return Cut::Single(k, v);
            }
            let mut fs = fields(n);
            fs.drain(at..at + s);
            fs[0] = int((dm ^ b) as i64);
            return Cut::Node(node(vm, sp, &fs));
        }
        if nm & b == 0 {
            return Cut::Absent;
        }
        let at = 2 + s * dm.count_ones() as usize + index(nm, b);
        match cut(vm, sp, field(n, at).as_ptr(), key, hash, shift + BITS, s, false) {
            Cut::Absent => Cut::Absent,
            Cut::Node(c) => {
                let mut fs = fields(n);
                fs[at] = c;
                Cut::Node(node(vm, sp, &fs))
            }
            Cut::Single(k, v) if !top && dm == 0 && nm.count_ones() == 1 => Cut::Single(k, v),
            Cut::Single(k, v) => {
                let mut fs = fields(n);
                fs.remove(at);
                let to = 2 + s * index(dm, b);
                fs.splice(to..to, [k, v].into_iter().take(s));
                fs[0] = int((dm | b) as i64);
                fs[1] = int((nm ^ b) as i64);
                Cut::Node(node(vm, sp, &fs))
            }
        }
    }
}

/// A dict or set object of `kind` with `count` entries in trie `root`.
fn wrap(vm: &mut Vm, sp: Space, kind: Kind, count: usize, root: Value) -> Value {
    let p = match sp {
        // Room was made for the nodes; this is one more object.
        Space::Young => {
            vm.scratch.push(root);
            let p = vm.alloc(3);
            let root = vm.scratch.pop().unwrap();
            unsafe { set_field(p, 1, root) };
            p
        }
        Space::Old => vm.heap.alloc_old(3),
        Space::Const => vm.heap.alloc_old_unremembered(3),
    };
    unsafe {
        *p = header(kind, 2, 0);
        set_field(p, 0, int(count as i64));
        if !matches!(sp, Space::Young) {
            set_field(p, 1, root);
        }
    }
    Value::ptr(p)
}

fn empty(vm: &mut Vm, sp: Space, kind: Kind) -> Value {
    if matches!(sp, Space::Young) && !vm.heap.has_room(3) {
        vm.collect();
    }
    let root = unsafe { node(vm, sp, &[int(0), int(0)]) };
    wrap(vm, sp, kind, 0, root)
}

/// Where the nodes of an update of `d` at `key` go, collecting first if
/// the nursery lacks the room; `d`, `key` and `val` again, as they may
/// have moved.
fn make_room(vm: &mut Vm, d: Value, key: Value, val: Value, hash: u32, grow: bool) -> (Space, Value, Value, Value) {
    let words = unsafe { path_words(root(d), hash, stride_of(d), grow) } + 3;
    if words > vm.heap.nursery_capacity() / 2 {
        return (Space::Old, d, key, val);
    }
    if vm.heap.has_room(words) {
        return (Space::Young, d, key, val);
    }
    let mark = vm.scratch.len();
    vm.scratch.extend([d, key, val]);
    vm.collect();
    let (d, key, val) = (vm.scratch[mark], vm.scratch[mark + 1], vm.scratch[mark + 2]);
    vm.scratch.truncate(mark);
    (Space::Young, d, key, val)
}

/// Dict or set `d` with `key` (mapped to `val` in a dict).
pub(crate) fn insert(vm: &mut Vm, d: Value, key: Value, val: Value) -> Value {
    let hash = key_hash(vm, key);
    let (sp, d, key, val) = make_room(vm, d, key, val, hash, true);
    insert_in(vm, sp, d, key, val, hash)
}

fn insert_in(vm: &mut Vm, sp: Space, d: Value, key: Value, val: Value, hash: u32) -> Value {
    let kind = if is_kind(d, Kind::Dict) { Kind::Dict } else { Kind::Set };
    match unsafe { put(vm, sp, root(d), key, val, hash, 0, stride_of(d)) } {
        Put::Same => d,
        Put::New(r, added) => {
            let n = count(d) + added as usize;
            wrap(vm, sp, kind, n, r)
        }
    }
}

/// Dict or set `d` without `key`.
pub(crate) fn remove(vm: &mut Vm, d: Value, key: Value) -> Value {
    let hash = key_hash(vm, key);
    let s = stride_of(d);
    if unsafe { find(root(d), key, hash, s) }.is_none() {
        return d;
    }
    let (sp, d, key, _) = make_room(vm, d, key, Value::VOID, hash, false);
    let kind = if s == 2 { Kind::Dict } else { Kind::Set };
    match unsafe { cut(vm, sp, root(d), key, hash, 0, s, true) } {
        Cut::Node(r) => {
            let n = count(d) - 1;
            wrap(vm, sp, kind, n, r)
        }
        Cut::Absent | Cut::Single(..) => unreachable!("the key is there, and the root keeps its last entry"),
    }
}

/// The value `key` has in dict `d` (`key` itself in a set).
pub(crate) fn lookup(vm: &mut Vm, d: Value, key: Value) -> Option<Value> {
    let hash = key_hash(vm, key);
    let s = stride_of(d);
    unsafe { find(root(d), key, hash, s).map(|(n, at)| field(n, at + s - 1)) }
}

/// The fields of every entry of dict or set `d`, keys and values in turn
/// for a dict, in the order of the trie; none for any other value.
pub fn entries(d: Value) -> Vec<Value> {
    if !is_dict_or_set(d) {
        return Vec::new();
    }
    let s = stride_of(d);
    let mut out = Vec::with_capacity(s * count(d));
    let mut todo = vec![root(d)];
    while let Some(n) = todo.pop() {
        unsafe {
            let len = len_of(n);
            if is_collision(n) {
                out.extend((2..len).map(|i| field(n, i)));
                continue;
            }
            let data = 2 + s * bitmaps(n).0.count_ones() as usize;
            out.extend((2..data).map(|i| field(n, i)));
            todo.extend((data..len).rev().map(|i| field(n, i).as_ptr()));
        }
    }
    out
}

/// A dict or set of `kind` made at once of the entries on `vm.scratch`
/// from `mark` (keys and values in turn for a dict), which it pops; a key
/// given twice keeps its last value. The trie is the one inserting them
/// one by one makes, without copying a node per entry.
pub(crate) fn from_scratch(vm: &mut Vm, kind: Kind, mark: usize) -> Value {
    let s = if matches!(kind, Kind::Dict) { 2 } else { 1 };
    let n = (vm.scratch.len() - mark) / s;
    let hashes: Vec<u32> = (0..n).map(|i| key_hash(vm, vm.scratch[mark + s * i])).collect();
    // Entries in the order of the trie: by their hash's chunks from the
    // first level's, then as given.
    let order = |h: u32| (0..7).fold(0u64, |acc, level| (acc << 5) | ((h >> (level * BITS)) & 31) as u64);
    let mut sorted: Vec<(u64, usize)> = (0..n).map(|i| (order(hashes[i]), i)).collect();
    sorted.sort_unstable();
    // A key given twice: the later one stays. Equal keys hash the same.
    let mut keep = vec![true; n];
    for run in sorted.chunk_by(|a, b| a.0 == b.0) {
        for (x, &(_, i)) in run.iter().enumerate() {
            if run[x + 1..].iter().any(|&(_, j)| equal(vm.scratch[mark + s * i], vm.scratch[mark + s * j])) {
                keep[i] = false;
            }
        }
    }
    let entries: Vec<(u32, usize)> = sorted.into_iter().filter(|&(_, i)| keep[i]).map(|(_, i)| (hashes[i], mark + s * i)).collect();
    let count = entries.len();
    let root = build(vm, &entries, 0, s);
    vm.scratch.truncate(mark);
    vm.scratch.push(root);
    let p = vm.alloc(3);
    let root = vm.scratch.pop().unwrap();
    unsafe {
        *p = header(kind, 2, 0);
        set_field(p, 0, int(count as i64));
        set_field(p, 1, root);
    }
    Value::ptr(p)
}

/// The node at `shift` of `entries`: their hash, and where their fields
/// are on `vm.scratch`, in the order of the trie.
fn build(vm: &mut Vm, entries: &[(u32, usize)], shift: u32, s: usize) -> Value {
    let field_of = |vm: &Vm, at: usize| [vm.scratch[at], vm.scratch[at + s - 1]].into_iter().take(s);
    if shift >= HASH_BITS {
        let fields: Vec<Value> =
            [int(-(s as i64)), int(entries[0].0 as i64)].into_iter().chain(entries.iter().flat_map(|&(_, at)| field_of(vm, at))).collect();
        return alloc_node(vm, &fields);
    }
    let chunk = |h: u32| (h >> shift) & 31;
    let groups: Vec<&[(u32, usize)]> = entries.chunk_by(|a, b| chunk(a.0) == chunk(b.0)).collect();
    // Subnodes first, onto the scratch stack (each may collect).
    let base = vm.scratch.len();
    for g in groups.iter().filter(|g| g.len() > 1) {
        let sub = crate::nested(|| build(vm, g, shift + BITS, s));
        vm.scratch.push(sub);
    }
    let (mut dm, mut nm) = (0u32, 0u32);
    for g in &groups {
        if g.len() > 1 { nm |= 1 << chunk(g[0].0) } else { dm |= 1 << chunk(g[0].0) }
    }
    let mut fields = vec![int(dm as i64), int(nm as i64)];
    fields.extend(groups.iter().filter(|g| g.len() == 1).flat_map(|g| field_of(vm, g[0].1)));
    fields.extend_from_slice(&vm.scratch[base..]);
    vm.scratch.truncate(base);
    alloc_node(vm, &fields)
}

/// A node of `fields`, which are rooted meanwhile.
fn alloc_node(vm: &mut Vm, fields: &[Value]) -> Value {
    let mark = vm.scratch.len();
    vm.scratch.extend_from_slice(fields);
    let p = vm.alloc(1 + fields.len());
    unsafe {
        *p = header(Kind::Node, fields.len(), 0);
        for (i, v) in vm.scratch.drain(mark..).enumerate() {
            set_field(p, i, v);
        }
    }
    Value::ptr(p)
}

/// A literal of `kind` with entries `items` (keys and values in turn for a
/// dict), allocated as constants are: in the old generation, without
/// collecting.
pub(crate) fn constant(vm: &mut Vm, kind: Kind, items: &[Value]) -> Value {
    let s = if matches!(kind, Kind::Dict) { 2 } else { 1 };
    items.chunks(s).fold(empty(vm, Space::Const, kind), |d, e| {
        let hash = key_hash(vm, e[0]);
        insert_in(vm, Space::Const, d, e[0], e[s - 1], hash)
    })
}

/// Whether collision nodes `p` and `q` hold equal entries, in any order.
/// Their entries are compared by `equal?` anew; a pair of collision nodes
/// met again inside them is taken as equal, as `equal?` takes a pair of
/// objects met again, so that circular data ends.
pub(crate) unsafe fn equal_collisions(p: *mut u64, q: *mut u64) -> bool {
    thread_local!(static COMPARING: std::cell::RefCell<rustc_hash::FxHashSet<(usize, usize)>> = Default::default());
    let key = (p as usize, q as usize);
    if !COMPARING.with(|c| c.borrow_mut().insert(key)) {
        return true;
    }
    let same = crate::nested(|| unsafe {
        // A collision node's count is minus its stride: 1 or 2, whatever
        // the field holds.
        let s = (-field(p, 0).as_int()).clamp(1, 2) as usize;
        let len = len_of(p);
        len == len_of(q)
            && (2..len).step_by(s).all(|i| {
                (2..len).step_by(s).any(|j| equal(field(p, i), field(q, j)) && (s == 1 || equal(field(p, i + 1), field(q, j + 1))))
            })
    });
    COMPARING.with(|c| c.borrow_mut().remove(&key));
    same
}

// ----- natives -----

fn arg(vm: &Vm, args: usize, i: usize) -> Value {
    vm.regs[args + i]
}

fn dict_arg(v: Value, who: &str) -> Result<Value, Error> {
    if is_kind(v, Kind::Dict) { Ok(v) } else { Err(type_error(who, "dict", v)) }
}

fn set_arg(v: Value, who: &str) -> Result<Value, Error> {
    if is_kind(v, Kind::Set) { Ok(v) } else { Err(type_error(who, "set", v)) }
}

/// A dict or set of `kind` from the entry fields of the arguments from `from`.
fn from_args(vm: &mut Vm, kind: Kind, args: usize, from: usize, n: usize) -> R {
    let s = if matches!(kind, Kind::Dict) { 2 } else { 1 };
    if !(n - from).is_multiple_of(s) {
        return Err(Error::new("dict: a key without a value"));
    }
    let mark = vm.scratch.len();
    let d = empty(vm, Space::Young, kind);
    vm.scratch.push(d);
    for i in (from..n).step_by(s) {
        let (d, k, v) = (vm.scratch[mark], arg(vm, args, i), arg(vm, args, i + s - 1));
        vm.scratch[mark] = insert(vm, d, k, v);
    }
    Ok(vm.scratch.pop().unwrap())
}

/// A dict or set of `kind` from list `l`: of (key . value) pairs for a
/// dict, of keys for a set.
fn from_list(vm: &mut Vm, kind: Kind, l: Value, who: &str) -> R {
    let items = list_values(l).ok_or_else(|| type_error(who, "list", l))?;
    if matches!(kind, Kind::Dict) && !items.iter().all(|&e| is_kind(e, Kind::Pair)) {
        return Err(type_error(who, "list of pairs", l));
    }
    // Admitting may collect.
    let mark = vm.scratch.len();
    vm.scratch.push(l);
    if let Err(e) = vm.admit_items(items.len(), 64, 0) {
        vm.scratch.truncate(mark);
        return Err(e);
    }
    let d = empty(vm, Space::Young, kind);
    vm.scratch.push(d);
    let mut l = vm.scratch[mark];
    while is_kind(l, Kind::Pair) {
        let e = unsafe { field(l.as_ptr(), 0) };
        let (k, v) = if matches!(kind, Kind::Dict) { unsafe { (field(e.as_ptr(), 0), field(e.as_ptr(), 1)) } } else { (e, e) };
        vm.scratch[mark] = unsafe { field(l.as_ptr(), 1) };
        let d = vm.scratch[mark + 1];
        // As `assoc` finds it: the first entry of a key.
        if lookup(vm, d, k).is_none() {
            vm.scratch[mark + 1] = insert(vm, d, k, v);
        }
        l = vm.scratch[mark];
    }
    let d = vm.scratch[mark + 1];
    vm.scratch.truncate(mark);
    Ok(d)
}

/// A list of every `pick`th entry field of `d` from `first`, or with
/// `pairs` of (key . value) pairs.
fn to_list(vm: &mut Vm, d: Value, first: usize, pick: usize, pairs: bool) -> R {
    // Admitting and making room may collect, and move `d`.
    vm.scratch.push(d);
    let admitted = vm.admit_items(count(d), if pairs { 48 } else { 24 }, 0);
    let d = vm.scratch.pop().unwrap();
    admitted?;
    if !pairs {
        let picked: Vec<Value> = entries(d).into_iter().skip(first).step_by(pick).collect();
        return Ok(vm.make_list_raw(&picked));
    }
    vm.scratch.push(d);
    let b = crate::builtins::Bulk::new(vm, 6 * count(d));
    let items = entries(vm.scratch.pop().unwrap());
    let mut b = b?;
    let mut acc = Value::NIL;
    for e in items.chunks(2).rev() {
        let entry = b.pair(vm, e[0], e[1]);
        acc = b.pair(vm, entry, acc);
    }
    Ok(acc)
}

fn dict_ref(vm: &mut Vm, args: usize, n: usize) -> R {
    let d = dict_arg(arg(vm, args, 0), "dict-ref")?;
    match lookup(vm, d, arg(vm, args, 1)) {
        Some(v) => Ok(v),
        None if n > 2 => Ok(arg(vm, args, 2)),
        None => Err(Error::new(format!("dict-ref: no key {} in the dict", crate::builtins::brief(arg(vm, args, 1))))),
    }
}

pub fn install(vm: &mut Vm) {
    crate::natives! { vm;
        /// Return a dict of KEYS-AND-VALUES: each key mapped to the value after it.
        /// A key given twice maps to its last value.
        "(dict . keys-and-values)" => |vm: &mut Vm, a, n| from_args(vm, Kind::Dict, a, 0, n);
        /// Return #t if OBJ is a dict.
        "(dict? obj)" => |vm: &mut Vm, a, _| Ok(Value::bool(is_kind(arg(vm, a, 0), Kind::Dict)));
        /// Return the number of keys of DICT.
        "(dict-size dict)" => |vm: &mut Vm, a, _| Ok(int(count(dict_arg(arg(vm, a, 0), "dict-size")?) as i64));
        /// Return the value of KEY in DICT, or DEFAULT if it has none.
        /// Without DEFAULT, a missing KEY is an error.
        "(dict-ref dict key [default])" => dict_ref;
        /// Return #t if DICT has KEY.
        "(dict-contains? dict key)" => |vm: &mut Vm, a, _| {
            let d = dict_arg(arg(vm, a, 0), "dict-contains?")?;
            Ok(Value::bool(lookup(vm, d, arg(vm, a, 1)).is_some())) };
        /// Return DICT with KEY mapped to VALUE.
        "(dict-set dict key value)" => |vm: &mut Vm, a, _| {
            let d = dict_arg(arg(vm, a, 0), "dict-set")?;
            Ok(insert(vm, d, arg(vm, a, 1), arg(vm, a, 2))) };
        /// Return DICT without KEY.
        "(dict-delete dict key)" => |vm: &mut Vm, a, _| {
            let d = dict_arg(arg(vm, a, 0), "dict-delete")?;
            Ok(remove(vm, d, arg(vm, a, 1))) };
        /// Return a list of the keys of DICT.
        "(dict-keys dict)" => |vm: &mut Vm, a, _| { let d = dict_arg(arg(vm, a, 0), "dict-keys")?; to_list(vm, d, 0, 2, false) };
        /// Return a list of the values of DICT.
        "(dict-values dict)" => |vm: &mut Vm, a, _| { let d = dict_arg(arg(vm, a, 0), "dict-values")?; to_list(vm, d, 1, 2, false) };
        /// Return the entries of DICT as a list of (key . value).
        "(dict->alist dict)" => |vm: &mut Vm, a, _| { let d = dict_arg(arg(vm, a, 0), "dict->alist")?; to_list(vm, d, 0, 1, true) };
        /// Return a dict of the (key . value) pairs of ALIST.
        /// A key given twice maps to its first value, as `assoc` finds it.
        "(alist->dict alist)" => |vm: &mut Vm, a, _| from_list(vm, Kind::Dict, arg(vm, a, 0), "alist->dict");

        /// Return a set of OBJS.
        "(set . objs)" => |vm: &mut Vm, a, n| from_args(vm, Kind::Set, a, 0, n);
        /// Return #t if OBJ is a set.
        "(set? obj)" => |vm: &mut Vm, a, _| Ok(Value::bool(is_kind(arg(vm, a, 0), Kind::Set)));
        /// Return the number of elements of SET.
        "(set-size set)" => |vm: &mut Vm, a, _| Ok(int(count(set_arg(arg(vm, a, 0), "set-size")?) as i64));
        /// Return #t if SET has OBJ.
        "(set-contains? set obj)" => |vm: &mut Vm, a, _| {
            let s = set_arg(arg(vm, a, 0), "set-contains?")?;
            Ok(Value::bool(lookup(vm, s, arg(vm, a, 1)).is_some())) };
        /// Return SET with OBJ.
        "(set-add set obj)" => |vm: &mut Vm, a, _| {
            let s = set_arg(arg(vm, a, 0), "set-add")?;
            let x = arg(vm, a, 1);
            Ok(insert(vm, s, x, x)) };
        /// Return SET without OBJ.
        "(set-delete set obj)" => |vm: &mut Vm, a, _| {
            let s = set_arg(arg(vm, a, 0), "set-delete")?;
            Ok(remove(vm, s, arg(vm, a, 1))) };
        /// Return a list of the elements of SET.
        "(set->list set)" => |vm: &mut Vm, a, _| { let s = set_arg(arg(vm, a, 0), "set->list")?; to_list(vm, s, 0, 1, false) };
        /// Return a set of the elements of LIST.
        "(list->set list)" => |vm: &mut Vm, a, _| from_list(vm, Kind::Set, arg(vm, a, 0), "list->set");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::BTreeMap};

    type Hasher = fn(Value) -> u32;
    thread_local!(pub(super) static FAKE: Cell<Option<Hasher>> = const { Cell::new(None) });

    /// The helpers take any value, also on a thread that never had a
    /// world (and so has no heap to look into).
    #[test]
    fn helpers_take_any_value() {
        std::thread::spawn(|| {
            assert_eq!(count(Value::FALSE), 0);
            assert!(entries(Value::NIL).is_empty());
        })
        .join()
        .unwrap();
    }

    /// Every key in a few buckets: collision nodes everywhere.
    fn colliding(k: Value) -> u32 {
        ((k.as_int() % 3) as u32).wrapping_mul(0x8421_0841)
    }
    /// Hashes agreeing in their low bits: deep paths.
    fn deep(k: Value) -> u32 {
        (k.as_int() as u32) << 20
    }

    fn contents(d: Value) -> BTreeMap<i64, i64> {
        entries(d).chunks(2).map(|e| (e[0].as_int(), e[1].as_int())).collect()
    }

    /// Random insertions and deletions agree with a map, and dicts with
    /// the same entries are equal, however they were made.
    fn check(hash: Option<Hasher>) {
        FAKE.with(|f| f.set(hash));
        let mut vm = Vm::new();
        let mut model = BTreeMap::new();
        let mut d = vm.eval_source_raw("(dict)").unwrap();
        let mut rng = 0x2545_f491_4f6c_dd1du64;
        let root = vm.root(d);
        for step in 0..4000 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let k = (rng % 97) as i64;
            d = if rng >> 60 < 9 {
                model.insert(k, step);
                insert(&mut vm, root.get(), int(k), int(step))
            } else {
                model.remove(&k);
                remove(&mut vm, root.get(), int(k))
            };
            root.set(d);
            assert_eq!(count(d), model.len());
            if step % 97 == 0 {
                assert_eq!(contents(d), model);
                // The same entries in another order.
                let again =
                    model.iter().rev().fold(vm.eval_source_raw("(dict)").unwrap(), |acc, (&k, &v)| insert(&mut vm, acc, int(k), int(v)));
                assert!(equal(root.get(), again));
                assert_eq!(equal_hash(&mut vm, root.get()), equal_hash(&mut vm, again));
                // Made at once, a stale value of a key first.
                let mark = vm.scratch.len();
                if let Some((&k, _)) = model.iter().next() {
                    vm.scratch.extend([int(k), int(-1)]);
                }
                vm.scratch.extend(model.iter().flat_map(|(&k, &v)| [int(k), int(v)]));
                let bulk = from_scratch(&mut vm, Kind::Dict, mark);
                assert_eq!(count(bulk), model.len());
                assert!(equal(root.get(), bulk));
            }
        }
        for (&k, &v) in &model {
            assert_eq!(lookup(&mut vm, root.get(), int(k)).map(Value::as_int), Some(v));
        }
        FAKE.with(|f| f.set(None));
    }

    #[test]
    fn hashes() {
        check(None);
    }

    #[test]
    fn collisions() {
        check(Some(colliding));
    }

    #[test]
    fn deep_paths() {
        check(Some(deep));
    }
}
