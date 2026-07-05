use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast_visit::VisitMut;
use oxc_codegen::{Codegen, Gen};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::scope::ScopeFlags;

// --- IR STRUCTURES ---

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct FileSkeleton {
    pub imports: Vec<String>,
    pub exports: Vec<String>,
    pub functions: Vec<String>,
    pub interfaces: Vec<String>,
    pub classes: Vec<String>,
    pub variables: Vec<String>,
    pub symbols: Vec<SymbolInfo>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SymbolInfo {
    pub name: String,
    /// function | arrow_function | class | method | interface | type | enum |
    /// variable | component
    pub kind: String,
    pub exported: bool,
    pub signature: String,
}

pub const CALLABLE_KINDS: &[&str] = &["function", "arrow_function", "method", "component"];

// --- SKELETONIZER ---

pub struct Skeletonizer;

impl<'a> VisitMut<'a> for Skeletonizer {
    fn visit_function(&mut self, func: &mut Function<'a>, flags: ScopeFlags) {
        if let Some(body) = &mut func.body {
            body.statements.clear();
        }
        oxc_ast_visit::walk_mut::walk_function(self, func, flags);
    }

    fn visit_arrow_function_expression(&mut self, expr: &mut ArrowFunctionExpression<'a>) {
        expr.body.statements.clear();
        oxc_ast_visit::walk_mut::walk_arrow_function_expression(self, expr);
    }

    fn visit_program(&mut self, program: &mut Program<'a>) {
        program.body.retain(|stmt| {
            if let Statement::ImportDeclaration(import) = stmt {
                let src = import.source.value.as_str();
                if src.contains(".css") || src.contains(".scss") || src.contains(".svg") {
                    return false;
                }
            }
            true
        });

        for stmt in program.body.iter_mut() {
            self.visit_statement(stmt);
        }
    }
}

// --- CORE ---

pub fn parse_source<'a>(
    allocator: &'a Allocator,
    source_text: &'a str,
    path: &Path,
) -> Result<Program<'a>> {
    let source_type = SourceType::from_path(path).unwrap_or_default();
    let ret = Parser::new(allocator, source_text, source_type).parse();

    if ret.errors.is_empty() {
        Ok(ret.program)
    } else {
        Err(anyhow::anyhow!("failed to parse module: {:?}", ret.errors))
    }
}

pub fn stringify_item<T: Gen>(item: &T) -> String {
    let mut codegen = Codegen::new();
    item.r#gen(&mut codegen, oxc_codegen::Context::default());
    codegen.into_source_text()
}

/// Collapse a skeletonized node into a single-line signature.
fn one_line(s: &str) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.len() > 200 {
        let mut cut = 200;
        while !joined.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}…", &joined[..cut])
    } else {
        joined
    }
}

fn is_pascal_case(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

/// True when a binding's type annotation references `React.FC` / `FC`.
fn annotation_is_fc(decl: &VariableDeclarator<'_>, source_text: &str) -> bool {
    decl.type_annotation.as_ref().is_some_and(|ann| {
        let span = ann.span();
        let text = &source_text[span.start as usize..span.end as usize];
        text.contains("React.FC")
            || text.contains("FC<")
            || text.trim_start_matches(':').trim() == "FC"
    })
}

struct SymbolContext<'s> {
    source_text: &'s str,
    is_tsx: bool,
}

fn component_or(ctx: &SymbolContext, name: &str, fallback: &str) -> String {
    if ctx.is_tsx && is_pascal_case(name) {
        "component".to_string()
    } else {
        fallback.to_string()
    }
}

