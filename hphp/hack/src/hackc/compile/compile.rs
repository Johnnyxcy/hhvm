// Copyright (c) Facebook, Inc. and its affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the "hack" directory of this source tree.

pub mod dump_expr_tree;

use aast_parser::{
    rust_aast_parser_types::{Env as AastEnv, ParserResult},
    AastParser, Error as AastError,
};
use anyhow::anyhow;
use bitflags::bitflags;
use bytecode_printer::{print_unit, Context};
use decl_provider::DeclProvider;
use emit_unit::{emit_unit, FromAstFlags};
use env::emitter::Emitter;
use error::{Error, ErrorKind};
use hhbc::{hackc_unit::HackCUnit, FatalOp};
use ocamlrep::{rc::RcOc, FromError, FromOcamlRep, Value};
use ocamlrep_derive::{FromOcamlRep, ToOcamlRep};
use options::{Arg, HackLang, Hhvm, HhvmFlags, LangFlags, Options, Php7Flags, RepoFlags};
use oxidized::{
    ast, namespace_env::Env as NamespaceEnv, parser_options::ParserOptions, pos::Pos,
    relative_path::RelativePath,
};
use parser_core_types::{
    indexed_source_text::IndexedSourceText, source_text::SourceText, syntax_error::ErrorType,
};
use rewrite_program::rewrite_program;
use thiserror::Error;

/// Common input needed for compilation.  Extra care is taken
/// so that everything is easily serializable at the FFI boundary
/// until the migration from OCaml is fully complete
#[derive(Debug, FromOcamlRep)]
pub struct Env<S> {
    pub filepath: RelativePath,
    pub config_jsons: Vec<S>,
    pub config_list: Vec<S>,
    pub flags: EnvFlags,
}

#[derive(Debug)]
pub struct NativeEnv<S> {
    pub filepath: RelativePath,
    pub aliased_namespaces: S,
    pub include_roots: S,
    pub emit_class_pointers: i32,
    pub check_int_overflow: i32,
    pub hhbc_flags: HHBCFlags,
    pub parser_flags: ParserFlags,
    pub flags: EnvFlags,
}

bitflags! {
    // Note: these flags are intentionally packed into bits to overcome
    // the limitation of to-OCaml FFI functions having at most 5 parameters
    pub struct EnvFlags: u8 {
        const IS_SYSTEMLIB = 1 << 0;
        const IS_EVALED = 1 << 1;
        const FOR_DEBUGGER_EVAL = 1 << 2;
        const UNUSED_PLACEHOLDER = 1 << 3;
        const DISABLE_TOPLEVEL_ELABORATION = 1 << 4;
    }
}

// Keep in sync with compiler_ffi.rs
bitflags! {
      pub struct HHBCFlags: u32 {
        const LTR_ASSIGN=1 << 0;
        const UVS=1 << 1;
        // No longer using bit 3.
        const AUTHORITATIVE=1 << 4;
        const JIT_ENABLE_RENAME_FUNCTION=1 << 5;
        const LOG_EXTERN_COMPILER_PERF=1 << 6;
        const ENABLE_INTRINSICS_EXTENSION=1 << 7;
        // No longer using bit 8.
        // No longer using bit 9.
        const EMIT_CLS_METH_POINTERS=1 << 10;
        const EMIT_METH_CALLER_FUNC_POINTERS=1 << 11;
        const ENABLE_IMPLICIT_CONTEXT=1 << 12;
        const ARRAY_PROVENANCE=1 << 13;
        // No longer using bit 14.
        const FOLD_LAZY_CLASS_KEYS=1 << 15;
        // No longer using bit 16.
    }
}

