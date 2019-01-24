// Copyright (c) 2016-2019 Fabian Schuiki

//! Representation of constant values and their operations
//!
//! This module implements a representation for values that may arise within a
//! SystemVerilog source text and provides ways of executing common operations
//! such as addition and multiplication. It also provides the ability to
//! evaluate the constant value of nodes in a context.
//!
//! The operations in this module are intended to panic if invalid combinations
//! of values are used. The compiler's type system should catch and prevent such
//! uses.

use crate::{
    crate_prelude::*,
    hir::HirNode,
    ty::{Type, TypeKind},
    ParamEnv, ParamEnvBinding,
};
use num::{BigInt, BigRational, One, Zero};

/// A verilog value.
pub type Value<'t> = &'t ValueData<'t>;

/// The data associated with a value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValueData<'t> {
    /// The type of the value.
    pub ty: Type<'t>,
    /// The actual value.
    pub kind: ValueKind,
}

/// The different forms a value can assume.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ValueKind {
    /// The `void` value.
    Void,
    /// An arbitrary precision integer.
    Int(BigInt),
    /// An arbitrary precision time interval.
    Time(BigRational),
}

/// Create a new integer value.
///
/// Panics if `ty` is not an integer type. Truncates the value to `ty`.
pub fn make_int(ty: Type, mut value: BigInt) -> ValueData {
    match *ty {
        TypeKind::Int(width, _) => {
            value = value % (BigInt::from(1) << width);
        }
        TypeKind::Bit(_) => {
            value = value % 2;
        }
        _ => panic!("create int value `{}` with non-int type {:?}", value, ty),
    }
    ValueData {
        ty: ty,
        kind: ValueKind::Int(value),
    }
}

/// Create a new time value.
pub fn make_time(value: BigRational) -> ValueData<'static> {
    ValueData {
        ty: &ty::TIME_TYPE,
        kind: ValueKind::Time(value),
    }
}

/// Determine the constant value of a node.
pub(crate) fn constant_value_of<'gcx>(
    cx: &impl Context<'gcx>,
    node_id: NodeId,
    env: ParamEnv,
) -> Result<Value<'gcx>> {
    let hir = cx.hir_of(node_id)?;
    match hir {
        HirNode::Expr(expr) => const_expr(cx, expr, env),
        HirNode::ValueParam(param) => {
            let env_data = cx.param_env_data(env);
            match env_data.find_value(node_id) {
                Some(ParamEnvBinding::Indirect(assigned_id)) => {
                    return cx.constant_value_of(assigned_id.0, assigned_id.1)
                }
                Some(ParamEnvBinding::Direct(v)) => return Ok(v),
                _ => (),
            }
            if let Some(default) = param.default {
                return cx.constant_value_of(default, env);
            }
            let mut d = DiagBuilder2::error(format!(
                "{} not assigned and has no default",
                param.desc_full(),
            ));
            let contexts = cx.param_env_contexts(env);
            for &context in &contexts {
                d = d.span(cx.span(context));
            }
            if contexts.is_empty() {
                d = d.span(param.human_span());
            }
            cx.emit(d);
            Err(())
        }
        _ => cx.unimp_msg("constant value computation of", &hir),
    }
}

