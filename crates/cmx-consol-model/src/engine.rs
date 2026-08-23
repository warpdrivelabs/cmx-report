//! engine —— 合并纯算法(借方正 signed 约定;全部可单测)。

use std::collections::BTreeMap;

use rust_decimal::Decimal;

use crate::types::*;

/// 一个下级(叶子主体或已合并子集团)对上级合并的贡献。
#[derive(Debug, Clone)]
pub struct Contribution {
    pub entity: String,
    pub method: ConsolMethod,
    pub ownership: Decimal,
    /// (集团科目, 借方正金额)。
    pub balances: Vec<(String, Decimal)>,
}

/// 资本抵销的科目配置(来自合并方案)。
#[derive(Debug, Clone)]
pub struct CapitalCfg {
    /// 长期股权投资科目(母公司资产,被抵销)。
    pub investment_account: String,
    /// 商誉/合并价差科目(差额)。
    pub goodwill_account: String,
    /// 少数股东权益科目(权益,单列)。
    pub nci_account: String,
    /// 少数股东损益科目(P&L 分摊)。
    pub minority_pl_account: String,
    /// 资本公积科目(同一控制下企业合并:差额入此,不确认商誉;缺省 4002)。
    pub capital_reserve_account: String,
}

/// ① 个别数聚合:各下级 balances × 并入比例(Full=1/Proportional=持股/Equity·Cost=0),逐科目求和。
/// 返回 (集团科目 → 借方正合计),按科目码有序(确定性)。
pub fn aggregate(children: &[Contribution]) -> BTreeMap<String, Decimal> {
    let mut out: BTreeMap<String, Decimal> = BTreeMap::new();
    for c in children {
        let ratio = c.method.include_ratio(c.ownership);
        if ratio == Decimal::ZERO {
            continue;
        }
        for (acc, amt) in &c.balances {
            *out.entry(acc.clone()).or_insert(Decimal::ZERO) += *amt * ratio;
        }
    }
    out
}

/// ② 资本抵销(长投 ↔ 子公司权益,差额→商誉,确认 NCI)。
///
/// 输入:
///   - `sub_equity`: 子公司**全部权益科目**的 (科目, 借方正金额)——权益为贷方,金额通常为负。
///   - `investment`: 母公司对该子的长期股权投资(借方正,资产,通常为正)。
///   - `ownership`: 持股比例 p(0~1)。
/// 生成一张平衡的资本抵销凭证:
///   Dr 各权益科目(全额消除) / Cr 长投 / Dr 商誉(差额>0) / Cr NCI(少数股东份额)。
pub fn capital_elimination(
    sub_equity: &[(String, Decimal)],
    investment: Decimal,
    ownership: Decimal,
    cfg: &CapitalCfg,
    rule_code: &str,
) -> ElimEntry {
    let mut lines: Vec<ElimLine> = Vec::new();

    // 子公司账面权益(自然口径 |E|)= 各权益科目借方正金额之和取负(贷方为正的自然权益)。
    // 借方正下权益科目通常为负;book_equity(自然,正=有净资产) = −Σ(sub_equity net)。
    let equity_dp_sum: Decimal = sub_equity.iter().map(|(_, v)| *v).sum();
    let book_equity = -equity_dp_sum; // 自然口径净资产

    // Dr 各权益科目全额(把子公司权益消除:权益为负,Dr 使其归零)。
    for (acc, amt) in sub_equity {
        // amt 为借方正(通常负);Dr = 使 net 增加 |amt| 抵消它 → dr = −amt(当 amt<0)。
        // 统一:要把该科目 net 归零,需追加 −amt 的 net → 若 −amt>0 记 Dr,否则记 Cr。
        let need = -*amt; // 目标追加 net
        if need >= Decimal::ZERO {
            lines.push(ElimLine::new(acc, need, Decimal::ZERO));
        } else {
            lines.push(ElimLine::new(acc, Decimal::ZERO, -need));
        }
    }
    // Cr 长期股权投资全额(消除母公司投资资产)。
    lines.push(ElimLine::new(&cfg.investment_account, Decimal::ZERO, investment));

    // 少数股东权益 = (1−p) × 账面净资产(自然,正)。以贷方计入 NCI(权益)。
    let nci = (Decimal::ONE - ownership) * book_equity;
    if nci != Decimal::ZERO {
        if nci >= Decimal::ZERO {
            lines.push(ElimLine::new(&cfg.nci_account, Decimal::ZERO, nci));
        } else {
            lines.push(ElimLine::new(&cfg.nci_account, -nci, Decimal::ZERO));
        }
    }

    // 商誉/合并价差 = 投资 − 母公司应享份额(p × 账面净资产);借差记商誉(资产,Dr),贷差记价差。
    let goodwill = investment - ownership * book_equity;
    if goodwill != Decimal::ZERO {
        if goodwill >= Decimal::ZERO {
            lines.push(ElimLine::new(&cfg.goodwill_account, goodwill, Decimal::ZERO));
        } else {
            lines.push(ElimLine::new(&cfg.goodwill_account, Decimal::ZERO, -goodwill));
        }
    }

    ElimEntry {
        elim_type: "capital".to_string(),
        source_rule: rule_code.to_string(),
        is_opening: false,
        lines,
    }
}

/// ②' 同一控制下企业合并的资本抵销(权益结合法 / 账面价值法,CAS 20)。
///
/// 与 [`capital_elimination`] 的唯一区别:**不确认商誉**——长投与享有净资产份额的差额
/// 计入**资本公积**(不足冲减,借方正下作为权益的调整),而非作为资产商誉。
/// 其余(消除子公司权益、消除长投、确认 NCI)完全一致。借方正下凭证自平衡。
///
/// 差额 = investment − p × book_equity:
///   - 差额>0(付出对价 > 应享份额):Dr 资本公积(减少集团资本公积)。
///   - 差额<0(付出对价 < 应享份额):Cr 资本公积(增加)。
pub fn common_control_elimination(
    sub_equity: &[(String, Decimal)],
    investment: Decimal,
    ownership: Decimal,
    cfg: &CapitalCfg,
    rule_code: &str,
) -> ElimEntry {
    let mut lines: Vec<ElimLine> = Vec::new();

    let equity_dp_sum: Decimal = sub_equity.iter().map(|(_, v)| *v).sum();
    let book_equity = -equity_dp_sum; // 自然口径净资产

    // Dr 各权益科目全额(消除子公司权益)。
    for (acc, amt) in sub_equity {
        let need = -*amt;
        if need >= Decimal::ZERO {
            lines.push(ElimLine::new(acc, need, Decimal::ZERO));
        } else {
            lines.push(ElimLine::new(acc, Decimal::ZERO, -need));
        }
    }
    // Cr 长期股权投资全额。
    lines.push(ElimLine::new(&cfg.investment_account, Decimal::ZERO, investment));

    // 少数股东权益 = (1−p) × 账面净资产。
    let nci = (Decimal::ONE - ownership) * book_equity;
    if nci != Decimal::ZERO {
        if nci >= Decimal::ZERO {
            lines.push(ElimLine::new(&cfg.nci_account, Decimal::ZERO, nci));
        } else {
            lines.push(ElimLine::new(&cfg.nci_account, -nci, Decimal::ZERO));
        }
    }

    // 差额 → 资本公积(不确认商誉)。差额>0 → Dr 资本公积(权益减少);差额<0 → Cr 资本公积。
    let diff = investment - ownership * book_equity;
    if diff != Decimal::ZERO {
        if diff >= Decimal::ZERO {
            lines.push(ElimLine::new(&cfg.capital_reserve_account, diff, Decimal::ZERO));
        } else {
            lines.push(ElimLine::new(&cfg.capital_reserve_account, Decimal::ZERO, -diff));
        }
    }

    ElimEntry {
        elim_type: "capital_common_control".to_string(),
        source_rule: rule_code.to_string(),
        is_opening: false,
        lines,
    }
}
///
/// `sub_net_profit`:子公司净利润(自然口径,正=盈利)。
/// 生成:Dr 少数股东损益(P&L 分摊) / Cr 少数股东权益。金额 = (1−p) × 净利润。
pub fn minority_pl(
    sub_net_profit: Decimal,
    ownership: Decimal,
    cfg: &CapitalCfg,
    rule_code: &str,
) -> Option<ElimEntry> {
    let amt = (Decimal::ONE - ownership) * sub_net_profit;
    if amt == Decimal::ZERO {
        return None;
    }
    // Dr 少数股东损益(借方正 += amt);Cr NCI(net −= amt)。amt 可正可负。
    let (dr_pl, cr_pl) = if amt >= Decimal::ZERO { (amt, Decimal::ZERO) } else { (Decimal::ZERO, -amt) };
    let (dr_nci, cr_nci) = if amt >= Decimal::ZERO { (Decimal::ZERO, amt) } else { (-amt, Decimal::ZERO) };
    Some(ElimEntry {
        elim_type: "nci".to_string(),
        source_rule: rule_code.to_string(),
        is_opening: false,
        lines: vec![
            ElimLine::new(&cfg.minority_pl_account, dr_pl, cr_pl),
            ElimLine::new(&cfg.nci_account, dr_nci, cr_nci),
        ],
    })
}

/// 借方正下按净额生成一行(net>0 记 Dr,否则记 Cr)。
fn signed_line(account: &str, net: Decimal) -> ElimLine {
    if net >= Decimal::ZERO {
        ElimLine::new(account, net, Decimal::ZERO)
    } else {
        ElimLine::new(account, Decimal::ZERO, -net)
    }
}

