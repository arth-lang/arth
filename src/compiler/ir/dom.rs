use std::collections::HashSet;

use super::Func;
use super::cfg::Cfg;

#[derive(Clone, Debug)]
pub struct DomInfo {
    pub idom: Vec<Option<usize>>, // immediate dominator (None for unreachable); idom[entry] = Some(entry)
    pub tree: Vec<Vec<usize>>,    // dominator tree children lists
    pub frontier: Vec<Vec<usize>>, // dominance frontiers
}

impl DomInfo {
    pub fn compute(func: &Func, cfg: &Cfg) -> Self {
        let n = func.blocks.len();
        let mut idom: Vec<Option<usize>> = vec![None; n];
        if n == 0 {
            return Self {
                idom,
                tree: vec![],
                frontier: vec![],
            };
        }

        // Reverse postorder; entry first
        let rpo = &cfg.rpo;
        let mut rpo_index = vec![usize::MAX; n];
        for (i, &b) in rpo.iter().enumerate() {
            rpo_index[b] = i;
        }

        let entry = rpo.first().copied().unwrap_or(0);
        idom[entry] = Some(entry);

        // Iterative Cooper/Harvey/Kennedy algorithm
        let mut changed = true;
        while changed {
            changed = false;
            for &b in rpo.iter().skip(1) {
                // skip entry
                if !cfg.reachable[b] {
                    continue;
                }
                // intersect idoms of predecessors that have idom defined
                let mut new_idom: Option<usize> = None;
                for &p in &cfg.preds[b] {
                    if !cfg.reachable[p] {
                        continue;
                    }
                    if idom[p].is_some() {
                        new_idom = Some(match new_idom {
                            None => p,
                            Some(q) => intersect(p, q, &idom, &rpo_index),
                        });
                    }
                }
                if new_idom.is_some() && idom[b] != new_idom {
                    idom[b] = new_idom;
                    changed = true;
                }
            }
        }

        let mut tree: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (b, id) in idom.iter().enumerate() {
            if let Some(p) = *id
                && p != b
            {
                tree[p].push(b);
            }
        }

        let mut frontier: Vec<Vec<usize>> = vec![Vec::new(); n];
        for b in 0..n {
            if !cfg.reachable[b] {
                continue;
            }
            if cfg.preds[b].len() >= 2 {
                for &p in &cfg.preds[b] {
                    let mut runner = p;
                    while Some(runner) != idom[b] {
                        frontier[runner].push(b);
                        if let Some(id) = idom[runner] {
                            runner = id;
                        } else {
                            break;
                        }
                    }
                }
            }
        }
        // Deduplicate frontier entries
        for f in &mut frontier {
            let mut set = HashSet::new();
            f.retain(|x| set.insert(*x));
        }

        Self {
            idom,
            tree,
            frontier,
        }
    }

    #[allow(dead_code)]
    pub fn dominates(&self, a: usize, b: usize) -> bool {
        // Does a dominate b? a dominates a; if b unreachable (idom[entry] is None) return false
        let mut x = Some(b);
        while let Some(v) = x {
            if v == a {
                return true;
            }
            x = self.idom[v];
            if x == Some(v) {
                break;
            } // reached entry
        }
        false
    }

    pub fn dump(&self) -> String {
        let mut out = String::new();
        out.push_str("idoms:\n");
        for (b, id) in self.idom.iter().enumerate() {
            match id {
                Some(p) => out.push_str(&format!("  bb{b} <- bb{p}\n")),
                None => out.push_str(&format!("  bb{b} <- none\n")),
            }
        }
        out.push_str("dom-tree:\n");
        for (p, ch) in self.tree.iter().enumerate() {
            let kids = ch
                .iter()
                .map(|x| format!("bb{x}"))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("  bb{p}: [{kids}]\n"));
        }
        out.push_str("frontiers:\n");
        for (b, df) in self.frontier.iter().enumerate() {
            let items = df
                .iter()
                .map(|x| format!("bb{x}"))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("  bb{b}: [{items}]\n"));
        }
        out
    }

    // Verify basic dominance properties on a computed DomInfo against a CFG.
    // Checks:
    // - Entry's idom is itself; every other reachable node has idom != itself and is Some.
    // - Each dom-tree edge parent->child satisfies parent dominates child.
    // - Dominance frontiers match the standard definition (sanity check).
    #[allow(dead_code)]
    pub fn verify_properties(&self, cfg: &Cfg) -> Result<(), Vec<String>> {
        let mut errs: Vec<String> = Vec::new();
        let n = self.idom.len();
        if n == 0 {
            return Ok(());
        }
        let entry = cfg.rpo.first().copied().unwrap_or(0);

        for b in 0..n {
            if !cfg.reachable[b] {
                continue;
            }
            match self.idom[b] {
                Some(p) => {
                    if b == entry {
                        if p != b {
                            errs.push(format!("entry idom must be itself: idom[bb{b}] = bb{p}"));
                        }
                    } else if p == b {
                        errs.push(format!("idom[bb{b}] must not equal itself"));
                    }
                }
                None => errs.push(format!("reachable bb{} has no immediate dominator", b)),
            }
        }

        // Parent dominates child
        for (p, kids) in self.tree.iter().enumerate() {
            for &c in kids {
                if !self.dominates(p, c) {
                    errs.push(format!(
                        "dom-tree violation: bb{} does not dominate child bb{}",
                        p, c
                    ));
                }
            }
        }

        // Recompute frontiers from the definition and compare.
        let mut expected: Vec<Vec<usize>> = vec![Vec::new(); n];
        for b in 0..n {
            if !cfg.reachable[b] {
                continue;
            }
            if cfg.preds[b].len() >= 2 {
                for &p in &cfg.preds[b] {
                    let mut runner = p;
                    while Some(runner) != self.idom[b] {
                        expected[runner].push(b);
                        if let Some(id) = self.idom[runner] {
                            runner = id;
                        } else {
                            break;
                        }
                    }
                }
            }
        }
        // Dedup expected
        for v in &mut expected {
            let mut set = HashSet::new();
            v.retain(|x| set.insert(*x));
        }

        for (b, rhs_v) in expected.iter().enumerate() {
            let lhs: HashSet<usize> = self.frontier[b].iter().copied().collect();
            let rhs: HashSet<usize> = rhs_v.iter().copied().collect();
            if lhs != rhs {
                errs.push(format!(
                    "dominance frontier mismatch at bb{}: computed={:?} expected={:?}",
                    b, lhs, rhs
                ));
            }
        }

        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }
}

