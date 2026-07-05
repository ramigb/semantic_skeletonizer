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
}

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

fn extract_ir(program: &Program<'_>) -> FileSkeleton {
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
    ir
}

pub fn skeletonize_source(source_text: &str, path: &Path) -> Result<FileSkeleton> {
    let allocator = Allocator::default();
    let mut program = parse_source(&allocator, source_text, path)?;

    let mut skeletonizer = Skeletonizer;
    skeletonizer.visit_program(&mut program);

    Ok(extract_ir(&program))
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
