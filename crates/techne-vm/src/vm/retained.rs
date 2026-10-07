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

/// The objects found so far: each with the one it was reached from (`None`
/// for a root) and how.
#[derive(Default)]
struct Search {
    nodes: Vec<(Value, Option<usize>, String)>,
    seen: FxHashSet<u64>,
    queue: VecDeque<usize>,
}

impl Search {
    fn reach(&mut self, v: Value, from: Option<usize>, how: impl FnOnce() -> String) {
        if v.is_ptr() && self.seen.insert(v.bits()) {
            self.nodes.push((v, from, how()));
            self.queue.push_back(self.nodes.len() - 1);
        }
    }

    /// How node `i` is reached, root first; runs of the same step (the
    /// pairs of a long list) are counted.
    fn chain(&self, mut i: usize) -> Vec<String> {
        let mut steps = vec![];
        loop {
            let (_, from, how) = &self.nodes[i];
            steps.push(how.clone());
            match from {
                Some(f) => i = *f,
                None => break,
            }
        }
        steps.reverse();
        steps
            .chunk_by(|a, b| a == b)
            .map(|run| if run.len() == 1 { run[0].clone() } else { format!("{} (x{})", run[0], run.len()) })
            .collect()
    }
}

impl Vm {
    /// How `target` is kept alive: a root, then each reference on the way to
    /// it. `None` when nothing reaches it, or it is not a heap object.
    /// Registers, which often hold what a computation just used, are
    /// searched from last, and those in `asking` (the arguments of the
    /// question) not at all.
    pub fn why_retained(&self, target: Value, asking: std::ops::Range<usize>) -> Option<Vec<String>> {
        if !target.is_ptr() {
            return None;
        }
        let codes: FxHashMap<u64, &Code> = self.codes.iter().flatten().map(|c| (c.handle.get().bits(), &**c)).collect();
        let mut s = Search::default();
        self.roots(&mut s);
        self.search(&mut s, target, &codes).or_else(|| {
            self.registers(&mut s, asking);
            self.search(&mut s, target, &codes)
        })
    }

    fn search(&self, s: &mut Search, target: Value, codes: &FxHashMap<u64, &Code>) -> Option<Vec<String>> {
        while let Some(i) = s.queue.pop_front() {
            let v = s.nodes[i].0;
            if v == target {
                return Some(s.chain(i));
            }
            for (child, how) in self.references(v, codes) {
                s.reach(child, Some(i), || how);
            }
        }
        None
    }

    fn registers(&self, s: &mut Search, asking: std::ops::Range<usize>) {
        for (_, &v) in self.regs[..self.stack_top].iter().enumerate().filter(|(i, _)| !asking.contains(i)) {
            s.reach(v, None, || "a register of the running code".into());
        }
        for t in &self.tasks {
            for &v in &t.stack.regs[..t.stack.stack_top.min(t.stack.regs.len())] {
                s.reach(v, None, || "the stack of a task".into());
            }
        }
    }

    fn roots(&self, s: &mut Search) {
        for (g, &v) in self.globals.iter().enumerate().filter(|&(g, _)| self.global_rooted[g]) {
            s.reach(v, None, || format!("the global {} of {}", self.global_name(g as u32), self.module_name(self.global_module[g])));
        }
        for &code in &self.running_codes() {
            let code = unsafe { &*code };
            s.reach(code.handle.get(), None, || format!("the code of {}, running", code.name));
        }
        for v in self.scratch.iter().chain(&self.specials) {
            s.reach(*v, None, || "the runtime".into());
        }
        for cell in self.roots.iter().filter_map(|w| w.upgrade()) {
            s.reach(cell.get(), None, || "a reference held by Rust".into());
        }
    }

    /// What `v` refers to, as the collector sees it, and how.
    fn references(&self, v: Value, codes: &FxHashMap<u64, &Code>) -> Vec<(Value, String)> {
        let p = v.as_ptr();
        let (kind, len) = unsafe { (kind_of(p), len_of(p)) };
        let fields = |how: &dyn Fn(usize) -> String| (0..len).map(|i| (unsafe { field(p, i) }, how(i))).collect::<Vec<_>>();
        match kind {
            k if k == Kind::Closure as u8 => {
                let code = unsafe { &*field(p, 0).as_untraced_ptr::<Code>() };
                let mut refs = vec![(code.handle.get(), format!("the code of {}", code.name))];
                refs.extend((1..len).map(|i| (unsafe { field(p, i) }, format!("a value {} captured", code.name))));
                refs
            }
            k if k == Kind::Code as u8 => {
                let Some(code) = codes.get(&v.bits()) else { return vec![] };
                let mut refs = fields(&|i| {
                    if i < code.consts.len() { format!("a constant of {}", code.name) } else { format!("code {} uses", code.name) }
                });
                let callees = code.jit.callees.get().into_iter().flatten();
                refs.extend(callees.map(|&v| (v, format!("a closure the machine code of {} calls", code.name))));
                // The globals of the retired modules it uses, which live as
                // long as it.
                for (m, globals) in self.retired.iter().filter(|(m, _)| code.uses.contains(m)) {
                    refs.extend(globals.iter().map(|&g| {
                        let how =
                            format!("the global {} of {} (retired), which {} uses", self.global_name(g), self.module_name(*m), code.name);
                        (self.globals[g as usize], how)
                    }));
                }
                refs
            }
            // Only values: a weak table does not keep its keys.
            k if k == Kind::Ephemerons as u8 => {
                (1..len).step_by(2).map(|i| (unsafe { field(p, i) }, "a value in a weak hash table".to_string())).collect()
            }
            k if k < Kind::String as u8 => {
                let what = match k {
                    k if k == Kind::Pair as u8 => "a pair",
                    k if k == Kind::Vector as u8 => "a vector",
                    k if k == Kind::Box as u8 => "a box (an assigned variable)",
                    k if k == Kind::Table as u8 => "a hash table",
                    k if k == Kind::Record as u8 => "a record",
                    k if k == Kind::Ratio as u8 || k == Kind::Complex as u8 => "a number",
                    _ => "a record type",
                };
                fields(&|_| format!("in {what}"))
            }
            _ => vec![],
        }
    }
}
