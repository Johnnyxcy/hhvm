//
// Copyright (c) Facebook, Inc. and its affiliates.
// This source code is licensed under the MIT license found in the
// LICENSE file in the "hack" directory of this source tree.

use crate::{Env, EnvFlags, ParseError, Profile};
// use crate::compile_rust as compile;
use ocamlrep::rc::RcOc;
use options::{LangFlags, Options};
use oxidized::namespace_env::Env as NamespaceEnv;
use oxidized::pos::Pos;
use oxidized::{
    aast,
    aast_visitor::{AstParams, Node, Visitor},
    ast,
};
use parser_core_types::source_text::SourceText;
use std::fs;

struct ExprTreeLiteralExtractor {
    literals: Vec<(Pos, ast::ExpressionTree)>,
}

impl<'ast> Visitor<'ast> for ExprTreeLiteralExtractor {
    type Params = AstParams<(), ()>;

    fn object(&mut self) -> &mut dyn Visitor<'ast, Params = Self::Params> {
        self
    }

    fn visit_expr(&mut self, env: &mut (), e: &ast::Expr) -> Result<(), ()> {
        use aast::Expr_;
        match &e.2 {
            Expr_::ExpressionTree(et) => {
                self.literals.push((e.1.clone(), (&**et).clone()));
            }
            _ => e.recurse(env, self)?,
        }
        Ok(())
    }
}

/// Extract the expression tree literals in `program` along with their
/// positions.
///
/// Given the code:
///
/// ````
/// function foo(): void {
///   $c = Code`bar()`;
/// }
/// ```
///
/// We want the `` Code`bar()` `` part.
fn find_et_literals(program: ast::Program) -> Vec<(Pos, ast::ExpressionTree)> {
    let mut visitor = ExprTreeLiteralExtractor { literals: vec![] };
    for def in program {
        visitor
            .visit_def(&mut (), &def)
            .expect("Failed to extract expression tree literals");
    }

    visitor.literals
}

fn sort_by_start_pos<T>(items: &mut Vec<(Pos, T)>) {
    items.sort_by(|(p1, _), (p2, _)| p1.start_offset().cmp(&p2.start_offset()));
}

/// The source code of `program` with expression tree literals
/// replaced with their desugared form.
fn desugar_and_replace_et_literals<S: AsRef<str>>(
    env: &Env<S>,
    program: ast::Program,
    src: &str,
) -> String {
    let mut literals = find_et_literals(program);
    sort_by_start_pos(&mut literals);

    // Start from the last literal in the source code, so earlier
    // positions stay valid after string replacements.
    literals.reverse();

    let mut src = src.to_string();
    for (pos, literal) in literals {
        let desugared_literal_src = crate::expr_to_string_lossy(env, &literal.runtime_expr);
        let (pos_start, pos_end) = pos.info_raw();
        src.replace_range(pos_start..pos_end, &desugared_literal_src);
    }

    src
}

/// Parse the file in `env`, desugar expression tree literals, and
/// print the source code as if the user manually wrote the desugared
/// syntax.
pub fn desugar_and_print<S: AsRef<str>>(env: &Env<S>) {
    let is_systemlib = env.flags.contains(EnvFlags::IS_SYSTEMLIB);
    let opts = Options::from_configs(&env.config_jsons, &env.config_list).expect("Invalid options");
    let filepath = env.filepath.clone();
    let content = fs::read(filepath.to_absolute()).unwrap();
    let source_text = SourceText::make(RcOc::new(filepath), &content);
    let ns = RcOc::new(NamespaceEnv::empty(
        opts.hhvm.aliased_namespaces_cloned().collect(),
        true,
        opts.hhvm
            .hack_lang
            .flags
            .contains(LangFlags::DISABLE_XHP_ELEMENT_MANGLING),
    ));
    match crate::parse_file(
        &opts,
        source_text,
        false,
        ns,
        is_systemlib,
        &mut Profile::default(),
    ) {
        Err(ParseError(_, msg, _)) => panic!("Parsing failed: {}", msg),
        Ok(ast) => {
            let old_src = String::from_utf8_lossy(&content);
            let new_src = desugar_and_replace_et_literals(env, ast, &old_src);
            print!("{}", new_src);
        }
    }
}