// Mapping must match getParserFlags() in runtime-option.cpp and compiler_ffi.rs
bitflags! {
    pub struct ParserFlags: u32 {
        const ABSTRACT_STATIC_PROPS=1 << 0;
        const ALLOW_NEW_ATTRIBUTE_SYNTAX=1 << 1;
        const ALLOW_UNSTABLE_FEATURES=1 << 2;
        const CONST_DEFAULT_FUNC_ARGS=1 << 3;
        const CONST_STATIC_PROPS=1 << 4;
        // No longer using bits 5-7
        const DISABLE_LVAL_AS_AN_EXPRESSION=1 << 8;
        // No longer using bit 9
        const DISALLOW_INST_METH=1 << 10;
        const DISABLE_XHP_ELEMENT_MANGLING=1 << 11;
        const DISALLOW_FUN_AND_CLS_METH_PSEUDO_FUNCS=1 << 12;
        const DISALLOW_FUNC_PTRS_IN_CONSTANTS=1 << 13;
        // No longer using bit 14.
        // No longer using bit 15.
        const ENABLE_ENUM_CLASSES=1 << 16;
        const ENABLE_XHP_CLASS_MODIFIER=1 << 17;
        // No longer using bits 18-19.
        const ENABLE_CLASS_LEVEL_WHERE_CLAUSES=1 << 20;
  }
}

impl FromOcamlRep for EnvFlags {
    fn from_ocamlrep(value: Value<'_>) -> Result<Self, FromError> {
        Ok(EnvFlags::from_bits(value.as_int().unwrap() as u8).unwrap())
    }
}

impl HHBCFlags {
    fn to_php7_flags(self) -> Php7Flags {
        let mut f = Php7Flags::empty();
        if self.contains(HHBCFlags::UVS) {
            f |= Php7Flags::UVS;
        }
        if self.contains(HHBCFlags::LTR_ASSIGN) {
            f |= Php7Flags::LTR_ASSIGN;
        }
        f
    }

    fn to_hhvm_flags(self) -> HhvmFlags {
        let mut f = HhvmFlags::empty();
        if self.contains(HHBCFlags::ARRAY_PROVENANCE) {
            f |= HhvmFlags::ARRAY_PROVENANCE;
        }
        if self.contains(HHBCFlags::EMIT_CLS_METH_POINTERS) {
            f |= HhvmFlags::EMIT_CLS_METH_POINTERS;
        }
        if self.contains(HHBCFlags::EMIT_METH_CALLER_FUNC_POINTERS) {
            f |= HhvmFlags::EMIT_METH_CALLER_FUNC_POINTERS;
        }
        if self.contains(HHBCFlags::ENABLE_INTRINSICS_EXTENSION) {
            f |= HhvmFlags::ENABLE_INTRINSICS_EXTENSION;
        }
        if self.contains(HHBCFlags::FOLD_LAZY_CLASS_KEYS) {
            f |= HhvmFlags::FOLD_LAZY_CLASS_KEYS;
        }
        if self.contains(HHBCFlags::JIT_ENABLE_RENAME_FUNCTION) {
            f |= HhvmFlags::JIT_ENABLE_RENAME_FUNCTION;
        }
        if self.contains(HHBCFlags::LOG_EXTERN_COMPILER_PERF) {
            f |= HhvmFlags::LOG_EXTERN_COMPILER_PERF;
        }
        if self.contains(HHBCFlags::ENABLE_IMPLICIT_CONTEXT) {
            f |= HhvmFlags::ENABLE_IMPLICIT_CONTEXT;
        }
        f
    }

    fn to_repo_flags(self) -> RepoFlags {
        let mut f = RepoFlags::empty();
        if self.contains(HHBCFlags::AUTHORITATIVE) {
            f |= RepoFlags::AUTHORITATIVE;
        }
        f
    }
}

