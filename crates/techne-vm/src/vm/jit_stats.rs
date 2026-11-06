//! Opt-in diagnostics at the interpreter/JIT boundary, not instrumentation
//! of generated code. Fixed storage holds no guest values or code pointers.
use super::Vm;
use crate::{
    alloc::{Budget, Reservation},
    code::{Code, Op},
    heap::{Kind, is_kind},
    jit,
    value::Value,
};

const SITES: usize = 64;
const NAME: usize = 80;

struct Site {
    name: [u8; NAME],
    len: usize,
    file: u32,
    pos: u32,
    pc: usize,
    status: u32,
    op: Option<Op>,
    kinds: [&'static str; 2],
    count: u64,
}

pub(super) struct Stats {
    statuses: [u64; 10],
    pub helpers: [u64; 3],
    sites: [Option<Site>; SITES],
    overflow: u64,
    _charged: Option<Reservation>,
}

impl Stats {
    pub fn new(budget: &Budget) -> Option<Box<Self>> {
        let _account = budget.enter();
        let charged = budget.charge(size_of::<Self>()).ok()?;
        Some(Box::new(Self { statuses: [0; 10], helpers: [0; 3], sites: std::array::from_fn(|_| None), overflow: 0, _charged: charged }))
    }

    fn record(&mut self, code: &Code, pc: usize, status: u32, op: Option<Op>, kinds: [&'static str; 2]) {
        self.statuses[status as usize] += 1;
        // This counts a native run returning, not each recursive native call.
        if status == jit::RETURNED {
            return;
        }
        let mut len = code.name.len().min(NAME);
        while !code.name.is_char_boundary(len) {
            len -= 1;
        }
        let name = &code.name.as_bytes()[..len];
        for slot in &mut self.sites {
            if let Some(site) = slot {
                if site.name[..site.len] == *name
                    && site.file == code.file
                    && site.pos == code.pos
                    && site.pc == pc
                    && site.status == status
                    && site.kinds == kinds
                {
                    site.count += 1;
                    return;
                }
            } else {
                let mut stored = [0; NAME];
                stored[..len].copy_from_slice(name);
                *slot = Some(Site { name: stored, len, file: code.file, pos: code.pos, pc, status, op, kinds, count: 1 });
                return;
            }
        }
        self.overflow += 1;
    }

    fn report(&self) {
        eprintln!(
            "jit-stats: boundary-statuses {:?}; helpers [step, call, tail] {:?}; unlisted {}",
            self.statuses, self.helpers, self.overflow
        );
        for site in self.sites.iter().flatten() {
            let name = std::str::from_utf8(&site.name[..site.len]).expect("a character boundary");
            eprintln!(
                "jit-exit: {} {} file={} pos={} pc={} status={} {:?} {:?}",
                site.count, name, site.file, site.pos, site.pc, site.status, site.op, site.kinds
            );
        }
    }
}

fn kind(v: Value) -> &'static str {
    if v.is_int() {
        "fixnum"
    } else if v.is_float() {
        "float"
    } else if is_kind(v, Kind::BigInt) {
        "bignum"
    } else if is_kind(v, Kind::Pair) {
        "pair"
    } else if is_kind(v, Kind::Vector) {
        "vector"
    } else {
        "other"
    }
}

impl Vm {
    /// Called only after registers have been published/rederived, before
    /// executing a slow instruction or resuming the interpreter.
    pub(super) fn note_jit_exit(&mut self, code: *const Code, pc: usize, bp: usize, status: u32) {
        let Some(stats) = &mut self.jit_stats else { return };
        let code = unsafe { &*code };
        let at = if matches!(status, jit::ERROR | jit::WAIT | jit::RESUME | jit::RET_MOVED) { pc.saturating_sub(1) } else { pc };
        let op = code.jit.ops.get().and_then(|ops| ops.get(at)).copied();
        let read = |r: u16| self.regs.get(bp + r as usize).copied().map_or("missing", kind);
        // Operand classes are meaningful for STEP only; other exits can
        // have overwritten the destination with a call's result already.
        let kinds = if status == jit::STEP {
            match op {
                Some(
                    Op::Add { a, b, .. }
                    | Op::Sub { a, b, .. }
                    | Op::Mul { a, b, .. }
                    | Op::Quo { a, b, .. }
                    | Op::Rem { a, b, .. }
                    | Op::Mod { a, b, .. }
                    | Op::Lt { a, b, .. }
                    | Op::Le { a, b, .. }
                    | Op::NumEq { a, b, .. }
                    | Op::JNLt { a, b, .. }
                    | Op::JNLe { a, b, .. }
                    | Op::JNNumEq { a, b, .. },
                ) => [read(a), read(b)],
                Some(Op::AddI { a, .. } | Op::JNLtI { a, .. } | Op::JNGtI { a, .. } | Op::JNEqI { a, .. }) => [read(a), "fixnum"],
                Some(Op::VRef { v, i, .. } | Op::VSet { v, i, .. }) => [read(v), read(i)],
                Some(Op::Car { a, .. } | Op::Cdr { a, .. }) => [read(a), "-"],
                _ => ["-", "-"],
            }
        } else {
            ["-", "-"]
        };
        stats.record(code, pc, status, op, kinds);
    }

    #[inline]
    pub(crate) fn note_jit_helper(&mut self, helper: usize) {
        if let Some(stats) = &mut self.jit_stats {
            stats.helpers[helper] += 1;
        }
    }

    pub(crate) fn report_jit_stats(&self) {
        if let Some(stats) = &self.jit_stats {
            stats.report();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_is_counted_without_changing_the_result() {
        let mut vm = Vm::new();
        let _world = vm.enter_account();
        vm.jit_stats = Stats::new(vm.regs.budget());
        vm.set_jit(Some(1));
        let value = vm.eval_source("(define (overflow x) (+ x 1)) (overflow 140737488355327)").unwrap();
        assert_eq!(vm.repr(&value), "140737488355328");
        let stats = vm.jit_stats.as_ref().unwrap();
        assert!(stats.statuses[jit::STEP as usize] > 0);
        assert!(stats.sites.iter().flatten().any(|s| s.status == jit::STEP && s.kinds == ["fixnum", "fixnum"]));
    }

    #[test]
    fn site_storage_is_bounded_and_unicode_names_are_truncated_safely() {
        let mut vm = Vm::new();
        let _world = vm.enter_account();
        vm.set_jit(None);
        let name = format!("{}🦀", "a".repeat(NAME - 1));
        let f = vm.eval_source_raw(&format!("(define ({name} x) x) {name}")).unwrap();
        let code = vm.closure_code(f);
        vm.jit_stats = Stats::new(vm.regs.budget());
        for pc in 0..SITES + 10 {
            vm.note_jit_exit(code, pc, 0, jit::EXIT);
        }
        let stats = vm.jit_stats.as_ref().unwrap();
        assert_eq!(stats.sites.iter().flatten().count(), SITES);
        assert_eq!(stats.overflow, 10);
        assert_eq!(stats.statuses[jit::EXIT as usize], (SITES + 10) as u64);
        for site in stats.sites.iter().flatten() {
            assert_eq!(site.len, NAME - 1);
            assert!(std::str::from_utf8(&site.name[..site.len]).is_ok());
        }
    }
}