/// L2 分步取得交易的会计口径(达到控制:原持有股权按公允价值重估)。
///
/// 分步取得达到控制时,原已持有的股权(权益法/成本法核算)在购买日按**公允价值重估**,
/// 原账面与公允的差额确认为**投资收益**(非同一控制下)或**资本公积**(同一控制下)。
/// - `prev_carrying`:原持股账面价值(自然口径,正)。
/// - `prev_fair_value`:原持股在购买日的公允价值(自然口径,正)。
/// 生成一张平衡的调整凭证:Dr 长期股权投资(增值 = 公允 − 账面)/ Cr 投资收益(或资本公积)。
/// 差额为负(减值)则反向。差额为 0 → None。
pub fn step_acquisition(
    prev_carrying: Decimal,
    prev_fair_value: Decimal,
    investment_account: &str,
    gain_account: &str,
    rule_code: &str,
) -> Option<ElimEntry> {
    let remeasure = prev_fair_value - prev_carrying;
    if remeasure == Decimal::ZERO {
        return None;
    }
    // Dr 长投 +remeasure(资产增);Cr 投资收益 −remeasure(收入为贷方,net −remeasure)。
    Some(ElimEntry {
        elim_type: "step_acquisition".to_string(),
        source_rule: rule_code.to_string(),
        is_opening: false,
        lines: vec![
            signed_line(investment_account, remeasure),
            signed_line(gain_account, -remeasure),
        ],
    })
}

/// L2 处置股权的会计口径(两种情形)。
///
/// - **不丧失控制**(`loses_control=false`):部分处置视为**权益交易**,处置价与所处置净资产
///   份额的差额调**资本公积**(不确认损益)。
///   `proceeds`=处置对价,`disposed_share`=所处置的净资产份额账面(自然,正)。
///   差额 = proceeds − disposed_share → Cr 资本公积(处置价高)/ Dr 资本公积(处置价低);
///   对手方为 NCI 变动(处置给少数股东)→ Dr/Cr NCI。
/// - **丧失控制**(`loses_control=true`):确认**处置损益**(处置价 + 剩余股权公允 − 原享有净资产),
///   计入投资收益。`retained_fair_value`=剩余股权公允。
///   损益 = proceeds + retained_fair_value − net_assets_share → Cr 投资收益(收益)。
///
/// 均生成一张平衡凭证(另一腿计 NCI 或货币资金对冲,这里以 NCI/资本公积 为对手保持自平衡)。
#[allow(clippy::too_many_arguments)]
pub fn disposal(
    loses_control: bool,
    proceeds: Decimal,
    disposed_share: Decimal,
    retained_fair_value: Decimal,
    net_assets_share: Decimal,
    nci_account: &str,
    capital_reserve_account: &str,
    gain_account: &str,
    rule_code: &str,
) -> Option<ElimEntry> {
    if loses_control {
        // 处置损益 = 处置价 + 剩余股权公允 − 原享有净资产份额。
        let pl = proceeds + retained_fair_value - net_assets_share;
        if pl == Decimal::ZERO {
            return None;
        }
        // Cr 投资收益(收益,net −pl);对冲腿计 NCI(丧失控制,原 NCI 转出)以自平衡。
        Some(ElimEntry {
            elim_type: "disposal_loss_control".to_string(),
            source_rule: rule_code.to_string(),
            is_opening: false,
            lines: vec![
                signed_line(gain_account, -pl),
                signed_line(nci_account, pl),
            ],
        })
    } else {
        // 权益交易:差额调资本公积,不确认损益。
        let diff = proceeds - disposed_share;
        if diff == Decimal::ZERO {
            return None;
        }
        // 处置价 > 份额 → 资本公积增加(Cr);对手 NCI 增加(处置给少数股东)。
        Some(ElimEntry {
            elim_type: "disposal_equity_txn".to_string(),
            source_rule: rule_code.to_string(),
            is_opening: false,
            lines: vec![
                signed_line(capital_reserve_account, -diff),
                signed_line(nci_account, diff),
            ],
        })
    }
}

/// ⑨ 权益法权益确认(联营/合营:不逐行并入,按份额确认投资收益 + 调增长投)。
///
/// `associate_net_profit`:被投资单位净利润(自然口径,正=盈利)。
/// 生成一张平衡的调整凭证:Dr 长期股权投资(增值)/ Cr 投资收益 = 持股比例 × 净利润。
/// (亏损时反向:Cr 长投 / Dr 投资收益。)
pub fn equity_pickup(
    associate_net_profit: Decimal,
    ownership: Decimal,
    investment_account: &str,
    income_account: &str,
    rule_code: &str,
) -> Option<ElimEntry> {
    let amt = ownership * associate_net_profit;
    if amt == Decimal::ZERO {
        return None;
    }
    // Dr 长投 +amt(资产增);Cr 投资收益 −amt(收入为贷方正,net −amt)。
    Some(ElimEntry {
        elim_type: "equity_method".to_string(),
        source_rule: rule_code.to_string(),
        is_opening: false,
        lines: vec![
            signed_line(investment_account, amt),
            signed_line(income_account, -amt),
        ],
    })
}

/// ⑩ 商誉减值(合并商誉计提减值:冲减商誉资产,确认资产减值损失)。
///
/// `amount`:本期减值额(自然口径,正)。生成:Dr 资产减值损失 / Cr 商誉。
pub fn goodwill_impairment(
    amount: Decimal,
    goodwill_account: &str,
    impairment_account: &str,
    rule_code: &str,
) -> Option<ElimEntry> {
    if amount == Decimal::ZERO {
        return None;
    }
    // Dr 资产减值损失(费用,借方正 +amount);Cr 商誉(资产,net −amount)。
    Some(ElimEntry {
        elim_type: "goodwill_impair".to_string(),
        source_rule: rule_code.to_string(),
        is_opening: false,
        lines: vec![
            signed_line(impairment_account, amount),
            signed_line(goodwill_account, -amount),
        ],
    })
}

/// 一对已对账的内部往来/交易匹配(供债务/购销抵销)。
#[derive(Debug, Clone)]
pub struct IcMatch {
    pub entity_a: String,
    pub entity_b: String,
    /// 匹配金额(自然口径,正)。
    pub amount: Decimal,
}

/// ④ 债务抵销:对每对已对账内部往来,借"应付"、贷"应收"。
/// `payable_account`/`receivable_account` 来自规则。金额为匹配额(自然,正)。
pub fn debt_elimination(
    matches: &[IcMatch],
    payable_account: &str,
    receivable_account: &str,
    rule_code: &str,
) -> Vec<ElimEntry> {
    matches
        .iter()
        .filter(|m| m.amount != Decimal::ZERO)
        .map(|m| ElimEntry {
            elim_type: "debt".to_string(),
            source_rule: rule_code.to_string(),
            is_opening: false,
            lines: vec![
                // 应付(负债,贷方正):Dr 消除 → net += amount
                ElimLine {
                    account: payable_account.to_string(),
                    dr: m.amount,
                    cr: Decimal::ZERO,
                    partner: Some(m.entity_b.clone()),
                },
                // 应收(资产,借方正):Cr 消除 → net −= amount
                ElimLine {
                    account: receivable_account.to_string(),
                    dr: Decimal::ZERO,
                    cr: m.amount,
                    partner: Some(m.entity_a.clone()),
                },
            ],
        })
        .collect()
}

/// ⑤ 内部购销抵销:借"营业收入"、贷"营业成本"(匹配额)。
pub fn sales_elimination(
    matches: &[IcMatch],
    revenue_account: &str,
    cost_account: &str,
    rule_code: &str,
) -> Vec<ElimEntry> {
    matches
        .iter()
        .filter(|m| m.amount != Decimal::ZERO)
        .map(|m| ElimEntry {
            elim_type: "sales".to_string(),
            source_rule: rule_code.to_string(),
            is_opening: false,
            lines: vec![
                // 营业收入(收入,贷方正):Dr 消除 → net += amount
                ElimLine {
                    account: revenue_account.to_string(),
                    dr: m.amount,
                    cr: Decimal::ZERO,
                    partner: Some(m.entity_a.clone()),
                },
                // 营业成本(费用,借方正):Cr 消除 → net −= amount
                ElimLine {
                    account: cost_account.to_string(),
                    dr: Decimal::ZERO,
                    cr: m.amount,
                    partner: Some(m.entity_b.clone()),
                },
            ],
        })
        .collect()
}

/// L4 内部股利抵销:子公司向母公司分配股利,母确认投资收益 vs 子分配利润的重复计量。
///
/// 母公司收到子公司分红,在个别报表确认**投资收益**;子公司分配利润减少**未分配利润**。
/// 合并层看,分红是集团内部转移,不产生集团损益 → 抵销:
///   Dr 投资收益(母,冲回收入,收入借方正 net +)/ Cr 未分配利润(还原子公司被分掉的留存,net −)。
/// `IcMatch.amount` = 母公司自子公司确认的股利收益(自然口径,正);entity_a=母、entity_b=子。
pub fn dividend_elimination(
    matches: &[IcMatch],
    investment_income_account: &str,
    retained_earnings_account: &str,
    rule_code: &str,
) -> Vec<ElimEntry> {
    matches
        .iter()
        .filter(|m| m.amount != Decimal::ZERO)
        .map(|m| ElimEntry {
            elim_type: "dividend".to_string(),
            source_rule: rule_code.to_string(),
            is_opening: false,
            lines: vec![
                // 投资收益(收入,贷方正):Dr 消除母确认的收益 → net += amount
                ElimLine {
                    account: investment_income_account.to_string(),
                    dr: m.amount,
                    cr: Decimal::ZERO,
                    partner: Some(m.entity_a.clone()),
                },
                // 未分配利润(权益,贷方正):Cr 还原子被分配的留存 → net −= amount
                ElimLine {
                    account: retained_earnings_account.to_string(),
                    dr: Decimal::ZERO,
                    cr: m.amount,
                    partner: Some(m.entity_b.clone()),
                },
            ],
        })
        .collect()
}

/// 一笔存货未实现内部利润(期初/期末),供 C6 抵销与期初结转。
#[derive(Debug, Clone)]
pub struct InventoryProfit {
    /// 卖方(利润产生方)。
    pub seller: String,
    /// 买方(期末仍持有存货方)。
    pub buyer: String,
    /// 期初存货中的未实现利润(自然口径,正)——来自上期结转。
    pub opening: Decimal,
    /// 期末存货中的未实现利润(自然口径,正)——本期新发生。
    pub ending: Decimal,
}

