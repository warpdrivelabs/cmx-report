//! eval —— 纯同步求值 + 求值上下文 + 取数键派生。
//!
//! 两遍法的 **pass2**：所有取数(QM/QC/…)、单元格引用、REF 已由 `resolve.rs` 异步预解析、
//! 填进 `Scope`；本模块只做纯内存运算（对齐 `doc/formula.rs` 语义）。取数/REF 的**键派生**
//! （`derive_balance_key`/`derive_ref_addr`）在此定义，供 `resolve.rs`（收集依赖）与本模块
//! （查表）共用同一份逻辑，保证"收集什么"和"查什么"逐位一致。

use std::collections::HashMap;

use rust_decimal::Decimal;
use rust_decimal::prelude::*;

use crate::ast::{Node, expand_range};
use crate::provider::{BalanceKey, FetchKind};
use crate::value::FValue;

/// 求值上下文：当前组织/期间 + 相对期间/组织解析所需字典。
#[derive(Debug, Clone, Default)]
pub struct EvalCtx {
    /// 当前组织码（@current 解析目标）。
    pub org: String,
    /// 当前期间码（相对期间偏移基准）。
    pub period: String,
    /// 升序叶子期间码序列（相对期间偏移用；空则相对期间退化为当前期间）。
    pub periods: Vec<String>,
    /// 组织父级映射（@parent 解析用）。
    pub org_parent: HashMap<String, String>,
}

impl EvalCtx {
    /// 相对期间偏移：offset<=0 向前回溯 |offset| 个叶子期间；越界返回当前期间。
    pub fn resolve_period_offset(&self, offset: i64) -> String {
        if offset == 0 || self.periods.is_empty() {
            return self.period.clone();
        }
        if let Some(idx) = self.periods.iter().position(|p| p == &self.period) {
            let target = idx as i64 + offset;
            if target >= 0 && (target as usize) < self.periods.len() {
                return self.periods[target as usize].clone();
            }
        }
        self.period.clone()
    }

    /// 组织记号解析：current→当前组织，parent→父级，root→顶层，其它→原样。
    pub fn resolve_org_ref(&self, name: &str) -> String {
        match name {
            "current" | "self" => self.org.clone(),
            "parent" => self
                .org_parent
                .get(&self.org)
                .cloned()
                .unwrap_or_else(|| self.org.clone()),
            "root" => {
                let mut cur = self.org.clone();
                while let Some(p) = self.org_parent.get(&cur) {
                    if p == &cur {
                        break;
                    }
                    cur = p.clone();
                }
                cur
            }
            other => other.to_string(),
        }
    }
}

/// 求值作用域：pass1 已把所有外部依赖解析进这些表，pass2 只读不算 IO。
pub struct Scope<'a> {
    /// 裸字段引用（缺失按 0）。
    pub fields: HashMap<String, FValue>,
    /// 本报表单元格引用值（cellRef → 已解析值）。
    pub cells: HashMap<String, FValue>,
    /// 取数结果（BalanceKey → 值），pass1 批量/按需填充。
    pub fetches: HashMap<BalanceKey, Decimal>,
    /// REF 跨表/表内目标值（目标地址串 → 已解析值）。
    pub refs: HashMap<String, FValue>,
    pub ctx: &'a EvalCtx,
}

impl<'a> Scope<'a> {
    pub fn new(ctx: &'a EvalCtx) -> Self {
        Scope {
            fields: HashMap::new(),
            cells: HashMap::new(),
            fetches: HashMap::new(),
            refs: HashMap::new(),
            ctx,
        }
    }
}