impl ParserFlags {
    fn to_lang_flags(self) -> LangFlags {
        let mut f = LangFlags::empty();
        if self.contains(ParserFlags::ABSTRACT_STATIC_PROPS) {
            f |= LangFlags::ABSTRACT_STATIC_PROPS;
        }
        if self.contains(ParserFlags::ALLOW_NEW_ATTRIBUTE_SYNTAX) {
            f |= LangFlags::ALLOW_NEW_ATTRIBUTE_SYNTAX;
        }
        if self.contains(ParserFlags::ALLOW_UNSTABLE_FEATURES) {
            f |= LangFlags::ALLOW_UNSTABLE_FEATURES;
        }
        if self.contains(ParserFlags::CONST_DEFAULT_FUNC_ARGS) {
            f |= LangFlags::CONST_DEFAULT_FUNC_ARGS;
        }
        if self.contains(ParserFlags::CONST_STATIC_PROPS) {
            f |= LangFlags::CONST_STATIC_PROPS;
        }
        if self.contains(ParserFlags::DISABLE_LVAL_AS_AN_EXPRESSION) {
            f |= LangFlags::DISABLE_LVAL_AS_AN_EXPRESSION;
        }
        if self.contains(ParserFlags::DISALLOW_INST_METH) {
            f |= LangFlags::DISALLOW_INST_METH;
        }
        if self.contains(ParserFlags::DISABLE_XHP_ELEMENT_MANGLING) {
            f |= LangFlags::DISABLE_XHP_ELEMENT_MANGLING;
        }
        if self.contains(ParserFlags::DISALLOW_FUN_AND_CLS_METH_PSEUDO_FUNCS) {
            f |= LangFlags::DISALLOW_FUN_AND_CLS_METH_PSEUDO_FUNCS;
        }
        if self.contains(ParserFlags::DISALLOW_FUNC_PTRS_IN_CONSTANTS) {
            f |= LangFlags::DISALLOW_FUNC_PTRS_IN_CONSTANTS;
        }
        if self.contains(ParserFlags::ENABLE_ENUM_CLASSES) {
            f |= LangFlags::ENABLE_ENUM_CLASSES;
        }
        if self.contains(ParserFlags::ENABLE_XHP_CLASS_MODIFIER) {
            f |= LangFlags::ENABLE_XHP_CLASS_MODIFIER;
        }
        if self.contains(ParserFlags::ENABLE_CLASS_LEVEL_WHERE_CLAUSES) {
            f |= LangFlags::ENABLE_CLASS_LEVEL_WHERE_CLAUSES;
        }
        f
    }
}

impl<S: AsRef<str>> NativeEnv<S> {
    pub fn to_options(native_env: &NativeEnv<S>) -> Options {
        let hhbc_flags = native_env.hhbc_flags;
        let config = [&native_env.aliased_namespaces, &native_env.include_roots];
        let opts = Options::from_configs(&config, &[]).unwrap();
        let hhvm = Hhvm {
            aliased_namespaces: opts.hhvm.aliased_namespaces,
            include_roots: opts.hhvm.include_roots,
            flags: hhbc_flags.to_hhvm_flags(),
            emit_class_pointers: Arg::new(native_env.emit_class_pointers.to_string()),
            hack_lang: HackLang {
                flags: native_env.parser_flags.to_lang_flags(),
                check_int_overflow: Arg::new(native_env.check_int_overflow.to_string()),
            },
        };
        Options {
            hhvm,
            php7_flags: hhbc_flags.to_php7_flags(),
            repo_flags: hhbc_flags.to_repo_flags(),
            ..Default::default()
        }
    }
}

/// Compilation profile. All times are in seconds,
/// except when they are ignored and should not be reported,
/// such as in the case hhvm.log_extern_compiler_perf is false
/// (this avoids the need to read Options from OCaml, as
/// they can be simply returned as NaNs to signal that
/// they should _not_ be passed back as JSON to HHVM process)
#[derive(Debug, Default, Clone, ToOcamlRep)]
pub struct Profile {
    /// Time in seconds spent in parsing and lowering.
    pub parsing_t: f64,

    /// Time in seconds spent in emitter.
    pub codegen_t: f64,

    /// Time in seconds spent in bytecode_printer.
    pub printing_t: f64,

    /// Parser arena allocation volume in bytes.
    pub parsing_bytes: i64,

    /// Emitter arena allocation volume in bytes.
    pub codegen_bytes: i64,

    /// Peak stack size during parsing, before lowering.
    pub parse_peak: i64,

    /// Peak stack size during parsing and lowering.
    pub lower_peak: i64,
    pub error_peak: i64,

    /// Peak stack size during codegen
    pub rewrite_peak: i64,
    pub emitter_peak: i64,

