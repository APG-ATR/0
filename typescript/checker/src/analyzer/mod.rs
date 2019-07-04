pub use self::name::Name;
use self::{
    scope::{Scope, ScopeKind},
    util::{NormalizeMut, PatExt},
};
use super::Checker;
use crate::{
    builtin_types::Lib,
    errors::Error,
    loader::Load,
    ty::{self, Alias, Param, Tuple, Type, TypeRefExt},
    util::IntoCow,
    Rule,
};
use fxhash::{FxHashMap, FxHashSet};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use std::{borrow::Cow, cell::RefCell, path::PathBuf, sync::Arc};
use swc_atoms::{js_word, JsWord};
use swc_common::{Span, Spanned, Visit, VisitWith, DUMMY_SP};
use swc_ecma_ast::*;

mod control_flow;
pub mod export;
mod expr;
mod generic;
mod name;
mod scope;
mod type_facts;
mod util;

struct Analyzer<'a, 'b> {
    info: Info,
    resolved_imports: FxHashMap<JsWord, Arc<Type<'static>>>,
    errored_imports: FxHashSet<JsWord>,
    pending_exports: Vec<((JsWord, Span), Box<Expr>)>,
    inferred_return_types: RefCell<Vec<Type<'static>>>,
    scope: Scope<'a>,
    /// This is false iff it should be treated as error when `1.contains()` is
    /// true
    allow_ref_declaring: bool,
    declaring: Vec<JsWord>,
    path: Arc<PathBuf>,
    loader: &'b dyn Load,
    libs: &'b [Lib],
    rule: Rule,

    /// The code below is valid even when `noImplicitAny` is given.
    ///
    /// ```typescript
    /// 
    /// var foo: () => [any] = function bar() {}
    /// ```
    span_allowed_implicit_any: Span,
}

impl<T> Visit<Vec<T>> for Analyzer<'_, '_>
where
    T: VisitWith<Self> + for<'any> VisitWith<ImportFinder<'any>> + Send + Sync,
    Vec<T>: VisitWith<Self>,
{
    fn visit(&mut self, items: &Vec<T>) {
        // We first load imports.

        let mut imports: Vec<ImportInfo> = vec![];

        items.iter().for_each(|item| {
            // EXtract imports
            item.visit_with(&mut ImportFinder { to: &mut imports });
            // item.visit_with(self);
        });

        let loader = self.loader;
        let path = self.path.clone();
        let import_results = imports
            .par_iter()
            .map(|import| {
                loader.load(path.clone(), &*import).map_err(|err| {
                    //
                    (import, err)
                })
            })
            .collect::<Vec<_>>();

        for res in import_results {
            match res {
                Ok(import) => {
                    self.resolved_imports.extend(import);
                }
                Err((import, mut err)) => {
                    match err {
                        Error::ModuleLoadFailed { ref mut errors, .. } => {
                            self.info.errors.append(errors);
                        }
                        _ => {}
                    }
                    // Mark errored imported types as any to prevent useless errors
                    self.errored_imports.extend(
                        import
                            .items
                            .iter()
                            .map(|&Specifier { ref local, .. }| local.0.clone()),
                    );

                    self.info.errors.push(err);
                }
            }
        }

        items.visit_children(self);

        self.handle_pending_exports();
    }
}

impl Visit<TsModuleDecl> for Analyzer<'_, '_> {
    fn visit(&mut self, decl: &TsModuleDecl) {
        // TODO: Uncomment the line below.
        // Uncommenting the line somehow returns without excuting subsequent codes.
        // decl.visit_children(self);

        // println!("after: visit<TsModuleDecl>: {:?}", decl.id);

        self.scope.register_type(
            match decl.id {
                TsModuleName::Ident(ref i) => i.sym.clone(),
                TsModuleName::Str(ref s) => s.value.clone(),
            },
            decl.clone().into(),
        );
    }
}

impl Visit<TsInterfaceDecl> for Analyzer<'_, '_> {
    fn visit(&mut self, decl: &TsInterfaceDecl) {
        self.scope
            .register_type(decl.id.sym.clone(), decl.clone().into());
    }
}

impl Visit<TsTypeAliasDecl> for Analyzer<'_, '_> {
    fn visit(&mut self, decl: &TsTypeAliasDecl) {
        let ty: Type<'_> = decl.type_ann.clone().into();

        let ty = if decl.type_params.is_none() {
            match self.expand_type(decl.span(), ty.owned()) {
                Ok(ty) => ty.to_static(),
                Err(err) => {
                    self.info.errors.push(err);
                    Type::any(decl.span())
                }
            }
        } else {
            ty
        };

        self.scope.register_type(
            decl.id.sym.clone(),
            Type::Alias(Alias {
                span: decl.span(),
                ty: box ty.owned(),
                type_params: decl.type_params.clone().map(From::from),
            }),
        );

        // TODO: Validate type
    }
}