/// 取数键派生：从 QM/QC/FS/JE 的实参算出 BalanceKey（period/org 已解析成绝对码）。
/// 与 resolve.rs 收集依赖时同一逻辑——保证"取什么"和"查什么"一致。
pub fn derive_balance_key(name: &str, args: &[Node], scope: &Scope) -> Option<BalanceKey> {
    let kind = match name {
        "QM" => FetchKind::EndBalance,
        "QC" => FetchKind::BeginBalance,
        "JE" => FetchKind::NetAmount,
        // 合并取数:CG/IND/ELIM(期间, 合并节点, 集团科目)——复用 period/org/object 参数布局。
        "CG" => FetchKind::Consolidated,
        "IND" => FetchKind::Individual,
        "ELIM" => FetchKind::Elimination,
        "FS" => {
            // 第 4 参方向：debit/credit，缺省 net
            let dir = args
                .get(3)
                .map(|a| eval_node(a, scope).as_text().to_ascii_lowercase());
            match dir.as_deref() {
                Some("debit") | Some("dr") | Some("借") => FetchKind::DebitAmount,
                Some("credit") | Some("cr") | Some("贷") => FetchKind::CreditAmount,
                _ => FetchKind::NetAmount,
            }
        }
        _ => return None,
    };

    // 期间：Num→相对偏移；Str→绝对码；缺省→当前
    let period = match args.first() {
        Some(n) => {
            let v = eval_node(n, scope);
            match v {
                FValue::Num(d) => scope.ctx.resolve_period_offset(d.to_i64().unwrap_or(0)),
                other => {
                    let t = other.as_text();
                    if t.is_empty() {
                        scope.ctx.period.clone()
                    } else {
                        t
                    }
                }
            }
        }
        None => scope.ctx.period.clone(),
    };

    // 组织：缺省当前
    let org = match args.get(1) {
        Some(a) => {
            let t = eval_node(a, scope).as_text();
            if t.is_empty() {
                scope.ctx.org.clone()
            } else {
                t
            }
        }
        None => scope.ctx.org.clone(),
    };

    // 取数对象：科目码/元素码（provider 负责元素→科目映射）
    let object = args
        .get(2)
        .map(|a| eval_node(a, scope).as_text())
        .unwrap_or_default();

    Some(BalanceKey {
        kind,
        period,
        org,
        object,
    })
}

/// REF 目标地址派生：REF(报表, 版本, 单元格 [,组织,期间])。
/// 返回 (report, version, org, period, cell)，org/period 缺省随当前上下文。
pub fn derive_ref_addr(
    args: &[Node],
    scope: &Scope,
) -> Option<(String, String, String, String, String)> {
    let report = args.first().map(|a| eval_node(a, scope).as_text())?;
    let version = args
        .get(1)
        .map(|a| eval_node(a, scope).as_text())
        .unwrap_or_default();
    // 单元格：字面 Cell 节点取名，否则求值取文本
    let cell = match args.get(2) {
        Some(Node::Cell(c)) => c.clone(),
        Some(other) => eval_node(other, scope).as_text().to_ascii_uppercase(),
        None => return None,
    };
    let org = args
        .get(3)
        .map(|a| eval_node(a, scope).as_text())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| scope.ctx.org.clone());
    let period = args
        .get(4)
        .map(|a| eval_node(a, scope).as_text())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| scope.ctx.period.clone());
    if report.is_empty() || cell.is_empty() {
        return None;
    }
    Some((report, version, org, period, cell))
}

/// 全局地址串（memo/refs 键）：report|version|org|period|cell。
pub fn addr_key(report: &str, version: &str, org: &str, period: &str, cell: &str) -> String {
    format!("{report}|{version}|{org}|{period}|{cell}")
}

// ─────────────────────── 同步求值 ───────────────────────