    /// Was the log_extern_compiler_perf flag set?
    pub log_enabled: bool,
}

impl std::ops::AddAssign for Profile {
    fn add_assign(&mut self, p: Self) {
        self.parsing_t += p.parsing_t;
        self.codegen_t += p.codegen_t;
        self.printing_t += p.printing_t;
        self.codegen_bytes += p.codegen_bytes;
        self.parse_peak += p.parse_peak;
        self.lower_peak += p.lower_peak;
        self.error_peak += p.error_peak;
        self.rewrite_peak += p.rewrite_peak;
        self.emitter_peak += p.emitter_peak;
    }
}

impl std::ops::Add for Profile {
    type Output = Self;
    fn add(mut self, p2: Self) -> Self {
        self += p2;
        self
    }
}

impl Profile {
    pub fn total_sec(&self) -> f64 {
        self.parsing_t + self.codegen_t + self.printing_t
    }
}

pub fn emit_fatal_unit<S: AsRef<str>>(
    env: &Env<S>,
    writer: &mut dyn std::io::Write,
    err_msg: &str,
) -> anyhow::Result<()> {
    let is_systemlib = env.flags.contains(EnvFlags::IS_SYSTEMLIB);
    let opts =
        Options::from_configs(&env.config_jsons, &env.config_list).map_err(anyhow::Error::msg)?;
    let alloc = bumpalo::Bump::new();
    let emitter = Emitter::new(
        opts,
        is_systemlib,
        env.flags.contains(EnvFlags::FOR_DEBUGGER_EVAL),
        &alloc,
        None,
    );

    let prog = emit_unit::emit_fatal_unit(&alloc, FatalOp::Parse, Pos::make_none(), err_msg);
    let prog = prog.map_err(|e| anyhow!("Unhandled Emitter error: {}", e))?;
    print_unit(&Context::new(&emitter, Some(&env.filepath)), writer, &prog)?;
    Ok(())
}

/// Compile Hack source code, write HHAS text to `writer`.
/// Update `profile` with stats from any passes that run,
/// even if the compiler ultimately returns Err.
pub fn from_text<'arena, 'decl, S: AsRef<str>>(
    alloc: &'arena bumpalo::Bump,
    env: &Env<S>,
    writer: &mut dyn std::io::Write,
    source_text: SourceText<'_>,
    native_env: Option<&NativeEnv<S>>,
    decl_provider: Option<&'decl dyn DeclProvider<'decl>>,
    mut profile: &mut Profile,
) -> anyhow::Result<()> {
    let mut emitter = create_emitter(env, native_env, decl_provider, alloc)?;
    let unit = emit_unit_from_text(&mut emitter, env, source_text, profile)?;

    let (print_result, printing_t) =
        time(|| print_unit(&Context::new(&emitter, Some(&env.filepath)), writer, &unit));
    print_result?;
    profile.printing_t = printing_t;
    profile.codegen_bytes = alloc.allocated_bytes() as i64;
    Ok(())
}

fn rewrite_and_emit<'p, 'arena, 'decl, S: AsRef<str>>(
    emitter: &mut Emitter<'arena, 'decl>,
    env: &Env<S>,
    namespace_env: RcOc<NamespaceEnv>,
    ast: &'p mut ast::Program,
    profile: &'p mut Profile,
) -> Result<HackCUnit<'arena>, Error> {
    // First rewrite and modify `ast` in place.
    stack_limit::reset();
    let result = rewrite(emitter, ast, RcOc::clone(&namespace_env));
    profile.rewrite_peak = stack_limit::peak() as i64;
    stack_limit::reset();
    let unit = match result {
        Ok(()) => {
            // Rewrite ok, now emit.
            emit_unit_from_ast(emitter, env, namespace_env, ast)
        }
        Err(e) => match e.into_kind() {
            ErrorKind::IncludeTimeFatalException(op, pos, msg) => {
                emit_unit::emit_fatal_unit(emitter.alloc, op, pos, msg)
            }
            ErrorKind::Unrecoverable(x) => Err(Error::unrecoverable(x)),
        },
    };
    profile.emitter_peak = stack_limit::peak() as i64;
    unit
}

