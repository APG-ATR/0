use crate::pass::Pass;
use ast::*;
use hashbrown::HashMap;
use std::cell::RefCell;
use swc_atoms::JsWord;
use swc_common::{Fold, FoldWith, SyntaxContext};

#[cfg(test)]
mod tests;

/// Ported from [`InlineVariables`](https://github.com/google/closure-compiler/blob/master/src/com/google/javascript/jscomp/InlineVariables.java)
/// of the google closure compiler.
pub fn inline_vars() -> impl 'static + Pass {
    Inline::default()
}

type Id = (JsWord, SyntaxContext);

fn id(i: &Ident) -> Id {
    (i.sym.clone(), i.span.ctxt())
}

#[derive(Debug, Default)]
struct Inline<'a> {
    scope: Scope<'a>,
}

impl Inline<'_> {
    fn child<T, F>(&mut self, op: F) -> T
    where
        F: for<'any> FnOnce(&mut Inline<'any>) -> T,
    {
        let mut c = Inline {
            scope: Scope {
                parent: Some(&self.scope),
                idents: Default::default(),
            },
        };

        op(&mut c)
    }
}

#[derive(Debug, Default)]
struct Scope<'a> {
    parent: Option<&'a Scope<'a>>,
    /// Stored only if value is statically known.
    idents: RefCell<HashMap<Id, Expr>>,
}

impl Scope<'_> {
    fn find(&self, i: &Ident) -> Option<Expr> {
        if let Some(e) = self
            .idents
            .borrow()
            .iter()
            .find(|e| (e.0).0 == i.sym && (e.0).1 == i.span.ctxt())
        {
            return Some(e.1.clone());
        }

        match self.parent {
            Some(ref p) => p.find(i),
            None => None,
        }
    }

    fn remove(&self, i: &Ident) {
        fn rem(s: &Scope, i: Id) {
            s.idents.borrow_mut().remove(&i);

            match s.parent {
                Some(ref p) => rem(p, i),
                _ => {}
            }
        }

        rem(self, id(i))
    }
}

impl Fold<Function> for Inline<'_> {
    fn fold(&mut self, f: Function) -> Function {
        self.child(|c| f.fold_children(c))
    }
}

impl Fold<AssignExpr> for Inline<'_> {
    fn fold(&mut self, e: AssignExpr) -> AssignExpr {
        let e = e.fold_children(self);

        match e.left {
            PatOrExpr::Pat(box Pat::Ident(ref i)) => match *e.right {
                Expr::Lit(..) if e.op == op!("=") => {
                    self.scope.idents.get_mut().insert(id(i), *e.right.clone());
                }
                _ => self.scope.remove(i),
            },
            _ => {}
        }

        e
    }
}

impl Fold<VarDecl> for Inline<'_> {
    fn fold(&mut self, v: VarDecl) -> VarDecl {
        let v = v.fold_children(self);

        for decl in &v.decls {
            match decl.name {
                Pat::Ident(ref i) => match decl.init {
                    Some(ref e @ box Expr::Lit(..)) => {
                        self.scope.idents.get_mut().insert(id(i), *e.clone());
                    }
                    _ => self.scope.remove(i),
                },
                _ => {}
            }
        }

        v
    }
}

impl Fold<Expr> for Inline<'_> {
    fn fold(&mut self, e: Expr) -> Expr {
        let e = e.fold_children(self);

        match e {
            Expr::Ident(ref i) => {
                if let Some(e) = self.scope.find(i) {
                    return e;
                }
            }
            _ => {}
        }

        e
    }
}
