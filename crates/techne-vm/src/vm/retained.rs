//! `why-retained` (PLAN.md, Stage 1, language step 7): the chain of
//! references that keeps an object alive, from a root of the VM through
//! the objects and code between, to the object. It follows what the
//! collector follows, so an object it finds no chain to is freed by the
//! next full collection.

use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};

use super::{Code, Vm};
use crate::{
    heap::{Kind, field, kind_of, len_of},
    value::Value,
};

/// Compact edge label stored during BFS without formatting strings until a
/// target chain is actually reconstructed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Edge {
    Global(u32),
    RunningCode(*const Code),
    Runtime,
    HostRoot,
    Register,
    TaskStack,
    ClosureCode(*const Code),
    ClosureCapture(*const Code),
    CodeConst(*const Code),
    CodeUses(*const Code),
    JitCallee(*const Code),
    JitDep(*const Code, u32),
    RetiredGlobal(*const Code, u32, u32),
    EphemeronValue,
    Container(u8),
}

impl Edge {
    fn format(self, vm: &Vm) -> String {
        match self {
            Edge::Global(g) => format!("the global {} of {}", vm.global_name(g), vm.module_name(vm.global_module[g as usize])),
            Edge::RunningCode(code) => {
                // SAFETY: `code` points into live `vm.codes` for the duration of the scan.
                format!("the code of {}, running", unsafe { &*code }.name)
            }
            Edge::Runtime => "the runtime".into(),
            Edge::HostRoot => "a reference held by Rust".into(),
            Edge::Register => "a register of the running code".into(),
            Edge::TaskStack => "the stack of a task".into(),
            Edge::ClosureCode(code) => format!("the code of {}", unsafe { &*code }.name),
            Edge::ClosureCapture(code) => format!("a value {} captured", unsafe { &*code }.name),
            Edge::CodeConst(code) => format!("a constant of {}", unsafe { &*code }.name),
            Edge::CodeUses(code) => format!("code {} uses", unsafe { &*code }.name),
            Edge::JitCallee(code) => format!("a closure the machine code of {} calls", unsafe { &*code }.name),
            Edge::JitDep(code, dep_id) => {
                format!("the code of {}, which the machine code of {} uses", vm.code(dep_id).name, unsafe { &*code }.name)
            }
            Edge::RetiredGlobal(code, m, g) => {
                format!("the global {} of {} (retired), which {} uses", vm.global_name(g), vm.module_name(m), unsafe { &*code }.name)
            }
            Edge::EphemeronValue => "a value in a weak hash table".into(),
            Edge::Container(k) => {
                let what = match k {
                    k if k == Kind::Pair as u8 => "a pair",
                    k if k == Kind::Vector as u8 => "a vector",
                    k if k == Kind::Box as u8 => "a box (an assigned variable)",
                    k if k == Kind::Table as u8 => "a hash table",
                    k if k == Kind::Dict as u8 => "a dict",
                    k if k == Kind::Set as u8 => "a set",
                    k if k == Kind::Node as u8 => "a dict or set",
                    k if k == Kind::Record as u8 => "a record",
                    k if k == Kind::Ratio as u8 || k == Kind::Complex as u8 => "a number",
                    _ => "a record type",
                };
                format!("in {what}")
            }
        }
    }
}

/// The objects found so far: each with the one it was reached from (`None`
/// for a root) and the compact edge taken.
struct Search {
    nodes: Vec<(Value, Option<usize>, Edge)>,
    seen: FxHashSet<u64>,
    queue: VecDeque<usize>,
    max_nodes: usize,
    truncated: bool,
}

impl Search {
    fn new(max_nodes: usize) -> Self {
        Self { nodes: Vec::new(), seen: FxHashSet::default(), queue: VecDeque::new(), max_nodes, truncated: false }
    }

    fn reach(&mut self, v: Value, from: Option<usize>, edge: Edge) {
        if !v.is_ptr() || self.seen.contains(&v.bits()) {
            return;
        }
        if self.nodes.len() >= self.max_nodes {
            self.truncated = true;
            return;
        }
        self.seen.insert(v.bits());
        self.nodes.push((v, from, edge));
        self.queue.push_back(self.nodes.len() - 1);
    }