fn rewrite<'p, 'arena, 'decl>(
    emitter: &mut Emitter<'arena, 'decl>,
    ast: &'p mut ast::Program,
    namespace_env: RcOc<NamespaceEnv>,
) -> Result<(), Error> {
    rewrite_program(emitter, ast, namespace_env)
}

pub fn unit_from_text<'arena, 'decl, S: AsRef<str>>(
    alloc: &'arena bumpalo::Bump,
    env: &Env<S>,
    source_text: SourceText<'_>,
    native_env: Option<&NativeEnv<S>>,
    decl_provider: Option<&'decl dyn DeclProvider<'decl>>,
    profile: &mut Profile,
) -> anyhow::Result<HackCUnit<'arena>> {
    let mut emitter = create_emitter(env, native_env, decl_provider, alloc)?;
    emit_unit_from_text(&mut emitter, env, source_text, profile)
}

pub fn unit_to_string<W: std::io::Write, S: AsRef<str>>(
    env: &Env<S>,
    native_env: Option<&NativeEnv<S>>,
    writer: &mut W,
    program: &HackCUnit<'_>,
) -> anyhow::Result<()> {
    let alloc = bumpalo::Bump::new();
    let emitter = create_emitter(env, native_env, None, &alloc)?;
    let (print_result, _) = time(|| {
        print_unit(
            &Context::new(&emitter, Some(&env.filepath)),
            writer,
            program,
        )
    });
    print_result.map_err(|e| anyhow!("{}", e))
}

fn emit_unit_from_ast<'p, 'arena, 'decl, S: AsRef<str>>(
    emitter: &mut Emitter<'arena, 'decl>,
    env: &Env<S>,
    namespace: RcOc<NamespaceEnv>,
    ast: &'p mut ast::Program,
) -> Result<HackCUnit<'arena>, Error> {
    let mut flags = FromAstFlags::empty();
    if env.flags.contains(EnvFlags::IS_EVALED) {
        flags |= FromAstFlags::IS_EVALED;
    }
    if env.flags.contains(EnvFlags::FOR_DEBUGGER_EVAL) {
        flags |= FromAstFlags::FOR_DEBUGGER_EVAL;
    }
    if env.flags.contains(EnvFlags::IS_SYSTEMLIB) {
        flags |= FromAstFlags::IS_SYSTEMLIB;
    }

    emit_unit(emitter, flags, namespace, ast)
}

fn emit_unit_from_text<'arena, 'decl, S: AsRef<str>>(
    emitter: &mut Emitter<'arena, 'decl>,
    env: &Env<S>,
    source_text: SourceText<'_>,
    profile: &mut Profile,
) -> anyhow::Result<HackCUnit<'arena>> {
    profile.log_enabled = emitter.options().log_extern_compiler_perf();

    let namespace_env = RcOc::new(NamespaceEnv::empty(
        emitter.options().hhvm.aliased_namespaces_cloned().collect(),
        true, /* is_codegen */
        emitter
            .options()
            .hhvm
            .hack_lang
            .flags
            .contains(LangFlags::DISABLE_XHP_ELEMENT_MANGLING),
    ));

    let (parse_result, parsing_t) = time(|| {
        parse_file(
            emitter.options(),
            source_text,
            !env.flags.contains(EnvFlags::DISABLE_TOPLEVEL_ELABORATION),
            RcOc::clone(&namespace_env),
            env.flags.contains(EnvFlags::IS_SYSTEMLIB),
            profile,
        )
    });
    profile.parsing_t = parsing_t;

    let ((unit, profile), codegen_t) = match parse_result {
        Ok(mut ast) => {
            elaborate_namespaces_visitor::elaborate_program(RcOc::clone(&namespace_env), &mut ast);
            time(move || {
                let u = rewrite_and_emit(emitter, env, namespace_env, &mut ast, profile);
                (u, profile)
            })
        }
        Err(ParseError(pos, msg, is_runtime_error)) => time(move || {
            (
                emit_fatal(emitter.alloc, is_runtime_error, pos, msg),
                profile,
            )
        }),
    };
    profile.codegen_t = codegen_t;
    match unit {
        Ok(unit) => Ok(unit),
        Err(e) => Err(anyhow!("Unhandled Emitter error: {}", e)),
    }
}

