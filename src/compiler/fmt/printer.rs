//! AST printer for code formatting.
//!
//! Walks the AST and emits properly formatted source code.

use crate::compiler::ast::*;
use crate::compiler::fmt::{BraceStyle, FormatConfig};

/// Printer that formats AST nodes to text.
pub struct Printer {
    config: FormatConfig,
    output: String,
    indent_level: usize,
    /// Current column position (for line width tracking).
    column: usize,
}

impl Printer {
    /// Create a new printer with the given configuration.
    pub fn new(config: FormatConfig) -> Self {
        Self {
            config,
            output: String::new(),
            indent_level: 0,
            column: 0,
        }
    }

    /// Get the current indentation string.
    fn indent_str(&self) -> String {
        " ".repeat(self.indent_level * self.config.indent)
    }

    /// Write a string to output.
    fn write(&mut self, s: &str) {
        self.output.push_str(s);
        // Update column tracking
        if let Some(last_newline) = s.rfind('\n') {
            self.column = s.len() - last_newline - 1;
        } else {
            self.column += s.len();
        }
    }

    /// Write a newline.
    fn newline(&mut self) {
        self.write("\n");
        self.column = 0;
    }

    /// Write indentation at current level.
    fn write_indent(&mut self) {
        let indent = self.indent_str();
        self.write(&indent);
    }

    /// Increase indentation level.
    fn indent(&mut self) {
        self.indent_level += 1;
    }

    /// Decrease indentation level.
    fn dedent(&mut self) {
        self.indent_level = self.indent_level.saturating_sub(1);
    }

    /// Print a complete file.
    pub fn print_file(&mut self, file: &FileAst) -> String {
        // Package declaration
        if let Some(pkg) = &file.package {
            self.write("package ");
            self.write(&pkg.to_string());
            self.write(";");
            self.newline();
            self.newline();
        }

        // Imports
        for import in &file.imports {
            self.print_import(import);
            self.newline();
        }

        if !file.imports.is_empty() {
            self.newline();
        }

        // Declarations
        for (i, decl) in file.decls.iter().enumerate() {
            if i > 0 {
                self.newline();
            }
            self.print_decl(decl);
        }

        self.output.clone()
    }

    /// Print an import specification.
    fn print_import(&mut self, import: &ImportSpec) {
        self.write("import ");
        let path = import
            .path
            .iter()
            .map(|i| i.0.as_str())
            .collect::<Vec<_>>()
            .join(".");
        self.write(&path);

        if import.star {
            self.write(".*");
        }

        if let Some(alias) = &import.alias {
            self.write(" as ");
            self.write(&alias.0);
        }

        self.write(";");
    }

    /// Print a declaration.
    fn print_decl(&mut self, decl: &Decl) {
        match decl {
            Decl::Module(m) => self.print_module(m),
            Decl::Struct(s) => self.print_struct(s),
            Decl::Interface(i) => self.print_interface(i),
            Decl::Enum(e) => self.print_enum(e),
            Decl::Provider(p) => self.print_provider(p),
            Decl::Function(f) => self.print_function(f),
            Decl::TypeAlias(t) => self.print_type_alias(t),
            Decl::ExternFunc(e) => self.print_extern_func(e),
        }
    }

    /// Print a module declaration.
    fn print_module(&mut self, module: &ModuleDecl) {
        // Doc comment
        self.print_doc(&module.doc);

        // Attributes
        for attr in &module.attrs {
            self.print_attr(attr);
            self.newline();
        }

        // Module header
        if module.is_exported {
            self.write("public ");
        }
        self.write("module ");
        self.write(&module.name.0);

        // Implements clause
        if !module.implements.is_empty() {
            self.write(" implements ");
            for (i, imp) in module.implements.iter().enumerate() {
                if i > 0 {
                    self.write(", ");
                }
                self.print_name_path(imp);
            }
        }

        self.print_opening_brace();
        self.indent();

        // Module functions
        for (i, func) in module.items.iter().enumerate() {
            if i > 0 {
                self.newline();
            }
            self.newline();
            self.print_function(func);
        }

        self.dedent();
        self.newline();
        self.write_indent();
        self.write("}");
        self.newline();
    }

