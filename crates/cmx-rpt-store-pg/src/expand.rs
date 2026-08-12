//! expand —— 报表浮动行列「模板 × 数据源 → 实例行」展开引擎（纯逻辑，无 DB 依赖）。
//!
//! 设计见 `docs/报表浮动行列(动态明细展开)设计方案.html`。核心洞察：一个浮动行 =
//! **模板行（设计态，1 条）× 数据源（运行态，N 条）**。运行时把模板行按数据源记录数
//! 复制 N 份、逐行套用模板的公式（占位符替换）与样式，即得浮动明细。
//!
//! 本模块只做「模板 + 数据源记录 → 实例行」的纯变换：
//!   1. [`stable_instance_row_id`] —— 由 (模板行 id, 维度键路径) 派生**稳定** row_id：
//!      跨期一致（可同比）、幂等重算不漂移、且与 pk52 真号段**不重叠**（JS 安全）。
//!   2. [`substitute`] —— 模板公式/标题里的占位符替换（`{{dim}}`/`{{r}}`/`{{total}}`/…）。
//!   3. [`expand_template`] —— 模板行 × 数据源记录 → 实例行序列。
//!
//! DB 装载（读浮动区/模板行/拉数据源/落 cr_cell_data）在 `lib.rs::expand_report` 里，
//! 本模块保持语义中立、可被单测直接覆盖。

/// 派生实例行 id 的号段基址 = 2^52。
///
/// pk52 真号（[`cmx_utils::next_pk_id`]）布局为 `(秒差<<20)|(node<<12)|seq`，其值域约在
/// `[0, 2^52)`（32 位秒差 << 20，跨度 ~136 年内不达 2^52）。故把派生实例行 id 放到
/// `[2^52, 2^53-1]` 这一段：与真号段**天然不相交**，且上界 2^53-1 = JS `MAX_SAFE_INTEGER`，
/// 序列化成 JSON number 传前端不丢精度（同 pk52 的 JS 安全约束）。
const INSTANCE_ID_BASE: i64 = 1 << 52;
/// 52 位掩码，把哈希收敛到 `[0, 2^52)`。
const INSTANCE_ID_MASK: u64 = (1 << 52) - 1;

/// FNV-1a 64 位哈希（确定性、跨进程/重启稳定——不同于 `HashMap` 的随机 `RandomState`）。
///
/// 稳定性是实例行 id「跨期对齐、幂等」的根基：同一 (模板, 维度键) 无论何时何进程展开，
/// 必得同一 id。故必须用固定种子哈希，绝不能用带随机化的默认哈希器。
fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV offset basis
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3); // FNV prime
    }
    h
}

/// 由「模板行 id + 维度键路径」派生稳定实例行 id。
///
/// - **稳定**：纯函数，同输入恒同输出，跨期/跨进程/跨重启一致 → 华东.C001 在任意期间 row_id 相同，
///   天然对齐做同比/环比；幂等重展开 → `cr_cell_data` ON CONFLICT UPSERT 不产生重复行。
/// - **JS 安全**：结果 ∈ `[2^52, 2^53-1]`，均 ≤ `Number.MAX_SAFE_INTEGER`。
/// - **不撞真号**：与 pk52 的 `[0, 2^52)` 号段不相交。
pub fn stable_instance_row_id(template_row_id: i64, dim_key_path: &str) -> i64 {
    let seed = format!("{template_row_id}|{dim_key_path}");
    let h = fnv1a64(&seed) & INSTANCE_ID_MASK;
    INSTANCE_ID_BASE + h as i64
}

/// 是否为派生的浮动实例行 id（落在保留号段 `[2^52, 2^52+2^52-1]` 内）。
/// 供读取/诊断侧区分「实例行 vs 设计态真行」。
pub fn is_instance_row_id(id: i64) -> bool {
    (INSTANCE_ID_BASE..=INSTANCE_ID_BASE + INSTANCE_ID_MASK as i64).contains(&id)
}

/// 浮动数据源产出的一条记录（运行态，N 条之一）。
#[derive(Debug, Clone, Default)]
pub struct SourceRecord {
    /// 展示名（喂给 `{{label}}`），如「上海 A 公司」。
    pub label: String,
    /// 有序维度键值对（喂给 `{{dim}}`），如 `[("cust_code","C001"),("region","华东")]`。
    /// 有序保证维度键路径确定 → 稳定 id 确定。
    pub dims: Vec<(String, String)>,
    /// 存储态该行的显式单元格值/公式覆盖：`(列标, 值或公式)`。非空时**覆盖模板公式**——
    /// 用户 CRUD 录入的具体数值（如 B=2600）优先于模板的 QM 取数公式。空表示走模板。
    pub cells: Vec<(String, String)>,
}