#[derive(Debug)]
struct ImportFinder<'a> {
    to: &'a mut Vec<ImportInfo>,
}

/// Extracts require('foo')
impl Visit<CallExpr> for ImportFinder<'_> {
    fn visit(&mut self, expr: &CallExpr) {
        let span = expr.span();

        match expr.callee {
            ExprOrSuper::Expr(box Expr::Ident(ref i)) if i.sym == js_word!("require") => {
                let src = expr
                    .args
                    .iter()
                    .map(|v| match *v.expr {
                        Expr::Lit(Lit::Str(Str { ref value, .. })) => value.clone(),
                        _ => unimplemented!("error reporting for dynamic require"),
                    })
                    .next()
                    .unwrap();
                self.to.push(ImportInfo {
                    span,
                    all: true,
                    items: vec![],
                    src,
                });
            }
            _ => return,
        }
    }
}

impl Visit<ImportDecl> for ImportFinder<'_> {
    fn visit(&mut self, import: &ImportDecl) {
        let span = import.span();
        let mut items = vec![];
        let mut all = false;

        for s in &import.specifiers {
            match *s {
                ImportSpecifier::Default(ref default) => items.push(Specifier {
                    export: (js_word!("default"), default.span),
                    local: (default.local.sym.clone(), default.local.span),
                }),
                ImportSpecifier::Specific(ref s) => {
                    items.push(Specifier {
                        export: (
                            s.imported
                                .clone()
                                .map(|v| v.sym)
                                .unwrap_or_else(|| s.local.sym.clone()),
                            s.span,
                        ),
                        local: (s.local.sym.clone(), s.local.span),
                    });
                }
                ImportSpecifier::Namespace(..) => all = true,
            }
        }

        if !items.is_empty() {
            self.to.push(ImportInfo {
                span,
                items,
                all,
                src: import.src.value.clone(),
            });
        }
    }
}

impl<'a, 'b> Analyzer<'a, 'b> {
    pub fn new(
        libs: &'b [Lib],
        rule: Rule,
        scope: Scope<'a>,
        path: Arc<PathBuf>,
        loader: &'b dyn Load,
    ) -> Self {
        Analyzer {
            libs,
            rule,
            scope,
            info: Default::default(),
            inferred_return_types: Default::default(),
            path,
            allow_ref_declaring: false,
            declaring: vec![],
            resolved_imports: Default::default(),
            errored_imports: Default::default(),
            pending_exports: Default::default(),
            loader,
            span_allowed_implicit_any: DUMMY_SP,
        }
    }
}