fn intersect(mut b1: usize, mut b2: usize, idom: &[Option<usize>], rpo_index: &[usize]) -> usize {
    let mut i1 = rpo_index[b1];
    let mut i2 = rpo_index[b2];
    while b1 != b2 {
        while i1 > i2 {
            b1 = idom[b1].expect("idom must exist");
            i1 = rpo_index[b1];
        }
        while i2 > i1 {
            b2 = idom[b2].expect("idom must exist");
            i2 = rpo_index[b2];
        }
    }
    b1
}

#[cfg(test)]
mod tests {
    use super::super::cfg::Cfg;
    use super::super::{Block, BlockData, Func, Linkage, Terminator, Ty};
    use super::*;

    #[test]
    fn dom_diamond() {
        // b0 -> b1, b2; b1 -> b3; b2 -> b3
        let b0 = BlockData {
            name: "entry".into(),
            insts: vec![],
            term: Terminator::CondBr {
                cond: super::super::Value(0),
                then_bb: Block(1),
                else_bb: Block(2),
            },
            span: None,
        };
        let b1 = BlockData {
            name: "b1".into(),
            insts: vec![],
            term: Terminator::Br(Block(3)),
            span: None,
        };
        let b2 = BlockData {
            name: "b2".into(),
            insts: vec![],
            term: Terminator::Br(Block(3)),
            span: None,
        };
        let b3 = BlockData {
            name: "join".into(),
            insts: vec![],
            term: Terminator::Ret(None),
            span: None,
        };
        let f = Func {
            name: "f".into(),
            params: vec![Ty::I64],
            ret: Ty::I64,
            blocks: vec![b0, b1, b2, b3],
            linkage: Linkage::External,
            span: None,
        };
        let cfg = Cfg::build(&f);
        let dom = DomInfo::compute(&f, &cfg);
        assert_eq!(dom.idom[0], Some(0));
        assert_eq!(dom.idom[1], Some(0));
        assert_eq!(dom.idom[2], Some(0));
        assert_eq!(dom.idom[3], Some(0));
        // Frontier: join appears in frontiers of the branches (b1, b2)
        assert!(dom.frontier[1].contains(&3));
        assert!(dom.frontier[2].contains(&3));
    }

    #[test]
    fn dom_verifier_diamond() {
        // b0 -> b1, b2; b1 -> b3; b2 -> b3
        let b0 = BlockData {
            name: "entry".into(),
            insts: vec![],
            term: Terminator::CondBr {
                cond: super::super::Value(0),
                then_bb: Block(1),
                else_bb: Block(2),
            },
            span: None,
        };
        let b1 = BlockData {
            name: "b1".into(),
            insts: vec![],
            term: Terminator::Br(Block(3)),
            span: None,
        };
        let b2 = BlockData {
            name: "b2".into(),
            insts: vec![],
            term: Terminator::Br(Block(3)),
            span: None,
        };
        let b3 = BlockData {
            name: "join".into(),
            insts: vec![],
            term: Terminator::Ret(None),
            span: None,
        };
        let f = Func {
            name: "f".into(),
            params: vec![Ty::I64],
            ret: Ty::I64,
            blocks: vec![b0, b1, b2, b3],
            linkage: Linkage::External,
            span: None,
        };
        let cfg = Cfg::build(&f);
        let dom = DomInfo::compute(&f, &cfg);
        assert!(dom.verify_properties(&cfg).is_ok());
    }

    #[test]
    fn dom_verifier_linear() {
        // b0 -> b1 -> b2
        let b0 = BlockData {
            name: "entry".into(),
            insts: vec![],
            term: Terminator::Br(Block(1)),
            span: None,
        };
        let b1 = BlockData {
            name: "mid".into(),
            insts: vec![],
            term: Terminator::Br(Block(2)),
            span: None,
        };
        let b2 = BlockData {
            name: "exit".into(),
            insts: vec![],
            term: Terminator::Ret(None),
            span: None,
        };
        let f = Func {
            name: "f".into(),
            params: vec![],
            ret: Ty::Void,
            blocks: vec![b0, b1, b2],
            linkage: Linkage::External,
            span: None,
        };
        let cfg = Cfg::build(&f);
        let dom = DomInfo::compute(&f, &cfg);
        assert!(dom.verify_properties(&cfg).is_ok());
    }
}