/// 求值一个 AST 节点（纯内存，读 scope）。错误值逐层传播。
pub fn eval_node(node: &Node, scope: &Scope) -> FValue {
    match node {
        Node::Num(n) => FValue::Num(*n),
        Node::Str(s) => FValue::Str(s.clone()),
        Node::Bool(b) => FValue::Bool(*b),
        Node::Null => FValue::Null,
        Node::Ident(name) => scope
            .fields
            .get(name)
            .cloned()
            .unwrap_or(FValue::Num(Decimal::ZERO)),
        Node::OrgRef(name) => FValue::Str(scope.ctx.resolve_org_ref(name)),
        Node::Cell(c) => scope
            .cells
            .get(c)
            .cloned()
            .unwrap_or(FValue::Num(Decimal::ZERO)),
        // 裸区间在标量上下文非法；仅在 SUM 等聚合里被 eval_call 特殊展开。
        Node::Range(_, _) => FValue::Error("#RANGE!".into()),
        Node::Unary(op, operand) => {
            let v = eval_node(operand, scope);
            if v.is_error() {
                return v;
            }
            match op.as_str() {
                "-" => FValue::Num(-v.as_num()),
                "!" => FValue::Bool(!v.as_bool()),
                _ => FValue::Error(format!("#OP!{op}")),
            }
        }
        Node::Binary(op, l, r) => eval_binary(op, l, r, scope),
        Node::Call(name, args) => eval_call(name, args, scope),
    }
}

fn eval_binary(op: &str, l: &Node, r: &Node, scope: &Scope) -> FValue {
    // 逻辑短路
    if op == "&&" {
        let lv = eval_node(l, scope);
        if lv.is_error() {
            return lv;
        }
        if !lv.as_bool() {
            return FValue::Bool(false);
        }
        let rv = eval_node(r, scope);
        return if rv.is_error() {
            rv
        } else {
            FValue::Bool(rv.as_bool())
        };
    }
    if op == "||" {
        let lv = eval_node(l, scope);
        if lv.is_error() {
            return lv;
        }
        if lv.as_bool() {
            return FValue::Bool(true);
        }
        let rv = eval_node(r, scope);
        return if rv.is_error() {
            rv
        } else {
            FValue::Bool(rv.as_bool())
        };
    }
    let lv = eval_node(l, scope);
    let rv = eval_node(r, scope);
    if let Some(e) = FValue::first_error([&lv, &rv]) {
        return e;
    }
    match op {
        "+" => {
            if matches!(lv, FValue::Str(_)) || matches!(rv, FValue::Str(_)) {
                FValue::Str(format!("{}{}", lv.as_text(), rv.as_text()))
            } else {
                FValue::Num(lv.as_num() + rv.as_num())
            }
        }
        "-" => FValue::Num(lv.as_num() - rv.as_num()),
        "*" => FValue::Num(lv.as_num() * rv.as_num()),
        "/" => {
            let d = rv.as_num();
            if d.is_zero() {
                FValue::Num(Decimal::ZERO) // 与 doc/formula.rs 一致：除零按 0
            } else {
                FValue::Num(lv.as_num() / d)
            }
        }
        ">" => FValue::Bool(lv.as_num() > rv.as_num()),
        "<" => FValue::Bool(lv.as_num() < rv.as_num()),
        ">=" => FValue::Bool(lv.as_num() >= rv.as_num()),
        "<=" => FValue::Bool(lv.as_num() <= rv.as_num()),
        "==" => FValue::Bool(values_eq(&lv, &rv)),
        "!=" => FValue::Bool(!values_eq(&lv, &rv)),
        _ => FValue::Error(format!("#OP!{op}")),
    }
}

/// 展开一个参数为其贡献的数值序列（Range→区间内各单元格；否则→单值）。
fn arg_nums(arg: &Node, scope: &Scope) -> Result<Vec<Decimal>, FValue> {
    match arg {
        Node::Range(a, b) => {
            let mut out = Vec::new();
            for c in expand_range(a, b) {
                let v = scope
                    .cells
                    .get(&c)
                    .cloned()
                    .unwrap_or(FValue::Num(Decimal::ZERO));
                if v.is_error() {
                    return Err(v);
                }
                out.push(v.as_num());
            }
            Ok(out)
        }
        other => {
            let v = eval_node(other, scope);
            if v.is_error() {
                return Err(v);
            }
            Ok(vec![v.as_num()])
        }
    }
}

