//! functions —— 首批函数的 inventory 注册（方案 §3 目录）。
//!
//! 每个函数一条 `inventory::submit!`（母版 cmx-core iam::registry 的 RegisteredPermission）。
//! 注册的是**元数据**（名/分类/原型/向导/帮助/示例）——算法本体在 `eval.rs` 按名分发。
//! 取数五元（QM/QC/FS/JE/REF）+ 汇总/逻辑/数学（SUM/IF/ROUND/ABS/MIN/MAX）。

use crate::registry::{
    FnCategory, Param, ParamKind, Prototype, RegisteredRptFn, RptFunction, ValueType, WizardSpec,
    p, p_opt,
};

// ─────────────────────── 取数类 ───────────────────────

fn qm() -> RptFunction {
    RptFunction {
        name: "QM",
        category: FnCategory::Fetch,
        ret_type: ValueType::Amount,
        is_fetch: true,
        prototype: Prototype {
            params: vec![
                p_opt(
                    "期间",
                    ParamKind::Period,
                    Some("0"),
                    "0本期/-1上期/-12上年同期，或绝对期间码",
                ),
                p_opt(
                    "组织机构",
                    ParamKind::Org,
                    Some("@current"),
                    "@current 当前组织，或组织码",
                ),
                p("取数对象", ParamKind::Object, "科目码或元素码"),
            ],
            variadic: None,
        },
        help: "期末余额",
        example: "QM(0,@current,'1001')",
        wizard: WizardSpec { preview: true },
    }
}

fn qc() -> RptFunction {
    RptFunction {
        name: "QC",
        category: FnCategory::Fetch,
        ret_type: ValueType::Amount,
        is_fetch: true,
        prototype: Prototype {
            params: vec![
                p_opt(
                    "期间",
                    ParamKind::Period,
                    Some("0"),
                    "0本期/-1上期/-12上年同期，或绝对期间码",
                ),
                p_opt(
                    "组织机构",
                    ParamKind::Org,
                    Some("@current"),
                    "@current 当前组织，或组织码",
                ),
                p("取数对象", ParamKind::Object, "科目码或元素码"),
            ],
            variadic: None,
        },
        help: "期初余额",
        example: "QC(0,@current,'1001')",
        wizard: WizardSpec { preview: true },
    }
}

fn fs() -> RptFunction {
    RptFunction {
        name: "FS",
        category: FnCategory::Fetch,
        ret_type: ValueType::Amount,
        is_fetch: true,
        prototype: Prototype {
            params: vec![
                p_opt(
                    "期间",
                    ParamKind::Period,
                    Some("0"),
                    "0本期/-1上期，或绝对期间码",
                ),
                p_opt(
                    "组织机构",
                    ParamKind::Org,
                    Some("@current"),
                    "@current 当前组织，或组织码",
                ),
                p("取数对象", ParamKind::Object, "科目码或元素码"),
                p_opt(
                    "方向",
                    ParamKind::Direction,
                    Some("net"),
                    "debit借方/credit贷方/net净额",
                ),
            ],
            variadic: None,
        },
        help: "本期发生额",
        example: "FS(0,@current,'1001','debit')",
        wizard: WizardSpec { preview: true },
    }
}

fn je() -> RptFunction {
    RptFunction {
        name: "JE",
        category: FnCategory::Fetch,
        ret_type: ValueType::Amount,
        is_fetch: true,
        prototype: Prototype {
            params: vec![
                p_opt(
                    "期间",
                    ParamKind::Period,
                    Some("0"),
                    "0本期/-1上期，或绝对期间码",
                ),
                p_opt(
                    "组织机构",
                    ParamKind::Org,
                    Some("@current"),
                    "@current 当前组织，或组织码",
                ),
                p("取数对象", ParamKind::Object, "科目码或元素码"),
            ],
            variadic: None,
        },
        help: "净发生额（借-贷）",
        example: "JE(0,@current,'1001')",
        wizard: WizardSpec { preview: true },
    }
}