    /// Print a struct declaration.
    fn print_struct(&mut self, s: &StructDecl) {
        self.print_doc(&s.doc);

        for attr in &s.attrs {
            self.print_attr(attr);
            self.newline();
        }

        self.write("struct ");
        self.write(&s.name.0);
        self.print_generics(&s.generics);
        self.print_opening_brace();
        self.indent();

        for field in &s.fields {
            self.newline();
            self.print_struct_field(field);
        }

        self.dedent();
        self.newline();
        self.write_indent();
        self.write("}");
        self.newline();
    }

    /// Print a struct field.
    fn print_struct_field(&mut self, field: &StructField) {
        self.print_doc(&field.doc);

        for attr in &field.attrs {
            self.print_attr(attr);
            self.newline();
            self.write_indent();
        }

        self.write_indent();

        // Visibility
        self.print_visibility(&field.vis);

        // Modifiers
        if field.is_final {
            self.write("final ");
        }
        if field.is_shared {
            self.write("shared ");
        }

        // Type and name
        self.print_name_path(&field.ty);
        self.write(" ");
        self.write(&field.name.0);
        self.write(";");
    }

    /// Print an interface declaration.
    fn print_interface(&mut self, iface: &InterfaceDecl) {
        self.print_doc(&iface.doc);

        for attr in &iface.attrs {
            self.print_attr(attr);
            self.newline();
        }

        self.write("interface ");
        self.write(&iface.name.0);
        self.print_generics(&iface.generics);

        if !iface.extends.is_empty() {
            self.write(" extends ");
            for (i, ext) in iface.extends.iter().enumerate() {
                if i > 0 {
                    self.write(", ");
                }
                self.print_name_path(ext);
            }
        }

        self.print_opening_brace();
        self.indent();

        for (i, method) in iface.methods.iter().enumerate() {
            if i > 0 {
                self.newline();
            }
            self.newline();
            self.print_interface_method(method);
        }

        self.dedent();
        self.newline();
        self.write_indent();
        self.write("}");
        self.newline();
    }

    /// Print an interface method.
    fn print_interface_method(&mut self, method: &InterfaceMethod) {
        self.write_indent();
        self.print_func_sig(&method.sig);

        if let Some(body) = &method.default_body {
            self.print_block(body);
        } else {
            self.write(";");
        }
    }

    /// Print an enum declaration.
    fn print_enum(&mut self, e: &EnumDecl) {
        self.print_doc(&e.doc);

        for attr in &e.attrs {
            self.print_attr(attr);
            self.newline();
        }

        if e.is_sealed {
            self.write("sealed ");
        }
        self.write("enum ");
        self.write(&e.name.0);
        self.print_generics(&e.generics);
        self.print_opening_brace();
        self.indent();

        for (i, variant) in e.variants.iter().enumerate() {
            self.newline();
            self.write_indent();
            self.print_enum_variant(variant);
            if i < e.variants.len() - 1 {
                self.write(",");
            }
        }

        self.dedent();
        self.newline();
        self.write_indent();
        self.write("}");
        self.newline();
    }

