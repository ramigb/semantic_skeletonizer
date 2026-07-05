use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast_visit::VisitMut;
use oxc_codegen::{Codegen, Gen};
use oxc_parser::Parser;
use oxc_span::SourceType;
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

pub fn get_implementation(path: &str, _target_node: &str) -> Result<serde_json::Value> {
    let allocator = Allocator::default();
    let path_buf = std::path::PathBuf::from(path);
    let source_text = std::fs::read_to_string(&path_buf)?;
    let program = parse_source(&allocator, &source_text, &path_buf)?;
    Ok(serde_json::json!(format!("{:#?}", program)))
}