fn collect_decl_symbols(
    decl: &Declaration<'_>,
    exported: bool,
    ctx: &SymbolContext,
    out: &mut Vec<SymbolInfo>,
) {
    match decl {
        Declaration::FunctionDeclaration(f) => {
            if let Some(id) = &f.id {
                out.push(SymbolInfo {
                    name: id.name.to_string(),
                    kind: component_or(ctx, &id.name, "function"),
                    exported,
                    signature: one_line(&stringify_item(&**f)),
                });
            }
        }
        Declaration::ClassDeclaration(c) => {
            if let Some(name) = c.id.as_ref().map(|id| id.name.to_string()) {
                out.push(SymbolInfo {
                    name: name.clone(),
                    kind: "class".to_string(),
                    exported,
                    signature: one_line(&stringify_item(&**c)),
                });
                for el in &c.body.body {
                    if let ClassElement::MethodDefinition(m) = el {
                        if let Some(mn) = m.key.static_name() {
                            out.push(SymbolInfo {
                                name: format!("{}.{}", name, mn),
                                kind: "method".to_string(),
                                exported,
                                signature: one_line(&stringify_item(&**m)),
                            });
                        }
                    }
                }
            }
        }
        Declaration::VariableDeclaration(v) => {
            for d in &v.declarations {
                let Some(name) = d.id.get_identifier_name() else {
                    continue;
                };
                let is_component = ctx.is_tsx
                    && (is_pascal_case(&name) || annotation_is_fc(d, ctx.source_text));
                let kind = match &d.init {
                    Some(Expression::ArrowFunctionExpression(_)) if is_component => "component",
                    Some(Expression::FunctionExpression(_)) if is_component => "component",
                    Some(Expression::ArrowFunctionExpression(_)) => "arrow_function",
                    Some(Expression::FunctionExpression(_)) => "function",
                    _ => "variable",
                };
                out.push(SymbolInfo {
                    name: name.to_string(),
                    kind: kind.to_string(),
                    exported,
                    signature: one_line(&format!("{} {}", v.kind.as_str(), stringify_item(d))),
                });
            }
        }
        Declaration::TSInterfaceDeclaration(i) => out.push(SymbolInfo {
            name: i.id.name.to_string(),
            kind: "interface".to_string(),
            exported,
            signature: one_line(&stringify_item(&**i)),
        }),
        Declaration::TSTypeAliasDeclaration(t) => out.push(SymbolInfo {
            name: t.id.name.to_string(),
            kind: "type".to_string(),
            exported,
            signature: one_line(&stringify_item(&**t)),
        }),
        Declaration::TSEnumDeclaration(e) => out.push(SymbolInfo {
            name: e.id.name.to_string(),
            kind: "enum".to_string(),
            exported,
            signature: one_line(&stringify_item(&**e)),
        }),
        _ => {}
    }
}

fn collect_symbols(program: &Program<'_>, ctx: &SymbolContext) -> Vec<SymbolInfo> {
    let mut out = Vec::new();
    for stmt in &program.body {
        match stmt {
            Statement::ExportNamedDeclaration(e) => {
                if let Some(d) = &e.declaration {
                    collect_decl_symbols(d, true, ctx, &mut out);
                }
            }
            Statement::ExportDefaultDeclaration(e) => {
                let (name, kind, sig) = match &e.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                        let name = f
                            .id
                            .as_ref()
                            .map(|id| id.name.to_string())
                            .unwrap_or_else(|| "default".to_string());
                        let kind = component_or(ctx, &name, "function");
                        (name, kind, one_line(&stringify_item(&**f)))
                    }
                    ExportDefaultDeclarationKind::ClassDeclaration(c) => {
                        let name = c
                            .id
                            .as_ref()
                            .map(|id| id.name.to_string())
                            .unwrap_or_else(|| "default".to_string());
                        (name, "class".to_string(), one_line(&stringify_item(&**c)))
                    }
                    _ => (
                        "default".to_string(),
                        "variable".to_string(),
                        one_line(&stringify_item(&**e)),
                    ),
                };
                out.push(SymbolInfo {
                    name,
                    kind,
                    exported: true,
                    signature: sig,
                });
            }
            _ => {
                if let Some(d) = stmt.as_declaration() {
                    collect_decl_symbols(d, false, ctx, &mut out);
                }
            }
        }
    }
    out
}