fn ref_fn() -> RptFunction {
    RptFunction {
        name: "REF",
        category: FnCategory::Ref,
        ret_type: ValueType::Amount,
        is_fetch: true,
        prototype: Prototype {
            params: vec![
                p("报表", ParamKind::Report, "目标报表编码（本表或他表）"),
                p("版本", ParamKind::Version, "目标报表版本编码"),
                p("单元格", ParamKind::CellRef, "目标单元格 A1 引用，如 C5"),
                p_opt(
                    "组织机构",
                    ParamKind::Org,
                    Some("@current"),
                    "缺省随当前组织",
                ),
                p_opt("期间", ParamKind::Period, Some("0"), "缺省随当前期间"),
            ],
            variadic: None,
        },
        help: "表间取数（递归解析目标格，检测循环引用）",
        example: "REF('PROFIT','v1',B20)",
        wizard: WizardSpec { preview: true },
    }
}

// ─────────────────────── 汇总/逻辑/数学 ───────────────────────

fn sum() -> RptFunction {
    RptFunction {
        name: "SUM",
        category: FnCategory::Agg,
        ret_type: ValueType::Amount,
        is_fetch: false,
        prototype: Prototype {
            params: vec![],
            variadic: Some(Param {
                name: "加数",
                kind: ParamKind::Expr,
                required: false,
                default: None,
                hint: "单元格/区间/数值/子表达式",
            }),
        },
        help: "求和（支持区间 SUM(D3:D7)）",
        example: "SUM(D3:D7)",
        wizard: WizardSpec { preview: false },
    }
}

fn if_fn() -> RptFunction {
    RptFunction {
        name: "IF",
        category: FnCategory::Logic,
        ret_type: ValueType::Amount,
        is_fetch: false,
        prototype: Prototype {
            params: vec![
                p("条件", ParamKind::Expr, "布尔表达式"),
                p("真值", ParamKind::Expr, "条件成立时取值"),
                p_opt(
                    "假值",
                    ParamKind::Expr,
                    None,
                    "条件不成立时取值（缺省 null）",
                ),
            ],
            variadic: None,
        },
        help: "条件取值",
        example: "IF(C5>0,C5,0)",
        wizard: WizardSpec { preview: false },
    }
}

fn round() -> RptFunction {
    RptFunction {
        name: "ROUND",
        category: FnCategory::Math,
        ret_type: ValueType::Amount,
        is_fetch: false,
        prototype: Prototype {
            params: vec![
                p("数值", ParamKind::Expr, "被四舍五入的值"),
                p_opt("位数", ParamKind::Number, Some("2"), "保留小数位"),
            ],
            variadic: None,
        },
        help: "四舍五入",
        example: "ROUND(C5,2)",
        wizard: WizardSpec { preview: false },
    }
}

fn abs_fn() -> RptFunction {
    RptFunction {
        name: "ABS",
        category: FnCategory::Math,
        ret_type: ValueType::Amount,
        is_fetch: false,
        prototype: Prototype {
            params: vec![p("数值", ParamKind::Expr, "取绝对值的值")],
            variadic: None,
        },
        help: "绝对值",
        example: "ABS(C5)",
        wizard: WizardSpec { preview: false },
    }
}

fn min_fn() -> RptFunction {
    RptFunction {
        name: "MIN",
        category: FnCategory::Math,
        ret_type: ValueType::Amount,
        is_fetch: false,
        prototype: Prototype {
            params: vec![],
            variadic: Some(Param {
                name: "值",
                kind: ParamKind::Expr,
                required: false,
                default: None,
                hint: "单元格/区间/数值",
            }),
        },
        help: "最小值",
        example: "MIN(C5:C9)",
        wizard: WizardSpec { preview: false },
    }
}

fn max_fn() -> RptFunction {
    RptFunction {
        name: "MAX",
        category: FnCategory::Math,
        ret_type: ValueType::Amount,
        is_fetch: false,
        prototype: Prototype {
            params: vec![],
            variadic: Some(Param {
                name: "值",
                kind: ParamKind::Expr,
                required: false,
                default: None,
                hint: "单元格/区间/数值",
            }),
        },
        help: "最大值",
        example: "MAX(C5:C9)",
        wizard: WizardSpec { preview: false },
    }
}

// ─────────────────────── 浮动数据源类 ───────────────────────