/// 浮动模板行（设计态，1 条，与 org/period 无关）。
#[derive(Debug, Clone)]
pub struct FloatTemplate {
    /// 模板行在 `cr_report_row` 里的真实 id（派生实例 id 的种子之一）。
    pub template_row_id: i64,
    /// 行标题模板，如 `"{{label}}"` 或 `"{{cust_name}}"`。
    pub name_tpl: String,
    /// 各列的公式模板：`(列标, 公式模板)`，如 `("B","QM(0,@current,'{{cust_code}}')")`。
    pub cell_tpls: Vec<(String, String)>,
}

/// 展开后的一条实例行（运行态派生物）。行浮动(P1 扁平)与分级浮动(P2 层级)统一用此结构，
/// 前端/后端单一渲染路径：`row_type` 区分明细/小计/合计，`level_no` 供缩进，`parent_row` 供分组。
#[derive(Debug, Clone, PartialEq)]
pub struct InstanceRow {
    /// 稳定派生 row_id。
    pub row_id: i64,
    /// 维度键路径（`k=v;k=v`），落库与诊断用。
    pub dim_key_path: String,
    /// 解析后的行标题。
    pub name: String,
    /// 解析后的各列公式：`(列标, 已替换占位符的公式)`。
    pub cells: Vec<(String, String)>,
    /// 展开序（1-based），落 `cr_report_row.sort_no` / 画布行序用。
    pub sort_no: i32,
    /// 实例行在画布上的物理行号（1-based），供上层 setValue/setFormula 定位。
    pub phys_row: i64,
    /// 行类型：`float` 明细 / `subtotal` 小计 / `total` 合计。对齐 cr_report_row.row_type。
    pub row_type: String,
    /// 层级深度（1-based；分级浮动里 合计=1、小计=2、明细=3 之类，扁平=1）。
    pub level_no: i32,
    /// 父行物理行号（分组归属；扁平/顶层为 None）。
    pub parent_row: Option<i64>,
}

/// 占位符替换上下文。维度 + 语义锚点行号（`{{r}}` 本行 / `{{total}}` 合计行 / `{{parent}}` 父行）
/// + 列浮动的当前列标（`{{c}}`）。
pub struct SubstCtx<'a> {
    pub dims: &'a [(String, String)],
    pub label: &'a str,
    pub phys_row: Option<i64>,
    pub total_row: Option<i64>,
    pub parent_row: Option<i64>,
    /// 列浮动：当前实例列的列标（如 `C`），供 `{{c}}` 重定位。行浮动为 None。
    pub col_letter: Option<String>,
}

impl<'a> SubstCtx<'a> {
    fn resolve(&self, key: &str) -> String {
        match key {
            "r" => self.phys_row.map(|n| n.to_string()).unwrap_or_default(),
            "total" => self.total_row.map(|n| n.to_string()).unwrap_or_default(),
            "parent" => self.parent_row.map(|n| n.to_string()).unwrap_or_default(),
            "c" => self.col_letter.clone().unwrap_or_default(),
            "label" => self.label.to_string(),
            _ => self
                .dims
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_default(),
        }
    }
}