/// ⑥ 存货未实现内部利润抵销 + 期初结转(C6;两期完整口径,借方正)。
///
/// 生成至多两张凭证:
///   1. **期初结转**(is_opening=true,仅 opening≠0):上期抵销的利润本期已随存货流转实现,
///      从期初未分配利润转回本期成本 —— Dr 期初未分配利润(opening) / Cr 营业成本(opening)。
///   2. **期末抵销**(ending≠0):期末存货仍含未实现利润 —— Dr 营业成本(ending) / Cr 存货(ending)。
///
/// 借方正下两张均自平衡(net 和=0),故合并试算表仍恒等 0。
/// - `cost_account` 营业成本(费用),`inventory_account` 存货(资产),`opening_re_account` 期初未分配利润(权益)。
pub fn inventory_profit_elimination(
    p: &InventoryProfit,
    cost_account: &str,
    inventory_account: &str,
    opening_re_account: &str,
    rule_code: &str,
    opening_rule_code: &str,
) -> Vec<ElimEntry> {
    let mut out = Vec::new();
    // ① 期初结转:Dr 期初未分配利润 / Cr 营业成本。
    if p.opening != Decimal::ZERO {
        out.push(ElimEntry {
            elim_type: "inventory_opening".to_string(),
            source_rule: opening_rule_code.to_string(),
            is_opening: true,
            lines: vec![
                // 期初未分配利润(权益,贷方正):Dr → net += opening(减少期初权益)
                ElimLine::new(opening_re_account, p.opening, Decimal::ZERO),
                // 营业成本(费用,借方正):Cr → net −= opening(本期成本减少=利润实现)
                ElimLine {
                    account: cost_account.to_string(),
                    dr: Decimal::ZERO,
                    cr: p.opening,
                    partner: Some(p.seller.clone()),
                },
            ],
        });
    }
    // ② 期末抵销:Dr 营业成本 / Cr 存货。
    if p.ending != Decimal::ZERO {
        out.push(ElimEntry {
            elim_type: "inventory".to_string(),
            source_rule: rule_code.to_string(),
            is_opening: false,
            lines: vec![
                // 营业成本(费用,借方正):Dr → net += ending(增加成本=消除未实现利润)
                ElimLine {
                    account: cost_account.to_string(),
                    dr: p.ending,
                    cr: Decimal::ZERO,
                    partner: Some(p.seller.clone()),
                },
                // 存货(资产,借方正):Cr → net −= ending(消除存货中含的内部利润)
                ElimLine {
                    account: inventory_account.to_string(),
                    dr: Decimal::ZERO,
                    cr: p.ending,
                    partner: Some(p.buyer.clone()),
                },
            ],
        });
    }
    out
}

/// 一笔固定资产内部交易未实现利润(L3;含逐期折旧转回)。
#[derive(Debug, Clone)]
pub struct FixedAssetProfit {
    /// 卖方(利润产生方)。
    pub seller: String,
    /// 买方(持有并计提折旧方)。
    pub buyer: String,
    /// 交易时固定资产原值中的未实现利润(自然口径,正)。
    pub unrealized: Decimal,
    /// 买方对该资产的折旧年限(总年限,>0)。
    pub dep_years: Decimal,
    /// 已使用年限(本期末累计已折旧年数;含本期)。
    pub elapsed_years: Decimal,
}

/// L3 固定资产内部交易未实现利润抵销 + 逐期折旧转回(借方正,两张凭证)。
///
/// 内部销售固定资产,买方按含内部利润的价格入账并计提折旧 → 合并需:
///   1. **抵原值利润**(当期发生,若 elapsed=0..1 视为交易期):Dr 营业外收入/资产处置损益(卖方利润)
///      / Cr 固定资产(消除原值中的内部利润)。这里统一:Dr 处置损益科目 / Cr 固定资产。
///   2. **折旧转回**:买方多提的折旧(按未实现利润 × 已用年限/总年限)在合并层转回:
///      Dr 累计折旧(冲多提)/ Cr 管理费用等(减少费用)。借方正下均自平衡。
///
/// 简化口径(与存货 LCA 抵销一致,单期可重算):
///   - 固定资产净额影响 = −unrealized(消除原值利润) + 累计多提折旧转回(+unrealized×elapsed/years)。
///   - 本函数产出两张平衡凭证,合并试算表恒等 0 保持。
pub fn fixed_asset_profit_elimination(
    p: &FixedAssetProfit,
    gain_account: &str,       // 卖方处置损益/营业外收入(费用类借方正,消除时 Cr)
    asset_account: &str,      // 固定资产(资产)
    accum_dep_account: &str,  // 累计折旧(资产备抵,借方正下为负)
    expense_account: &str,    // 折旧费用(费用,借方正)
    rule_code: &str,
    dep_rule_code: &str,
) -> Vec<ElimEntry> {
    let mut out = Vec::new();
    // ① 抵原值利润:Dr 处置损益(消除卖方确认的利润,费用类 net +)/ Cr 固定资产。
    if p.unrealized != Decimal::ZERO {
        out.push(ElimEntry {
            elim_type: "fixed_asset_profit".to_string(),
            source_rule: rule_code.to_string(),
            is_opening: false,
            lines: vec![
                ElimLine { account: gain_account.to_string(), dr: p.unrealized, cr: Decimal::ZERO, partner: Some(p.seller.clone()) },
                ElimLine { account: asset_account.to_string(), dr: Decimal::ZERO, cr: p.unrealized, partner: Some(p.buyer.clone()) },
            ],
        });
    }
    // ② 折旧转回:多提折旧 = unrealized × elapsed/years(买方按含利润价多提)。
    //    Dr 累计折旧(冲减多提,资产备抵 net +)/ Cr 折旧费用(减少费用,net −)。
    if p.dep_years > Decimal::ZERO && p.elapsed_years > Decimal::ZERO && p.unrealized != Decimal::ZERO {
        let excess_dep = p.unrealized * p.elapsed_years / p.dep_years;
        if excess_dep != Decimal::ZERO {
            out.push(ElimEntry {
                elim_type: "fixed_asset_depreciation".to_string(),
                source_rule: dep_rule_code.to_string(),
                is_opening: false,
                lines: vec![
                    signed_line(accum_dep_account, excess_dep),
                    signed_line(expense_account, -excess_dep),
                ],
            });
        }
    }
    out
}

/// ⑥ 工作底稿组装:个别数 + 调整 + 抵销 → 合并数(逐科目)。
/// `individual`: 聚合后的个别合计;`adjust`/`elim`: 调整/抵销凭证列表。
/// 返回全科目(个别与凭证涉及的并集)的四栏,按科目码有序。
pub fn worksheet(
    node: &str,
    individual: &BTreeMap<String, Decimal>,
    adjust_entries: &[ElimEntry],
    elim_entries: &[ElimEntry],
) -> Vec<WorksheetCell> {
    // 收集所有涉及科目。
    let mut accounts: BTreeMap<String, ()> = BTreeMap::new();
    for a in individual.keys() {
        accounts.insert(a.clone(), ());
    }
    for e in adjust_entries.iter().chain(elim_entries.iter()) {
        for l in &e.lines {
            accounts.insert(l.account.clone(), ());
        }
    }
    accounts
        .keys()
        .map(|acc| {
            let ind = individual.get(acc).copied().unwrap_or(Decimal::ZERO);
            let adj: Decimal = adjust_entries.iter().map(|e| e.net_for(acc)).sum();
            let elm: Decimal = elim_entries.iter().map(|e| e.net_for(acc)).sum();
            WorksheetCell {
                node: node.to_string(),
                account: acc.clone(),
                individual: ind,
                adjust: adj,
                elim: elm,
                consolidated: ind + adj + elm,
            }
        })
        .collect()
}

/// 校验:一组凭证是否全部借贷平衡。
pub fn all_balanced(entries: &[ElimEntry]) -> bool {
    entries.iter().all(|e| e.is_balanced())
}

/// 一方申报的内部往来头寸(C4 对账输入)。
#[derive(Debug, Clone)]
pub struct IcDeclaration {
    /// 申报主体。
    pub entity: String,
    /// 交易对手。
    pub partner: String,
    /// 往来类型(debt/sales)。
    pub ic_type: String,
    /// 方向:receivable/revenue(债权/收入方=A);payable/cost(债务/成本方=B)。
    pub direction: String,
    /// 申报金额(自然口径,正)。
    pub amount: Decimal,
}

impl IcDeclaration {
    /// 是否债权/收入方(A 侧)。
    fn is_claim(&self) -> bool {
        matches!(
            self.direction.to_ascii_lowercase().as_str(),
            "receivable" | "revenue" | "ar" | "债权" | "收入"
        )
    }
}

/// 双边对账结果(C4 输出;可查差异工作台 + 抵销输入)。
#[derive(Debug, Clone, PartialEq)]
pub struct IcReconResult {
    /// 债权/收入方。
    pub entity_a: String,
    /// 债务/成本方。
    pub entity_b: String,
    pub ic_type: String,
    /// A 侧(债权/收入)申报合计。
    pub a_amount: Decimal,
    /// B 侧(债务/成本)申报合计。
    pub b_amount: Decimal,
    /// 匹配额 = min(A,B)(双边都在时);用于抵销。
    pub matched: Decimal,
    /// 差异 = A − B(>0 债权大;<0 债务大)。
    pub diff: Decimal,
    /// 状态:matched(平) / diff(有差异) / one_sided(单边未达)。
    pub status: String,
}