fn extract_ir(program: &Program<'_>, ctx: &SymbolContext) -> FileSkeleton {
    let mut ir = FileSkeleton::default();
    for stmt in &program.body {
        match stmt {
            Statement::ImportDeclaration(decl) => ir.imports.push(stringify_item(&**decl)),
            Statement::ExportNamedDeclaration(decl) => ir.exports.push(stringify_item(&**decl)),
            Statement::ExportDefaultDeclaration(decl) => ir.exports.push(stringify_item(&**decl)),
            Statement::ExportAllDeclaration(decl) => ir.exports.push(stringify_item(&**decl)),
            Statement::TSImportEqualsDeclaration(decl) => ir.imports.push(stringify_item(&**decl)),
            Statement::TSExportAssignment(decl) => ir.exports.push(stringify_item(&**decl)),
            Statement::TSNamespaceExportDeclaration(decl) => {
                ir.exports.push(stringify_item(&**decl))
            }

            Statement::ClassDeclaration(decl) => ir.classes.push(stringify_item(&**decl)),
            Statement::FunctionDeclaration(decl) => ir.functions.push(stringify_item(&**decl)),
            Statement::VariableDeclaration(decl) => ir.variables.push(stringify_item(&**decl)),
            Statement::TSInterfaceDeclaration(decl) => ir.interfaces.push(stringify_item(&**decl)),
            Statement::TSTypeAliasDeclaration(decl) => ir.interfaces.push(stringify_item(&**decl)),
            Statement::TSEnumDeclaration(decl) => ir.interfaces.push(stringify_item(&**decl)),
            Statement::TSModuleDeclaration(decl) => ir.interfaces.push(stringify_item(&**decl)),

            _ => {}
        }
    }
    ir.symbols = collect_symbols(program, ctx);
    ir
}

pub fn skeletonize_source(source_text: &str, path: &Path) -> Result<FileSkeleton> {
    let allocator = Allocator::default();
    let mut program = parse_source(&allocator, source_text, path)?;

    let mut skeletonizer = Skeletonizer;
    skeletonizer.visit_program(&mut program);

    let ctx = SymbolContext {
        source_text,
        is_tsx: path.extension().and_then(|e| e.to_str()) == Some("tsx"),
    };
    Ok(extract_ir(&program, &ctx))
}

pub fn skeletonize_file(path: &Path) -> Result<FileSkeleton> {
    let source_text = std::fs::read_to_string(path).context("failed to load file")?;
    skeletonize_source(&source_text, path)
}

pub enum ImplLookup {
    /// Original source text of the requested node, sliced by span.
    Found(String),
    /// Node not found; carries the available top-level symbol names.
    NotFound(Vec<String>),
}

pub fn get_implementation(path: &Path, target_node: &str) -> Result<ImplLookup> {
    let allocator = Allocator::default();
    let source_text = std::fs::read_to_string(path).context("failed to load file")?;
    let program = parse_source(&allocator, &source_text, path)?;

    match find_target_span(&program, target_node) {
        Some(span) => Ok(ImplLookup::Found(
            source_text[span.start as usize..span.end as usize].to_string(),
        )),
        None => Ok(ImplLookup::NotFound(top_level_names(&program))),
    }
}

fn class_name<'a>(class: &Class<'a>) -> Option<&'a str> {
    class.id.as_ref().map(|id| id.name.as_str())
}

fn method_span(class: &Class<'_>, method: &str) -> Option<Span> {
    class.body.body.iter().find_map(|el| match el {
        ClassElement::MethodDefinition(m)
            if m.key.static_name().is_some_and(|n| n == method) =>
        {
            Some(m.span)
        }
        _ => None,
    })
}

