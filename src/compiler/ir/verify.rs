use std::collections::HashSet;

use super::{Block, Func, InstKind, Terminator, cfg::Cfg};

pub fn verify_func(func: &Func) -> Result<(), Vec<String>> {
    let mut errs: Vec<String> = Vec::new();
    let n = func.blocks.len();

    // Basic structural checks per block terminator
    for (i, b) in func.blocks.iter().enumerate() {
        match &b.term {
            Terminator::Ret(_) => {}
            Terminator::Br(bb) => check_block_in_range(&mut errs, i, bb, n, "br target"),
            Terminator::CondBr {
                then_bb, else_bb, ..
            } => {
                check_block_in_range(&mut errs, i, then_bb, n, "condbr then");
                check_block_in_range(&mut errs, i, else_bb, n, "condbr else");
            }
            Terminator::Switch { default, cases, .. } => {
                check_block_in_range(&mut errs, i, default, n, "switch default");
                let mut seen: HashSet<i64> = HashSet::new();
                for (val, bb) in cases {
                    check_block_in_range(&mut errs, i, bb, n, "switch case");
                    if !seen.insert(*val) {
                        errs.push(format!("bb{}: duplicate switch case value {}", i, val));
                    }
                }
            }
            Terminator::Unreachable => {}
            Terminator::Throw(_) => {}
            Terminator::Panic(_) => {}
            Terminator::Invoke {
                ret,
                result,
                normal,
                unwind,
                ..
            } => {
                check_block_in_range(&mut errs, i, normal, n, "invoke normal");
                check_block_in_range(&mut errs, i, unwind, n, "invoke unwind");
                let is_void = matches!(ret, super::Ty::Void);
                if is_void {
                    if result.is_some() {
                        errs.push(format!(
                            "bb{}: invoke has void return but 'result' is Some",
                            i
                        ));
                    }
                } else if result.is_none() {
                    errs.push(format!(
                        "bb{}: invoke missing result for non-void return",
                        i
                    ));
                }
            }
            // PollReturn is a terminal - no block successors to verify
            Terminator::PollReturn { .. } => {}
        }
    }

    // Phi validation: arity must match predecessor count and predecessor blocks must align (order and set).
    // Also ensure phis come first in each block (no non-phi before a phi).
    let cfg = Cfg::build(func);
    for (i, b) in func.blocks.iter().enumerate() {
        let preds: HashSet<usize> = cfg.preds[i].iter().copied().collect();
        // Phis must appear contiguously at the top of the block
        let mut seen_non_phi = false;
        for inst in &b.insts {
            if let InstKind::Phi(ops) = &inst.kind {
                if seen_non_phi {
                    errs.push(format!("bb{}: phi appears after non-phi instruction", i));
                }
                if ops.len() != cfg.preds[i].len() {
                    errs.push(format!(
                        "bb{}: phi operand count {} does not match predecessor count {}",
                        i,
                        ops.len(),
                        cfg.preds[i].len()
                    ));
                }
                let op_blocks: HashSet<usize> = ops.iter().map(|(bb, _)| bb.0 as usize).collect();
                if op_blocks != preds {
                    errs.push(format!(
                        "bb{}: phi predecessor set {:?} does not match CFG preds {:?}",
                        i, op_blocks, preds
                    ));
                }
                // Require operand order to match cfg predecessor order
                for (k, (bb, _)) in ops.iter().enumerate() {
                    if let Some(exp) = cfg.preds[i].get(k).copied()
                        && (bb.0 as usize) != exp
                    {
                        errs.push(format!(
                            "bb{}: phi operand {} predecessor bb{} does not match cfg order bb{}",
                            i, k, bb.0, exp
                        ));
                        break;
                    }
                }
            } else {
                seen_non_phi = true;
            }
        }
    }

    // Critical edge detection: flag edges from multi-succ to multi-pred blocks
    for s in 0..func.blocks.len() {
        if cfg.succs[s].len() > 1 {
            for &t in &cfg.succs[s] {
                if cfg.preds[t].len() > 1 {
                    errs.push(format!(
                        "critical edge detected: bb{} -> bb{} (consider splitting)",
                        s, t
                    ));
                }
            }
        }
    }

    if errs.is_empty() { Ok(()) } else { Err(errs) }
}

fn check_block_in_range(errs: &mut Vec<String>, cur: usize, bb: &Block, n: usize, what: &str) {
    let ix = bb.0 as usize;
    if ix >= n {
        errs.push(format!(
            "bb{}: {} out of range: bb{} >= {}",
            cur, what, ix, n
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::super::{BlockData, Func, Inst, Linkage, Terminator, Ty, Value};
    use super::*;

    #[test]
    fn verify_demo_add_ok() {
        let f = super::super::demo_add_module()
            .funcs
            .into_iter()
            .next()
            .unwrap();
        assert!(verify_func(&f).is_ok());
    }

    #[test]
    fn verify_invoke_result_check() {
        // entry: invoke i64 @foo() to normal bb1 unwind bb2
        // bb1: ret void
        // bb2: <landing pad> ret void
        let entry = BlockData {
            name: "entry".into(),
            insts: vec![],
            term: Terminator::Invoke {
                callee: "foo".into(),
                args: vec![],
                ret: Ty::I64,
                result: None, // error: missing result
                normal: Block(1),
                unwind: Block(2),
            },
            span: None,
        };
        let bb1 = BlockData {
            name: "bb1".into(),
            insts: vec![],
            term: Terminator::Ret(None),
            span: None,
        };
        let bb2 = BlockData {
            name: "bb2".into(),
            insts: vec![Inst {
                result: Value(0),
                kind: InstKind::LandingPad {
                    catch_types: vec![],
                    is_catch_all: true,
                },
                span: None,
            }],
            term: Terminator::Ret(None),
            span: None,
        };
        let f = Func {
            name: "f".into(),
            params: vec![],
            ret: Ty::Void,
            blocks: vec![entry, bb1, bb2],
            linkage: Linkage::External,
            span: None,
        };
        assert!(verify_func(&f).is_err());
    }
}
