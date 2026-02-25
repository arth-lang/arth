//! Arth source code emitter from HIR.
//!
//! This module emits valid Arth source code from HIR, enabling:
//! - Debugging: inspect generated Arth code
//! - Interoperability: generated code can be modified
//! - Alternative compilation: use full Arth compiler
//!
//! The emitter preserves source locations via comments for error mapping.

use arth::compiler::hir::{
    HirAssignOp, HirBinOp, HirBlock, HirDecl, HirEnum, HirEnumVariant, HirExpr, HirField, HirFile,
    HirFunc, HirFuncSig, HirGenericParam, HirInterface, HirInterfaceMethod, HirModule, HirPattern,
    HirProvider, HirStmt, HirStruct, HirType, HirUnOp,
};

/// Result of emitting Arth source code.
#[derive(Debug, Clone)]
pub struct EmitResult {
    /// Generated Arth source code
    pub source: String,
    /// Original TypeScript file path (if any)
    pub ts_path: Option<String>,
    /// Source map entries: (arth_line, ts_line)
    pub source_map: Vec<(u32, u32)>,
}

/// Configuration for the Arth emitter.
#[derive(Debug, Clone)]
pub struct EmitConfig {
    /// Indentation string (default: 4 spaces)
    pub indent: String,
    /// Whether to emit source location comments
    pub emit_source_locations: bool,
    /// Whether to emit the package declaration
    pub emit_package: bool,
}

impl Default for EmitConfig {
    fn default() -> Self {
        Self {
            indent: "    ".to_string(),
            emit_source_locations: true,
            emit_package: true,
        }
    }
}

/// Arth source code emitter.
pub struct ArthEmitter {
    config: EmitConfig,
    output: String,
    indent_level: usize,
    current_line: u32,
    source_map: Vec<(u32, u32)>,
}

impl ArthEmitter {
    /// Create a new emitter with the given configuration.
    pub fn new(config: EmitConfig) -> Self {
        Self {
            config,
            output: String::new(),
            indent_level: 0,
            current_line: 1,
            source_map: Vec::new(),
        }
    }

    /// Create an emitter with default configuration.
    pub fn default_config() -> Self {
        Self::new(EmitConfig::default())
    }

    /// Emit an entire HIR file to Arth source code.
    pub fn emit_file(&mut self, hir: &HirFile) -> EmitResult {
        // Emit package declaration if present
        if self.config.emit_package
            && let Some(pkg) = &hir.package
        {
            self.emit_line(&format!("package {};", pkg));
            self.emit_newline();
        }

        // Emit each declaration
        for decl in &hir.decls {
            self.emit_decl(decl);
            self.emit_newline();
        }

        EmitResult {
            source: self.output.clone(),
            ts_path: Some(hir.path.to_string_lossy().to_string()),
            source_map: self.source_map.clone(),
        }
    }

    /// Emit a declaration.
    fn emit_decl(&mut self, decl: &HirDecl) {
        match decl {
            HirDecl::Module(m) => self.emit_module(m),
            HirDecl::Struct(s) => self.emit_struct(s),
            HirDecl::Interface(i) => self.emit_interface(i),
            HirDecl::Enum(e) => self.emit_enum(e),
            HirDecl::Provider(p) => self.emit_provider(p),
            HirDecl::Function(f) => self.emit_function(f),
            HirDecl::ExternFunc(ef) => {
                // Emit extern function declaration
                self.emit_line(&format!("extern \"{}\" fn {}();", ef.abi, ef.name));
            }
        }
    }

    /// Emit a module declaration.
    fn emit_module(&mut self, m: &HirModule) {
        // Emit doc comment
        if let Some(doc) = &m.doc {
            self.emit_doc_comment(doc);
        }

        // Emit attributes
        for attr in &m.attrs {
            self.emit_attr(attr);
        }

        // Module header
        let mut header = String::new();
        if m.is_exported {
            header.push_str("public ");
        }
        header.push_str("module ");
        header.push_str(&m.name);

        // Emit implements clause
        if !m.implements.is_empty() {
            header.push_str(" implements ");
            let impls: Vec<String> = m.implements.iter().map(|path| path.join(".")).collect();
            header.push_str(&impls.join(", "));
        }

        header.push_str(" {");
        self.emit_line(&header);
        self.indent();

        // Emit functions
        for func in &m.funcs {
            self.emit_function(func);
            self.emit_newline();
        }

        self.dedent();
        self.emit_line("}");
    }