/// ⑧ 内部往来双边对账(C4):把各主体申报的两侧头寸配对,算匹配额与差异。
///
/// 按 {无序对, 往来类型} 归组:A 侧(债权/收入)合计 vs B 侧(债务/成本)合计。
///   - matched = min(A,B)(双边都在)→ 抵销以此为准(差异保留至查明,不硬抵销)。
///   - diff = A − B;status ∈ matched(0差异)/diff/one_sided(缺一侧)。
/// 结果按 (A,B,类型) 有序(确定性)。
pub fn reconcile(decls: &[IcDeclaration]) -> Vec<IcReconResult> {
    use std::collections::BTreeMap;
    #[derive(Default)]
    struct Acc {
        a_amount: Decimal,
        b_amount: Decimal,
        a_entity: Option<String>,
        b_entity: Option<String>,
        a_partner: Option<String>,
        b_partner: Option<String>,
    }
    // key: (对_lo, 对_hi, 类型)
    let mut groups: BTreeMap<(String, String, String), Acc> = BTreeMap::new();
    for d in decls {
        if d.amount == Decimal::ZERO {
            continue;
        }
        let (lo, hi) = if d.entity <= d.partner {
            (d.entity.clone(), d.partner.clone())
        } else {
            (d.partner.clone(), d.entity.clone())
        };
        let acc = groups.entry((lo, hi, d.ic_type.clone())).or_default();
        if d.is_claim() {
            acc.a_amount += d.amount;
            acc.a_entity.get_or_insert(d.entity.clone());
            acc.a_partner.get_or_insert(d.partner.clone());
        } else {
            acc.b_amount += d.amount;
            acc.b_entity.get_or_insert(d.entity.clone());
            acc.b_partner.get_or_insert(d.partner.clone());
        }
    }
    let mut out: Vec<IcReconResult> = groups
        .into_iter()
        .map(|((_, _, ic_type), acc)| {
            // A/B 主体:优先各自申报者;缺失用对方的 partner 推断。
            let entity_a = acc.a_entity.or(acc.b_partner).unwrap_or_default();
            let entity_b = acc.b_entity.or(acc.a_partner).unwrap_or_default();
            let both = acc.a_amount != Decimal::ZERO && acc.b_amount != Decimal::ZERO;
            let matched = if both { acc.a_amount.min(acc.b_amount) } else { Decimal::ZERO };
            let diff = acc.a_amount - acc.b_amount;
            let status = if !both {
                "one_sided"
            } else if diff == Decimal::ZERO {
                "matched"
            } else {
                "diff"
            };
            IcReconResult {
                entity_a,
                entity_b,
                ic_type,
                a_amount: acc.a_amount,
                b_amount: acc.b_amount,
                matched,
                diff,
                status: status.to_string(),
            }
        })
        .collect();
    out.sort_by(|x, y| {
        (x.entity_a.as_str(), x.entity_b.as_str(), x.ic_type.as_str())
            .cmp(&(y.entity_a.as_str(), y.entity_b.as_str(), y.ic_type.as_str()))
    });
    out
}

/// ⑪ 合并范围变动(CAS 33/IFRS 10 附注:两期范围对比)。
///
/// 逐主体对比本期 vs 上期范围,归类为:
///   - `first_time`(新纳入/非同一控制下合并):本期在、上期不在。
///   - `disposal`(处置/不再合并):上期在、本期不在。
///   - `ownership_up`/`ownership_down`(持股比例变动):两期都在、比例变。
///   - `method_change`(合并方法变更):两期都在、方法变(全额↔权益等)。
///   - `unchanged`:两期都在且方法/比例不变(默认不产出到变动清单,除非 `include_unchanged`)。
/// 结果按 org_code 有序(确定性)。
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeChange {
    pub org_code: String,
    pub org_name: String,
    /// 变动类型:first_time/disposal/ownership_up/ownership_down/method_change/unchanged。
    pub change_type: String,
    /// 本期合并方法(处置时为空)。
    pub curr_method: Option<ConsolMethod>,
    /// 上期合并方法(新纳入时为空)。
    pub prev_method: Option<ConsolMethod>,
    /// 本期持股(处置时 0)。
    pub curr_ownership: Decimal,
    /// 上期持股(新纳入时 0)。
    pub prev_ownership: Decimal,
}

/// 对比两期合并范围,输出范围变动清单。`include_unchanged=false` 只出真正变动的主体。
pub fn diff_scope(prev: &[ScopeNode], curr: &[ScopeNode], include_unchanged: bool) -> Vec<ScopeChange> {
    use std::collections::BTreeMap;
    let prev_map: BTreeMap<&str, &ScopeNode> = prev.iter().map(|n| (n.code.as_str(), n)).collect();
    let curr_map: BTreeMap<&str, &ScopeNode> = curr.iter().map(|n| (n.code.as_str(), n)).collect();
    // 全部涉及的 org(并集,有序)。
    let mut codes: Vec<&str> = prev_map.keys().chain(curr_map.keys()).copied().collect();
    codes.sort_unstable();
    codes.dedup();

    let mut out = Vec::new();
    for code in codes {
        let p = prev_map.get(code).copied();
        let c = curr_map.get(code).copied();
        match (p, c) {
            (None, Some(cn)) => out.push(ScopeChange {
                org_code: code.to_string(),
                org_name: cn.name.clone(),
                change_type: "first_time".into(),
                curr_method: Some(cn.method),
                prev_method: None,
                curr_ownership: cn.ownership,
                prev_ownership: Decimal::ZERO,
            }),
            (Some(pn), None) => out.push(ScopeChange {
                org_code: code.to_string(),
                org_name: pn.name.clone(),
                change_type: "disposal".into(),
                curr_method: None,
                prev_method: Some(pn.method),
                curr_ownership: Decimal::ZERO,
                prev_ownership: pn.ownership,
            }),
            (Some(pn), Some(cn)) => {
                let change_type = if pn.method != cn.method {
                    "method_change"
                } else if cn.ownership > pn.ownership {
                    "ownership_up"
                } else if cn.ownership < pn.ownership {
                    "ownership_down"
                } else {
                    "unchanged"
                };
                if change_type == "unchanged" && !include_unchanged {
                    continue;
                }
                out.push(ScopeChange {
                    org_code: code.to_string(),
                    org_name: cn.name.clone(),
                    change_type: change_type.into(),
                    curr_method: Some(cn.method),
                    prev_method: Some(pn.method),
                    curr_ownership: cn.ownership,
                    prev_ownership: pn.ownership,
                });
            }
            (None, None) => {}
        }
    }
    out
}

/// 外币折算汇率(某主体功能币 → 集团报告币)。
#[derive(Debug, Clone, Copy)]
pub struct FxRates {
    /// 期末汇率(资产/负债)。
    pub closing: Decimal,
    /// 平均汇率(收入/费用)。
    pub average: Decimal,
    /// 历史汇率(权益/实收资本)。
    pub historical: Decimal,
}

/// ⑦ 外币报表折算(IAS 21 净投资法):按科目性质选汇率折算,轧差入 CTA(折算差额)。
///
/// - 资产/负债 × closing;收入/费用 × average;权益/NCI × historical。
/// - 折算后借贷不再为 0(各科目用不同汇率),差额 = CTA → 计入 `cta_account`(其他综合收益·外币折算差额)。
/// - 借方正约定:CTA = −Σ(折算后各科目),使折算后试算表仍平衡(和=0)。
/// 返回折算后的 (科目, 集团币金额) 列表(含 CTA 行)。
pub fn translate_entity<F>(
    balances: &[(String, Decimal)],
    account_type_of: F,
    rates: FxRates,
    cta_account: &str,
) -> Vec<(String, Decimal)>
where
    F: Fn(&str) -> AccountType,
{
    let mut out: Vec<(String, Decimal)> = Vec::with_capacity(balances.len() + 1);
    let mut sum = Decimal::ZERO;
    for (acc, amt_fc) in balances {
        let rate = match account_type_of(acc) {
            AccountType::Asset | AccountType::Liability => rates.closing,
            AccountType::Income | AccountType::Expense => rates.average,
            AccountType::Equity | AccountType::Nci => rates.historical,
        };
        let amt_gc = *amt_fc * rate;
        sum += amt_gc;
        out.push((acc.clone(), amt_gc));
    }
    // CTA 轧差:使折算后仍平衡。
    let cta = -sum;
    if cta != Decimal::ZERO {
        out.push((cta_account.to_string(), cta));
    }
    out
}

// ============================================================================
// 合并四表补充:现金流量项目流水 / 权益变动流水 的逐级聚合 + 内部抵销(借方正)
// ============================================================================

/// 一条现金流量项目流水(某主体某项目;借方正净额:流入+/流出−)。
#[derive(Debug, Clone, PartialEq)]
pub struct CashFlowRow {
    /// 申报主体。
    pub entity: String,
    /// 活动分类(operating/investing/financing)。
    pub activity: String,
    /// 现金流量项目码。
    pub item_code: String,
    /// 借方正净额(流入+/流出−)。
    pub amount: Decimal,
    /// 是否内部现金流(合并层需抵销)。
    pub is_intercompany: bool,
}

/// 一条权益变动流水(某主体某权益项目某变动类型;借方正净额)。
#[derive(Debug, Clone, PartialEq)]
pub struct EquityChangeRow {
    pub entity: String,
    /// 权益变动表列码(EC01…)。
    pub column_code: String,
    /// 权益项目(实收资本/未分配利润/…)。
    pub equity_item: String,
    /// 变动类型(opening/comprehensive_income/…)。
    pub change_type: String,
    /// 借方正净额。
    pub amount: Decimal,
}

/// ⑫ 现金流量逐级聚合 + 内部现金流抵销(借方正,某合并节点)。
///
/// - `members`:本合并节点子树内的主体集合(含各级)。
/// - 只并入子树内主体的流水;**内部现金流**(`is_intercompany` 且对手也在子树内)在本节点抵销
///   —— 借方正下集团内部一收一付净额相消,按 `item_code` 汇总后内部项自然轧平。
///   工程口径:内部现金流不计入合并现金流量表(集团视角非现金流入/流出),故聚合时**跳过**
///   `is_intercompany` 行(两端都在集团内 → 相互抵销 → 合并净额为 0)。
/// 返回 (item_code → 借方正合计),按项目码有序(确定性)。
pub fn aggregate_cash_flow(rows: &[CashFlowRow], members: &std::collections::HashSet<String>) -> BTreeMap<String, Decimal> {
    let mut out: BTreeMap<String, Decimal> = BTreeMap::new();
    for r in rows {
        if !members.contains(&r.entity) {
            continue;
        }
        // 内部现金流:集团视角非现金流入/流出,合并层抵销(不并入)。
        if r.is_intercompany {
            continue;
        }
        *out.entry(r.item_code.clone()).or_insert(Decimal::ZERO) += r.amount;
    }
    out
}