    /// Print an enum variant.
    fn print_enum_variant(&mut self, variant: &EnumVariant) {
        match variant {
            EnumVariant::Unit { name, discriminant } => {
                self.write(&name.0);
                if let Some(disc) = discriminant {
                    self.write(" = ");
                    self.print_expr(disc);
                }
            }
            EnumVariant::Tuple {
                name,
                types,
                discriminant,
            } => {
                self.write(&name.0);
                self.write("(");
                for (i, ty) in types.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.print_name_path(ty);
                }
                self.write(")");
                if let Some(disc) = discriminant {
                    self.write(" = ");
                    self.print_expr(disc);
                }
            }
        }
    }

    /// Print a provider declaration.
    fn print_provider(&mut self, p: &ProviderDecl) {
        self.print_doc(&p.doc);

        for attr in &p.attrs {
            self.print_attr(attr);
            self.newline();
        }

        self.write("provider ");
        self.write(&p.name.0);
        self.print_opening_brace();
        self.indent();

        for field in &p.fields {
            self.newline();
            self.print_struct_field(field);
        }

        self.dedent();
        self.newline();
        self.write_indent();
        self.write("}");
        self.newline();
    }

    /// Print a function declaration.
    fn print_function(&mut self, func: &FuncDecl) {
        self.print_doc(&func.sig.doc);

        for attr in &func.sig.attrs {
            self.write_indent();
            self.print_attr(attr);
            self.newline();
        }

        self.write_indent();
        self.print_func_sig(&func.sig);

        if let Some(body) = &func.body {
            self.print_block(body);
        } else {
            self.write(";");
        }
        self.newline();
    }

    /// Print a function signature.
    fn print_func_sig(&mut self, sig: &FuncSig) {
        self.print_visibility(&sig.vis);

        if sig.is_static {
            self.write("static ");
        }
        if sig.is_final {
            self.write("final ");
        }
        if sig.is_async {
            self.write("async ");
        }
        if sig.is_unsafe {
            self.write("unsafe ");
        }

        // Return type
        if let Some(ret) = &sig.ret {
            self.print_name_path(ret);
            self.write(" ");
        } else {
            self.write("void ");
        }

        self.write(&sig.name.0);
        self.print_generics(&sig.generics);
        self.write("(");

        for (i, param) in sig.params.iter().enumerate() {
            if i > 0 {
                self.write(", ");
            }
            self.print_name_path(&param.ty);
            self.write(" ");
            self.write(&param.name.0);
        }

        self.write(")");

        // Throws clause
        if !sig.throws.is_empty() {
            self.write(" throws (");
            for (i, ex) in sig.throws.iter().enumerate() {
                if i > 0 {
                    self.write(", ");
                }
                self.print_name_path(ex);
            }
            self.write(")");
        }
    }

    /// Print a type alias.
    fn print_type_alias(&mut self, t: &TypeAliasDecl) {
        self.print_doc(&t.doc);

        for attr in &t.attrs {
            self.print_attr(attr);
            self.newline();
        }

        self.write("type ");
        self.write(&t.name.0);
        self.write(" = ");
        self.print_name_path(&t.aliased);
        self.write(";");
        self.newline();
    }

    /// Print an extern function declaration.
    fn print_extern_func(&mut self, e: &ExternFuncDecl) {
        self.print_doc(&e.doc);

        for attr in &e.attrs {
            self.print_attr(attr);
            self.newline();
        }

        self.print_visibility(&e.vis);
        self.write("extern \"");
        self.write(&e.abi);
        self.write("\" fn ");
        self.write(&e.name.0);
        self.write("(");

        for (i, param) in e.params.iter().enumerate() {
            if i > 0 {
                self.write(", ");
            }
            self.print_name_path(&param.ty);
            self.write(" ");
            self.write(&param.name.0);
        }

        self.write(")");

        if let Some(ret) = &e.ret {
            self.write(" -> ");
            self.print_name_path(ret);
        }

        self.write(";");
        self.newline();
    }

    /// Print a block.
    fn print_block(&mut self, block: &Block) {
        self.print_opening_brace();
        self.indent();

        for stmt in &block.stmts {
            self.newline();
            self.write_indent();
            self.print_stmt(stmt);
        }

        self.dedent();
        self.newline();
        self.write_indent();
        self.write("}");
    }

    /// Print a statement.
    fn print_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::PrintStr(s) => {
                self.write("print ");
                self.write_string_literal(s);
                self.write(";");
            }
            Stmt::PrintExpr(e) => {
                self.write("print ");
                self.print_expr(e);
                self.write(";");
            }
            Stmt::PrintRawStr(s) => {
                self.write("printr ");
                self.write_string_literal(s);
                self.write(";");
            }
            Stmt::PrintRawExpr(e) => {
                self.write("printr ");
                self.print_expr(e);
                self.write(";");
            }
            Stmt::If {
                cond,
                then_blk,
                else_blk,
            } => {
                self.write("if (");
                self.print_expr(cond);
                self.write(")");
                self.print_block(then_blk);
                if let Some(else_blk) = else_blk {
                    self.write(" else");
                    self.print_block(else_blk);
                }
            }
            Stmt::While { cond, body } => {
                self.write("while (");
                self.print_expr(cond);
                self.write(")");
                self.print_block(body);
            }
            Stmt::For {
                init,
                cond,
                step,
                body,
            } => {
                self.write("for (");
                if let Some(init) = init {
                    self.print_stmt_inline(init);
                }
                self.write("; ");
                if let Some(cond) = cond {
                    self.print_expr(cond);
                }
                self.write("; ");
                if let Some(step) = step {
                    self.print_stmt_inline(step);
                }
                self.write(")");
                self.print_block(body);
            }
            Stmt::Labeled { label, stmt } => {
                self.write(&label.0);
                self.write(": ");
                self.print_stmt(stmt);
            }
            Stmt::Switch {
                expr,
                cases,
                pattern_cases,
                default,
            } => {
                self.write("switch (");
                self.print_expr(expr);
                self.write(")");
                self.print_opening_brace();
                self.indent();

                for (case_expr, case_block) in cases {
                    self.newline();
                    self.write_indent();
                    self.write("case ");
                    self.print_expr(case_expr);
                    self.write(":");
                    self.print_block(case_block);
                }

                for (pattern, block) in pattern_cases {
                    self.newline();
                    self.write_indent();
                    self.write("case ");
                    self.print_pattern(pattern);
                    self.write(":");
                    self.print_block(block);
                }

                if let Some(default) = default {
                    self.newline();
                    self.write_indent();
                    self.write("default:");
                    self.print_block(default);
                }

                self.dedent();
                self.newline();
                self.write_indent();
                self.write("}");
            }
            Stmt::Try {
                try_blk,
                catches,
                finally_blk,
            } => {
                self.write("try");
                self.print_block(try_blk);
                for catch in catches {
                    self.write(" catch");
                    if catch.ty.is_some() || catch.var.is_some() {
                        self.write(" (");
                        if let Some(ty) = &catch.ty {
                            self.print_name_path(ty);
                        }
                        if let Some(var) = &catch.var {
                            self.write(" ");
                            self.write(&var.0);
                        }
                        self.write(")");
                    }
                    self.print_block(&catch.blk);
                }
                if let Some(finally) = finally_blk {
                    self.write(" finally");
                    self.print_block(finally);
                }
            }
            Stmt::Assign { name, expr } => {
                self.write(&name.0);
                self.write(" = ");
                self.print_expr(expr);
                self.write(";");
            }
            Stmt::FieldAssign {
                object,
                field,
                expr,
            } => {
                self.print_expr(object);
                self.write(".");
                self.write(&field.0);
                self.write(" = ");
                self.print_expr(expr);
                self.write(";");
            }
            Stmt::AssignOp { name, op, expr } => {
                self.write(&name.0);
                self.write(" ");
                self.print_assign_op(op);
                self.write(" ");
                self.print_expr(expr);
                self.write(";");
            }
            Stmt::VarDecl {
                is_final,
                is_shared,
                ty,
                generics,
                fn_params: _,
                name,
                init,
            } => {
                if *is_final {
                    self.write("final ");
                }
                if *is_shared {
                    self.write("shared ");
                }
                self.print_name_path(ty);
                if !generics.is_empty() {
                    self.write("<");
                    for (i, g) in generics.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.print_name_path(g);
                    }
                    self.write(">");
                }
                self.write(" ");
                self.write(&name.0);
                if let Some(init) = init {
                    self.write(" = ");
                    self.print_expr(init);
                }
                self.write(";");
            }
            Stmt::Break(label) => {
                self.write("break");
                if let Some(label) = label {
                    self.write(" ");
                    self.write(&label.0);
                }
                self.write(";");
            }
            Stmt::Continue(label) => {
                self.write("continue");
                if let Some(label) = label {
                    self.write(" ");
                    self.write(&label.0);
                }
                self.write(";");
            }
            Stmt::Return(expr) => {
                self.write("return");
                if let Some(e) = expr {
                    self.write(" ");
                    self.print_expr(e);
                }
                self.write(";");
            }
            Stmt::Throw(expr) => {
                self.write("throw ");
                self.print_expr(expr);
                self.write(";");
            }
            Stmt::Panic(expr) => {
                self.write("panic(");
                self.print_expr(expr);
                self.write(");");
            }
            Stmt::Block(block) => {
                self.print_block(block);
            }
            Stmt::Expr(expr) => {
                self.print_expr(expr);
                self.write(";");
            }
            Stmt::Unsafe(block) => {
                self.write("unsafe");
                self.print_block(block);
            }
        }
    }

    /// Print a statement inline (without trailing semicolon, for for-loop).
    fn print_stmt_inline(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Assign { name, expr } => {
                self.write(&name.0);
                self.write(" = ");
                self.print_expr(expr);
            }
            Stmt::AssignOp { name, op, expr } => {
                self.write(&name.0);
                self.write(" ");
                self.print_assign_op(op);
                self.write(" ");
                self.print_expr(expr);
            }
            Stmt::VarDecl {
                is_final,
                is_shared,
                ty,
                generics,
                fn_params: _,
                name,
                init,
            } => {
                if *is_final {
                    self.write("final ");
                }
                if *is_shared {
                    self.write("shared ");
                }
                self.print_name_path(ty);
                if !generics.is_empty() {
                    self.write("<");
                    for (i, g) in generics.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.print_name_path(g);
                    }
                    self.write(">");
                }
                self.write(" ");
                self.write(&name.0);
                if let Some(init) = init {
                    self.write(" = ");
                    self.print_expr(init);
                }
            }
            Stmt::Expr(expr) => {
                self.print_expr(expr);
            }
            _ => self.print_stmt(stmt),
        }
    }

    /// Print a pattern.
    fn print_pattern(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Wildcard => self.write("_"),
            Pattern::Binding(id) => self.write(&id.0),
            Pattern::Literal(expr) => self.print_expr(expr),
            Pattern::Variant {
                enum_ty,
                variant,
                payloads,
                ..
            } => {
                self.print_name_path(enum_ty);
                self.write(".");
                self.write(&variant.0);
                if !payloads.is_empty() {
                    self.write("(");
                    for (i, p) in payloads.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.print_pattern(p);
                    }
                    self.write(")");
                }
            }
        }
    }

    /// Print an expression.
    fn print_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Int(n) => self.write(&n.to_string()),
            Expr::Float(f) => self.write(&format!("{:?}", f)),
            Expr::Str(s) => self.write_string_literal(s),
            Expr::Char(c) => {
                self.write("'");
                self.write_char_escaped(*c);
                self.write("'");
            }
            Expr::Bool(b) => self.write(if *b { "true" } else { "false" }),
            Expr::Ident(id) => self.write(&id.0),
            Expr::Await(e) => {
                self.write("await ");
                self.print_expr(e);
            }
            Expr::Cast(ty, e) => {
                self.write("(");
                self.print_name_path(ty);
                self.write(")");
                self.print_expr(e);
            }
            Expr::FnLiteral(params, body) => {
                self.write("fn(");
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.print_name_path(&p.ty);
                    self.write(" ");
                    self.write(&p.name.0);
                }
                self.write(")");
                self.print_block(body);
            }
            Expr::Ternary(cond, then_expr, else_expr) => {
                self.print_expr(cond);
                self.write(" ? ");
                self.print_expr(then_expr);
                self.write(" : ");
                self.print_expr(else_expr);
            }
            Expr::Binary(left, op, right) => {
                self.print_expr(left);
                self.write(" ");
                self.print_bin_op(op);
                self.write(" ");
                self.print_expr(right);
            }
            Expr::Unary(op, e) => {
                self.print_un_op(op);
                self.print_expr(e);
            }
            Expr::Call(callee, args) => {
                self.print_expr(callee);
                self.write("(");
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.print_expr(arg);
                }
                self.write(")");
            }
            Expr::Member(obj, field) => {
                self.print_expr(obj);
                self.write(".");
                self.write(&field.0);
            }
            Expr::OptionalMember(obj, field) => {
                self.print_expr(obj);
                self.write("?.");
                self.write(&field.0);
            }
            Expr::Index(obj, idx) => {
                self.print_expr(obj);
                self.write("[");
                self.print_expr(idx);
                self.write("]");
            }
            Expr::ListLit(items) => {
                self.write("[");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.print_expr(item);
                }
                self.write("]");
            }
            Expr::MapLit { pairs, spread } => {
                self.write("{");
                if self.config.spaces_in_braces && (!pairs.is_empty() || spread.is_some()) {
                    self.write(" ");
                }
                let mut first = true;
                if let Some(s) = spread {
                    self.write("..");
                    self.print_expr(s);
                    first = false;
                }
                for (k, v) in pairs {
                    if !first {
                        self.write(", ");
                    }
                    self.print_expr(k);
                    self.write(": ");
                    self.print_expr(v);
                    first = false;
                }
                if self.config.spaces_in_braces && (!pairs.is_empty() || spread.is_some()) {
                    self.write(" ");
                }
                self.write("}");
            }
            Expr::StructLit {
                type_name,
                fields,
                spread,
            } => {
                self.print_expr(type_name);
                self.write(" {");
                if self.config.spaces_in_braces && (!fields.is_empty() || spread.is_some()) {
                    self.write(" ");
                }
                let mut first = true;
                if let Some(s) = spread {
                    self.write("..");
                    self.print_expr(s);
                    first = false;
                }
                for (name, val) in fields {
                    if !first {
                        self.write(", ");
                    }
                    self.write(&name.0);
                    self.write(": ");
                    self.print_expr(val);
                    first = false;
                }
                if self.config.spaces_in_braces && (!fields.is_empty() || spread.is_some()) {
                    self.write(" ");
                }
                self.write("}");
            }
        }
    }

    /// Print a binary operator.
    fn print_bin_op(&mut self, op: &BinOp) {
        let s = match op {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::And => "&&",
            BinOp::Or => "||",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
        };
        self.write(s);
    }

    /// Print a unary operator.
    fn print_un_op(&mut self, op: &UnOp) {
        let s = match op {
            UnOp::Neg => "-",
            UnOp::Not => "!",
        };
        self.write(s);
    }

    /// Print an assignment operator.
    fn print_assign_op(&mut self, op: &AssignOp) {
        let s = match op {
            AssignOp::Add => "+=",
            AssignOp::Sub => "-=",
            AssignOp::Mul => "*=",
            AssignOp::Div => "/=",
            AssignOp::Mod => "%=",
            AssignOp::Shl => "<<=",
            AssignOp::Shr => ">>=",
            AssignOp::And => "&=",
            AssignOp::Or => "|=",
            AssignOp::Xor => "^=",
        };
        self.write(s);
    }

    /// Print visibility.
    fn print_visibility(&mut self, vis: &Visibility) {
        match vis {
            Visibility::Default => {}
            Visibility::Public => self.write("public "),
            Visibility::Internal => self.write("internal "),
            Visibility::Private => self.write("private "),
        }
    }

    /// Print a name path.
    fn print_name_path(&mut self, np: &NamePath) {
        let path = np
            .path
            .iter()
            .map(|i| i.0.as_str())
            .collect::<Vec<_>>()
            .join(".");
        self.write(&path);

        if !np.type_args.is_empty() {
            self.write("<");
            for (i, arg) in np.type_args.iter().enumerate() {
                if i > 0 {
                    self.write(", ");
                }
                self.print_name_path(arg);
            }
            self.write(">");
        }
    }

    /// Print generic parameters.
    fn print_generics(&mut self, generics: &[GenericParam]) {
        if generics.is_empty() {
            return;
        }
        self.write("<");
        for (i, g) in generics.iter().enumerate() {
            if i > 0 {
                self.write(", ");
            }
            self.write(&g.name.0);
            if let Some(bound) = &g.bound {
                self.write(": ");
                self.print_name_path(bound);
            }
        }
        self.write(">");
    }

    /// Print an attribute.
    fn print_attr(&mut self, attr: &Attr) {
        self.write("@");
        self.print_name_path(&attr.name);
        if let Some(args) = &attr.args {
            self.write("(");
            self.write(args);
            self.write(")");
        }
    }

    /// Print a doc comment.
    fn print_doc(&mut self, doc: &Option<String>) {
        if let Some(doc) = doc {
            for line in doc.lines() {
                self.write_indent();
                self.write("/// ");
                self.write(line);
                self.newline();
            }
        }
    }

    /// Print opening brace according to style.
    fn print_opening_brace(&mut self) {
        match self.config.brace_style {
            BraceStyle::SameLine => {
                self.write(" {");
            }
            BraceStyle::NextLine => {
                self.newline();
                self.write_indent();
                self.write("{");
            }
        }
    }

    /// Write a string literal with escapes.
    fn write_string_literal(&mut self, s: &str) {
        self.write("\"");
        for c in s.chars() {
            self.write_char_escaped(c);
        }
        self.write("\"");
    }

    /// Write a character with proper escaping.
    fn write_char_escaped(&mut self, c: char) {
        match c {
            '\n' => self.write("\\n"),
            '\r' => self.write("\\r"),
            '\t' => self.write("\\t"),
            '\\' => self.write("\\\\"),
            '"' => self.write("\\\""),
            '\'' => self.write("\\'"),
            c if c.is_ascii_control() => {
                self.write(&format!("\\x{:02x}", c as u32));
            }
            c => self.write(&c.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_printer_simple() {
        let config = FormatConfig::default();
        let mut printer = Printer::new(config);

        printer.write("hello");
        assert_eq!(printer.output, "hello");
    }

    #[test]
    fn test_printer_indent() {
        let config = FormatConfig::new().with_indent(2);
        let mut printer = Printer::new(config);

        printer.write("start");
        printer.newline();
        printer.indent();
        printer.write_indent();
        printer.write("indented");

        assert_eq!(printer.output, "start\n  indented");
    }
}