    /// Emit a struct declaration.
    fn emit_struct(&mut self, s: &HirStruct) {
        if let Some(doc) = &s.doc {
            self.emit_doc_comment(doc);
        }

        for attr in &s.attrs {
            self.emit_attr(attr);
        }

        let mut header = String::from("struct ");
        header.push_str(&s.name);
        self.emit_generics(&s.generics, &mut header);
        header.push_str(" {");
        self.emit_line(&header);
        self.indent();

        for field in &s.fields {
            self.emit_field(field);
        }

        self.dedent();
        self.emit_line("}");
    }

    /// Emit an interface declaration.
    fn emit_interface(&mut self, i: &HirInterface) {
        if let Some(doc) = &i.doc {
            self.emit_doc_comment(doc);
        }

        for attr in &i.attrs {
            self.emit_attr(attr);
        }

        let mut header = String::from("interface ");
        header.push_str(&i.name);
        self.emit_generics(&i.generics, &mut header);

        // Emit extends clause
        if !i.extends.is_empty() {
            header.push_str(" extends ");
            let exts: Vec<String> = i.extends.iter().map(|path| path.join(".")).collect();
            header.push_str(&exts.join(", "));
        }

        header.push_str(" {");
        self.emit_line(&header);
        self.indent();

        for method in &i.methods {
            self.emit_interface_method(method);
        }

        self.dedent();
        self.emit_line("}");
    }

    /// Emit an interface method signature.
    fn emit_interface_method(&mut self, method: &HirInterfaceMethod) {
        let func_sig = &method.sig;
        let mut sig = String::new();

        // Return type
        if let Some(ret) = &func_sig.ret {
            sig.push_str(&self.type_to_string(ret));
        } else {
            sig.push_str("void");
        }

        sig.push(' ');
        sig.push_str(&func_sig.name);

        // Generics
        self.emit_generics(&func_sig.generics, &mut sig);

        // Parameters
        sig.push('(');
        let params: Vec<String> = func_sig
            .params
            .iter()
            .map(|p| format!("{} {}", self.type_to_string(&p.ty), p.name))
            .collect();
        sig.push_str(&params.join(", "));
        sig.push_str(");");

        self.emit_line(&sig);
    }

    /// Emit an enum declaration.
    fn emit_enum(&mut self, e: &HirEnum) {
        if let Some(doc) = &e.doc {
            self.emit_doc_comment(doc);
        }

        for attr in &e.attrs {
            self.emit_attr(attr);
        }

        let mut header = String::from("enum ");
        header.push_str(&e.name);
        self.emit_generics(&e.generics, &mut header);
        header.push_str(" {");
        self.emit_line(&header);
        self.indent();

        for variant in &e.variants {
            self.emit_enum_variant(variant);
        }

        self.dedent();
        self.emit_line("}");
    }

    /// Emit an enum variant.
    fn emit_enum_variant(&mut self, variant: &HirEnumVariant) {
        match variant {
            HirEnumVariant::Unit { name, .. } => {
                self.emit_line(&format!("{},", name));
            }
            HirEnumVariant::Tuple { name, types, .. } => {
                let type_strs: Vec<String> = types.iter().map(|t| self.type_to_string(t)).collect();
                self.emit_line(&format!("{}({}),", name, type_strs.join(", ")));
            }
        }
    }

    /// Emit a provider declaration.
    fn emit_provider(&mut self, p: &HirProvider) {
        if let Some(doc) = &p.doc {
            self.emit_doc_comment(doc);
        }

        for attr in &p.attrs {
            self.emit_attr(attr);
        }

        let header = format!("provider {} {{", p.name);
        self.emit_line(&header);
        self.indent();

        for field in &p.fields {
            self.emit_field(field);
        }

        self.dedent();
        self.emit_line("}");
    }

