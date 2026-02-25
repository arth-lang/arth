//! Structured input fuzzer - generates syntactically plausible Arth code
//!
//! This fuzzer uses the `arbitrary` crate to generate structured inputs
//! that are more likely to exercise interesting parser and typechecker paths.
//!
//! Run with: cargo +nightly fuzz run fuzz_structured

#![no_main]

use arth::compiler::diagnostics::Reporter;
use arth::compiler::parser::parse_file;
use arth::compiler::source::SourceFile;
use arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

/// Represents a fuzzed Arth program
#[derive(Debug, Arbitrary)]
struct FuzzProgram {
    package_name: FuzzIdent,
    declarations: Vec<FuzzDecl>,
}

#[derive(Debug, Arbitrary)]
struct FuzzIdent {
    name: String,
}

impl FuzzIdent {
    fn as_ident(&self) -> String {
        // Ensure valid identifier
        let name: String = self
            .name
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_')
            .take(20)
            .collect();
        if name.is_empty() || name.chars().next().unwrap().is_numeric() {
            format!("id_{}", name)
        } else {
            name
        }
    }
}

#[derive(Debug, Arbitrary)]
enum FuzzDecl {
    Struct(FuzzStruct),
    Module(FuzzModule),
    Enum(FuzzEnum),
}

#[derive(Debug, Arbitrary)]
struct FuzzStruct {
    name: FuzzIdent,
    fields: Vec<FuzzField>,
}

#[derive(Debug, Arbitrary)]
struct FuzzField {
    name: FuzzIdent,
    ty: FuzzType,
    is_final: bool,
}

#[derive(Debug, Arbitrary)]
enum FuzzType {
    Int,
    Str,
    Bool,
    Float,
    Generic(Box<FuzzType>),
}

impl FuzzType {
    fn as_arth(&self) -> String {
        match self {
            FuzzType::Int => "int".to_string(),
            FuzzType::Str => "String".to_string(),
            FuzzType::Bool => "bool".to_string(),
            FuzzType::Float => "float".to_string(),
            FuzzType::Generic(inner) => format!("List<{}>", inner.as_arth()),
        }
    }
}

#[derive(Debug, Arbitrary)]
struct FuzzModule {
    name: FuzzIdent,
    functions: Vec<FuzzFunction>,
}

#[derive(Debug, Arbitrary)]
struct FuzzFunction {
    name: FuzzIdent,
    params: Vec<FuzzParam>,
    return_type: FuzzType,
    body: Vec<FuzzStmt>,
}

#[derive(Debug, Arbitrary)]
struct FuzzParam {
    name: FuzzIdent,
    ty: FuzzType,
}

#[derive(Debug, Arbitrary)]
enum FuzzStmt {
    VarDecl {
        name: FuzzIdent,
        ty: FuzzType,
        value: FuzzExpr,
    },
    Assign {
        name: FuzzIdent,
        value: FuzzExpr,
    },
    If {
        cond: FuzzExpr,
        then_stmts: Vec<FuzzStmt>,
    },
    Return(FuzzExpr),
}

#[derive(Debug, Arbitrary)]
enum FuzzExpr {
    IntLit(i64),
    StringLit(String),
    BoolLit(bool),
    Ident(FuzzIdent),
    Binary {
        left: Box<FuzzExpr>,
        op: FuzzBinOp,
        right: Box<FuzzExpr>,
    },
}

#[derive(Debug, Arbitrary)]
enum FuzzBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Ne,
    Lt,
    Gt,
}

#[derive(Debug, Arbitrary)]
struct FuzzEnum {
    name: FuzzIdent,
    variants: Vec<FuzzIdent>,
}

impl FuzzProgram {
    fn to_source(&self) -> String {
        let mut s = format!("package {};\n\n", self.package_name.as_ident());
        for decl in &self.declarations {
            s.push_str(&decl.to_source());
            s.push_str("\n\n");
        }
        s
    }
}

impl FuzzDecl {
    fn to_source(&self) -> String {
        match self {
            FuzzDecl::Struct(st) => st.to_source(),
            FuzzDecl::Module(m) => m.to_source(),
            FuzzDecl::Enum(e) => e.to_source(),
        }
    }
}