fn eval_call(name: &str, args: &[Node], scope: &Scope) -> FValue {
    match name {
        // ── 取数：从 fetch 缓存命中（resolve.rs 已预取） ──
        // ── 取数：从 fetch 缓存命中（resolve.rs 已预取）。CG/IND/ELIM 合并取数同路。 ──
        "QM" | "QC" | "FS" | "JE" | "CG" | "IND" | "ELIM" => match derive_balance_key(name, args, scope) {
            Some(k) => scope
                .fetches
                .get(&k)
                .copied()
                .map(FValue::Num)
                .unwrap_or(FValue::Num(Decimal::ZERO)),
            None => FValue::Error("#ARG!".into()),
        },
        // ── REF：从 refs 缓存命中（resolve.rs 已递归解析目标） ──
        "REF" => match derive_ref_addr(args, scope) {
            Some((r, v, o, p, c)) => scope
                .refs
                .get(&addr_key(&r, &v, &o, &p, &c))
                .cloned()
                .unwrap_or(FValue::Num(Decimal::ZERO)),
            None => FValue::Error("#REF!".into()),
        },
        // ── 汇总（支持区间） ──
        "SUM" => {
            let mut s = Decimal::ZERO;
            for a in args {
                match arg_nums(a, scope) {
                    Ok(ns) => s += ns.iter().sum::<Decimal>(),
                    Err(e) => return e,
                }
            }
            FValue::Num(s)
        }
        "MIN" | "MAX" => {
            let mut vals: Vec<Decimal> = Vec::new();
            for a in args {
                match arg_nums(a, scope) {
                    Ok(ns) => vals.extend(ns),
                    Err(e) => return e,
                }
            }
            let r = if name == "MIN" {
                vals.into_iter().min()
            } else {
                vals.into_iter().max()
            };
            FValue::Num(r.unwrap_or(Decimal::ZERO))
        }
        // ── 数学 ──
        "ABS" => {
            let v = eval_node(&args[0], scope);
            if v.is_error() {
                return v;
            }
            FValue::Num(v.as_num().abs())
        }
        "ROUND" => {
            let v = eval_node(&args[0], scope);
            if v.is_error() {
                return v;
            }
            let digits = args
                .get(1)
                .map(|a| eval_node(a, scope).as_num().to_u32().unwrap_or(0))
                .unwrap_or(0);
            FValue::Num(v.as_num().round_dp(digits))
        }
        // ── 逻辑 ──
        "IF" => {
            if args.len() < 2 {
                return FValue::Error("#ARG!".into());
            }
            let cond = eval_node(&args[0], scope);
            if cond.is_error() {
                return cond;
            }
            if cond.as_bool() {
                eval_node(&args[1], scope)
            } else {
                args.get(2)
                    .map(|a| eval_node(a, scope))
                    .unwrap_or(FValue::Null)
            }
        }
        "AND" => {
            for a in args {
                let v = eval_node(a, scope);
                if v.is_error() {
                    return v;
                }
                if !v.as_bool() {
                    return FValue::Bool(false);
                }
            }
            FValue::Bool(true)
        }
        "OR" => {
            for a in args {
                let v = eval_node(a, scope);
                if v.is_error() {
                    return v;
                }
                if v.as_bool() {
                    return FValue::Bool(true);
                }
            }
            FValue::Bool(false)
        }
        "NOT" => {
            let v = eval_node(&args[0], scope);
            if v.is_error() {
                return v;
            }
            FValue::Bool(!v.as_bool())
        }
        "ISEMPTY" => {
            let v = eval_node(&args[0], scope);
            FValue::Bool(v.is_empty())
        }
        "COALESCE" => {
            for a in args {
                let v = eval_node(a, scope);
                if v.is_error() {
                    return v;
                }
                if !v.is_empty() {
                    return v;
                }
            }
            FValue::Null
        }
        _ => FValue::Error(format!("#NAME?{name}")),
    }
}

fn values_eq(a: &FValue, b: &FValue) -> bool {
    match (a, b) {
        (FValue::Str(x), FValue::Str(y)) => x == y,
        (FValue::Null, FValue::Null) => true,
        (FValue::Bool(x), FValue::Bool(y)) => x == y,
        _ => (a.as_num() - b.as_num()).abs() < Decimal::new(1, 9),
    }
}