    /// How node `i` is reached, root first; runs of the same step (the
    /// pairs of a long list) are counted before formatting strings.
    fn chain(&self, vm: &Vm, mut i: usize, max_chain: usize) -> (Vec<String>, bool) {
        let mut raw = Vec::new();
        loop {
            let (_, from, edge) = self.nodes[i];
            raw.push(edge);
            match from {
                Some(f) => i = f,
                None => break,
            }
        }
        raw.reverse();
        let runs: Vec<(Edge, usize)> = raw.chunk_by(|a, b| a == b).map(|run| (run[0], run.len())).collect();
        if max_chain > 0 && runs.len() > max_chain {
            let keep = max_chain.saturating_sub(1);
            let omitted = runs.len() - keep;
            let mut out = Vec::with_capacity(max_chain);
            for &(edge, count) in &runs[..keep] {
                let label = edge.format(vm);
                out.push(if count == 1 { label } else { format!("{label} (x{count})") });
            }
            out.push(format!("... ({omitted} more steps omitted)"));
            (out, true)
        } else {
            let out = runs
                .into_iter()
                .map(|(edge, count)| {
                    let label = edge.format(vm);
                    if count == 1 { label } else { format!("{label} (x{count})") }
                })
                .collect();
            (out, false)
        }
    }
}

impl Vm {
    /// How `target` is kept alive: a root, then each reference on the way to
    /// it. `None` when nothing reaches it, or it is not a heap object.
    /// Registers, which often hold what a computation just used, are
    /// searched from last, and those in `asking` (the arguments of the
    /// question) not at all.
    pub(crate) fn why_retained_raw(&self, target: Value, asking: std::ops::Range<usize>) -> Option<Vec<String>> {
        if !target.is_ptr() {
            return None;
        }
        let codes: FxHashMap<u64, &Code> = self.live_code_refs().map(|c| (c.handle.get().bits(), c)).collect();
        let mut s = Search::new(usize::MAX);
        self.roots(&mut s);
        self.search_one(&mut s, target, &codes).or_else(|| {
            self.registers(&mut s, asking);
            self.search_one(&mut s, target, &codes)
        })
    }

    /// Bounded single-pass BFS retention check for multiple targets at once.
    /// Returns one optional retention chain per input target plus a boolean
    /// indicating whether the node budget or any chain length cap was hit.
    pub(crate) fn why_retained_many_raw(&self, targets: &[Value], max_nodes: usize, max_chain: usize) -> (Vec<Option<Vec<String>>>, bool) {
        let mut results = vec![None; targets.len()];
        let mut wanted: FxHashMap<u64, Vec<usize>> = FxHashMap::default();
        for (idx, &v) in targets.iter().enumerate() {
            if v.is_ptr() {
                wanted.entry(v.bits()).or_default().push(idx);
            }
        }
        if wanted.is_empty() {
            return (results, false);
        }
        let codes: FxHashMap<u64, &Code> = self.live_code_refs().map(|c| (c.handle.get().bits(), c)).collect();
        let mut s = Search::new(max_nodes);
        let mut any_chain_truncated = false;
        self.roots(&mut s);
        self.search_many(&mut s, &mut wanted, &mut results, &codes, max_chain, &mut any_chain_truncated);
        if !wanted.is_empty() && !s.truncated {
            self.registers(&mut s, 0..0);
            self.search_many(&mut s, &mut wanted, &mut results, &codes, max_chain, &mut any_chain_truncated);
        }
        let truncated = any_chain_truncated || (s.truncated && !wanted.is_empty());
        (results, truncated)
    }

    fn search_one(&self, s: &mut Search, target: Value, codes: &FxHashMap<u64, &Code>) -> Option<Vec<String>> {
        while let Some(i) = s.queue.pop_front() {
            let v = s.nodes[i].0;
            if v == target {
                return Some(s.chain(self, i, usize::MAX).0);
            }
            self.visit_references(v, i, s, codes);
        }
        None
    }