/// FLIST —— 罗列一组维度记录，驱动浮动行/列展开（非单元格取数函数）。
///
/// 与 QM/QC 等「单元格取数」不同：FLIST 产出**行集合**（带维度键），由展开引擎
/// （`cmx-rpt-store-pg::expand`）消费，为浮动区把模板行复制成 N 条实例行。登记其元数据
/// 供设计器「浮动区数据源向导」选择与拼串；求值不走单元格 eval（`is_fetch:false`）。
fn flist() -> RptFunction {
    RptFunction {
        name: "FLIST",
        category: FnCategory::Fetch,
        ret_type: ValueType::Text,
        is_fetch: false,
        prototype: Prototype {
            params: vec![
                p("取数对象", ParamKind::Object, "维度对象码，如 'ar_cust' 应收客户"),
                p_opt("组织机构", ParamKind::Org, Some("@current"), "@current 当前组织，或组织码"),
                p_opt("期间", ParamKind::Period, Some("0"), "0本期/-1上期，或绝对期间码"),
                p_opt("取前N", ParamKind::Number, Some("10"), "按度量降序取前 N（0=全量）"),
                p_opt("排序度量", ParamKind::Text, Some("amt"), "排序依据的度量字段，如 amt 余额"),
            ],
            variadic: None,
        },
        help: "罗列维度记录驱动浮动展开（前 N 大客户/供应商等）",
        example: "FLIST('ar_cust',@current,0,10,'amt')",
        wizard: WizardSpec { preview: false },
    }
}

// ─────────────────────── inventory 提交（每函数一条） ───────────────────────

inventory::submit! { RegisteredRptFn { def: qm } }
inventory::submit! { RegisteredRptFn { def: qc } }
inventory::submit! { RegisteredRptFn { def: fs } }
inventory::submit! { RegisteredRptFn { def: je } }
inventory::submit! { RegisteredRptFn { def: ref_fn } }
inventory::submit! { RegisteredRptFn { def: sum } }
inventory::submit! { RegisteredRptFn { def: if_fn } }
inventory::submit! { RegisteredRptFn { def: round } }
inventory::submit! { RegisteredRptFn { def: abs_fn } }
inventory::submit! { RegisteredRptFn { def: min_fn } }
inventory::submit! { RegisteredRptFn { def: max_fn } }
inventory::submit! { RegisteredRptFn { def: flist } }

// ─────────────────────── 合并取数类(CG/IND/ELIM) ───────────────────────

/// 合并数:某合并节点某集团科目的合并后金额(cg_consol_data.consolidated)。
fn cg() -> RptFunction {
    RptFunction {
        name: "CG",
        category: FnCategory::Fetch,
        ret_type: ValueType::Amount,
        is_fetch: true,
        prototype: Prototype {
            params: vec![
                p_opt("期间", ParamKind::Period, Some("0"), "0本期,或绝对期间码"),
                p_opt("合并节点", ParamKind::Org, Some("@current"), "@current 当前节点,或合并节点码"),
                p("集团科目", ParamKind::Object, "集团科目代码"),
            ],
            variadic: None,
        },
        help: "合并数(某合并节点某科目合并后金额)",
        example: "CG(0,@current,'1001')",
        wizard: WizardSpec { preview: true },
    }
}

/// 个别合计(未抵销):cg_consol_data.individual。
fn ind() -> RptFunction {
    RptFunction {
        name: "IND",
        category: FnCategory::Fetch,
        ret_type: ValueType::Amount,
        is_fetch: true,
        prototype: Prototype {
            params: vec![
                p_opt("期间", ParamKind::Period, Some("0"), "0本期,或绝对期间码"),
                p_opt("合并节点", ParamKind::Org, Some("@current"), "@current 当前节点,或合并节点码"),
                p("集团科目", ParamKind::Object, "集团科目代码"),
            ],
            variadic: None,
        },
        help: "个别合计(下级并入,未抵销)",
        example: "IND(0,@current,'1001')",
        wizard: WizardSpec { preview: false },
    }
}

/// 抵销额:cg_consol_data.elim。
fn elim() -> RptFunction {
    RptFunction {
        name: "ELIM",
        category: FnCategory::Fetch,
        ret_type: ValueType::Amount,
        is_fetch: true,
        prototype: Prototype {
            params: vec![
                p_opt("期间", ParamKind::Period, Some("0"), "0本期,或绝对期间码"),
                p_opt("合并节点", ParamKind::Org, Some("@current"), "@current 当前节点,或合并节点码"),
                p("集团科目", ParamKind::Object, "集团科目代码"),
            ],
            variadic: None,
        },
        help: "抵销额(该科目在该节点的抵销净额)",
        example: "ELIM(0,@current,'1122')",
        wizard: WizardSpec { preview: false },
    }
}

inventory::submit! { RegisteredRptFn { def: cg } }
inventory::submit! { RegisteredRptFn { def: ind } }
inventory::submit! { RegisteredRptFn { def: elim } }
