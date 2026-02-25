use std::collections::VecDeque;

use super::{Block, Func, Terminator};

#[derive(Clone, Debug)]
pub struct Cfg {
    pub succs: Vec<Vec<usize>>, // block index -> successors
    pub preds: Vec<Vec<usize>>, // block index -> predecessors
    pub reachable: Vec<bool>,   // block index -> reachable from entry
    pub rpo: Vec<usize>,        // reverse postorder of reachable blocks (entry first)
}

impl Cfg {
    pub fn build(func: &Func) -> Self {
        let n = func.blocks.len();
        let mut succs: Vec<Vec<usize>> = vec![Vec::new(); n];

        for (i, b) in func.blocks.iter().enumerate() {
            let mut s: Vec<usize> = Vec::new();
            match &b.term {
                Terminator::Ret(_) => {}
                Terminator::Br(bb) => {
                    if let Some(ix) = block_index(bb, n) {
                        s.push(ix);
                    }
                }
                Terminator::CondBr {
                    then_bb, else_bb, ..
                } => {
                    if let Some(ix) = block_index(then_bb, n) {
                        s.push(ix);
                    }
                    if let Some(ix) = block_index(else_bb, n) {
                        s.push(ix);
                    }
                }
                Terminator::Switch { default, cases, .. } => {
                    if let Some(ix) = block_index(default, n) {
                        s.push(ix);
                    }
                    for (_, bb) in cases {
                        if let Some(ix) = block_index(bb, n) {
                            s.push(ix);
                        }
                    }
                }
                Terminator::Unreachable => {}
                Terminator::Throw(_) => {}
                Terminator::Panic(_) => {}
                Terminator::Invoke { normal, unwind, .. } => {
                    if let Some(ix) = block_index(normal, n) {
                        s.push(ix);
                    }
                    if let Some(ix) = block_index(unwind, n) {
                        s.push(ix);
                    }
                }
                // PollReturn is a terminal - no successors
                Terminator::PollReturn { .. } => {}
            }
            succs[i] = s;
        }

        let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, ss) in succs.iter().enumerate() {
            for &t in ss {
                preds[t].push(i);
            }
        }

        let reachable = mark_reachable(&succs, 0);
        let rpo = compute_rpo(&succs, &reachable, 0);

        Self {
            succs,
            preds,
            reachable,
            rpo,
        }
    }

    pub fn dump(&self, func: &Func) -> String {
        let mut out = String::new();
        for i in 0..func.blocks.len() {
            let name = func.blocks[i].name.as_str();
            let preds = join_list(&self.preds[i]);
            let succs = join_list(&self.succs[i]);
            let r = if self.reachable[i] {
                "reachable"
            } else {
                "unreachable"
            };
            out.push_str(&format!(
                "bb{idx}({name}) {r}: preds=[{preds}] succs=[{succs}]\n",
                idx = i,
                name = name,
                r = r,
                preds = preds,
                succs = succs
            ));
        }
        out.push_str("rpo: ");
        out.push_str(&join_list(&self.rpo));
        out.push('\n');
        out
    }
}

fn block_index(b: &Block, n: usize) -> Option<usize> {
    let ix = b.0 as usize;
    if ix < n { Some(ix) } else { None }
}

fn mark_reachable(succs: &[Vec<usize>], entry: usize) -> Vec<bool> {
    let n = succs.len();
    let mut vis = vec![false; n];
    if n == 0 {
        return vis;
    }
    let mut q = VecDeque::new();
    vis[entry] = true;
    q.push_back(entry);
    while let Some(u) = q.pop_front() {
        for &v in &succs[u] {
            if !vis[v] {
                vis[v] = true;
                q.push_back(v);
            }
        }
    }
    vis
}

fn compute_rpo(succs: &[Vec<usize>], reachable: &[bool], entry: usize) -> Vec<usize> {
    let n = succs.len();
    if n == 0 {
        return vec![];
    }
    let mut visited = vec![false; n];
    let mut stack: Vec<(usize, usize)> = Vec::new(); // (node, next edge idx)
    let mut order: Vec<usize> = Vec::new();

    if reachable[entry] {
        stack.push((entry, 0));
        visited[entry] = true;
        while let Some((u, ei)) = stack.pop() {
            if ei < succs[u].len() {
                // revisit u after exploring next succ
                stack.push((u, ei + 1));
                let v = succs[u][ei];
                if reachable[v] && !visited[v] {
                    visited[v] = true;
                    stack.push((v, 0));
                }
            } else {
                order.push(u);
            }
        }
    }
    order.reverse();
    order
}

fn join_list(v: &[usize]) -> String {
    v.iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::super::{BlockData, Func, Linkage, Terminator, Ty};
    use super::*;

    fn mk_block(name: &str, term: Terminator) -> BlockData {
        BlockData {
            name: name.to_string(),
            insts: vec![],
            term,
            span: None,
        }
    }

    #[test]
    fn cfg_linear() {
        // b0 -> b1 -> b2
        let b0 = mk_block("entry", Terminator::Br(Block(1)));
        let b1 = mk_block("mid", Terminator::Br(Block(2)));
        let b2 = mk_block("exit", Terminator::Ret(None));
        let f = Func {
            name: "f".into(),
            params: vec![Ty::I64],
            ret: Ty::I64,
            blocks: vec![b0, b1, b2],
            linkage: Linkage::External,
            span: None,
        };
        let cfg = Cfg::build(&f);
        assert_eq!(cfg.succs[0], vec![1]);
        assert_eq!(cfg.succs[1], vec![2]);
        assert_eq!(cfg.succs[2], Vec::<usize>::new());
        assert_eq!(cfg.preds[0], Vec::<usize>::new());
        assert_eq!(cfg.preds[1], vec![0]);
        assert_eq!(cfg.preds[2], vec![1]);
        assert_eq!(cfg.rpo, vec![0, 1, 2]);
        assert!(cfg.reachable[0] && cfg.reachable[1] && cfg.reachable[2]);
    }
}