/// ⑬ 权益变动逐级聚合(借方正,某合并节点)。
///
/// - 只并入子树内主体的流水;按 `column_code` 汇总(权益变动表按列取数)。
/// - 权益变动不做内部抵销(资本抵销已在 run_consolidation 的资本抵销环节处理;这里是披露口径的
///   列聚合,母子权益的抵销体现在合并权益余额,不在变动流水层重复抵销)。
/// 返回 (column_code → 借方正合计),按列码有序。
pub fn aggregate_equity_change(rows: &[EquityChangeRow], members: &std::collections::HashSet<String>) -> BTreeMap<String, Decimal> {
    let mut out: BTreeMap<String, Decimal> = BTreeMap::new();
    for r in rows {
        if !members.contains(&r.entity) {
            continue;
        }
        if r.column_code.is_empty() {
            continue;
        }
        *out.entry(r.column_code.clone()).or_insert(Decimal::ZERO) += r.amount;
    }
    out
}

/// L5 现金流量表·工作底稿法(间接法):从两期合并科目余额差额推导现金流量净额(不吃录入流水)。
///
/// 借方正约定下,资产负债表恒等式 Σ(所有科目)=0 恒成立;现金也是资产。故:
///   现金变动 Δcash = −Σ(非现金科目 Δ)  (两期合并数之差)。
/// 把非现金科目的期间变动按性质归入三大活动,得到工作底稿法现金流量:
///   - 经营:损益类(income/expense)+ 经营性资产负债(应收/应付/存货等,非投资非筹资的资产负债)。
///   - 投资:长期资产(固定资产/无形/长投/商誉等)。
///   - 筹资:权益(实收资本/资本公积/NCI)+ 借款类负债。
/// 每项现金流量 = −Δ(该非现金科目)(借方正下:资产增加耗现金→负;负债/权益增加提供现金→正)。
///
/// 分类由调用方传入 `classify`(account_code → CfActivity),现金科目传入 `is_cash` 跳过。
/// 返回 (CfActivity, 借方正现金净额),三活动各一条 + 现金净增加校验条。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfActivity {
    Operating,
    Investing,
    Financing,
    /// 现金及现金等价物本身(不计入三活动,用于校验)。
    Cash,
}

/// 工作底稿法现金流量一行(借方正现金净额:流入+/流出−)。
#[derive(Debug, Clone, PartialEq)]
pub struct CashFlowLine {
    pub activity: CfActivity,
    /// 借方正现金净额(该活动对现金的净影响)。
    pub amount: Decimal,
}

