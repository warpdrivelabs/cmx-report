//! statements —— C7 出表:合并四表模板 seed(资产负债表/利润表/现金流量表/所有者权益变动表)。
//!
//! 把合并四表**报表定义**(cr_report_list/version/sheet/region + cr_cell_element_map 单元格公式)
//! 幂等 seed 进 fico-db,公式用已接好的合并取数函数 `CG(期间偏移,@current,'集团科目')` 从
//! cg_consol_data 读合并数。之后走标准报表计算态 `POST /report-design/reports/{code}/compute`
//! (body 带 schemeCode)即出表——完整复用 cmx-rpt-formula 引擎,不新增计算路径。
//!
//! ## 数据支撑边界(诚实标注)
//! - **CBS 合并资产负债表 / CIS 合并利润表**:科目余额型,逐科目 CG 取数 + SUM 小计 → **真出数**。
//! - **CCF 合并现金流量表 / CSE 所有者权益变动表**:需现金流量项目流水 / 权益变动流水,
//!   引擎当前只有科目余额,故 seed **结构化模板壳**(行/列/表头齐全,数据格暂留占位),
//!   待补现金流量/权益变动数据模型后填公式。四表定义齐全,报表目录/工作台可见可打开。
//!
//! 落库全走 `crate::execute`(fico-db 非事务单语句),报表码在 seed 前先 DELETE 同码定义→幂等重跑。

use serde_json::{Value, json};

use cmx_core::model::cell::DataValue;

use crate::{Result, execute};

const VERSION: &str = "V1";
const SHEET: &str = "Sheet1";
const REGION: &str = "__default__";

fn pk() -> DataValue {
    DataValue::Int(cmx_utils::next_pk_id())
}