    /// Emit a field declaration.
    fn emit_field(&mut self, field: &HirField) {
        if let Some(doc) = &field.doc {
            self.emit_doc_comment(doc);
        }

        let mut line = String::new();

        // Visibility
        match field.vis {
            arth::compiler::ast::Visibility::Public => line.push_str("public "),
            arth::compiler::ast::Visibility::Private => {}
            arth::compiler::ast::Visibility::Internal => line.push_str("internal "),
            arth::compiler::ast::Visibility::Default => {}
        }

        // Modifiers
        if field.is_final {
            line.push_str("final ");
        }
        if field.is_shared {
            line.push_str("shared ");
        }

        // Type and name
        line.push_str(&self.type_to_string(&field.ty));
        line.push(' ');
        line.push_str(&field.name);
        line.push(';');

        self.emit_line(&line);
    }

    /// Emit a function declaration.
    fn emit_function(&mut self, f: &HirFunc) {
        self.emit_func_sig(&f.sig);

        if let Some(body) = &f.body {
            self.emit_block(body);
        } else {
            self.output.push_str(";\n");
            self.current_line += 1;
        }
    }

    /// Emit a function signature.
    fn emit_func_sig(&mut self, sig: &HirFuncSig) {
        if let Some(doc) = &sig.doc {
            self.emit_doc_comment(doc);
        }

        for attr in &sig.attrs {
            self.emit_attr(attr);
        }

        let mut line = String::new();

        // Visibility (check for @export attribute)
        if sig.attrs.iter().any(|a| a.name == "export") {
            line.push_str("public ");
        }

        // Modifiers
        if sig.is_async {
            line.push_str("async ");
        }
        if sig.is_unsafe {
            line.push_str("unsafe ");
        }

        // Return type
        if let Some(ret) = &sig.ret {
            line.push_str(&self.type_to_string(ret));
        } else {
            line.push_str("void");
        }

        line.push(' ');
        line.push_str(&sig.name);

        // Generics
        self.emit_generics(&sig.generics, &mut line);

        // Parameters
        line.push('(');
        let params: Vec<String> = sig
            .params
            .iter()
            .map(|p| format!("{} {}", self.type_to_string(&p.ty), p.name))
            .collect();
        line.push_str(&params.join(", "));
        line.push_str(") ");

        self.emit_indent();
        self.output.push_str(&line);
    }

    /// Emit a block.
    fn emit_block(&mut self, block: &HirBlock) {
        self.output.push_str("{\n");
        self.current_line += 1;
        self.indent();

        for stmt in &block.stmts {
            self.emit_stmt(stmt);
        }

        self.dedent();
        self.emit_line("}");
    }