/// Determine the constant value of an expression.
fn const_expr<'gcx>(
    cx: &impl Context<'gcx>,
    expr: &hir::Expr,
    env: ParamEnv,
) -> Result<Value<'gcx>> {
    let ty = cx.type_of(expr.id, env)?;
    #[allow(unreachable_patterns)]
    match expr.kind {
        hir::ExprKind::IntConst(ref k) => Ok(cx.intern_value(make_int(ty, k.clone()))),
        hir::ExprKind::TimeConst(ref k) => Ok(cx.intern_value(make_time(k.clone()))),
        hir::ExprKind::Ident(_) => cx.constant_value_of(cx.resolve_node(expr.id, env)?, env),
        hir::ExprKind::Unary(op, arg) => {
            let arg_val = cx.constant_value_of(arg, env)?;
            debug!("exec {:?}({:?})", op, arg_val);
            match arg_val.kind {
                ValueKind::Int(ref arg) => Ok(cx.intern_value(make_int(
                    ty,
                    const_unary_op_on_int(cx, expr.span, ty, op, arg)?,
                ))),
                _ => {
                    cx.emit(
                        DiagBuilder2::error(format!(
                            "{} cannot be applied to the given arguments",
                            op.desc_full(),
                        ))
                        .span(expr.span()),
                    );
                    Err(())
                }
            }
        }
        hir::ExprKind::Binary(op, lhs, rhs) => {
            let lhs_val = cx.constant_value_of(lhs, env)?;
            let rhs_val = cx.constant_value_of(rhs, env)?;
            debug!("exec {:?}({:?}, {:?})", op, lhs_val, rhs_val);
            match (&lhs_val.kind, &rhs_val.kind) {
                (&ValueKind::Int(ref lhs), &ValueKind::Int(ref rhs)) => Ok(cx.intern_value(
                    make_int(ty, const_binary_op_on_int(cx, expr.span, ty, op, lhs, rhs)?),
                )),
                _ => {
                    cx.emit(
                        DiagBuilder2::error(format!(
                            "{} cannot be applied to the given arguments",
                            op.desc_full(),
                        ))
                        .span(expr.span()),
                    );
                    Err(())
                }
            }
        }
        _ => cx.unimp_msg("constant value computation of", expr),
    }
}

fn const_unary_op_on_int<'gcx>(
    cx: &impl Context<'gcx>,
    span: Span,
    ty: Type<'gcx>,
    op: hir::UnaryOp,
    arg: &BigInt,
) -> Result<BigInt> {
    Ok(match op {
        hir::UnaryOp::BitNot => (BigInt::one() << ty.width()) - 1 - arg,
        hir::UnaryOp::LogicNot => (arg.is_zero() as usize).into(),
        _ => {
            cx.emit(
                DiagBuilder2::error(format!(
                    "{} cannot be applied to integer `{}`",
                    op.desc_full(),
                    arg,
                ))
                .span(span),
            );
            return Err(());
        }
    })
}

fn const_binary_op_on_int<'gcx>(
    cx: &impl Context<'gcx>,
    span: Span,
    _ty: Type<'gcx>,
    op: hir::BinaryOp,
    lhs: &BigInt,
    rhs: &BigInt,
) -> Result<BigInt> {
    Ok(match op {
        hir::BinaryOp::Add => lhs + rhs,
        hir::BinaryOp::Sub => lhs - rhs,
        hir::BinaryOp::Eq => ((lhs == rhs) as usize).into(),
        hir::BinaryOp::Neq => ((lhs != rhs) as usize).into(),
        hir::BinaryOp::Lt => ((lhs < rhs) as usize).into(),
        hir::BinaryOp::Leq => ((lhs <= rhs) as usize).into(),
        hir::BinaryOp::Gt => ((lhs > rhs) as usize).into(),
        hir::BinaryOp::Geq => ((lhs >= rhs) as usize).into(),
        _ => {
            cx.emit(
                DiagBuilder2::error(format!(
                    "{} cannot be applied to integers `{}` and `{}`",
                    op.desc_full(),
                    lhs,
                    rhs
                ))
                .span(span),
            );
            return Err(());
        }
    })
}

/// Determine the default value of a type.
pub(crate) fn type_default_value<'gcx>(cx: &impl Context<'gcx>, ty: Type<'gcx>) -> Value<'gcx> {
    match *ty {
        TypeKind::Void => cx.intern_value(ValueData {
            ty: &ty::VOID_TYPE,
            kind: ValueKind::Void,
        }),
        TypeKind::Time => cx.intern_value(make_time(Zero::zero())),
        TypeKind::Bit(..) | TypeKind::Int(..) => cx.intern_value(make_int(ty, Zero::zero())),
        TypeKind::Named(_, _, ty) => type_default_value(cx, ty),
    }
}