fn emit_fatal<'arena>(
    alloc: &'arena bumpalo::Bump,
    is_runtime_error: bool,
    pos: Pos,
    msg: impl AsRef<str> + 'arena,
) -> Result<HackCUnit<'arena>, Error> {
    let op = if is_runtime_error {
        FatalOp::Runtime
    } else {
        FatalOp::Parse
    };
    emit_unit::emit_fatal_unit(alloc, op, pos, msg)
}

fn create_emitter<'arena, 'decl, S: AsRef<str>>(
    env: &Env<S>,
    native_env: Option<&NativeEnv<S>>,
    decl_provider: Option<&'decl dyn DeclProvider<'decl>>,
    alloc: &'arena bumpalo::Bump,
) -> anyhow::Result<Emitter<'arena, 'decl>> {
    let opts = match native_env {
        None => Options::from_configs(&env.config_jsons, &env.config_list)
            .map_err(anyhow::Error::msg)?,
        Some(native_env) => NativeEnv::to_options(native_env),
    };
    Ok(Emitter::new(
        opts,
        env.flags.contains(EnvFlags::IS_SYSTEMLIB),
        env.flags.contains(EnvFlags::FOR_DEBUGGER_EVAL),
        alloc,
        decl_provider,
    ))
}

fn create_parser_options(opts: &Options) -> ParserOptions {
    let hack_lang_flags = |flag| opts.hhvm.hack_lang.flags.contains(flag);
    ParserOptions {
        po_auto_namespace_map: opts.hhvm.aliased_namespaces_cloned().collect(),
        po_codegen: true,
        po_disallow_silence: false,
        po_disable_lval_as_an_expression: hack_lang_flags(LangFlags::DISABLE_LVAL_AS_AN_EXPRESSION),
        po_enable_class_level_where_clauses: hack_lang_flags(
            LangFlags::ENABLE_CLASS_LEVEL_WHERE_CLAUSES,
        ),
        po_disable_legacy_soft_typehints: hack_lang_flags(LangFlags::DISABLE_LEGACY_SOFT_TYPEHINTS),
        po_allow_new_attribute_syntax: hack_lang_flags(LangFlags::ALLOW_NEW_ATTRIBUTE_SYNTAX),
        po_disable_legacy_attribute_syntax: hack_lang_flags(
            LangFlags::DISABLE_LEGACY_ATTRIBUTE_SYNTAX,
        ),
        po_const_default_func_args: hack_lang_flags(LangFlags::CONST_DEFAULT_FUNC_ARGS),
        po_const_default_lambda_args: hack_lang_flags(LangFlags::CONST_DEFAULT_LAMBDA_ARGS),
        tco_const_static_props: hack_lang_flags(LangFlags::CONST_STATIC_PROPS),
        po_abstract_static_props: hack_lang_flags(LangFlags::ABSTRACT_STATIC_PROPS),
        po_disallow_func_ptrs_in_constants: hack_lang_flags(
            LangFlags::DISALLOW_FUNC_PTRS_IN_CONSTANTS,
        ),
        po_enable_xhp_class_modifier: hack_lang_flags(LangFlags::ENABLE_XHP_CLASS_MODIFIER),
        po_disable_xhp_element_mangling: hack_lang_flags(LangFlags::DISABLE_XHP_ELEMENT_MANGLING),
        po_enable_enum_classes: hack_lang_flags(LangFlags::ENABLE_ENUM_CLASSES),
        po_allow_unstable_features: hack_lang_flags(LangFlags::ALLOW_UNSTABLE_FEATURES),
        po_disallow_fun_and_cls_meth_pseudo_funcs: hack_lang_flags(
            LangFlags::DISALLOW_FUN_AND_CLS_METH_PSEUDO_FUNCS,
        ),
        po_disallow_inst_meth: hack_lang_flags(LangFlags::DISALLOW_INST_METH),
        ..Default::default()
    }
}