/// 一行报表定义:行号、A 列标签、B 列公式(空=纯标签/小计行)、是否粗体(小计/合计)。
struct Line {
    label: &'static str,
    formula: Option<String>,
    bold: bool,
}
fn lbl(label: &'static str) -> Line { Line { label, formula: None, bold: false } }
fn amt(label: &'static str, formula: String) -> Line { Line { label, formula: Some(formula), bold: false } }
fn sub(label: &'static str, formula: String) -> Line { Line { label, formula: Some(formula), bold: true } }

/// CG 取数:某集团科目在当前合并节点当期的合并数(借方正)。
fn cg(acc: &str) -> String { format!("CG(0,@current,'{acc}')") }
/// 展示口径翻正:权益/负债/收入类科目 CG 为负,展示取负号变正。
fn cg_neg(acc: &str) -> String { format!("-CG(0,@current,'{acc}')") }
/// CF 取数:某现金流量项目在当前合并节点当期的合并数(借方正:流入+/流出−)。
fn cf(item: &str) -> String { format!("CF(0,@current,'{item}')") }
/// 展示口径翻正:权益为贷方(借方正为负),权益变动表列展示取负号变正。
fn eqc_neg(col: &str) -> String { format!("-EQC(0,@current,'{col}')") }

/// 一张报表的完整定义。
struct Statement {
    code: &'static str,
    name: &'static str,
    lines: Vec<Line>,
}

/// 合并资产负债表(CBS):资产侧 + 权益负债侧,均展示为正,末行校验总资产=总权益负债。
fn consol_balance_sheet() -> Statement {
    let lines = vec![
        lbl("一、资产"),
        amt("　货币资金", cg("1001")),
        amt("　应收账款", cg("1122")),
        amt("　长期股权投资", cg("1511")),
        amt("　商誉", cg("1801")),
        sub("资产合计", "SUM(B2:B5)".into()),
        lbl("二、负债"),
        amt("　应付账款", cg_neg("2202")),
        sub("负债合计", "SUM(B8:B8)".into()),
        lbl("三、所有者权益"),
        amt("　实收资本", cg_neg("4001")),
        amt("　未分配利润", cg_neg("4104")),
        amt("　外币折算差额", cg_neg("4106")),
        amt("　少数股东权益", cg_neg("4400")),
        sub("所有者权益合计", "SUM(B11:B14)".into()),
        sub("负债和所有者权益合计", "B9+B15".into()),
    ];
    Statement { code: "CBS", name: "合并资产负债表", lines }
}

/// 合并利润表(CIS):收入 − 成本 = 利润总额,分归母/少数股东。
fn consol_income_statement() -> Statement {
    let lines = vec![
        amt("一、营业收入", cg_neg("6001")),
        amt("　减:营业成本", cg("6401")),
        sub("二、营业利润", "B1-B2".into()),
        sub("三、利润总额", "B3".into()),
        lbl("四、净利润归属"),
        amt("　少数股东损益", cg("4900")),
        sub("　归属于母公司股东的净利润", "B4-B6".into()),
    ];
    Statement { code: "CIS", name: "合并利润表", lines }
}

/// 合并现金流量表(CCF):经营/投资/筹资三活动,数据经 CF 取数(cg_cash_flow_item 聚合)真出数。
/// 借方正口径:流入+/流出−;各活动净额 = SUM,现金净增加 = 三活动净额之和。
fn consol_cash_flow() -> Statement {
    let lines = vec![
        lbl("一、经营活动产生的现金流量"),
        amt("　销售商品、提供劳务收到的现金", cf("CF01")),
        amt("　收到的其他与经营活动有关的现金", cf("CF02")),
        amt("　购买商品、接受劳务支付的现金", cf("CF03")),
        amt("　支付给职工以及为职工支付的现金", cf("CF04")),
        amt("　支付的各项税费", cf("CF05")),
        amt("　支付的其他与经营活动有关的现金", cf("CF06")),
        sub("　经营活动现金流量净额", "SUM(B2:B7)".into()),
        lbl("二、投资活动产生的现金流量"),
        amt("　收回投资收到的现金", cf("CF11")),
        amt("　购建固定资产、无形资产支付的现金", cf("CF12")),
        amt("　投资支付的现金", cf("CF13")),
        sub("　投资活动现金流量净额", "SUM(B10:B12)".into()),
        lbl("三、筹资活动产生的现金流量"),
        amt("　吸收投资收到的现金", cf("CF21")),
        amt("　取得借款收到的现金", cf("CF22")),
        amt("　偿还债务支付的现金", cf("CF23")),
        amt("　分配股利、利润或偿付利息支付的现金", cf("CF24")),
        sub("　筹资活动现金流量净额", "SUM(B15:B18)".into()),
        sub("四、现金及现金等价物净增加额", "B8+B13+B19".into()),
    ];
    Statement { code: "CCF", name: "合并现金流量表", lines }
}

/// 合并所有者权益变动表(CSE):按变动列 EC 取数(cg_equity_change 聚合)真出数。
/// 借方正下权益为负,展示列取负号翻正;期末 = 期初 + 本年增减。
fn consol_equity_changes() -> Statement {
    let lines = vec![
        sub("一、上年年末余额", eqc_neg("EC01")),
        lbl("二、本年增减变动"),
        amt("　(一)综合收益总额", eqc_neg("EC02")),
        amt("　(二)所有者投入和减少资本", eqc_neg("EC03")),
        amt("　(三)利润分配", eqc_neg("EC04")),
        amt("　(四)少数股东权益变动", eqc_neg("EC05")),
        amt("　(五)其他权益变动", eqc_neg("EC06")),
        sub("　本年增减变动小计", "SUM(B3:B7)".into()),
        sub("三、本年年末余额", "B1+B8".into()),
    ];
    Statement { code: "CSE", name: "合并所有者权益变动表", lines }
}

/// seed 一张报表定义(先删同码 → 建 list/version/sheet/region → 逐行建 cell_element_map)。
async fn seed_one(st: &Statement) -> Result<usize> {
    let code = st.code;
    // 幂等:先删该报表所有定义分片。
    for tbl in [
        "cr_cell_element_map", "cr_report_region", "cr_report_sheet", "cr_report_version", "cr_report_list",
    ] {
        execute(
            &format!("DELETE FROM {tbl} WHERE report_code=$1"),
            vec![DataValue::String(code.to_string())],
        )
        .await
        .ok(); // cr_report_list 用 code 列
    }
    execute(
        "DELETE FROM cr_report_list WHERE code=$1",
        vec![DataValue::String(code.to_string())],
    )
    .await?;

    // 1) 报表清单。
    execute(
        "INSERT INTO cr_report_list (code, name, report_type, report_category, period_type, \
            is_statutory, sort_no, status, create_time, update_time) \
         VALUES ($1,$2,'consol','consolidation','month',1,0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)",
        vec![DataValue::String(code.to_string()), DataValue::String(st.name.to_string())],
    )
    .await?;

    // 2) 版本(当前版)。
    execute(
        "INSERT INTO cr_report_version (code, name, report_code, version_no, version_status, is_current, \
            sort_no, status, create_time, update_time) \
         VALUES ($1,$2,$3,1,'published',1,0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)",
        vec![
            DataValue::String(VERSION.to_string()),
            DataValue::String(format!("{}·V1", st.name)),
            DataValue::String(code.to_string()),
        ],
    )
    .await?;

    // 3) sheet。
    execute(
        "INSERT INTO cr_report_sheet (report_code, version_code, sheet_index, name, sheet_type, \
            row_count, col_count, show_gridline, is_hidden, sort_no, status, create_time, update_time) \
         VALUES ($1,$2,0,$3,'data',$4,2,1,0,0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)",
        vec![
            DataValue::String(code.to_string()),
            DataValue::String(VERSION.to_string()),
            DataValue::String(SHEET.to_string()),
            DataValue::Int(st.lines.len() as i64),
        ],
    )
    .await?;

    // 4) region(默认整表区)。
    execute(
        "INSERT INTO cr_report_region (report_code, version_code, sheet_code, region_code, region_name, \
            region_type, is_repeatable, is_merged, freeze_flag, sort_no, status, create_time, update_time) \
         VALUES ($1,$2,$3,$4,'默认区','data',0,0,0,0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)",
        vec![
            DataValue::String(code.to_string()),
            DataValue::String(VERSION.to_string()),
            DataValue::String(SHEET.to_string()),
            DataValue::String(REGION.to_string()),
        ],
    )
    .await?;

    // 5) 逐行:A 列标签 + B 列公式。A 列标签用**字符串字面量公式** `'标签'`,
    //    经计算引擎求值为文本并落 cr_cell_data(渲染读 cr_cell_data.text_value,非 remark)。
    let mut cells = 0usize;
    for (i, line) in st.lines.iter().enumerate() {
        let row = (i + 1) as i64;
        // A 列:标签 = 字符串字面量公式(单引号内转义单引号)。
        let label_formula = format!("'{}'", line.label.replace('\'', "\\'"));
        insert_cell(code, row, 1, &format!("A{row}"), "text", &label_formula, line.bold).await?;
        cells += 1;
        // B 列:数值公式(有则建)。
        if let Some(f) = &line.formula {
            insert_cell(code, row, 2, &format!("B{row}"), "amount", f, line.bold).await?;
            cells += 1;
        }
    }
    Ok(cells)
}

/// 建一个单元格映射(A 列标签 + B 列公式均走 calc_formula;A 列标签是字符串字面量公式)。
async fn insert_cell(
    code: &str,
    row: i64,
    col: i64,
    cell_ref: &str,
    value_type: &str,
    formula: &str,
    bold: bool,
) -> Result<()> {
    let name = format!("{code}!{cell_ref}");
    execute(
        "INSERT INTO cr_cell_element_map (id, code, name, report_code, version_code, sheet_code, region_code, \
            row_id, col_id, cell_ref, value_type, calc_formula, is_editable, remark, sort_no, status, create_time, update_time) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,0,$13,0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)",
        vec![
            pk(),
            DataValue::String(format!("{code}|{cell_ref}")),
            DataValue::String(name),
            DataValue::String(code.to_string()),
            DataValue::String(VERSION.to_string()),
            DataValue::String(SHEET.to_string()),
            DataValue::String(REGION.to_string()),
            DataValue::Int(row),
            DataValue::Int(col),
            DataValue::String(cell_ref.to_string()),
            DataValue::String(value_type.to_string()),
            DataValue::String(formula.to_string()),
            // 粗体标记(小计/合计行)存 remark 供前端加粗。
            DataValue::String(if bold { "[b]".to_string() } else { String::new() }),
        ],
    )
    .await?;
    Ok(())
}

/// C7 出表:seed 合并四表模板定义。幂等重跑。返回 { ok, reports:[{code,name,cells}] }。
pub async fn seed_consol_statements() -> Result<Value> {
    let statements = [
        consol_balance_sheet(),
        consol_income_statement(),
        consol_cash_flow(),
        consol_equity_changes(),
    ];
    let mut out = Vec::new();
    for st in &statements {
        let cells = seed_one(st).await?;
        out.push(json!({ "code": st.code, "name": st.name, "cells": cells, "rows": st.lines.len() }));
    }
    Ok(json!({
        "ok": true,
        "message": format!("合并四表模板已 seed({} 张)", statements.len()),
        "reports": out,
    }))
}
