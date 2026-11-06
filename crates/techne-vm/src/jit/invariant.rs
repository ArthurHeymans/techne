//! Find repeated indirect inline calls whose callee is a copy of an
//! immutable argument. Cache only code identity, never the closure/env.
use super::*;

pub(super) fn calls(job: &Job, sites: &[Site]) -> Vec<(usize, Reg)> {
    if job.closure_calls.is_empty() || sites.is_empty() || std::env::var("TECHNE_JIT_CALL_CACHE").as_deref() == Ok("0") {
        return Vec::new();
    }
    let ops = &job.ops;
    let effects: Vec<_> = ops.iter().map(|op| uses_defs(op, &job.captures)).collect();
    let defined: Vec<_> = effects.iter().flat_map(|(_, defs)| defs.iter().copied()).collect();
    let succs: Vec<_> = ops.iter().enumerate().map(|(pc, op)| successors(op, pc, ops.len())).collect();
    let mut preds = vec![Vec::new(); ops.len()];
    for (pc, next) in succs.iter().enumerate() {
        for &s in next {
            preds[s].push(pc);
        }
    }
    let repeated = |pc: usize| {
        let mut seen = vec![false; ops.len()];
        let mut todo = succs[pc].clone();
        while let Some(s) = todo.pop() {
            if s == pc {
                return true;
            }
            if !std::mem::replace(&mut seen[s], true) {
                todo.extend(&succs[s]);
            }
        }
        false
    };
    sites
        .iter()
        .enumerate()
        .filter_map(|(site, s)| {
            if s.call >= ops.len() || !job.closure_calls.contains_key(&(s.call as u32)) || !repeated(s.call) {
                return None;
            }
            let SiteClosure::Register(mut r) = s.closure else {
                return None;
            };
            let mut at = s.call;
            loop {
                if r < job.nparams && !defined.contains(&r) {
                    return Some((site, r));
                }
                // Follow copies backwards only along a single straight-line
                // predecessor. Stop at a join/backedge, not a guessed alias.
                if at == 0 || preds[at].as_slice() != [at - 1] {
                    return None;
                }
                at -= 1;
                if effects[at].1.contains(&r) {
                    match ops[at] {
                        Op::Mov { dst, src } if dst == r => r = src,
                        _ => return None,
                    }
                }
            }
        })
        .collect()
}