impl FuzzStruct {
    fn to_source(&self) -> String {
        let mut s = format!("public struct {} {{\n", self.name.as_ident());
        for field in &self.fields {
            let final_kw = if field.is_final { "final " } else { "" };
            s.push_str(&format!(
                "    public {}{} {};\n",
                final_kw,
                field.ty.as_arth(),
                field.name.as_ident()
            ));
        }
        s.push('}');
        s
    }
}

impl FuzzModule {
    fn to_source(&self) -> String {
        let mut s = format!("module {} {{\n", self.name.as_ident());
        for func in &self.functions {
            s.push_str(&func.to_source());
            s.push('\n');
        }
        s.push('}');
        s
    }
}

impl FuzzFunction {
    fn to_source(&self) -> String {
        let params: Vec<String> = self
            .params
            .iter()
            .map(|p| format!("{} {}", p.ty.as_arth(), p.name.as_ident()))
            .collect();
        let mut s = format!(
            "    public {} {}({}) {{\n",
            self.return_type.as_arth(),
            self.name.as_ident(),
            params.join(", ")
        );
        for stmt in &self.body {
            s.push_str("        ");
            s.push_str(&stmt.to_source());
            s.push('\n');
        }
        s.push_str("    }");
        s
    }
}

impl FuzzStmt {
    fn to_source(&self) -> String {
        match self {
            FuzzStmt::VarDecl { name, ty, value } => {
                format!("{} {} = {};", ty.as_arth(), name.as_ident(), value.to_source())
            }
            FuzzStmt::Assign { name, value } => {
                format!("{} = {};", name.as_ident(), value.to_source())
            }
            FuzzStmt::If { cond, then_stmts } => {
                let mut s = format!("if ({}) {{\n", cond.to_source());
                for stmt in then_stmts {
                    s.push_str("            ");
                    s.push_str(&stmt.to_source());
                    s.push('\n');
                }
                s.push_str("        }");
                s
            }
            FuzzStmt::Return(expr) => {
                format!("return {};", expr.to_source())
            }
        }
    }
}

impl FuzzExpr {
    fn to_source(&self) -> String {
        match self {
            FuzzExpr::IntLit(n) => n.to_string(),
            FuzzExpr::StringLit(s) => {
                let escaped: String = s
                    .chars()
                    .filter(|c| c.is_ascii() && *c != '\n' && *c != '\r' && *c != '"')
                    .take(50)
                    .collect();
                format!("\"{}\"", escaped)
            }
            FuzzExpr::BoolLit(b) => {
                if *b { "true" } else { "false" }.to_string()
            }
            FuzzExpr::Ident(id) => id.as_ident(),
            FuzzExpr::Binary { left, op, right } => {
                let op_str = match op {
                    FuzzBinOp::Add => "+",
                    FuzzBinOp::Sub => "-",
                    FuzzBinOp::Mul => "*",
                    FuzzBinOp::Div => "/",
                    FuzzBinOp::Eq => "==",
                    FuzzBinOp::Ne => "!=",
                    FuzzBinOp::Lt => "<",
                    FuzzBinOp::Gt => ">",
                };
                format!("({} {} {})", left.to_source(), op_str, right.to_source())
            }
        }
    }
}

impl FuzzEnum {
    fn to_source(&self) -> String {
        let variants: Vec<String> = self.variants.iter().map(|v| v.as_ident()).take(10).collect();
        if variants.is_empty() {
            format!("public enum {} {{ DEFAULT }}", self.name.as_ident())
        } else {
            format!(
                "public enum {} {{ {} }}",
                self.name.as_ident(),
                variants.join(", ")
            )
        }
    }
}

fuzz_target!(|data: &[u8]| {
    // Try to generate a structured program
    let mut u = Unstructured::new(data);
    if let Ok(program) = FuzzProgram::arbitrary(&mut u) {
        let source = program.to_source();

        let sf = SourceFile {
            path: std::path::PathBuf::from("fuzz.arth"),
            text: source,
        };
        let mut reporter = Reporter::new();
        let _ast = parse_file(&sf, &mut reporter);
    }
});