#[derive(Error, Debug)]
#[error("{0}: {1}")]
pub(crate) struct ParseError(Pos, String, bool);

fn parse_file(
    opts: &Options,
    source_text: SourceText<'_>,
    elaborate_namespaces: bool,
    namespace_env: RcOc<NamespaceEnv>,
    is_systemlib: bool,
    profile: &mut Profile,
) -> Result<ast::Program, ParseError> {
    let aast_env = AastEnv {
        codegen: true,
        fail_open: false,
        // Ocaml's implementation
        // let enable_uniform_variable_syntax o = o.option_php7_uvs in
        // php5_compat_mode:
        //   (not (Hhbc_options.enable_uniform_variable_syntax hhbc_options))
        php5_compat_mode: !opts.php7_flags.contains(Php7Flags::UVS),
        keep_errors: false,
        is_systemlib,
        elaborate_namespaces,
        parser_options: create_parser_options(opts),
        ..AastEnv::default()
    };

    let indexed_source_text = IndexedSourceText::new(source_text);
    let ast_result =
        AastParser::from_text_with_namespace_env(&aast_env, namespace_env, &indexed_source_text);
    match ast_result {
        Err(AastError::Other(msg)) => Err(ParseError(Pos::make_none(), msg, false)),
        Err(AastError::NotAHackFile()) => Err(ParseError(
            Pos::make_none(),
            "Not a Hack file".to_string(),
            false,
        )),
        Err(AastError::ParserFatal(syntax_error, pos)) => {
            Err(ParseError(pos, syntax_error.message.to_string(), false))
        }
        Ok(ast) => match ast {
            ParserResult { syntax_errors, .. } if !syntax_errors.is_empty() => {
                let error = syntax_errors
                    .iter()
                    .find(|e| e.error_type == ErrorType::RuntimeError)
                    .unwrap_or(&syntax_errors[0]);
                let pos = indexed_source_text.relative_pos(error.start_offset, error.end_offset);
                Err(ParseError(
                    pos,
                    error.message.to_string(),
                    error.error_type == ErrorType::RuntimeError,
                ))
            }
            ParserResult { lowpri_errors, .. } if !lowpri_errors.is_empty() => {
                let (pos, msg) = lowpri_errors.into_iter().next().unwrap();
                Err(ParseError(pos, msg, false))
            }
            ParserResult {
                errors,
                aast,
                parse_peak,
                lower_peak,
                error_peak,
                arena_bytes,
                ..
            } => {
                profile.parse_peak = parse_peak;
                profile.lower_peak = lower_peak;
                profile.error_peak = error_peak;
                profile.parsing_bytes = arena_bytes;
                let mut errors = errors.iter().filter(|e| {
                    e.code() != 2086
                        /* Naming.MethodNeedsVisibility */
                        && e.code() != 2102
                        /* Naming.UnsupportedTraitUseAs */
                        && e.code() != 2103
                });
                match errors.next() {
                    Some(e) => Err(ParseError(e.pos().clone(), String::from(e.msg()), false)),
                    None => match aast {
                        Ok(aast) => Ok(aast),
                        Err(msg) => Err(ParseError(Pos::make_none(), msg, false)),
                    },
                }
            }
        },
    }
}

fn time<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let (r, t) = profile_rust::time(f);
    (r, t.as_secs_f64())
}

pub fn expr_to_string_lossy<S: AsRef<str>>(env: &Env<S>, expr: &ast::Expr) -> String {
    use print_expr::Context;

    let opts =
        Options::from_configs(&env.config_jsons, &env.config_list).expect("Malformed options");

    let alloc = bumpalo::Bump::new();
    let emitter = Emitter::new(
        opts,
        env.flags.contains(EnvFlags::IS_SYSTEMLIB),
        env.flags.contains(EnvFlags::FOR_DEBUGGER_EVAL),
        &alloc,
        None,
    );
    let ctx = Context::new(&emitter);

    print_expr::expr_to_string_lossy(ctx, expr)
}
