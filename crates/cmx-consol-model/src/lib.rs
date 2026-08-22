//! cmx-consol-model —— 合并报表领域中立层(纯逻辑,零 DB/HTTP)。
//!
//! 合并会计的核心类型与**纯引擎算法**:科目聚合、少数股东权益、抵销分录生成、逐级汇总。
//! 全部可被单测直接覆盖。DB 装载/落库在 `cmx-consol-store-pg`,本 crate 不碰。
//!
//! ## ★ 借方正 signed 约定(consolidation 引擎的根基)
//! 所有金额用**借方为正**的净额表示(debit +, credit −):
//!   资产/费用 正常为正(借方余额);负债/权益/收入 正常为负(贷方余额)。
//! 这样:
//!   - 多主体个别数聚合 = **纯加法**(sum)。
//!   - 抵销分录 = 每行 (借 dr, 贷 cr) → net = dr − cr,直接 `+=` 到该科目。
//!   - 平衡凭证 = 各行 net 之和 = 0。
//!   - 合并数 = 个别 + 调整 + 抵销(逐科目相加)。
//! 展示时按科目性质翻正负号。整套合并算法塌缩成加减 → 易测、易审。

pub mod engine;
pub mod types;

pub use engine::*;
pub use types::*;

/// 合并数据源 id(固定 fico-db,与报表同库)。
pub const CONSOL_DB_ID: &str = "fico-db";

/// 合并分类账代号(抵销/调整凭证所属并行分类账)。
pub const CONSOL_LEDGER: &str = "CONSOL";
