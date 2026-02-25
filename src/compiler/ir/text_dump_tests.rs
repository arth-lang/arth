#[cfg(test)]
mod tests {
    use crate::compiler::ir::{cfg::Cfg, dom::DomInfo, ssa::dump_ssa};
    use crate::compiler::ir::{Block, BlockData, Func, Inst, InstKind, Linkage, Terminator, Ty, Value};

    fn make_diamond() -> Func {
        let b0 = BlockData { name: "entry".into(), insts: vec![], term: Terminator::CondBr { cond: Value(0), then_bb: Block(1), else_bb: Block(2) }, span: None };
        let b1 = BlockData { name: "b1".into(), insts: vec![], term: Terminator::Br(Block(3)), span: None };
        let b2 = BlockData { name: "b2".into(), insts: vec![], term: Terminator::Br(Block(3)), span: None };
        let b3 = BlockData { name: "join".into(), insts: vec![], term: Terminator::Ret(None), span: None };
        Func { name: "f".into(), params: vec![Ty::I64], ret: Ty::I64, blocks: vec![b0, b1, b2, b3], linkage: Linkage::External, span: None }
    }

    #[test]
    fn cfg_dump_stable_diamond() {
        let f = make_diamond();
        let cfg = Cfg::build(&f);
        let dump = cfg.dump(&f);
        let expected = "bb0(entry) reachable: preds=[] succs=[1, 2]\n\
bb1(b1) reachable: preds=[0] succs=[3]\n\
bb2(b2) reachable: preds=[0] succs=[3]\n\
bb3(join) reachable: preds=[1, 2] succs=[]\n\
rpo: 0, 2, 1, 3\n";
        assert_eq!(dump, expected);
    }

    #[test]
    fn dom_dump_stable_diamond() {
        let f = make_diamond();
        let cfg = Cfg::build(&f);
        let dom = DomInfo::compute(&f, &cfg);
        let dump = dom.dump();
        let expected = "idoms:\n  bb0 <- bb0\n  bb1 <- bb0\n  bb2 <- bb0\n  bb3 <- bb0\n\
dom-tree:\n  bb0: [bb1, bb2, bb3]\n  bb1: []\n  bb2: []\n  bb3: []\n\
frontiers:\n  bb0: []\n  bb1: [bb3]\n  bb2: [bb3]\n  bb3: []\n";
        assert_eq!(dump, expected);
    }

    #[test]
    fn ssa_dump_shows_phi_and_values() {
        // Build a small SSA with a join phi
        let b0 = BlockData { name: "entry".into(), insts: vec![], term: Terminator::CondBr { cond: Value(0), then_bb: Block(1), else_bb: Block(2) }, span: None };
        let b1 = BlockData { name: "b1".into(), insts: vec![
            Inst { result: Value(1), kind: InstKind::ConstI64(1), span: None },
            Inst { result: Value(3), kind: InstKind::Copy(Value(1)), span: None },
        ], term: Terminator::Br(Block(3)), span: None };
        let b2 = BlockData { name: "b2".into(), insts: vec![
            Inst { result: Value(2), kind: InstKind::ConstI64(2), span: None },
        ], term: Terminator::Br(Block(3)), span: None };
        let b3 = BlockData { name: "join".into(), insts: vec![
            Inst { result: Value(10), kind: InstKind::Phi(vec![(Block(1), Value(3)), (Block(2), Value(2))]), span: None },
            Inst { result: Value(11), kind: InstKind::Copy(Value(10)), span: None },
        ], term: Terminator::Ret(Some(Value(11))), span: None };
        let f = Func { name: "f".into(), params: vec![], ret: Ty::Void, blocks: vec![b0,b1,b2,b3], linkage: Linkage::External, span: None };
        let dump = dump_ssa(&f);
        let expected = "bb0(entry):\n\
bb1(b1):\n  %1 = const 1\n\
  %3 = copy %1\n\
bb2(b2):\n  %2 = const 2\n\
bb3(join):\n  %10 = phi [bb1: %3], [bb2: %2]\n\
  %11 = copy %10\n";
        assert_eq!(dump, expected);
    }
}