    fn search_many(
        &self,
        s: &mut Search,
        wanted: &mut FxHashMap<u64, Vec<usize>>,
        results: &mut [Option<Vec<String>>],
        codes: &FxHashMap<u64, &Code>,
        max_chain: usize,
        any_chain_truncated: &mut bool,
    ) {
        while let Some(i) = s.queue.pop_front() {
            let v = s.nodes[i].0;
            if let Some(indices) = wanted.remove(&v.bits()) {
                let (chain, chain_truncated) = s.chain(self, i, max_chain);
                *any_chain_truncated |= chain_truncated;
                for idx in indices {
                    results[idx] = Some(chain.clone());
                }
                if wanted.is_empty() {
                    return;
                }
            }
            self.visit_references(v, i, s, codes);
        }
    }

    fn registers(&self, s: &mut Search, asking: std::ops::Range<usize>) {
        for (_, &v) in self.regs[..self.stack_top].iter().enumerate().filter(|(i, _)| !asking.contains(i)) {
            s.reach(v, None, Edge::Register);
        }
        for t in &self.tasks {
            for &v in &t.stack.regs[..t.stack.stack_top.min(t.stack.regs.len())] {
                s.reach(v, None, Edge::TaskStack);
            }
        }
    }

    fn roots(&self, s: &mut Search) {
        for (g, &v) in self.globals.iter().enumerate().filter(|&(g, _)| self.global_rooted[g]) {
            s.reach(v, None, Edge::Global(g as u32));
        }
        for &code in &self.running_codes() {
            let code_ref = unsafe { &*code };
            s.reach(code_ref.handle.get(), None, Edge::RunningCode(code));
        }
        for v in self.scratch.iter().chain(&self.specials) {
            s.reach(*v, None, Edge::Runtime);
        }
        for cell in self.roots.borrow().iter().filter_map(|w| w.upgrade()) {
            s.reach(cell.get(), None, Edge::HostRoot);
        }
    }

    /// Push every reference from `v` (as the collector sees it) into `s`.
    fn visit_references(&self, v: Value, from: usize, s: &mut Search, codes: &FxHashMap<u64, &Code>) {
        let p = v.as_ptr();
        let kind = unsafe { kind_of(p) };
        // Fields only of what has them: a string's length is in bytes.
        let len = if kind < Kind::String as u8 { unsafe { len_of(p) } } else { 0 };
        match kind {
            k if k == Kind::Closure as u8 => {
                let code_ptr = self.closure_code(v);
                let code = unsafe { &*code_ptr };
                s.reach(code.handle.get(), Some(from), Edge::ClosureCode(code_ptr));
                for i in 1..len {
                    s.reach(unsafe { field(p, i) }, Some(from), Edge::ClosureCapture(code_ptr));
                }
            }
            k if k == Kind::Code as u8 => {
                let Some(&code) = codes.get(&v.bits()) else { return };
                let code_ptr = code as *const Code;
                for i in 0..len {
                    let edge = if i < code.consts.len() { Edge::CodeConst(code_ptr) } else { Edge::CodeUses(code_ptr) };
                    s.reach(unsafe { field(p, i) }, Some(from), edge);
                }
                for &callee in code.jit.callees.get().into_iter().flatten() {
                    s.reach(callee, Some(from), Edge::JitCallee(code_ptr));
                }
                // As the collector keeps them (`Vm::retained`).
                for &id in code.jit.deps.borrow().iter() {
                    if self.code_live(id) {
                        let dep = self.code(id);
                        s.reach(dep.handle.get(), Some(from), Edge::JitDep(code_ptr, id));
                    }
                }
                // The globals of the retired modules it uses, which live as
                // long as it.
                for (m, globals) in self.retired.iter().filter(|(m, _)| code.uses.contains(m)) {
                    for &g in globals {
                        s.reach(self.globals[g as usize], Some(from), Edge::RetiredGlobal(code_ptr, *m, g));
                    }
                }
            }
            // Only values: a weak table does not keep its keys.
            k if k == Kind::Ephemerons as u8 => {
                for i in (1..len).step_by(2) {
                    s.reach(unsafe { field(p, i) }, Some(from), Edge::EphemeronValue);
                }
            }
            k if k < Kind::String as u8 => {
                let norm = if k == Kind::Complex as u8 { Kind::Ratio as u8 } else { k };
                let edge = Edge::Container(norm);
                for i in 0..len {
                    s.reach(unsafe { field(p, i) }, Some(from), edge);
                }
            }
            _ => {}
        }
    }
}