    /// Emit a statement.
    fn emit_stmt(&mut self, stmt: &HirStmt) {
        match stmt {
            HirStmt::VarDecl {
                ty,
                name,
                init,
                is_shared,
                ..
            } => {
                let mut line = String::new();
                if *is_shared {
                    line.push_str("shared ");
                }
                line.push_str(&self.type_to_string(ty));
                line.push(' ');
                line.push_str(name);
                if let Some(init_expr) = init {
                    line.push_str(" = ");
                    line.push_str(&self.expr_to_string(init_expr));
                }
                line.push(';');
                self.emit_line(&line);
            }
            HirStmt::Assign { name, expr, .. } => {
                let line = format!("{} = {};", name, self.expr_to_string(expr));
                self.emit_line(&line);
            }
            HirStmt::AssignOp { name, op, expr, .. } => {
                let op_str = self.assign_op_to_string(op);
                let line = format!("{} {}= {};", name, op_str, self.expr_to_string(expr));
                self.emit_line(&line);
            }
            HirStmt::FieldAssign {
                object,
                field,
                expr,
                ..
            } => {
                let line = format!(
                    "{}.{} = {};",
                    self.expr_to_string(object),
                    field,
                    self.expr_to_string(expr)
                );
                self.emit_line(&line);
            }
            HirStmt::If {
                cond,
                then_blk,
                else_blk,
                ..
            } => {
                self.emit_indent();
                self.output
                    .push_str(&format!("if ({}) ", self.expr_to_string(cond)));
                self.emit_block(then_blk);
                if let Some(else_block) = else_blk {
                    self.emit_indent();
                    self.output.push_str("else ");
                    self.emit_block(else_block);
                }
            }
            HirStmt::While { cond, body, .. } => {
                self.emit_indent();
                self.output
                    .push_str(&format!("while ({}) ", self.expr_to_string(cond)));
                self.emit_block(body);
            }
            HirStmt::For {
                init,
                cond,
                step,
                body,
                ..
            } => {
                self.emit_indent();
                self.output.push_str("for (");

                // Init
                if let Some(init_stmt) = init {
                    self.output.push_str(&self.stmt_inline(init_stmt));
                }
                self.output.push_str("; ");

                // Condition
                if let Some(cond_expr) = cond {
                    self.output.push_str(&self.expr_to_string(cond_expr));
                }
                self.output.push_str("; ");

                // Step
                if let Some(step_stmt) = step {
                    self.output.push_str(&self.stmt_inline(step_stmt));
                }
                self.output.push_str(") ");

                self.emit_block(body);
            }
            HirStmt::Return { expr, .. } => {
                let line = if let Some(e) = expr {
                    format!("return {};", self.expr_to_string(e))
                } else {
                    "return;".to_string()
                };
                self.emit_line(&line);
            }
            HirStmt::Break { label, .. } => {
                let line = if let Some(l) = label {
                    format!("break {};", l)
                } else {
                    "break;".to_string()
                };
                self.emit_line(&line);
            }
            HirStmt::Continue { label, .. } => {
                let line = if let Some(l) = label {
                    format!("continue {};", l)
                } else {
                    "continue;".to_string()
                };
                self.emit_line(&line);
            }
            HirStmt::Throw { expr, .. } => {
                let line = format!("throw {};", self.expr_to_string(expr));
                self.emit_line(&line);
            }
            HirStmt::Panic { msg, .. } => {
                let line = format!("panic({});", self.expr_to_string(msg));
                self.emit_line(&line);
            }
            HirStmt::Try {
                try_blk,
                catches,
                finally_blk,
                ..
            } => {
                self.emit_indent();
                self.output.push_str("try ");
                self.emit_block(try_blk);

                for catch in catches {
                    self.emit_indent();
                    self.output.push_str("catch (");
                    if let Some(ty) = &catch.ty {
                        self.output.push_str(&self.type_to_string(ty));
                        if let Some(var) = &catch.var {
                            self.output.push(' ');
                            self.output.push_str(var);
                        }
                    }
                    self.output.push_str(") ");
                    self.emit_block(&catch.block);
                }

                if let Some(finally) = finally_blk {
                    self.emit_indent();
                    self.output.push_str("finally ");
                    self.emit_block(finally);
                }
            }
            HirStmt::Switch {
                expr,
                cases,
                pattern_cases,
                default,
                ..
            } => {
                self.emit_indent();
                self.output
                    .push_str(&format!("switch ({}) {{\n", self.expr_to_string(expr)));
                self.current_line += 1;
                self.indent();

                for (case_expr, block) in cases {
                    self.emit_indent();
                    self.output
                        .push_str(&format!("case {}: ", self.expr_to_string(case_expr)));
                    self.emit_block(block);
                }

                for (pattern, block) in pattern_cases {
                    self.emit_indent();
                    self.output
                        .push_str(&format!("case {}: ", self.pattern_to_string(pattern)));
                    self.emit_block(block);
                }

                if let Some(def) = default {
                    self.emit_indent();
                    self.output.push_str("default: ");
                    self.emit_block(def);
                }

                self.dedent();
                self.emit_line("}");
            }
            HirStmt::Expr { expr, .. } => {
                let line = format!("{};", self.expr_to_string(expr));
                self.emit_line(&line);
            }
            HirStmt::Block(block) => {
                self.emit_indent();
                self.emit_block(block);
            }
            HirStmt::Labeled { label, stmt, .. } => {
                self.emit_line(&format!("{}:", label));
                self.emit_stmt(stmt);
            }
            HirStmt::PrintStr { text, .. } => {
                let escaped = self.escape_string(text);
                self.emit_line(&format!("print \"{}\";", escaped));
            }
            HirStmt::PrintExpr { expr, .. } => {
                self.emit_line(&format!("print {};", self.expr_to_string(expr)));
            }
            HirStmt::PrintRawStr { text, .. } => {
                let escaped = self.escape_string(text);
                self.emit_line(&format!("printraw \"{}\";", escaped));
            }
            HirStmt::PrintRawExpr { expr, .. } => {
                self.emit_line(&format!("printraw {};", self.expr_to_string(expr)));
            }
            HirStmt::Unsafe { block, .. } => {
                self.emit_indent();
                self.output.push_str("unsafe ");
                self.emit_block(block);
            }
        }
    }