/// Match `target` against a top-level declaration. `outer_span` is the span of
/// the enclosing statement (so `export function foo` slices include `export`).
fn match_declaration(decl: &Declaration<'_>, target: &str, outer_span: Span) -> Option<Span> {
    let (class_part, method_part) = match target.split_once('.') {
        Some((c, m)) => (Some(c), Some(m)),
        None => (None, None),
    };

    match decl {
        Declaration::FunctionDeclaration(f) => {
            (f.id.as_ref().is_some_and(|id| id.name == target)).then_some(outer_span)
        }
        Declaration::ClassDeclaration(c) => {
            if class_name(c) == Some(target) {
                return Some(outer_span);
            }
            if let (Some(cls), Some(m)) = (class_part, method_part) {
                if class_name(c) == Some(cls) {
                    return method_span(c, m);
                }
            }
            None
        }
        Declaration::VariableDeclaration(v) => v.declarations.iter().find_map(|d| {
            if d.id.get_identifier_name().is_some_and(|n| n == target) {
                if v.declarations.len() == 1 {
                    Some(outer_span)
                } else {
                    Some(d.span)
                }
            } else {
                None
            }
        }),
        Declaration::TSInterfaceDeclaration(i) => {
            (i.id.name == target).then_some(outer_span)
        }
        Declaration::TSTypeAliasDeclaration(t) => (t.id.name == target).then_some(outer_span),
        Declaration::TSEnumDeclaration(e) => (e.id.name == target).then_some(outer_span),
        _ => None,
    }
}

fn find_target_span(program: &Program<'_>, target: &str) -> Option<Span> {
    for stmt in &program.body {
        let outer_span = stmt.span();
        let found = match stmt {
            Statement::FunctionDeclaration(_)
            | Statement::ClassDeclaration(_)
            | Statement::VariableDeclaration(_)
            | Statement::TSInterfaceDeclaration(_)
            | Statement::TSTypeAliasDeclaration(_)
            | Statement::TSEnumDeclaration(_) => stmt
                .as_declaration()
                .and_then(|d| match_declaration(d, target, outer_span)),
            Statement::ExportNamedDeclaration(e) => e
                .declaration
                .as_ref()
                .and_then(|d| match_declaration(d, target, outer_span)),
            Statement::ExportDefaultDeclaration(e) => {
                let named = match &e.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                        f.id.as_ref().is_some_and(|id| id.name == target)
                    }
                    ExportDefaultDeclarationKind::ClassDeclaration(c) => {
                        class_name(c) == Some(target)
                    }
                    _ => false,
                };
                (target == "default" || named).then_some(outer_span)
            }
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn declaration_names(decl: &Declaration<'_>, out: &mut Vec<String>) {
    match decl {
        Declaration::FunctionDeclaration(f) => {
            if let Some(id) = &f.id {
                out.push(id.name.to_string());
            }
        }
        Declaration::ClassDeclaration(c) => {
            if let Some(name) = class_name(c) {
                out.push(name.to_string());
                for el in &c.body.body {
                    if let ClassElement::MethodDefinition(m) = el {
                        if let Some(mn) = m.key.static_name() {
                            out.push(format!("{}.{}", name, mn));
                        }
                    }
                }
            }
        }
        Declaration::VariableDeclaration(v) => {
            for d in &v.declarations {
                if let Some(name) = d.id.get_identifier_name() {
                    out.push(name.to_string());
                }
            }
        }
        Declaration::TSInterfaceDeclaration(i) => out.push(i.id.name.to_string()),
        Declaration::TSTypeAliasDeclaration(t) => out.push(t.id.name.to_string()),
        Declaration::TSEnumDeclaration(e) => out.push(e.id.name.to_string()),
        _ => {}
    }
}

/// All addressable top-level symbol names in a file (used for actionable
/// "not found" errors).
pub fn top_level_names(program: &Program<'_>) -> Vec<String> {
    let mut names = Vec::new();
    for stmt in &program.body {
        match stmt {
            Statement::ExportNamedDeclaration(e) => {
                if let Some(d) = &e.declaration {
                    declaration_names(d, &mut names);
                }
            }
            Statement::ExportDefaultDeclaration(_) => names.push("default".to_string()),
            _ => {
                if let Some(d) = stmt.as_declaration() {
                    declaration_names(d, &mut names);
                }
            }
        }
    }
    names
}