#[derive(Debug, Default)]
pub struct Info {
    pub exports: FxHashMap<JsWord, Arc<Type<'static>>>,
    pub errors: Vec<Error>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ImportInfo {
    pub span: Span,
    pub items: Vec<Specifier>,
    pub all: bool,
    pub src: JsWord,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Specifier {
    pub local: (JsWord, Span),
    pub export: (JsWord, Span),
}

impl Visit<TsEnumDecl> for Analyzer<'_, '_> {
    fn visit(&mut self, e: &TsEnumDecl) {
        e.visit_children(self);

        self.scope.register_type(e.id.sym.clone(), e.clone().into());
    }
}

impl Visit<ClassExpr> for Analyzer<'_, '_> {
    fn visit(&mut self, c: &ClassExpr) {
        let ty = match self.validate_type_of_class(&c.class) {
            Ok(ty) => ty,
            Err(err) => {
                self.info.errors.push(err);
                Type::any(c.span()).into()
            }
        };

        self.scope.this = Some(ty.clone());

        if let Some(ref i) = c.ident {
            self.scope.register_type(i.sym.clone(), ty.clone());

            match self.scope.declare_var(
                ty.span(),
                VarDeclKind::Var,
                i.sym.clone(),
                Some(ty),
                // initialized = true
                true,
                // declare Class does not allow multiple declarations.
                false,
            ) {
                Ok(()) => {}
                Err(err) => {
                    self.info.errors.push(err);
                }
            }
        }

        c.visit_children(self);

        self.scope.this = None;
    }
}

impl Visit<ClassDecl> for Analyzer<'_, '_> {
    fn visit(&mut self, c: &ClassDecl) {
        let ty = match self.validate_type_of_class(&c.class) {
            Ok(ty) => ty,
            Err(err) => {
                self.info.errors.push(err);
                Type::any(c.span()).into()
            }
        };

        self.scope.this = Some(ty.clone());

        self.scope.register_type(c.ident.sym.clone(), ty.clone());

        match self.scope.declare_var(
            ty.span(),
            VarDeclKind::Var,
            c.ident.sym.clone(),
            Some(ty),
            // initialized = true
            true,
            // declare Class does not allow multiple declarations.
            false,
        ) {
            Ok(()) => {}
            Err(err) => {
                self.info.errors.push(err);
            }
        }

        c.visit_children(self);

        self.scope.this = None;
    }
}

impl Analyzer<'_, '_> {
    /// TODO: Handle recursive funciton
    fn visit_fn(&mut self, name: Option<&Ident>, f: &Function) -> Type<'static> {
        let fn_ty = self.with_child(ScopeKind::Fn, Default::default(), |child| {
            let no_implicit_any_span = name.as_ref().map(|name| name.span);

            if let Some(name) = name {
                // We use `typeof function` to infer recursive function's return type.
                match child.scope.declare_var(
                    f.span,
                    VarDeclKind::Var,
                    name.sym.clone(),
                    Some(Type::Simple(Cow::Owned(
                        TsTypeQuery {
                            span: f.span,
                            expr_name: TsEntityName::Ident(name.clone()),
                        }
                        .into(),
                    ))),
                    // value is initialized
                    true,
                    // Allow overriding
                    true,
                ) {
                    Ok(()) => {}
                    Err(err) => {
                        child.info.errors.push(err);
                    }
                }
            }

            match f.type_params {
                Some(TsTypeParamDecl { ref params, .. }) => {
                    params.iter().for_each(|param| {
                        let ty = Type::Param(Param {
                            span: param.span,
                            name: param.name.sym.clone(),
                            constraint: param.constraint.as_ref().map(|v| box v.clone().into_cow()),
                            default: param.default.as_ref().map(|v| box v.clone().into_cow()),
                        });

                        child
                            .scope
                            .facts
                            .types
                            .insert(param.name.sym.clone().into(), ty);
                    });
                }
                None => {}
            }

            let old = child.allow_ref_declaring;
            child.allow_ref_declaring = false;

            f.params.iter().for_each(|pat| {
                let mut names = vec![];

                let mut visitor = VarVisitor { names: &mut names };

                pat.visit_with(&mut visitor);

                child.declaring.extend_from_slice(&names);

                debug_assert_eq!(child.allow_ref_declaring, false);
                match child.declare_vars(VarDeclKind::Let, pat) {
                    Ok(()) => {}
                    Err(err) => {
                        child.info.errors.push(err);
                    }
                }
                for n in names {
                    child.declaring.remove_item(&n).unwrap();
                }
            });

            if let Some(name) = name {
                assert_eq!(child.scope.declaring_fn, None);
                child.scope.declaring_fn = Some(name.sym.clone());
            }

            f.visit_children(child);

            let mut fn_ty = child.type_of_fn(f)?;
            match fn_ty {
                // Handle tuple widening of the return type.
                Type::Function(ty::Function { ref mut ret_ty, .. }) => {
                    match *ret_ty.normalize_mut() {
                        Type::Tuple(Tuple { ref mut types, .. }) => {
                            for t in types.iter_mut() {
                                let span = t.span();

                                match t.normalize() {
                                    Type::Keyword(TsKeywordType {
                                        kind: TsKeywordTypeKind::TsUndefinedKeyword,
                                        ..
                                    })
                                    | Type::Keyword(TsKeywordType {
                                        kind: TsKeywordTypeKind::TsNullKeyword,
                                        ..
                                    }) => {}
                                    _ => continue,
                                }

                                if child.rule.no_implicit_any
                                    && child.span_allowed_implicit_any != f.span
                                {
                                    child.info.errors.push(Error::ImplicitAny {
                                        span: no_implicit_any_span.unwrap_or(span),
                                    });
                                }

                                *t = Type::any(span).owned();
                            }
                        }
                        _ => {}
                    }
                }

                _ => unreachable!(),
            }

            if let Some(name) = name {
                child.scope.declaring_fn = Some(name.sym.clone());
            }

            debug_assert_eq!(child.allow_ref_declaring, false);
            child.allow_ref_declaring = old;

            Ok(fn_ty)
        });

        match fn_ty {
            Ok(ty) => ty.to_static(),
            Err(err) => {
                self.info.errors.push(err);
                Type::any(f.span)
            }
        }
    }
}

impl Visit<FnDecl> for Analyzer<'_, '_> {
    /// NOTE: This method **should not call f.visit_children(self)**
    fn visit(&mut self, f: &FnDecl) {
        println!("Visiting {}", f.ident.sym);
        let fn_ty = self.visit_fn(Some(&f.ident), &f.function);

        match self
            .scope
            .override_var(VarDeclKind::Var, f.ident.sym.clone(), fn_ty)
        {
            Ok(()) => {}
            Err(err) => {
                self.info.errors.push(err);
            }
        }
    }
}

impl Visit<FnExpr> for Analyzer<'_, '_> {
    /// NOTE: This method **should not call f.visit_children(self)**
    fn visit(&mut self, f: &FnExpr) {
        self.visit_fn(f.ident.as_ref(), &f.function);
    }
}

impl Visit<Function> for Analyzer<'_, '_> {
    fn visit(&mut self, f: &Function) {
        self.visit_fn(None, f);
    }
}

impl Visit<ArrowExpr> for Analyzer<'_, '_> {
    fn visit(&mut self, f: &ArrowExpr) {
        self.with_child(ScopeKind::Fn, Default::default(), |child| {
            match f.type_params {
                Some(TsTypeParamDecl { ref params, .. }) => {
                    params.iter().for_each(|param| {
                        let ty = Type::Param(Param {
                            span: param.span,
                            name: param.name.sym.clone(),
                            constraint: param.constraint.as_ref().map(|v| box v.clone().into_cow()),
                            default: param.default.as_ref().map(|v| box v.clone().into_cow()),
                        });

                        child
                            .scope
                            .facts
                            .types
                            .insert(param.name.sym.clone().into(), ty);
                    });
                }
                None => {}
            }

            for pat in f.params.iter() {
                match child.declare_vars(VarDeclKind::Let, pat) {
                    Ok(()) => {}
                    Err(err) => {
                        child.info.errors.push(err);
                    }
                }
            }

            f.visit_children(child);

            match f.body {
                BlockStmtOrExpr::Expr(ref expr) => {
                    child.visit_return_arg(expr.span(), Some(expr));
                }
                _ => {}
            }
        });
    }
}

impl Visit<BlockStmt> for Analyzer<'_, '_> {
    fn visit(&mut self, stmt: &BlockStmt) {
        self.with_child(ScopeKind::Block, Default::default(), |analyzer| {
            stmt.visit_children(analyzer);
        })
    }
}

impl Visit<AssignExpr> for Analyzer<'_, '_> {
    fn visit(&mut self, expr: &AssignExpr) {
        let span = expr.span();
        expr.visit_children(self);

        let rhs_ty = match self
            .type_of(&expr.right)
            .and_then(|ty| self.expand_type(span, ty))
        {
            Ok(rhs_ty) => rhs_ty.to_static(),
            Err(err) => {
                self.info.errors.push(err);
                return;
            }
        };
        if expr.op == op!("=") {
            self.try_assign(&expr.left, &rhs_ty);
        }
    }
}

impl Visit<VarDecl> for Analyzer<'_, '_> {
    fn visit(&mut self, var: &VarDecl) {
        let kind = var.kind;

        var.decls.iter().for_each(|v| {
            let old = self.allow_ref_declaring;

            let debug_declaring = if cfg!(debug_assertions) {
                Some(self.declaring.clone())
            } else {
                None
            };
            let mut names = vec![];

            macro_rules! remove_declaring {
                () => {{
                    for n in names {
                        self.declaring.remove_item(&n).unwrap();
                    }
                    debug_assert_eq!(Some(self.declaring.clone()), debug_declaring);
                }};
            }

            if let Some(ref init) = v.init {
                let span = init.span();
                let declared_ty = v.name.get_ty();
                if declared_ty.is_some() {
                    self.span_allowed_implicit_any = span;
                }

                self.allow_ref_declaring = true;

                {
                    let mut visitor = VarVisitor { names: &mut names };

                    v.name.visit_with(&mut visitor);

                    self.declaring.extend_from_slice(&names);

                    v.visit_children(self);
                }
                debug_assert_eq!(self.allow_ref_declaring, true);

                //  Check if v_ty is assignable to ty
                let value_ty = match self.type_of(&init).and_then(|ty| {
                    // println!(
                    //     "Visit<VarDecl>: [{:?}] type_of(initializer): {:#?}",
                    //     self.declaring, ty
                    // );
                    self.expand_type(span, ty)
                }) {
                    Ok(ty) => ty,
                    Err(err) => {
                        self.info.errors.push(err);
                        remove_declaring!();
                        return;
                    }
                };

                match declared_ty {
                    Some(ty) => {
                        let ty = Type::from(ty.clone());
                        let ty = match self.expand_type(span, Cow::Owned(ty)) {
                            Ok(ty) => ty,
                            Err(err) => {
                                self.info.errors.push(err);
                                remove_declaring!();
                                return;
                            }
                        };
                        let error = value_ty.assign_to(&ty, v.span());
                        let ty = ty.to_static();
                        match error {
                            Ok(()) => {
                                match self.scope.declare_complex_vars(kind, &v.name, ty) {
                                    Ok(()) => {}
                                    Err(err) => {
                                        self.info.errors.push(err);
                                    }
                                }
                                remove_declaring!();
                                return;
                            }
                            Err(err) => {
                                self.info.errors.push(err);
                            }
                        }
                    }
                    None => {
                        // infer type from value.
                        let mut ty = value_ty.to_static();

                        let mut type_errors = vec![];

                        // Handle implicit any

                        match ty {
                            Type::Tuple(Tuple { ref mut types, .. }) => {
                                for (i, t) in types.iter_mut().enumerate() {
                                    let span = t.span();

                                    match *t.normalize() {
                                        Type::Keyword(TsKeywordType {
                                            kind: TsKeywordTypeKind::TsUndefinedKeyword,
                                            ..
                                        })
                                        | Type::Keyword(TsKeywordType {
                                            kind: TsKeywordTypeKind::TsNullKeyword,
                                            ..
                                        }) => {}
                                        _ => {
                                            continue;
                                        }
                                    }
                                    // Widen tuple types
                                    *t = Type::any(span).owned();

                                    if self.rule.no_implicit_any {
                                        match v.name {
                                            Pat::Ident(ref i) => {
                                                let span = i.span;
                                                type_errors.push(Error::ImplicitAny { span });
                                                break;
                                            }
                                            Pat::Array(ArrayPat { ref elems, .. }) => {
                                                let span = elems[i].span();
                                                type_errors.push(Error::ImplicitAny { span });
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }

                        if !type_errors.is_empty() {
                            self.info.errors.extend(type_errors);
                            remove_declaring!();
                            return;
                        }

                        match self.scope.declare_complex_vars(kind, &v.name, ty) {
                            Ok(()) => {}
                            Err(err) => {
                                self.info.errors.push(err);
                            }
                        }
                        remove_declaring!();
                        return;
                    }
                }
            } else {
                match v.name {
                    Pat::Ident(Ident {
                        span,
                        ref sym,
                        ref type_ann,
                        ..
                    }) => {
                        //
                        let sym = sym.clone();
                        let ty = match type_ann.as_ref().map(|t| Type::from(t.type_ann.clone())) {
                            Some(ty) => match self.expand_type(span, ty.owned()) {
                                Ok(ty) => Some(ty.to_static()),
                                Err(err) => {
                                    self.info.errors.push(err);
                                    remove_declaring!();
                                    return;
                                }
                            },
                            None => None,
                        };

                        match self.scope.declare_var(
                            span,
                            kind,
                            sym,
                            ty,
                            // initialized
                            false,
                            // allow_multiple
                            kind == VarDeclKind::Var,
                        ) {
                            Ok(()) => {}
                            Err(err) => {
                                self.info.errors.push(err);
                            }
                        };
                    }
                    _ => unreachable!(
                        "complex pattern without initializer is invalid syntax and parser should \
                         handle it"
                    ),
                };
                remove_declaring!();
                return;
            }

            debug_assert_eq!(self.allow_ref_declaring, true);
            self.allow_ref_declaring = old;
            match self.declare_vars(kind, &v.name) {
                Ok(()) => {}
                Err(err) => {
                    self.info.errors.push(err);
                }
            }

            remove_declaring!();
        });
    }
}

/// Analyzes a module.
///
/// Constants are propagated, and
impl Checker<'_> {
    pub fn analyze_module(&self, rule: Rule, path: Arc<PathBuf>, m: &Module) -> Info {
        ::swc_common::GLOBALS.set(&self.globals, || {
            let mut a = Analyzer::new(&self.libs, rule, Scope::root(), path, &self);
            m.visit_with(&mut a);

            a.info
        })
    }
}

struct VarVisitor<'a> {
    pub names: &'a mut Vec<JsWord>,
}

impl Visit<Expr> for VarVisitor<'_> {
    fn visit(&mut self, _: &Expr) {}
}

impl Visit<Ident> for VarVisitor<'_> {
    fn visit(&mut self, i: &Ident) {
        self.names.push(i.sym.clone())
    }
}

fn _assert_types() {
    fn is_sync<T: Sync>() {}
    fn is_send<T: Send>() {}
    is_sync::<Info>();
    is_send::<Info>();
}