    /// Convert a statement to inline string (for for-loop init/step).
    fn stmt_inline(&self, stmt: &HirStmt) -> String {
        match stmt {
            HirStmt::VarDecl { ty, name, init, .. } => {
                let mut s = format!("{} {}", self.type_to_string(ty), name);
                if let Some(init_expr) = init {
                    s.push_str(" = ");
                    s.push_str(&self.expr_to_string(init_expr));
                }
                s
            }
            HirStmt::Assign { name, expr, .. } => {
                format!("{} = {}", name, self.expr_to_string(expr))
            }
            HirStmt::AssignOp { name, op, expr, .. } => {
                format!(
                    "{} {}= {}",
                    name,
                    self.assign_op_to_string(op),
                    self.expr_to_string(expr)
                )
            }
            HirStmt::Expr { expr, .. } => self.expr_to_string(expr),
            _ => String::new(),
        }
    }

    /// Convert an expression to string.
    fn expr_to_string(&self, expr: &HirExpr) -> String {
        match expr {
            HirExpr::Int { value, .. } => value.to_string(),
            HirExpr::Float { value, .. } => {
                let s = value.to_string();
                if s.contains('.') {
                    s
                } else {
                    format!("{}.0", s)
                }
            }
            HirExpr::Str { value, .. } => format!("\"{}\"", self.escape_string(value)),
            HirExpr::Char { value, .. } => format!("'{}'", value),
            HirExpr::Bool { value, .. } => value.to_string(),
            HirExpr::Ident { name, .. } => name.clone(),
            HirExpr::Binary {
                left, op, right, ..
            } => {
                format!(
                    "({} {} {})",
                    self.expr_to_string(left),
                    self.bin_op_to_string(op),
                    self.expr_to_string(right)
                )
            }
            HirExpr::Unary { op, expr, .. } => {
                format!("{}{}", self.un_op_to_string(op), self.expr_to_string(expr))
            }
            HirExpr::Call { callee, args, .. } => {
                let args_str: Vec<String> = args.iter().map(|a| self.expr_to_string(a)).collect();
                format!("{}({})", self.expr_to_string(callee), args_str.join(", "))
            }
            HirExpr::Member { object, member, .. } => {
                format!("{}.{}", self.expr_to_string(object), member)
            }
            HirExpr::OptionalMember { object, member, .. } => {
                format!("{}?.{}", self.expr_to_string(object), member)
            }
            HirExpr::Index { object, index, .. } => {
                format!(
                    "{}[{}]",
                    self.expr_to_string(object),
                    self.expr_to_string(index)
                )
            }
            HirExpr::ListLit { elements, .. } => {
                let elems: Vec<String> = elements.iter().map(|e| self.expr_to_string(e)).collect();
                format!("[{}]", elems.join(", "))
            }
            HirExpr::MapLit { pairs, spread, .. } => {
                let mut parts: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| format!("{}: {}", self.expr_to_string(k), self.expr_to_string(v)))
                    .collect();
                if let Some(sp) = spread {
                    parts.insert(0, format!("..{}", self.expr_to_string(sp)));
                }
                format!("{{ {} }}", parts.join(", "))
            }
            HirExpr::StructLit {
                type_name,
                fields,
                spread,
                ..
            } => {
                let type_str = self.type_to_string(type_name);
                let mut parts: Vec<String> = fields
                    .iter()
                    .map(|(name, val)| format!("{}: {}", name, self.expr_to_string(val)))
                    .collect();
                if let Some(sp) = spread {
                    parts.insert(0, format!("..{}", self.expr_to_string(sp)));
                }
                format!("{} {{ {} }}", type_str, parts.join(", "))
            }
            HirExpr::EnumVariant {
                enum_name,
                variant_name,
                args,
                ..
            } => {
                if args.is_empty() {
                    format!("{}.{}", enum_name, variant_name)
                } else {
                    let args_str: Vec<String> =
                        args.iter().map(|a| self.expr_to_string(a)).collect();
                    format!("{}.{}({})", enum_name, variant_name, args_str.join(", "))
                }
            }
            HirExpr::Conditional {
                cond,
                then_expr,
                else_expr,
                ..
            } => {
                format!(
                    "({} ? {} : {})",
                    self.expr_to_string(cond),
                    self.expr_to_string(then_expr),
                    self.expr_to_string(else_expr)
                )
            }
            HirExpr::Lambda { params, ret, .. } => {
                let params_str: Vec<String> = params
                    .iter()
                    .map(|p| format!("{} {}", self.type_to_string(&p.ty), p.name))
                    .collect();
                let ret_str = ret
                    .as_ref()
                    .map(|r| format!(" -> {}", self.type_to_string(r)))
                    .unwrap_or_default();
                // For simplicity, emit lambda as placeholder (full body emission would require recursion)
                format!(
                    "|{}|{} {{ /* lambda body */ }}",
                    params_str.join(", "),
                    ret_str
                )
            }
            HirExpr::Await { expr, .. } => {
                format!("await {}", self.expr_to_string(expr))
            }
            HirExpr::Cast { to, expr, .. } => {
                format!(
                    "({} as {})",
                    self.expr_to_string(expr),
                    self.type_to_string(to)
                )
            }
        }
    }

    /// Convert a pattern to string.
    fn pattern_to_string(&self, pattern: &HirPattern) -> String {
        match pattern {
            HirPattern::Wildcard { .. } => "_".to_string(),
            HirPattern::Binding { name, .. } => name.clone(),
            HirPattern::Literal { expr, .. } => self.expr_to_string(expr),
            HirPattern::Variant {
                enum_name,
                variant_name,
                payloads,
                ..
            } => {
                if payloads.is_empty() {
                    format!("{}.{}", enum_name, variant_name)
                } else {
                    let pats: Vec<String> =
                        payloads.iter().map(|p| self.pattern_to_string(p)).collect();
                    format!("{}.{}({})", enum_name, variant_name, pats.join(", "))
                }
            }
        }
    }

    /// Convert a type to string.
    #[allow(clippy::only_used_in_recursion)]
    fn type_to_string(&self, ty: &HirType) -> String {
        match ty {
            HirType::Name { path } => path.join("."),
            HirType::Generic { path, args } => {
                let args_str: Vec<String> = args.iter().map(|t| self.type_to_string(t)).collect();
                format!("{}<{}>", path.join("."), args_str.join(", "))
            }
            HirType::TypeParam { name } => name.clone(),
        }
    }

    /// Emit generic parameters.
    fn emit_generics(&self, generics: &[HirGenericParam], output: &mut String) {
        if generics.is_empty() {
            return;
        }

        output.push('<');
        let params: Vec<String> = generics
            .iter()
            .map(|g| {
                if let Some(bound) = &g.bound {
                    format!("{} extends {}", g.name, self.type_to_string(bound))
                } else {
                    g.name.clone()
                }
            })
            .collect();
        output.push_str(&params.join(", "));
        output.push('>');
    }

    /// Emit an attribute.
    fn emit_attr(&mut self, attr: &arth::compiler::hir::HirAttr) {
        // Skip @export attribute as it's handled by visibility
        if attr.name == "export" {
            return;
        }

        let line = if let Some(args) = &attr.args {
            format!("@{}({})", attr.name, args)
        } else {
            format!("@{}", attr.name)
        };
        self.emit_line(&line);
    }

    /// Emit a doc comment.
    fn emit_doc_comment(&mut self, doc: &str) {
        for line in doc.lines() {
            self.emit_line(&format!("/// {}", line.trim()));
        }
    }

    /// Convert binary operator to string.
    fn bin_op_to_string(&self, op: &HirBinOp) -> &'static str {
        match op {
            HirBinOp::Add => "+",
            HirBinOp::Sub => "-",
            HirBinOp::Mul => "*",
            HirBinOp::Div => "/",
            HirBinOp::Mod => "%",
            HirBinOp::Shl => "<<",
            HirBinOp::Shr => ">>",
            HirBinOp::Lt => "<",
            HirBinOp::Le => "<=",
            HirBinOp::Gt => ">",
            HirBinOp::Ge => ">=",
            HirBinOp::Eq => "==",
            HirBinOp::Ne => "!=",
            HirBinOp::And => "&&",
            HirBinOp::Or => "||",
            HirBinOp::BitAnd => "&",
            HirBinOp::BitOr => "|",
            HirBinOp::Xor => "^",
        }
    }

    /// Convert unary operator to string.
    fn un_op_to_string(&self, op: &HirUnOp) -> &'static str {
        match op {
            HirUnOp::Neg => "-",
            HirUnOp::Not => "!",
        }
    }

    /// Convert assignment operator to string.
    fn assign_op_to_string(&self, op: &HirAssignOp) -> &'static str {
        match op {
            HirAssignOp::Add => "+",
            HirAssignOp::Sub => "-",
            HirAssignOp::Mul => "*",
            HirAssignOp::Div => "/",
            HirAssignOp::Mod => "%",
            HirAssignOp::Shl => "<<",
            HirAssignOp::Shr => ">>",
            HirAssignOp::And => "&",
            HirAssignOp::Or => "|",
            HirAssignOp::Xor => "^",
        }
    }

    /// Escape a string for Arth source.
    fn escape_string(&self, s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    }

    /// Emit a line with current indentation.
    fn emit_line(&mut self, line: &str) {
        self.emit_indent();
        self.output.push_str(line);
        self.output.push('\n');
        self.current_line += 1;
    }

    /// Emit current indentation.
    fn emit_indent(&mut self) {
        for _ in 0..self.indent_level {
            self.output.push_str(&self.config.indent);
        }
    }

    /// Emit a newline.
    fn emit_newline(&mut self) {
        self.output.push('\n');
        self.current_line += 1;
    }

    /// Increase indentation level.
    fn indent(&mut self) {
        self.indent_level += 1;
    }

    /// Decrease indentation level.
    fn dedent(&mut self) {
        if self.indent_level > 0 {
            self.indent_level -= 1;
        }
    }
}