/// 从两期合并科目余额(借方正)推导三大活动现金流量净额 + 现金实际变动校验。
/// `opening`/`closing`:期初/期末合并数(account → 借方正)。
/// `classify`:account_code → CfActivity(现金科目返回 Cash)。
/// 返回四条:Operating/Investing/Financing 的现金净额 + Cash(现金实际 Δ,应 = 三活动之和)。
pub fn derive_cash_flow_worksheet<F>(
    opening: &BTreeMap<String, Decimal>,
    closing: &BTreeMap<String, Decimal>,
    classify: F,
) -> Vec<CashFlowLine>
where
    F: Fn(&str) -> CfActivity,
{
    let mut all: BTreeMap<&str, ()> = BTreeMap::new();
    for k in opening.keys().chain(closing.keys()) {
        all.insert(k.as_str(), ());
    }
    let (mut op, mut inv, mut fin, mut cash_delta) =
        (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
    for acc in all.keys() {
        let o = opening.get(*acc).copied().unwrap_or(Decimal::ZERO);
        let c = closing.get(*acc).copied().unwrap_or(Decimal::ZERO);
        let delta = c - o; // 借方正下的期间变动
        match classify(acc) {
            CfActivity::Cash => cash_delta += delta,
            // 非现金科目对现金的贡献 = −Δ(借方正:资产↑耗现金、负债/权益/收入↑提供现金)。
            CfActivity::Operating => op += -delta,
            CfActivity::Investing => inv += -delta,
            CfActivity::Financing => fin += -delta,
        }
    }
    vec![
        CashFlowLine { activity: CfActivity::Operating, amount: op },
        CashFlowLine { activity: CfActivity::Investing, amount: inv },
        CashFlowLine { activity: CfActivity::Financing, amount: fin },
        CashFlowLine { activity: CfActivity::Cash, amount: cash_delta },
    ]
}

/// L6 交叉持股·有效持股比例(矩阵法 / 迭代收敛)。
///
/// 存在交叉持股(A 持 B、B 持 A)或环持时,母公司对各主体的**有效持股**需解联立:
///   eff(X) = 母对X直接持股 + Σ_Y 母对Y有效持股 × Y对X直接持股。
/// 即 `eff = d + eff · M`,其中 d=母对各主体直接持股行向量、M=主体间直接持股矩阵。
/// 解 `eff = d (I − M)^{-1}`。这里用**迭代法**(Jacobi):eff_0=d,eff_{k+1}=d+eff_k·M,
/// 收敛到不动点(持股比例<1 保证谱半径<1 → 收敛)。对小规模集团足够且无需求逆。
///
/// 输入:
///   - `entities`:主体代码列表(顺序即矩阵维度)。
///   - `parent_direct`:母公司对各主体的直接持股(code → 比例)。
///   - `cross`:主体间直接持股 (holder, held) → 比例。
/// 返回:每个主体的有效持股(code → 比例),按输入顺序。迭代上限 200 次或变动<1e-9 收敛。
pub fn effective_ownership(
    entities: &[String],
    parent_direct: &std::collections::HashMap<String, Decimal>,
    cross: &std::collections::HashMap<(String, String), Decimal>,
) -> BTreeMap<String, Decimal> {
    let n = entities.len();
    let idx: std::collections::HashMap<&str, usize> =
        entities.iter().enumerate().map(|(i, e)| (e.as_str(), i)).collect();
    // d 向量。
    let d: Vec<Decimal> = entities.iter().map(|e| parent_direct.get(e).copied().unwrap_or(Decimal::ZERO)).collect();
    // M 矩阵:m[holder][held]。
    let mut m = vec![vec![Decimal::ZERO; n]; n];
    for ((holder, held), pct) in cross {
        if let (Some(&h), Some(&k)) = (idx.get(holder.as_str()), idx.get(held.as_str())) {
            m[h][k] += *pct;
        }
    }
    // 迭代:eff_{k+1}[j] = d[j] + Σ_i eff_k[i] * m[i][j]。
    let mut eff = d.clone();
    let eps = Decimal::new(1, 9); // 1e-9
    for _ in 0..200 {
        let mut next = d.clone();
        for i in 0..n {
            if eff[i] == Decimal::ZERO { continue; }
            for j in 0..n {
                if m[i][j] != Decimal::ZERO {
                    next[j] += eff[i] * m[i][j];
                }
            }
        }
        let mut max_delta = Decimal::ZERO;
        for j in 0..n {
            let dlt = (next[j] - eff[j]).abs();
            if dlt > max_delta { max_delta = dlt; }
        }
        eff = next;
        if max_delta < eps { break; }
    }
    entities.iter().cloned().zip(eff).collect()
}

/// O1 净投资套期(境外经营净投资套期,CAS 24 / IAS 39·IFRS 9)。
///
/// 套期工具(如外币借款)对境外经营净投资的套期,**有效部分**的利得/损失计入其他综合收益
/// (与外币折算差额 CTA 同处,对冲净投资的折算风险),不进当期损益;无效部分进损益(此处不建模)。
/// 借方正下把有效部分从**套期工具科目**重分类至 **CTA(OCI)**:两腿自平衡。
/// `effective_amount`:有效部分借方正净额(+ 表示工具端借方增加、CTA 端贷方增加)。0 → None。
pub fn net_investment_hedge(
    effective_amount: Decimal,
    cta_account: &str,
    hedge_instrument_account: &str,
    rule_code: &str,
) -> Option<ElimEntry> {
    if effective_amount == Decimal::ZERO {
        return None;
    }
    // 有效部分:Dr 套期工具(将其 FX 损益移出)/ Cr CTA(计入 OCI);借方正下 net 相消自平衡。
    Some(ElimEntry {
        elim_type: "net_investment_hedge".to_string(),
        source_rule: rule_code.to_string(),
        is_opening: false,
        lines: vec![
            signed_line(hedge_instrument_account, effective_amount),
            signed_line(cta_account, -effective_amount),
        ],
    })
}

/// O2 商誉减值测试(CGU 可收回金额法,CAS 8 / IAS 36)。
///
/// 现金产出单元(CGU)账面价值(含分摊的商誉)与**可收回金额**(公允价值减处置费用、使用价值孰高)
/// 比较:减值损失 = max(0, 账面 − 可收回);减值**先冲减商誉**,故封顶在商誉账面。
/// 返回本期应计提的商誉减值额(自然口径,正;0 表示未减值)。纯函数,供 store 算出后走 C6 减值路径。
pub fn goodwill_impairment_test(
    carrying_amount: Decimal,
    recoverable_amount: Decimal,
    goodwill_carrying: Decimal,
) -> Decimal {
    let shortfall = carrying_amount - recoverable_amount;
    if shortfall <= Decimal::ZERO {
        return Decimal::ZERO;
    }
    // 减值先冲商誉,封顶在商誉账面(简化:不再向下分摊到其他长期资产)。
    shortfall.min(goodwill_carrying.max(Decimal::ZERO))
}


#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros_lite::dec;

    // 轻量 dec! 宏(避免额外依赖):从整数/字符串构造 Decimal。
    mod rust_decimal_macros_lite {
        #[macro_export]
        macro_rules! dec {
            ($v:literal) => {{
                let s = stringify!($v);
                s.parse::<rust_decimal::Decimal>().unwrap()
            }};
        }
        pub(crate) use dec;
    }

    fn cfg() -> CapitalCfg {
        CapitalCfg {
            investment_account: "1511".into(), // 长期股权投资
            goodwill_account: "1801".into(),   // 商誉
            nci_account: "4400".into(),        // 少数股东权益
            minority_pl_account: "4900".into(),// 少数股东损益
            capital_reserve_account: "4002".into(), // 资本公积(同一控制下差额)
        }
    }

    #[test]
    fn aggregate_full_and_proportional() {
        let children = vec![
            Contribution {
                entity: "P".into(),
                method: ConsolMethod::Full,
                ownership: dec!(1),
                balances: vec![("1001".into(), dec!(100)), ("2001".into(), dec!(-40))],
            },
            Contribution {
                entity: "S".into(),
                method: ConsolMethod::Full,
                ownership: dec!(0.8),
                balances: vec![("1001".into(), dec!(50)), ("2001".into(), dec!(-30))],
            },
            Contribution {
                entity: "JV".into(),
                method: ConsolMethod::Equity, // 不逐行并入
                ownership: dec!(0.5),
                balances: vec![("1001".into(), dec!(999))],
            },
        ];
        let agg = aggregate(&children);
        assert_eq!(agg["1001"], dec!(150)); // 100+50,JV 不并
        assert_eq!(agg["2001"], dec!(-70));
    }

    #[test]
    fn capital_elimination_balanced_with_goodwill_and_nci() {
        // 子公司权益:实收资本 -100、盈余公积 -20、未分配 -30(借方正,贷方为负)→ 账面净资产 150。
        let sub_equity = vec![
            ("4001".into(), dec!(-100)),
            ("4101".into(), dec!(-20)),
            ("4104".into(), dec!(-30)),
        ];
        let investment = dec!(140); // 母投资成本
        let ownership = dec!(0.8);
        let e = capital_elimination(&sub_equity, investment, ownership, &cfg(), "R_CAP");
        assert!(e.is_balanced(), "资本抵销必须借贷平衡");
        // 商誉 = 140 − 0.8×150 = 140 − 120 = 20
        assert_eq!(e.net_for("1801"), dec!(20));
        // NCI = 0.2×150 = 30(贷方,借方正为 −30)
        assert_eq!(e.net_for("4400"), dec!(-30));
        // 长投消除:−140
        assert_eq!(e.net_for("1511"), dec!(-140));
        // 各权益科目归零(net = 原值的相反数)
        assert_eq!(e.net_for("4001"), dec!(100));
        assert_eq!(e.net_for("4104"), dec!(30));
    }

    #[test]
    fn common_control_uses_capital_reserve_no_goodwill() {
        // 同一控制下:子公司权益 150(实收100+盈余20+未分30),母 80% 出资 140。
        // 差额 = 140 − 0.8×150 = 20 → 计入资本公积(非商誉);NCI = 0.2×150 = 30。
        let sub_equity = vec![
            ("4001".into(), dec!(-100)),
            ("4101".into(), dec!(-20)),
            ("4104".into(), dec!(-30)),
        ];
        let e = common_control_elimination(&sub_equity, dec!(140), dec!(0.8), &cfg(), "R_CC");
        assert!(e.is_balanced(), "同一控制下资本抵销必须借贷平衡");
        assert_eq!(e.elim_type, "capital_common_control");
        // 差额 20 入资本公积(Dr,减少集团资本公积)。
        assert_eq!(e.net_for("4002"), dec!(20));
        // ★不确认商誉。
        assert_eq!(e.net_for("1801"), dec!(0));
        // NCI = 0.2×150 = 30(贷方,借方正 −30)。
        assert_eq!(e.net_for("4400"), dec!(-30));
        // 长投消除 −140。
        assert_eq!(e.net_for("1511"), dec!(-140));
        // 子公司权益归零。
        assert_eq!(e.net_for("4001"), dec!(100));
        assert_eq!(e.net_for("4104"), dec!(30));
    }

    #[test]
    fn step_acquisition_remeasures_prev_holding() {
        // 原持股账面 100,购买日公允 130 → 重估增值 30 确认投资收益。
        let e = step_acquisition(dec!(100), dec!(130), "1511", "6111", "R_STEP").unwrap();
        assert!(e.is_balanced());
        assert_eq!(e.net_for("1511"), dec!(30)); // Dr 长投 +30
        assert_eq!(e.net_for("6111"), dec!(-30)); // Cr 投资收益 −30
        assert!(step_acquisition(dec!(100), dec!(100), "1511", "6111", "R_STEP").is_none());
    }

    #[test]
    fn disposal_equity_txn_vs_loss_of_control() {
        // 不丧失控制:处置价 80,所处置份额账面 60 → 差额 20 调资本公积(权益交易,不确认损益)。
        let e = disposal(false, dec!(80), dec!(60), dec!(0), dec!(0), "4400", "4002", "6111", "R_DISP").unwrap();
        assert!(e.is_balanced());
        assert_eq!(e.net_for("4002"), dec!(-20)); // Cr 资本公积 +20(处置价高)
        assert_eq!(e.net_for("4400"), dec!(20));  // Dr NCI(对手)
        assert_eq!(e.net_for("6111"), dec!(0));   // ★不确认损益
        // 丧失控制:处置价 100 + 剩余公允 50 − 原享有净资产 120 = 损益 30。
        let l = disposal(true, dec!(100), dec!(0), dec!(50), dec!(120), "4400", "4002", "6111", "R_DISP").unwrap();
        assert!(l.is_balanced());
        assert_eq!(l.net_for("6111"), dec!(-30)); // Cr 投资收益 +30(收益)
        assert_eq!(l.elim_type, "disposal_loss_control");
    }

    #[test]
    fn minority_pl_splits_profit() {
        // 子公司净利润 50,少数 20% → 少数股东损益 10
        let e = minority_pl(dec!(50), dec!(0.8), &cfg(), "R_NCI").unwrap();
        assert!(e.is_balanced());
        assert_eq!(e.net_for("4900"), dec!(10)); // Dr 少数股东损益 +10
        assert_eq!(e.net_for("4400"), dec!(-10)); // Cr NCI −10
    }

    #[test]
    fn debt_and_sales_elimination_balanced() {
        let matches = vec![IcMatch { entity_a: "P".into(), entity_b: "S".into(), amount: dec!(60) }];
        let debt = debt_elimination(&matches, "2202", "1122", "R_DEBT");
        assert_eq!(debt.len(), 1);
        assert!(debt[0].is_balanced());
        assert_eq!(debt[0].net_for("2202"), dec!(60)); // 应付 Dr +60
        assert_eq!(debt[0].net_for("1122"), dec!(-60)); // 应收 Cr −60
        assert_eq!(debt[0].lines[1].partner.as_deref(), Some("P"));

        let sales = sales_elimination(&matches, "6001", "6401", "R_SALES");
        assert!(sales[0].is_balanced());
        assert_eq!(sales[0].net_for("6001"), dec!(60)); // 收入 Dr +60
        assert_eq!(sales[0].net_for("6401"), dec!(-60)); // 成本 Cr −60
    }

    #[test]
    fn dividend_elimination_removes_intragroup_income() {
        // 母 P 自子 S 确认股利收益 40 → 抵销:Dr 投资收益 40 / Cr 未分配利润 40。
        let matches = vec![IcMatch { entity_a: "P".into(), entity_b: "S".into(), amount: dec!(40) }];
        let d = dividend_elimination(&matches, "6111", "4104", "R_DIV");
        assert_eq!(d.len(), 1);
        assert!(d[0].is_balanced());
        assert_eq!(d[0].net_for("6111"), dec!(40));  // 投资收益 Dr +40(冲回集团内部收益)
        assert_eq!(d[0].net_for("4104"), dec!(-40)); // 未分配利润 Cr −40(还原子被分配的留存)
        assert_eq!(d[0].lines[0].partner.as_deref(), Some("P"));
        assert_eq!(d[0].lines[1].partner.as_deref(), Some("S"));
    }

    #[test]
    fn worksheet_consolidated_equals_ind_plus_adjust_plus_elim() {
        let mut individual = BTreeMap::new();
        individual.insert("1122".to_string(), dec!(200)); // 应收合计
        individual.insert("2202".to_string(), dec!(-150)); // 应付合计
        let matches = vec![IcMatch { entity_a: "P".into(), entity_b: "S".into(), amount: dec!(60) }];
        let elim = debt_elimination(&matches, "2202", "1122", "R_DEBT");
        let cells = worksheet("GROUP", &individual, &[], &elim);
        let ar = cells.iter().find(|c| c.account == "1122").unwrap();
        assert_eq!(ar.individual, dec!(200));
        assert_eq!(ar.elim, dec!(-60));
        assert_eq!(ar.consolidated, dec!(140)); // 200 − 60
        let ap = cells.iter().find(|c| c.account == "2202").unwrap();
        assert_eq!(ap.consolidated, dec!(-90)); // −150 + 60
    }

    #[test]
    fn full_worked_example_balance_sheet_ties() {
        // 母 P 全资(100%)子 S。抵销后合并资产负债表应平衡(资产 = 负债 + 权益)。
        // 借方正:资产>0,负债/权益<0。个别数已含 P 对 S 的长投 120,S 权益 −120。
        // P: 现金80 长投120 | 实收资本 −150 未分配 −50
        // S: 现金140         | 实收资本 −100 未分配 −20  (净资产120)
        let p = Contribution {
            entity: "P".into(), method: ConsolMethod::Full, ownership: dec!(1),
            balances: vec![
                ("1001".into(), dec!(80)), ("1511".into(), dec!(120)),
                ("4001".into(), dec!(-150)), ("4104".into(), dec!(-50)),
            ],
        };
        let s = Contribution {
            entity: "S".into(), method: ConsolMethod::Full, ownership: dec!(1),
            balances: vec![
                ("1001".into(), dec!(120)),
                ("4001".into(), dec!(-100)), ("4104".into(), dec!(-20)),
            ],
        };
        let individual = aggregate(&[p, s]);
        // 资本抵销:长投120 vs S权益120,100% → 无商誉无 NCI。
        let cap = capital_elimination(
            &[("4001".into(), dec!(-100)), ("4104".into(), dec!(-20))],
            dec!(120), dec!(1), &cfg(), "R_CAP",
        );
        assert!(cap.is_balanced());
        let cells = worksheet("GROUP", &individual, &[], &[cap]);
        let get = |a: &str| cells.iter().find(|c| c.account == a).map(|c| c.consolidated).unwrap_or(Decimal::ZERO);
        // 合并现金 = 80+120 = 200
        assert_eq!(get("1001"), dec!(200));
        // 合并长投 = 120 − 120 = 0(消除)
        assert_eq!(get("1511"), dec!(0));
        // 合并实收资本 = 母 −150(S 的 −100 被抵销归零)
        assert_eq!(get("4001"), dec!(-150));
        assert_eq!(get("4104"), dec!(-50));
        // 商誉/NCI = 0
        assert_eq!(get("1801"), dec!(0));
        assert_eq!(get("4400"), dec!(0));
        // ★ 资产负债表恒等:所有科目合并数之和 = 0(借方正下总账平衡)
        let total: Decimal = cells.iter().map(|c| c.consolidated).sum();
        assert_eq!(total, Decimal::ZERO, "合并后借贷仍平衡");
    }

    #[test]
    fn fx_translate_balances_with_cta() {
        // 境外子:现金(asset)100、实收资本(equity)-100(FC 平衡)。
        // closing 7.0、historical 6.5 → 700 − 650 = 50 差额 → CTA −50。
        let bal = vec![("1001".to_string(), dec!(100)), ("4001".to_string(), dec!(-100))];
        let ty = |a: &str| if a == "1001" { AccountType::Asset } else { AccountType::Equity };
        let rates = FxRates { closing: dec!(7.0), average: dec!(6.8), historical: dec!(6.5) };
        let out = translate_entity(&bal, ty, rates, "4106");
        let get = |a: &str| out.iter().find(|(x, _)| x == a).map(|(_, v)| *v).unwrap_or(Decimal::ZERO);
        assert_eq!(get("1001"), dec!(700.0)); // 100×7.0
        assert_eq!(get("4001"), dec!(-650.0)); // -100×6.5
        assert_eq!(get("4106"), dec!(-50.0)); // CTA 轧差
        let total: Decimal = out.iter().map(|(_, v)| *v).sum();
        assert_eq!(total, Decimal::ZERO, "折算后借贷仍平衡");
    }

    #[test]
    fn inventory_unrealized_profit_two_periods() {
        // 期初存货含上期未实现利润 30、期末含本期未实现利润 50。
        let p = InventoryProfit { seller: "A".into(), buyer: "B".into(), opening: dec!(30), ending: dec!(50) };
        let es = inventory_profit_elimination(&p, "6401", "1401", "4104", "R_INV", "R_INV_OPEN");
        assert_eq!(es.len(), 2);
        assert!(all_balanced(&es), "两张凭证均借贷平衡");
        // 期初结转:Dr 期初未分配利润 30 / Cr 营业成本 30。
        let opening = es.iter().find(|e| e.is_opening).unwrap();
        assert_eq!(opening.net_for("4104"), dec!(30));
        assert_eq!(opening.net_for("6401"), dec!(-30));
        // 期末抵销:Dr 营业成本 50 / Cr 存货 50。
        let ending = es.iter().find(|e| !e.is_opening).unwrap();
        assert_eq!(ending.net_for("6401"), dec!(50));
        assert_eq!(ending.net_for("1401"), dec!(-50));
        // 合计:营业成本净 = 50 − 30 = 20(本期利润净减 20);存货 −50;期初权益 +30。
        let cost: Decimal = es.iter().map(|e| e.net_for("6401")).sum();
        assert_eq!(cost, dec!(20));
        // 仅期末(首期,opening=0)→ 单张凭证。
        let first = InventoryProfit { seller: "A".into(), buyer: "B".into(), opening: dec!(0), ending: dec!(50) };
        let es1 = inventory_profit_elimination(&first, "6401", "1401", "4104", "R_INV", "R_INV_OPEN");
        assert_eq!(es1.len(), 1);
        assert!(!es1[0].is_opening);
    }

    #[test]
    fn fixed_asset_profit_with_depreciation_reversal() {
        // 内部售固定资产,原值含未实现利润 100,买方 5 年折旧,已用 2 年。
        let p = FixedAssetProfit {
            seller: "A".into(), buyer: "B".into(),
            unrealized: dec!(100), dep_years: dec!(5), elapsed_years: dec!(2),
        };
        let es = fixed_asset_profit_elimination(&p, "6301", "1601", "1602", "6602", "R_FA", "R_FA_DEP");
        assert_eq!(es.len(), 2);
        assert!(all_balanced(&es), "两张凭证均借贷平衡");
        // ① 抵原值利润:Dr 处置损益 100 / Cr 固定资产 100。
        let orig = es.iter().find(|e| e.elim_type == "fixed_asset_profit").unwrap();
        assert_eq!(orig.net_for("6301"), dec!(100));
        assert_eq!(orig.net_for("1601"), dec!(-100));
        // ② 折旧转回:多提折旧 = 100×2/5 = 40 → Dr 累计折旧 40 / Cr 折旧费用 40。
        let dep = es.iter().find(|e| e.elim_type == "fixed_asset_depreciation").unwrap();
        assert_eq!(dep.net_for("1602"), dec!(40));
        assert_eq!(dep.net_for("6602"), dec!(-40));
        // 无折旧(已用0年)→ 仅一张。
        let p0 = FixedAssetProfit { seller: "A".into(), buyer: "B".into(), unrealized: dec!(100), dep_years: dec!(5), elapsed_years: dec!(0) };
        assert_eq!(fixed_asset_profit_elimination(&p0, "6301", "1601", "1602", "6602", "R_FA", "R_FA_DEP").len(), 1);
    }

    #[test]
    fn ic_reconcile_matched_diff_onesided() {
        let decls = vec![
            // 对 A↔B 债务:A 报应收 100,B 报应付 100 → 平。
            IcDeclaration { entity: "A".into(), partner: "B".into(), ic_type: "debt".into(), direction: "receivable".into(), amount: dec!(100) },
            IcDeclaration { entity: "B".into(), partner: "A".into(), ic_type: "debt".into(), direction: "payable".into(), amount: dec!(100) },
            // 对 A↔C 购销:A 报收入 200,C 报成本 180 → 差异 20,匹配 180。
            IcDeclaration { entity: "A".into(), partner: "C".into(), ic_type: "sales".into(), direction: "revenue".into(), amount: dec!(200) },
            IcDeclaration { entity: "C".into(), partner: "A".into(), ic_type: "sales".into(), direction: "cost".into(), amount: dec!(180) },
            // 对 B↔C 债务:仅 B 报应收 50 → 单边未达。
            IcDeclaration { entity: "B".into(), partner: "C".into(), ic_type: "debt".into(), direction: "receivable".into(), amount: dec!(50) },
        ];
        let r = reconcile(&decls);
        assert_eq!(r.len(), 3);
        // A↔B debt:平。
        let ab = r.iter().find(|x| x.entity_a == "A" && x.entity_b == "B" && x.ic_type == "debt").unwrap();
        assert_eq!(ab.matched, dec!(100));
        assert_eq!(ab.diff, dec!(0));
        assert_eq!(ab.status, "matched");
        // A↔C sales:差异 20,匹配 180。
        let ac = r.iter().find(|x| x.ic_type == "sales").unwrap();
        assert_eq!(ac.entity_a, "A");
        assert_eq!(ac.entity_b, "C");
        assert_eq!(ac.matched, dec!(180));
        assert_eq!(ac.diff, dec!(20));
        assert_eq!(ac.status, "diff");
        // B↔C debt:单边(B 报债权,C 未报)→ entity_a=B,entity_b=C(由 partner 推断),matched 0。
        let bc = r.iter().find(|x| x.entity_a == "B" && x.ic_type == "debt").unwrap();
        assert_eq!(bc.entity_b, "C");
        assert_eq!(bc.matched, dec!(0));
        assert_eq!(bc.status, "one_sided");
    }

    #[test]
    fn equity_pickup_and_loss() {
        // 联营 30%,被投资净利润 200 → 投资收益 60,长投 +60。
        let e = equity_pickup(dec!(200), dec!(0.3), "1511", "6111", "R_EM").unwrap();
        assert!(e.is_balanced());
        assert_eq!(e.net_for("1511"), dec!(60)); // Dr 长投 +60
        assert_eq!(e.net_for("6111"), dec!(-60)); // Cr 投资收益 −60(贷方正)
        // 亏损 −100 → 反向:长投 −30 / 投资收益 +30。
        let l = equity_pickup(dec!(-100), dec!(0.3), "1511", "6111", "R_EM").unwrap();
        assert!(l.is_balanced());
        assert_eq!(l.net_for("1511"), dec!(-30));
        assert_eq!(l.net_for("6111"), dec!(30));
        // 零利润 → 无凭证。
        assert!(equity_pickup(dec!(0), dec!(0.3), "1511", "6111", "R_EM").is_none());
    }

    #[test]
    fn goodwill_impairment_entry() {
        // 商誉减值 15:Dr 资产减值损失 15 / Cr 商誉 15。
        let e = goodwill_impairment(dec!(15), "1801", "6701", "R_GW").unwrap();
        assert!(e.is_balanced());
        assert_eq!(e.net_for("6701"), dec!(15)); // 减值损失(费用,借方正)+15
        assert_eq!(e.net_for("1801"), dec!(-15)); // 商誉(资产)−15
        assert!(goodwill_impairment(dec!(0), "1801", "6701", "R_GW").is_none());
    }

    #[test]
    fn diff_scope_classifies_changes() {
        let node = |code: &str, method: ConsolMethod, own: &str| ScopeNode {
            code: code.into(), name: format!("{code}名"), parent: None,
            method, ownership: own.parse().unwrap(), is_leaf: true, level: 2,
        };
        // 上期:A(全额100%)、B(全额80%)、C(全额100%)。
        let prev = vec![node("A", ConsolMethod::Full, "1"), node("B", ConsolMethod::Full, "0.8"), node("C", ConsolMethod::Full, "1")];
        // 本期:A 不变、B 增持到 0.9、C 处置、D 新纳入、E 由权益法转全额(方法变)。
        let curr = vec![
            node("A", ConsolMethod::Full, "1"),
            node("B", ConsolMethod::Full, "0.9"),
            node("D", ConsolMethod::Full, "1"),
            node("E", ConsolMethod::Full, "0.6"),
        ];
        // 上期 E 是权益法。
        let mut prev2 = prev.clone();
        prev2.push(node("E", ConsolMethod::Equity, "0.6"));
        let changes = diff_scope(&prev2, &curr, false);
        let by = |c: &str| changes.iter().find(|x| x.org_code == c).cloned();
        // A 不变 → 不出(include_unchanged=false)。
        assert!(by("A").is_none());
        assert_eq!(by("B").unwrap().change_type, "ownership_up");
        assert_eq!(by("C").unwrap().change_type, "disposal");
        assert_eq!(by("D").unwrap().change_type, "first_time");
        assert_eq!(by("E").unwrap().change_type, "method_change");
        // include_unchanged=true 时 A 也出。
        let all = diff_scope(&prev2, &curr, true);
        assert_eq!(all.iter().find(|x| x.org_code == "A").unwrap().change_type, "unchanged");
        // 处置项:本期方法/持股空、上期在。
        let c = by("C").unwrap();
        assert_eq!(c.curr_ownership, dec!(0));
        assert_eq!(c.prev_ownership, dec!(1));
        assert!(c.curr_method.is_none());
    }

    #[test]
    fn aggregate_cash_flow_skips_intercompany_and_nonmembers() {
        use std::collections::HashSet;
        let members: HashSet<String> = ["P", "S"].iter().map(|s| s.to_string()).collect();
        let rows = vec![
            // P 对外销售收现 +200(经营)。
            CashFlowRow { entity: "P".into(), activity: "operating".into(), item_code: "CF01".into(), amount: dec!(200), is_intercompany: false },
            // S 对外销售收现 +150。
            CashFlowRow { entity: "S".into(), activity: "operating".into(), item_code: "CF01".into(), amount: dec!(150), is_intercompany: false },
            // P↔S 内部往来收付(内部现金流)→ 合并层不计入。
            CashFlowRow { entity: "P".into(), activity: "operating".into(), item_code: "CF01".into(), amount: dec!(80), is_intercompany: true },
            CashFlowRow { entity: "S".into(), activity: "operating".into(), item_code: "CF01".into(), amount: dec!(-80), is_intercompany: true },
            // 非子树成员 X 的流水不并入。
            CashFlowRow { entity: "X".into(), activity: "operating".into(), item_code: "CF01".into(), amount: dec!(999), is_intercompany: false },
            // P 购建固定资产付现 −60(投资)。
            CashFlowRow { entity: "P".into(), activity: "investing".into(), item_code: "CF20".into(), amount: dec!(-60), is_intercompany: false },
        ];
        let agg = aggregate_cash_flow(&rows, &members);
        assert_eq!(agg["CF01"], dec!(350)); // 200+150,内部±80 跳过,X 不并入
        assert_eq!(agg["CF20"], dec!(-60));
        assert_eq!(agg.len(), 2);
    }

    #[test]
    fn aggregate_equity_change_by_column() {
        use std::collections::HashSet;
        let members: HashSet<String> = ["P", "S"].iter().map(|s| s.to_string()).collect();
        let rows = vec![
            EquityChangeRow { entity: "P".into(), column_code: "EC01".into(), equity_item: "paid_in".into(), change_type: "opening".into(), amount: dec!(-150) },
            EquityChangeRow { entity: "S".into(), column_code: "EC01".into(), equity_item: "paid_in".into(), change_type: "opening".into(), amount: dec!(-100) },
            // 无列码 → 跳过(不进披露列)。
            EquityChangeRow { entity: "P".into(), column_code: "".into(), equity_item: "x".into(), change_type: "other".into(), amount: dec!(-9) },
            // 非成员 → 跳过。
            EquityChangeRow { entity: "X".into(), column_code: "EC01".into(), equity_item: "paid_in".into(), change_type: "opening".into(), amount: dec!(-999) },
            EquityChangeRow { entity: "P".into(), column_code: "EC05".into(), equity_item: "retained".into(), change_type: "comprehensive_income".into(), amount: dec!(-40) },
        ];
        let agg = aggregate_equity_change(&rows, &members);
        assert_eq!(agg["EC01"], dec!(-250)); // −150 + −100
        assert_eq!(agg["EC05"], dec!(-40));
        assert_eq!(agg.len(), 2);
    }

    #[test]
    fn cash_flow_worksheet_ties_to_cash_delta() {
        // 期初 → 期末合并数(借方正)。现金1001、应收1122(经营)、固定资产1601(投资)、实收资本4001(筹资)。
        let mut opening = BTreeMap::new();
        opening.insert("1001".to_string(), dec!(100)); // 现金
        opening.insert("1122".to_string(), dec!(50));  // 应收
        opening.insert("1601".to_string(), dec!(200)); // 固定资产
        opening.insert("4001".to_string(), dec!(-350)); // 实收资本(权益,负)
        let mut closing = BTreeMap::new();
        closing.insert("1001".to_string(), dec!(180)); // 现金 +80
        closing.insert("1122".to_string(), dec!(30));  // 应收 −20(收现→经营+20)
        closing.insert("1601".to_string(), dec!(260)); // 固定资产 +60(购建→投资−60)
        closing.insert("4001".to_string(), dec!(-470)); // 实收资本 −120(增资→筹资+120)
        let classify = |a: &str| match a {
            "1001" => CfActivity::Cash,
            "1122" => CfActivity::Operating,
            "1601" => CfActivity::Investing,
            "4001" => CfActivity::Financing,
            _ => CfActivity::Operating,
        };
        let lines = derive_cash_flow_worksheet(&opening, &closing, classify);
        let get = |act: CfActivity| lines.iter().find(|l| l.activity == act).unwrap().amount;
        assert_eq!(get(CfActivity::Operating), dec!(20));   // −Δ应收 = −(−20)=+20
        assert_eq!(get(CfActivity::Investing), dec!(-60));  // −Δ固资 = −(+60)=−60
        assert_eq!(get(CfActivity::Financing), dec!(120));  // −Δ实收 = −(−120)=+120
        assert_eq!(get(CfActivity::Cash), dec!(80));        // 现金实际 Δ
        // ★ 校验:三活动之和 = 现金实际变动。
        assert_eq!(get(CfActivity::Operating) + get(CfActivity::Investing) + get(CfActivity::Financing), get(CfActivity::Cash));
    }

    #[test]
    fn effective_ownership_cross_holding_converges() {
        use std::collections::HashMap;
        // 母对 A 直接 80%,A 持 B 60%,B 持 A 10%(交叉持股)。
        let entities = vec!["A".to_string(), "B".to_string()];
        let mut parent = HashMap::new();
        parent.insert("A".to_string(), dec!(0.8));
        let mut cross = HashMap::new();
        cross.insert(("A".to_string(), "B".to_string()), dec!(0.6)); // A→B 60%
        cross.insert(("B".to_string(), "A".to_string()), dec!(0.1)); // B→A 10%
        let eff = effective_ownership(&entities, &parent, &cross);
        // 联立:effA = 0.8 + effB×0.1 ; effB = effA×0.6。
        //  → effA = 0.8 + 0.6·effA·0.1 = 0.8 + 0.06 effA → effA(0.94)=0.8 → effA=0.851063...
        //  → effB = 0.6·effA = 0.510638...
        let a = eff["A"]; let b = eff["B"];
        assert!((a - dec!(0.851063)).abs() < dec!(0.0001), "effA={a}");
        assert!((b - dec!(0.510638)).abs() < dec!(0.0001), "effB={b}");
    }

    #[test]
    fn effective_ownership_no_cross_equals_direct() {
        use std::collections::HashMap;
        let entities = vec!["A".to_string(), "B".to_string()];
        let mut parent = HashMap::new();
        parent.insert("A".to_string(), dec!(1));
        parent.insert("B".to_string(), dec!(0.75));
        let eff = effective_ownership(&entities, &parent, &HashMap::new());
        assert_eq!(eff["A"], dec!(1));
        assert_eq!(eff["B"], dec!(0.75));
    }

    #[test]
    fn net_investment_hedge_reclassifies_to_cta() {
        // 有效部分 50 从套期工具重分类至 CTA(OCI)。
        let e = net_investment_hedge(dec!(50), "4106", "2501", "R_NIH").unwrap();
        assert!(e.is_balanced());
        assert_eq!(e.net_for("2501"), dec!(50));  // Dr 套期工具 +50
        assert_eq!(e.net_for("4106"), dec!(-50)); // Cr CTA(OCI) −50
        assert_eq!(e.elim_type, "net_investment_hedge");
        assert!(net_investment_hedge(dec!(0), "4106", "2501", "R_NIH").is_none());
    }

    #[test]
    fn goodwill_impairment_test_capped_at_goodwill() {
        // 账面 500(含商誉 80),可收回 460 → 缺口 40 ≤ 商誉 80 → 减值 40。
        assert_eq!(goodwill_impairment_test(dec!(500), dec!(460), dec!(80)), dec!(40));
        // 缺口 120 > 商誉 80 → 封顶 80(先冲商誉)。
        assert_eq!(goodwill_impairment_test(dec!(500), dec!(380), dec!(80)), dec!(80));
        // 可收回 ≥ 账面 → 不减值。
        assert_eq!(goodwill_impairment_test(dec!(500), dec!(520), dec!(80)), dec!(0));
    }
}
