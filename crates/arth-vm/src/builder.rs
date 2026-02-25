use crate::{Op, Program};

pub fn compile_messages_to_program(msgs: &[String]) -> Program {
    let mut strings = Vec::new();
    let mut code = Vec::new();
    for m in msgs {
        let ix = strings.len() as u32;
        strings.push(m.clone());
        code.push(Op::Print(ix));
    }
    code.push(Op::Halt);
    Program::new(strings, code)
}