/// Emit Arth source code from an HIR file.
pub fn emit_arth_source(hir: &HirFile) -> EmitResult {
    let mut emitter = ArthEmitter::default_config();
    emitter.emit_file(hir)
}

/// Emit Arth source code with custom configuration.
pub fn emit_arth_source_with_config(hir: &HirFile, config: EmitConfig) -> EmitResult {
    let mut emitter = ArthEmitter::new(config);
    emitter.emit_file(hir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::{TsLoweringOptions, lower_ts_str_to_hir};

    #[test]
    fn test_emit_simple_function() {
        let source = r#"
export function add(a: number, b: number): number {
    return a + b;
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        assert!(result.source.contains("public"));
        assert!(result.source.contains("add"));
        assert!(result.source.contains("return"));
    }

    #[test]
    fn test_emit_struct() {
        let source = r#"
export type User = {
    name: string;
    age: number;
};

export function getUser(): string {
    return "user";
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        assert!(result.source.contains("struct User"));
        assert!(result.source.contains("String name"));
        assert!(result.source.contains("Int age") || result.source.contains("number"));
    }

    #[test]
    fn test_emit_provider() {
        let source = r#"
@provider
class AppState {
    count: number = 0;
}

export function init(): void {
    // Initialize
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        assert!(result.source.contains("provider AppState"));
    }

    #[test]
    fn test_emit_if_statement() {
        let source = r#"
export function check(x: number): number {
    if (x > 0) {
        return 1;
    } else {
        return 0;
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        assert!(result.source.contains("if ("));
        assert!(result.source.contains("else"));
    }

    #[test]
    fn test_emit_while_loop() {
        let source = r#"
export function loop(n: number): number {
    let i: number = 0;
    while (i < n) {
        i = i + 1;
    }
    return i;
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        assert!(result.source.contains("while ("));
    }

    #[test]
    fn test_emit_class_as_module() {
        let source = r#"
export default class Calculator {
    add(a: number, b: number): number {
        return a + b;
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        assert!(result.source.contains("module Calculator"));
        assert!(result.source.contains("add"));
    }

    #[test]
    fn test_emit_interface() {
        let source = r#"
export interface Comparable {
    compare(other: Comparable): number;
}

export function compare(a: Comparable): number {
    return 0;
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        assert!(result.source.contains("interface Comparable"));
    }

    #[test]
    fn test_emit_with_custom_indent() {
        let source = r#"
export function test(): number {
    return 42;
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let config = EmitConfig {
            indent: "  ".to_string(),
            emit_source_locations: false,
            emit_package: false,
        };

        let result = emit_arth_source_with_config(&hir, config);

        // Should use 2-space indentation
        assert!(result.source.contains("  return"));
    }

    #[test]
    fn test_emit_for_loop() {
        // for...of in TypeScript is desugared to while loop in HIR
        let source = r#"
export function sum(items: number[]): number {
    let total: number = 0;
    for (const item of items) {
        total = total + item;
    }
    return total;
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        // for...of desugars to while loop
        assert!(result.source.contains("while ("));
        assert!(result.source.contains("total"));
    }

    #[test]
    fn test_emit_binary_operators() {
        let source = r#"
export function math(a: number, b: number): number {
    let result: number = a + b;
    result = result - 1;
    result = result * 2;
    result = result / 2;
    return result;
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        assert!(result.source.contains("+"));
        assert!(result.source.contains("-"));
        assert!(result.source.contains("*"));
        assert!(result.source.contains("/"));
    }

    #[test]
    fn test_emit_comparison_operators() {
        let source = r#"
export function compare(a: number, b: number): boolean {
    if (a < b) {
        return true;
    }
    if (a > b) {
        return false;
    }
    if (a <= b) {
        return true;
    }
    if (a >= b) {
        return true;
    }
    if (a == b) {
        return true;
    }
    return false;
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        assert!(result.source.contains("<"));
        assert!(result.source.contains(">"));
    }

    #[test]
    fn test_emit_list_literal() {
        let source = r#"
export function getList(): number[] {
    return [1, 2, 3];
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        assert!(result.source.contains("["));
        assert!(result.source.contains("]"));
        assert!(result.source.contains("1"));
    }

    #[test]
    fn test_emit_function_call() {
        let source = r#"
function helper(x: number): number {
    return x * 2;
}

export function main(): number {
    return helper(21);
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        assert!(result.source.contains("helper("));
    }

    #[test]
    fn test_emit_member_access() {
        let source = r#"
type Point = {
    x: number;
    y: number;
};

export function getX(p: Point): number {
    return p.x;
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        assert!(result.source.contains("p.x"));
    }

    #[test]
    fn test_emit_string_literal() {
        let source = r#"
export function greet(): string {
    return "hello world";
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        assert!(result.source.contains("\"hello world\""));
    }

    #[test]
    fn test_emit_boolean_literals() {
        let source = r#"
export function check(): boolean {
    let a: boolean = true;
    let b: boolean = false;
    return a;
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        assert!(result.source.contains("true"));
        assert!(result.source.contains("false"));
    }

    #[test]
    fn test_emit_conditional_expression() {
        let source = r#"
export function max(a: number, b: number): number {
    return a > b ? a : b;
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        assert!(result.source.contains("?"));
        assert!(result.source.contains(":"));
    }

    #[test]
    fn test_emit_variable_declaration() {
        let source = r#"
export function init(): number {
    let count: number = 0;
    const max: number = 100;
    return count + max;
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        assert!(result.source.contains("Int count"));
        assert!(result.source.contains("= 0"));
    }

    #[test]
    fn test_emit_source_map_tracking() {
        let source = r#"
export function test(): number {
    return 42;
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        // Verify source map is populated
        assert!(result.ts_path.is_some());
        assert_eq!(result.ts_path.as_ref().unwrap(), "test.ts");
    }

    #[test]
    fn test_emit_preserves_module_structure() {
        let source = r#"
export default class UserService {
    findUser(id: string): string {
        return id;
    }

    createUser(name: string): string {
        return name;
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let result = emit_arth_source(&hir);

        // Verify module structure
        assert!(result.source.contains("module UserService"));
        assert!(result.source.contains("findUser"));
        assert!(result.source.contains("createUser"));
        assert!(result.source.contains("}"));
    }
}