/// 把模板串里的 `{{token}}` 占位符替换为上下文值。
///
/// - `{{r}}` / `{{total}}` / `{{parent}}` → 语义锚点行号（模板禁写死坐标，展开时重定位，坐标漂移免疫）。
/// - `{{label}}` → 记录展示名。
/// - 其它 `{{name}}` → 维度值（找不到则空串）。
/// - 未闭合的 `{{` 原样保留。UTF-8 安全（`{{`/`}}` 均 ASCII，`find` 返回字符边界偏移）。
pub fn substitute(tpl: &str, ctx: &SubstCtx) -> String {
    let mut out = String::with_capacity(tpl.len());
    let mut rest = tpl;
    while let Some(pos) = rest.find("{{") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 2..];
        if let Some(endp) = after.find("}}") {
            let key = after[..endp].trim();
            out.push_str(&ctx.resolve(key));
            rest = &after[endp + 2..];
        } else {
            out.push_str("{{");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// 维度键路径：`k=v;k=v`（有序，确定性）。派生稳定 id 与落库诊断均以此为键。
pub fn dim_key_path(dims: &[(String, String)]) -> String {
    dims.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(";")
}

/// 构造一行的各列内容：存储态显式 `cells`（用户 CRUD 值）优先覆盖模板公式，其余列走模板占位符替换。
/// 返回 `(列标, 值或公式)` 序列（列顺序以模板为准，模板未覆盖的存储列追加在后）。
fn resolve_row_cells(
    cell_tpls: &[(String, String)],
    stored: &[(String, String)],
    ctx: &SubstCtx,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = cell_tpls
        .iter()
        .map(|(col, tpl)| {
            // 存储态该列有显式值 → 用它（覆盖模板 QM 公式）；否则模板占位符替换。
            match stored.iter().find(|(c, _)| c == col) {
                Some((_, v)) if !v.is_empty() => (col.clone(), v.clone()),
                _ => (col.clone(), substitute(tpl, ctx)),
            }
        })
        .collect();
    // 存储态里有、但模板没有的列（用户额外录入的列）也带上。
    for (col, v) in stored {
        if !v.is_empty() && !out.iter().any(|(c, _)| c == col) {
            out.push((col.clone(), v.clone()));
        }
    }
    out
}

/// 单层展开：模板行 × 数据源记录 → 实例行序列。
///
/// - `region_start_row`：实例行在画布上的起始物理行（1-based）。
/// - `total_row`：合计行物理行号（`{{total}}` 用；无则占位空）。
///
/// 分级展开（多层 levels + subtotal/total 归集）在 P2 的 `expand_hierarchy` 里，本函数是其单层基元。
pub fn expand_template(
    tpl: &FloatTemplate,
    records: &[SourceRecord],
    region_start_row: i64,
    total_row: Option<i64>,
) -> Vec<InstanceRow> {
    records
        .iter()
        .enumerate()
        .map(|(idx, rec)| {
            let phys = region_start_row + idx as i64;
            let path = dim_key_path(&rec.dims);
            let ctx = SubstCtx {
                dims: &rec.dims,
                label: &rec.label,
                phys_row: Some(phys),
                total_row,
                parent_row: None,
                col_letter: None,
            };
            let name = substitute(&tpl.name_tpl, &ctx);
            let cells = resolve_row_cells(&tpl.cell_tpls, &rec.cells, &ctx);
            InstanceRow {
                row_id: stable_instance_row_id(tpl.template_row_id, &path),
                dim_key_path: path,
                name,
                cells,
                sort_no: (idx as i32) + 1,
                phys_row: phys,
                row_type: "float".to_string(),
                level_no: 1,
                parent_row: None,
            }
        })
        .collect()
}

// ============================================================================
// 分级浮动（P2）：多层维度 + 小计/合计归集
// ============================================================================
//
// 数据源产出**带父子关系**的记录（每条 dims 含全部层级维度，如 region=华东;cust=C001）。
// 按 levels 顺序分组：外层维度分组出「小计行」，内层维度出「明细行」，顶部一条「总计行」。
// 归集用**语义锚点物理行区间**（小计 = SUM(其明细首行:末行)，总计 = SUM(全体明细) 或各小计之和），
// 模板禁写死坐标 → 展开引擎按实际物理行重定位，坐标漂移免疫。

/// 分级浮动的一层定义。
#[derive(Debug, Clone)]
pub struct HierLevel {
    /// 本层分组维度键（如 `region` / `cust_code`）。
    pub dim: String,
    /// 归集方式：`subtotal` 本层每组出一条小计行；`none` 不出（末级明细层用）。
    pub rollup: String,
    /// 小计行标题模板（`{{label}}` = 本组维度值），如 `"{{label}} 小计"`。仅 rollup=subtotal 用。
    pub subtotal_name_tpl: String,
}

/// 分级模板：明细层用 [`FloatTemplate`]（列公式模板），外加层级定义与总计配置。
#[derive(Debug, Clone)]
pub struct HierTemplate {
    /// 明细行模板（叶子层的列公式，占位符含各层维度键）。
    pub leaf: FloatTemplate,
    /// 层级（从外到内，最后一层通常 rollup=none 的明细层）。
    pub levels: Vec<HierLevel>,
    /// 总计：`total` 出一条顶部总计行；`none` 不出。
    pub grand_total: String,
    /// 总计行标题（如 `"合计"`）。
    pub grand_total_name: String,
    /// 小计/合计对哪些列做 SUM 归集（列标，如 `["B","C"]`）。占比等比率列不归集。
    pub rollup_cols: Vec<String>,
}

/// 分级展开：带父子的数据源记录 → 总计行 + 各组小计行 + 明细实例行（物理行连续）。
///
/// `region_start_row`：首行（总计行）的画布物理行（1-based）。返回全部行，物理行连续递增。
/// P2 支持两层（外层分组 + 叶子明细）为主；实现按 levels 递归，理论支持更多层。
pub fn expand_hierarchy(
    tpl: &HierTemplate,
    records: &[SourceRecord],
    region_start_row: i64,
) -> Vec<InstanceRow> {
    let mut out: Vec<InstanceRow> = Vec::new();
    let mut next_row = region_start_row;

    // 总计行占位（先占物理行，明细展开后回填 SUM 区间）。
    let total_row_phys = if tpl.grand_total == "total" {
        let r = next_row;
        next_row += 1;
        Some(r)
    } else {
        None
    };

    // 取外层分组维度（levels[0]）。无 levels 或单层 → 退化为扁平明细。
    let group_dim = tpl.levels.first().map(|l| l.dim.clone());
    let group_level = tpl.levels.first().cloned();

    // 明细行的物理行区间（用于总计归集）。
    let mut all_detail_rows: Vec<i64> = Vec::new();

    if let (Some(gdim), Some(glevel)) = (group_dim, group_level) {
        // 按外层维度分组，保持首次出现顺序（确定性）。
        let mut groups: Vec<(String, Vec<&SourceRecord>)> = Vec::new();
        for rec in records {
            let gval = rec
                .dims
                .iter()
                .find(|(k, _)| *k == gdim)
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            if let Some(entry) = groups.iter_mut().find(|(k, _)| *k == gval) {
                entry.1.push(rec);
            } else {
                groups.push((gval, vec![rec]));
            }
        }

        for (gval, members) in &groups {
            // 小计行占位（先占物理行，明细展开后回填其组 SUM 区间）。
            let subtotal_phys = if glevel.rollup == "subtotal" {
                let r = next_row;
                next_row += 1;
                Some(r)
            } else {
                None
            };

            // 明细行：叶子模板逐条展开。
            let group_first_detail = next_row;
            for rec in members {
                let phys = next_row;
                next_row += 1;
                let path = dim_key_path(&rec.dims);
                let ctx = SubstCtx {
                    dims: &rec.dims,
                    label: &rec.label,
                    phys_row: Some(phys),
                    total_row: total_row_phys,
                    parent_row: subtotal_phys,
                    col_letter: None,
                };
                let name = substitute(&tpl.leaf.name_tpl, &ctx);
                let cells = resolve_row_cells(&tpl.leaf.cell_tpls, &rec.cells, &ctx);
                out.push(InstanceRow {
                    row_id: stable_instance_row_id(tpl.leaf.template_row_id, &path),
                    dim_key_path: path,
                    name,
                    cells,
                    sort_no: out.len() as i32 + 1,
                    phys_row: phys,
                    row_type: "float".to_string(),
                    level_no: 3,
                    parent_row: subtotal_phys,
                });
                all_detail_rows.push(phys);
            }
            let group_last_detail = next_row - 1;

            // 小计行：SUM(本组明细首行:末行)，插到占位处。
            if let Some(sub_phys) = subtotal_phys {
                let sctx = SubstCtx {
                    dims: &[(gdim.clone(), gval.clone())],
                    label: gval,
                    phys_row: Some(sub_phys),
                    total_row: total_row_phys,
                    parent_row: total_row_phys,
                    col_letter: None,
                };
                let sub_name = substitute(&glevel.subtotal_name_tpl, &sctx);
                let cells = rollup_cells(
                    &tpl.rollup_cols,
                    group_first_detail,
                    group_last_detail,
                );
                out.push(InstanceRow {
                    row_id: stable_instance_row_id(
                        tpl.leaf.template_row_id,
                        &format!("__subtotal__;{gdim}={gval}"),
                    ),
                    dim_key_path: format!("{gdim}={gval}"),
                    name: sub_name,
                    cells,
                    sort_no: out.len() as i32 + 1,
                    phys_row: sub_phys,
                    row_type: "subtotal".to_string(),
                    level_no: 2,
                    parent_row: total_row_phys,
                });
            }
        }
    } else {
        // 无层级：退化为扁平明细（复用 expand_template 语义）。
        for rec in records {
            let phys = next_row;
            next_row += 1;
            let path = dim_key_path(&rec.dims);
            let ctx = SubstCtx {
                dims: &rec.dims,
                label: &rec.label,
                phys_row: Some(phys),
                total_row: total_row_phys,
                parent_row: total_row_phys,
                col_letter: None,
            };
            out.push(InstanceRow {
                row_id: stable_instance_row_id(tpl.leaf.template_row_id, &path),
                dim_key_path: path.clone(),
                name: substitute(&tpl.leaf.name_tpl, &ctx),
                cells: resolve_row_cells(&tpl.leaf.cell_tpls, &rec.cells, &ctx),
                sort_no: out.len() as i32 + 1,
                phys_row: phys,
                row_type: "float".to_string(),
                level_no: 2,
                parent_row: total_row_phys,
            });
            all_detail_rows.push(phys);
        }
    }

    // 总计行：SUM(全体明细)。放到 out 头部（物理行在最上）。
    if let Some(tphys) = total_row_phys {
        let cells = if all_detail_rows.is_empty() {
            Vec::new()
        } else {
            let first = *all_detail_rows.iter().min().unwrap();
            let last = *all_detail_rows.iter().max().unwrap();
            rollup_cells(&tpl.rollup_cols, first, last)
        };
        let total = InstanceRow {
            row_id: stable_instance_row_id(tpl.leaf.template_row_id, "__grand_total__"),
            dim_key_path: "__grand_total__".to_string(),
            name: tpl.grand_total_name.clone(),
            cells,
            sort_no: 0,
            phys_row: tphys,
            row_type: "total".to_string(),
            level_no: 1,
            parent_row: None,
        };
        out.insert(0, total);
    }

    out
}

/// 为小计/合计行构造归集列公式：每列 = `=SUM(<列><first>:<列><last>)`。
/// 只对 rollup_cols 指定的数值列（比率列不归集，留空由明细/模板另算）。
fn rollup_cells(cols: &[String], first_row: i64, last_row: i64) -> Vec<(String, String)> {
    cols.iter()
        .map(|c| {
            (
                c.clone(),
                format!("=SUM({c}{first_row}:{c}{last_row})"),
            )
        })
        .collect()
}

// ============================================================================
// 列浮动（P3）：模板列 × 数据源 → N 实例列（行浮动的转置）
// ============================================================================
//
// 列浮动 = 行浮动转置：数据源产出「列集合」（如 2026-01..2026-12 每月一列），模板列的
// 每个单元格公式带 `{{period}}` 维度 + `{{c}}` 当前列标锚点。展开时横向复制列，每列一个
// 数据记录，逐（行×新列）落公式。列结构 cr_report_col 与行同构，故复用同一套占位符引擎。

/// 0-based 列序号 → 列标（0→A, 25→Z, 26→AA）。
pub fn col_index_to_letter(mut idx: i64) -> String {
    let mut s = String::new();
    idx += 1;
    while idx > 0 {
        let r = ((idx - 1) % 26) as u8;
        s.insert(0, (b'A' + r) as char);
        idx = (idx - 1) / 26;
    }
    if s.is_empty() {
        s.push('A');
    }
    s
}

/// 列浮动模板：模板列里「每个数据行 × 本列」的公式（占位符含维度 + `{{c}}`）。
#[derive(Debug, Clone)]
pub struct ColFloatTemplate {
    /// 模板列在 cr_report_col 的真实 id（派生实例列稳定 id 的种子）。
    pub template_col_id: i64,
    /// 列头标题模板（`{{label}}` = 记录展示名，如月份名）。
    pub header_tpl: String,
    /// 该列每个数据行的公式模板：`(行号 1-based, 公式模板)`。公式里 `{{c}}` = 本实例列列标。
    pub row_tpls: Vec<(i64, String)>,
}

/// 展开后的一条实例列。
#[derive(Debug, Clone, PartialEq)]
pub struct ColInstance {
    /// 稳定派生 col_id（复用行的号段策略：[2^52, 2^53-1]，与 pk52 真号不撞、JS 安全）。
    pub col_id: i64,
    /// 维度键路径。
    pub dim_key_path: String,
    /// 列头标题（解析后）。
    pub header: String,
    /// 实例列的物理列标（如 `C`）。
    pub col_letter: String,
    /// 实例列的物理列序（0-based）。
    pub col_index: i64,
    /// 该列各行公式：`(行号 1-based, 已替换占位符的公式)`。
    pub cells: Vec<(i64, String)>,
    /// 展开序（1-based）。
    pub sort_no: i32,
}

/// 列展开：模板列 × 数据源记录 → 实例列序列（横向）。
///
/// `region_start_col`：首个实例列的画布物理列序（0-based）。返回 N 列，列序连续递增。
pub fn expand_columns(
    tpl: &ColFloatTemplate,
    records: &[SourceRecord],
    region_start_col: i64,
) -> Vec<ColInstance> {
    records
        .iter()
        .enumerate()
        .map(|(idx, rec)| {
            let ci = region_start_col + idx as i64;
            let letter = col_index_to_letter(ci);
            let path = dim_key_path(&rec.dims);
            let ctx = SubstCtx {
                dims: &rec.dims,
                label: &rec.label,
                phys_row: None,
                total_row: None,
                parent_row: None,
                col_letter: Some(letter.clone()),
            };
            let header = substitute(&tpl.header_tpl, &ctx);
            let cells = tpl
                .row_tpls
                .iter()
                .map(|(row_no, f)| {
                    // 存储态该行有显式值（cells 键=行号字符串）→ 覆盖模板公式（用户 CRUD 值优先）。
                    if let Some((_, v)) = rec
                        .cells
                        .iter()
                        .find(|(k, _)| k == &row_no.to_string())
                    {
                        if !v.is_empty() {
                            return (*row_no, v.clone());
                        }
                    }
                    // 行锚点：单元格所在行 = row_no（{{r}} 在列浮动里指本单元格的行）。
                    let cell_ctx = SubstCtx {
                        dims: &rec.dims,
                        label: &rec.label,
                        phys_row: Some(*row_no),
                        total_row: None,
                        parent_row: None,
                        col_letter: Some(letter.clone()),
                    };
                    (*row_no, substitute(f, &cell_ctx))
                })
                .collect();
            ColInstance {
                col_id: stable_instance_row_id(tpl.template_col_id, &path),
                dim_key_path: path,
                header,
                col_letter: letter,
                col_index: ci,
                cells,
                sort_no: (idx as i32) + 1,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_SAFE: i64 = 9_007_199_254_740_991; // 2^53 - 1

    fn rec(label: &str, dims: &[(&str, &str)]) -> SourceRecord {
        SourceRecord {
            label: label.to_string(),
            dims: dims
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            cells: Vec::new(),
        }
    }

    #[test]
    fn stable_id_is_js_safe_and_in_reserved_band() {
        for tid in [1i64, 9001, 4_500_000_000_000_000] {
            for path in ["region=华东;cust=C001", "cust=C999", ""] {
                let id = stable_instance_row_id(tid, path);
                assert!(id >= INSTANCE_ID_BASE, "落在保留号段内: {id}");
                assert!(id <= MAX_SAFE, "JS 安全: {id}");
            }
        }
    }

    #[test]
    fn stored_cells_override_template_formula() {
        // 存储态显式 B=2600 覆盖模板 QM 公式；C 无覆盖走模板；D(比率)走模板。
        let tpl = FloatTemplate {
            template_row_id: 9001,
            name_tpl: "{{label}}".into(),
            cell_tpls: vec![
                ("B".into(), "QM(0,@current,'{{cust_code}}')".into()),
                ("C".into(), "QC(0,@current,'{{cust_code}}')".into()),
                ("D".into(), "=B{{r}}/B{{total}}".into()),
            ],
        };
        let mut r = rec("上海A", &[("cust_code", "C001")]);
        r.cells = vec![("B".to_string(), "2600".to_string())]; // 用户 CRUD 值
        let rows = expand_template(&tpl, &[r], 2, Some(1));
        assert_eq!(rows[0].cells[0], ("B".into(), "2600".into()), "B 用存储值");
        assert_eq!(
            rows[0].cells[1],
            ("C".into(), "QC(0,@current,'C001')".into()),
            "C 无覆盖走模板"
        );
        assert_eq!(rows[0].cells[2], ("D".into(), "=B2/B1".into()), "D 走模板");
    }

    #[test]
    fn stable_id_is_deterministic() {
        let a = stable_instance_row_id(9001, "region=华东;cust=C001");
        let b = stable_instance_row_id(9001, "region=华东;cust=C001");
        assert_eq!(a, b, "同输入必同输出（跨期对齐/幂等的根基）");
    }

    #[test]
    fn stable_id_differs_by_input() {
        let a = stable_instance_row_id(9001, "cust=C001");
        let b = stable_instance_row_id(9001, "cust=C002");
        let c = stable_instance_row_id(9002, "cust=C001");
        assert_ne!(a, b, "不同维度键 → 不同 id");
        assert_ne!(a, c, "不同模板 → 不同 id");
    }

    #[test]
    fn stable_id_disjoint_from_pk52_band() {
        // pk52 真号在 [0, 2^52)；派生实例号在 [2^52, 2^53-1]。取几个当下真号验证无交集。
        for _ in 0..200 {
            let real = cmx_utils::next_pk_id();
            assert!(real < INSTANCE_ID_BASE, "真号 {real} 应 < 2^52，不入保留段");
        }
        let inst = stable_instance_row_id(9001, "cust=C001");
        assert!(inst >= INSTANCE_ID_BASE);
        assert!(is_instance_row_id(inst));
    }

    #[test]
    fn substitute_dims_and_anchors() {
        let ctx = SubstCtx {
            dims: &[("cust_code".into(), "C001".into())],
            label: "上海A",
            phys_row: Some(3),
            total_row: Some(1),
            parent_row: Some(2),
            col_letter: None,
        };
        assert_eq!(
            substitute("QM(0,@current,'{{cust_code}}')", &ctx),
            "QM(0,@current,'C001')"
        );
        assert_eq!(substitute("=B{{r}}/B{{total}}", &ctx), "=B3/B1");
        assert_eq!(substitute("=SUM(C{{parent}}:C{{r}})", &ctx), "=SUM(C2:C3)");
        assert_eq!(substitute("{{label}}", &ctx), "上海A");
    }

    #[test]
    fn substitute_unknown_and_unclosed() {
        let ctx = SubstCtx {
            dims: &[],
            label: "",
            phys_row: None,
            total_row: None,
            parent_row: None,
            col_letter: None,
        };
        assert_eq!(substitute("x={{nope}}", &ctx), "x=", "未知 token → 空");
        assert_eq!(substitute("a{{b", &ctx), "a{{b", "未闭合原样保留");
        assert_eq!(substitute("纯中文无占位", &ctx), "纯中文无占位");
    }

    #[test]
    fn expand_produces_n_rows_with_resolved_formulas() {
        let tpl = FloatTemplate {
            template_row_id: 9001,
            name_tpl: "{{label}}".into(),
            cell_tpls: vec![
                ("B".into(), "QM(0,@current,'{{cust_code}}')".into()),
                ("D".into(), "=B{{r}}/B{{total}}".into()),
            ],
        };
        let records = vec![
            rec("上海A", &[("cust_code", "C001")]),
            rec("杭州B", &[("cust_code", "C002")]),
            rec("南京C", &[("cust_code", "C003")]),
        ];
        // 合计行在第 1 行，实例行从第 2 行起。
        let rows = expand_template(&tpl, &records, 2, Some(1));
        assert_eq!(rows.len(), 3);

        assert_eq!(rows[0].name, "上海A");
        assert_eq!(rows[0].phys_row, 2);
        assert_eq!(rows[0].cells[0], ("B".into(), "QM(0,@current,'C001')".into()));
        assert_eq!(rows[0].cells[1], ("D".into(), "=B2/B1".into()));

        assert_eq!(rows[2].name, "南京C");
        assert_eq!(rows[2].phys_row, 4);
        assert_eq!(rows[2].cells[1], ("D".into(), "=B4/B1".into()));

        // 稳定 id：与独立计算一致，且两两不同。
        assert_eq!(
            rows[0].row_id,
            stable_instance_row_id(9001, "cust_code=C001")
        );
        assert_ne!(rows[0].row_id, rows[1].row_id);
    }

    fn hier_tpl() -> HierTemplate {
        HierTemplate {
            leaf: FloatTemplate {
                template_row_id: 9001,
                name_tpl: "{{label}}".into(),
                cell_tpls: vec![
                    ("B".into(), "QM(0,@current,'{{cust_code}}')".into()),
                    ("D".into(), "=B{{r}}/B{{total}}".into()),
                ],
            },
            levels: vec![
                HierLevel {
                    dim: "region".into(),
                    rollup: "subtotal".into(),
                    subtotal_name_tpl: "{{label}} 小计".into(),
                },
                HierLevel {
                    dim: "cust_code".into(),
                    rollup: "none".into(),
                    subtotal_name_tpl: String::new(),
                },
            ],
            grand_total: "total".into(),
            grand_total_name: "应收账款合计".into(),
            rollup_cols: vec!["B".into(), "C".into()],
        }
    }

    #[test]
    fn hierarchy_layout_total_subtotal_detail() {
        // 华东(上海A/杭州B) + 华北(北京D) —— 总计 + 2 小计 + 3 明细 = 6 行。
        let records = vec![
            rec("上海A", &[("region", "华东"), ("cust_code", "C001")]),
            rec("杭州B", &[("region", "华东"), ("cust_code", "C002")]),
            rec("北京D", &[("region", "华北"), ("cust_code", "C004")]),
        ];
        let rows = expand_hierarchy(&hier_tpl(), &records, 1);
        assert_eq!(rows.len(), 6, "1总计+2小计+3明细");

        // 物理行布局：1 总计 / 2 华东小计 / 3 上海A / 4 杭州B / 5 华北小计 / 6 北京D
        let by_phys: std::collections::HashMap<i64, &InstanceRow> =
            rows.iter().map(|r| (r.phys_row, r)).collect();
        assert_eq!(by_phys[&1].row_type, "total");
        assert_eq!(by_phys[&1].name, "应收账款合计");
        assert_eq!(by_phys[&2].row_type, "subtotal");
        assert_eq!(by_phys[&2].name, "华东 小计");
        assert_eq!(by_phys[&3].row_type, "float");
        assert_eq!(by_phys[&3].name, "上海A");
        assert_eq!(by_phys[&5].row_type, "subtotal");
        assert_eq!(by_phys[&5].name, "华北 小计");
        assert_eq!(by_phys[&6].name, "北京D");
    }

    #[test]
    fn hierarchy_rollup_sum_ranges_are_relocated() {
        let records = vec![
            rec("上海A", &[("region", "华东"), ("cust_code", "C001")]),
            rec("杭州B", &[("region", "华东"), ("cust_code", "C002")]),
            rec("北京D", &[("region", "华北"), ("cust_code", "C004")]),
        ];
        let rows = expand_hierarchy(&hier_tpl(), &records, 1);
        let by_phys: std::collections::HashMap<i64, &InstanceRow> =
            rows.iter().map(|r| (r.phys_row, r)).collect();

        // 华东小计(行2) = SUM(明细 行3:行4)
        let sub_hd = by_phys[&2];
        assert_eq!(sub_hd.cells[0], ("B".into(), "=SUM(B3:B4)".into()));
        assert_eq!(sub_hd.cells[1], ("C".into(), "=SUM(C3:C4)".into()));
        // 华北小计(行5) = SUM(行6:行6)
        let sub_hb = by_phys[&5];
        assert_eq!(sub_hb.cells[0], ("B".into(), "=SUM(B6:B6)".into()));
        // 总计(行1) = SUM(全体明细 行3:行6)
        let total = by_phys[&1];
        assert_eq!(total.cells[0], ("B".into(), "=SUM(B3:B6)".into()));
        assert_eq!(total.cells[1], ("C".into(), "=SUM(C3:C6)".into()));

        // 明细 D 列占比锚点重定位：上海A(行3) = B3/B1(总计行)
        assert_eq!(by_phys[&3].cells[1], ("D".into(), "=B3/B1".into()));
    }

    #[test]
    fn hierarchy_parent_and_level_wiring() {
        let records = vec![
            rec("上海A", &[("region", "华东"), ("cust_code", "C001")]),
            rec("北京D", &[("region", "华北"), ("cust_code", "C004")]),
        ];
        let rows = expand_hierarchy(&hier_tpl(), &records, 1);
        let by_phys: std::collections::HashMap<i64, &InstanceRow> =
            rows.iter().map(|r| (r.phys_row, r)).collect();
        // 层级：总计 level1、小计 level2、明细 level3
        assert_eq!(by_phys[&1].level_no, 1);
        assert_eq!(by_phys[&2].level_no, 2);
        assert_eq!(by_phys[&3].level_no, 3);
        // 明细父 = 其小计行；小计父 = 总计行
        assert_eq!(by_phys[&3].parent_row, Some(2));
        assert_eq!(by_phys[&2].parent_row, Some(1));
        assert_eq!(by_phys[&1].parent_row, None);
        // 稳定 id 全不同（明细/小计/总计各异）
        let ids: std::collections::HashSet<i64> = rows.iter().map(|r| r.row_id).collect();
        assert_eq!(ids.len(), rows.len(), "所有行 row_id 唯一");
    }

    #[test]
    fn hierarchy_ids_stable_across_calls() {
        let records = vec![
            rec("上海A", &[("region", "华东"), ("cust_code", "C001")]),
            rec("北京D", &[("region", "华北"), ("cust_code", "C004")]),
        ];
        let a = expand_hierarchy(&hier_tpl(), &records, 1);
        let b = expand_hierarchy(&hier_tpl(), &records, 1);
        let ida: Vec<i64> = a.iter().map(|r| r.row_id).collect();
        let idb: Vec<i64> = b.iter().map(|r| r.row_id).collect();
        assert_eq!(ida, idb, "分级展开 row_id 跨调用稳定（幂等）");
    }

    // ── P3 列浮动 ──

    #[test]
    fn col_index_to_letter_maps() {
        assert_eq!(col_index_to_letter(0), "A");
        assert_eq!(col_index_to_letter(2), "C");
        assert_eq!(col_index_to_letter(25), "Z");
        assert_eq!(col_index_to_letter(26), "AA");
        assert_eq!(col_index_to_letter(27), "AB");
    }

    fn col_tpl() -> ColFloatTemplate {
        ColFloatTemplate {
            template_col_id: 7001,
            header_tpl: "{{label}}".into(),
            // 两个数据行：行2=收入(QM按月)、行3=占比(本列/固定汇总列B)
            row_tpls: vec![
                (2, "QM('{{period_code}}',@current,'6001')".into()),
                (3, "={{c}}2/B2".into()),
            ],
        }
    }

    #[test]
    fn columns_expand_transposed_with_c_anchor() {
        // 三个月 → 三实例列，从物理列 C(idx2) 起。
        let records = vec![
            rec("2026-01", &[("period_code", "2026-01")]),
            rec("2026-02", &[("period_code", "2026-02")]),
            rec("2026-03", &[("period_code", "2026-03")]),
        ];
        let cols = expand_columns(&col_tpl(), &records, 2);
        assert_eq!(cols.len(), 3);

        // 列标 C/D/E
        assert_eq!(cols[0].col_letter, "C");
        assert_eq!(cols[1].col_letter, "D");
        assert_eq!(cols[2].col_letter, "E");
        assert_eq!(cols[0].header, "2026-01");

        // 行2 公式：{{period_code}} 替换
        assert_eq!(
            cols[0].cells[0],
            (2, "QM('2026-01',@current,'6001')".into())
        );
        assert_eq!(
            cols[2].cells[0],
            (2, "QM('2026-03',@current,'6001')".into())
        );
        // 行3 占比：{{c}} = 本实例列列标
        assert_eq!(cols[0].cells[1], (3, "=C2/B2".into()));
        assert_eq!(cols[1].cells[1], (3, "=D2/B2".into()));
        assert_eq!(cols[2].cells[1], (3, "=E2/B2".into()));
    }

    #[test]
    fn columns_stable_and_js_safe_ids() {
        let records = vec![
            rec("2026-01", &[("period_code", "2026-01")]),
            rec("2026-02", &[("period_code", "2026-02")]),
        ];
        let a = expand_columns(&col_tpl(), &records, 2);
        let b = expand_columns(&col_tpl(), &records, 2);
        assert_eq!(
            a.iter().map(|c| c.col_id).collect::<Vec<_>>(),
            b.iter().map(|c| c.col_id).collect::<Vec<_>>(),
            "列 id 跨调用稳定（幂等）"
        );
        for c in &a {
            assert!(c.col_id >= INSTANCE_ID_BASE, "在保留号段");
            assert!(c.col_id <= MAX_SAFE, "JS 安全");
        }
        assert_ne!(a[0].col_id, a[1].col_id, "不同月列 id 不同");
    }
}
