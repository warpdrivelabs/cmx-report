// 生成 cmx-report 合并报表平台 阶段性总结的全部图（自包含浅色卡片 SVG，共用 dataviz 验证过的 CVD-安全调色板）。
// 每张图自绘 #fcfcfb 卡面 → 在任意浅/深 markdown 渲染器上都清晰可读。
// 用法: node docs/summary/assets/gen-svgs.mjs  → 写出 fig-*.svg
import { writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
const DIR = join(dirname(fileURLToPath(import.meta.url)), 'v2')

// ── 调色板（dataviz references/palette.md，浅色卡面）──
const P = {
  surface: '#fcfcfb', plane: '#f4f4f1', ink: '#0b0b0b', ink2: '#52514e', muted: '#898781',
  grid: '#e1e0d9', base: '#c3c2b7', border: 'rgba(11,11,11,0.12)',
  blue: '#2a78d6', orange: '#eb6834', aqua: '#1baf7a', yellow: '#eda100',
  magenta: '#e87ba4', green: '#008300', violet: '#4a3aa7', red: '#e34948',
  good: '#0ca30c', warning: '#fab219', serious: '#ec835a', critical: '#d03b3b',
  blue100: '#cde2fb', blue550: '#1c5cab',
}
const FONT = "system-ui,-apple-system,'Segoe UI','PingFang SC','Hiragino Sans GB','Microsoft YaHei',sans-serif"
const esc = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
const wOf = (s, per = 11) => [...String(s)].reduce((a, c) => a + (/[\x00-\xff]/.test(c) ? per * 0.58 : per), 0)

const T = (x, y, s, o = {}) => {
  const { size = 13, w = 400, fill = P.ink, anchor = 'start', op = 1, mono = false } = o
  return `<text x="${x}" y="${y}" font-family="${FONT}" font-size="${size}" font-weight="${w}" fill="${fill}" text-anchor="${anchor}" opacity="${op}"${mono ? ' font-variant-numeric="tabular-nums"' : ''}>${esc(s)}</text>`
}
const R = (x, y, w, h, o = {}) => {
  const { rx = 10, fill = 'none', stroke = 'none', sw = 1, fop = 1, sop = 1 } = o
  return `<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="${rx}" fill="${fill}" fill-opacity="${fop}" stroke="${stroke}" stroke-opacity="${sop}" stroke-width="${sw}"/>`
}
const LINE = (x1, y1, x2, y2, o = {}) => {
  const { stroke = P.muted, sw = 1.5, dash = '', marker = true } = o
  return `<line x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}" stroke="${stroke}" stroke-width="${sw}"${dash ? ` stroke-dasharray="${dash}"` : ''}${marker ? ' marker-end="url(#arr)"' : ''}/>`
}
const card = (w, h) => R(0, 0, w, h, { rx: 16, fill: P.surface, stroke: P.border, sw: 1 })
const defs = `<defs>
  <marker id="arr" markerWidth="9" markerHeight="9" refX="6.5" refY="3" orient="auto"><path d="M0,0 L6.5,3 L0,6 Z" fill="${P.muted}"/></marker>
</defs>`
const doc = (w, h, body) => `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}" role="img">${defs}${card(w, h)}${body}</svg>`

const band = (x, y, w, h, hue, title, sub, loc) => {
  let s = R(x, y, w, h, { rx: 10, fill: hue, fop: 0.10, stroke: hue, sop: 0.32, sw: 1 })
  s += R(x, y, 4, h, { rx: 2, fill: hue })
  s += T(x + 18, y + (sub ? h / 2 - 4 : h / 2 + 5), title, { size: 14.5, w: 700 })
  if (sub) s += T(x + 18, y + h / 2 + 15, sub, { size: 11.5, fill: P.ink2 })
  if (loc) s += T(x + w - 14, y + h / 2 + 5, loc, { size: 12, fill: P.muted, anchor: 'end', mono: true })
  return s
}
const cell = (x, y, w, h, hue, title, sub) => {
  let s = R(x, y, w, h, { rx: 9, fill: hue, fop: 0.10, stroke: hue, sop: 0.30, sw: 1 })
  s += R(x, y, 4, h, { rx: 2, fill: hue })
  s += T(x + w / 2 + 2, y + (sub ? h / 2 - 2 : h / 2 + 4), title, { size: 12.5, w: 700, anchor: 'middle' })
  if (sub) s += T(x + w / 2 + 2, y + h / 2 + 13, sub, { size: 10, fill: P.ink2, anchor: 'middle' })
  return s
}
const chip = (x, y, label, hue, o = {}) => {
  const { size = 11, pad = 11, h = 22 } = o
  const w = Math.round(wOf(label, size) + pad * 2)
  let s = R(x, y, w, h, { rx: h / 2, fill: hue, fop: 0.13, stroke: hue, sop: 0.34, sw: 1 })
  s += `<circle cx="${x + pad - 2}" cy="${y + h / 2}" r="3" fill="${hue}"/>`
  s += T(x + pad + 5, y + h / 2 + 4, label, { size, fill: P.ink })
  return { svg: s, w }
}
const chipFlow = (x0, y0, maxX, items, hue, o = {}) => {
  const { gap = 7, lh = 28 } = o
  let x = x0, y = y0, out = ''
  for (const it of items) {
    const c = chip(x, y, it, hue, o)
    if (x + c.w > maxX && x > x0) { x = x0; y += lh; }
    const c2 = chip(x, y, it, hue, o)
    out += c2.svg; x += c2.w + gap
  }
  return { svg: out, height: y + lh - y0 }
}
const title = (w, s, sub) => T(w / 2, 34, s, { size: 19, w: 800, anchor: 'middle' }) +
  (sub ? T(w / 2, 54, sub, { size: 12.5, fill: P.ink2, anchor: 'middle' }) : '')

const ST = { ok: P.good, warn: P.warning, no: P.muted }
const GLY = { ok: '✓', warn: '!', no: '–' }
const statusCell = (x, y, label, st) => {
  const hue = ST[st], h = 24, w = Math.round(wOf(label, 11.5) + 40)
  let s = R(x, y, w, h, { rx: 7, fill: hue, fop: st === 'no' ? 0.07 : 0.13, stroke: hue, sop: st === 'no' ? 0.3 : 0.36, sw: 1 })
  s += `<circle cx="${x + 13}" cy="${y + h / 2}" r="7.5" fill="${hue}"/>`
  s += T(x + 13, y + h / 2 + 4, GLY[st], { size: 11, w: 800, anchor: 'middle', fill: '#fff' })
  s += T(x + 27, y + h / 2 + 4.5, label, { size: 11.5, fill: st === 'no' ? P.ink2 : P.ink })
  return { svg: s, w }
}
const statusFlow = (x0, y0, maxX, items) => {
  const gap = 7, lh = 30
  let x = x0, y = y0, out = ''
  for (const [label, st] of items) {
    const probe = statusCell(x, y, label, st)
    if (x + probe.w > maxX && x > x0) { x = x0; y += lh }
    const c = statusCell(x, y, label, st)
    out += c.svg; x += c.w + gap
  }
  return { svg: out, height: y + lh - y0 }
}

// ══════════════ 图1 · 架构总览：借方正内核 + 三 crate + 出表复用 RPT ══════════════
function fig1 () {
  const W = 940, H = 600
  const x = 40, w = W - 80
  let b = title(W, '架构总览 · 借方正 signed 内核（One Signed-Convention Engine）',
    '3 consol crate · ~3.4k 域 LOC · 22 张 cg_* 元数据表 · 复用报表 RPT 计算态出表 · 落 cmx-report 独立微服务')
  // 前端工作台壳
  b += cell(x, 74, w, 46, P.violet, '合并报表工作台（native page · portal.consol.workbench）',
    '六区：方案/期间/范围树 + 工作底稿 / 内部往来对账 / 合并分类账 / 范围变动 / 合并报表 / 附注')
  b += LINE(W / 2, 120, W / 2, 140)
  // API + 出表
  const half = (w - 16) / 2
  b += cell(x, 142, half, 46, P.blue, 'cmx-rpt-app::consol · 30+ 端点', 'consol_routes::<S>() · 门户 /consol/* 反代 + 白名单')
  b += cell(x + half + 16, 142, half, 46, P.aqua, '出表：复用 RPT 计算态', 'CG/IND/ELIM/CF/EQC 取数函数 → 合并四表 CBS/CIS/CCF/CSE')
  b += LINE(W / 2, 188, W / 2, 206)
  // store 编排
  b += band(x, 208, w, 52, P.orange, 'cmx-consol-store-pg · DB 编排 + 引擎驱动',
    'run_consolidation 逐级 · CoA/IC/外币/范围变动 · CF/权益聚合 · 工作底稿法 · 交叉持股 · 关账编排 · 附注', '2,300+')
  b += LINE(W / 2, 260, W / 2, 278)
  // model 纯引擎
  b += band(x, 280, w, 52, P.blue, 'cmx-consol-model · 纯算法内核（借方正 signed，零 DB/HTTP，22 单测）',
    'aggregate · capital/common_control · minority_pl · 5类内部抵销 · step_acq/disposal · equity_pickup · goodwill_impair · cashflow_ws · effective_ownership', '1,400+')
  b += LINE(W / 2, 332, W / 2, 350)
  // 借方正约定说明带
  b += R(x, 352, w, 66, { rx: 10, fill: P.green, fop: 0.09, stroke: P.green, sop: 0.3, sw: 1 })
  b += R(x, 352, 4, 66, { rx: 2, fill: P.green })
  b += T(x + 18, 374, '★ 借方正 signed 约定（引擎根基）', { size: 13.5, w: 800 })
  b += T(x + 18, 394, '资产/费用为正、负债/权益/收入为负 → 聚合=纯加法、抵销分录 net=dr−cr 直接 += 、平衡凭证 net 和=0；', { size: 11.5, fill: P.ink2 })
  b += T(x + 18, 411, '合并数=个别+调整+抵销；资产负债表恒等式塌缩成「全科目合并数之和=0」（所有 E2E 的核心断言）。', { size: 11.5, fill: P.ink2 })
  // 元数据建表说明
  b += R(x, 428, w, 52, { rx: 10, fill: P.plane, stroke: P.border, sw: 1 })
  b += R(x, 428, 4, 52, { rx: 2, fill: P.magenta })
  b += T(x + 18, 450, '元数据建表（不在程序创建）', { size: 12.5, w: 800, fill: P.ink })
  b += T(x + 18, 468, '22 张 cg_* 表声明于 cmxfico_consol_dct_meta_v1.json，经 model-center /api/model/deploy 部署（additive-only）', { size: 11, fill: P.muted })
  // footer 依赖
  b += R(x, 490, w, 66, { rx: 10, fill: P.plane, stroke: P.border, sw: 1 })
  b += T(x + 16, 512, '单向借用基础库（编译期 path 依赖，无反向引用）：', { size: 11.5, w: 700, fill: P.ink2 })
  b += T(x + 16, 531, 'rust_decimal · cmx-database-pg · cmx-core(DataValue) · cmx-utils(next_pk_id) · cmx-biz(BizError) · cmx-api-types', { size: 11, fill: P.muted, mono: true })
  b += T(x + 16, 548, '复用报表 RPT：cmx-rpt-formula(CG/IND/ELIM 取数) · cmx-rpt-store-pg(compute 计算态) · native-pages 投递', { size: 11, fill: P.muted, mono: true })
  return doc(W, H, b)
}

// ══════════════ 图2 · 合并七段流水线 ══════════════
function fig2 () {
  const W = 940, H = 470
  let b = title(W, '合并流水线 · 自底向上逐级合并', '一次 run_consolidation：从叶子个别数到根合并数，逐级抵销，落工作底稿 + 合并分类账')
  const steps = [
    ['① 采集映射', P.aqua, 'CoA 映射', '本地→集团科目\n× sign 归一'],
    ['② 外币折算', P.blue, 'IAS 21', '资产负债×期末\n损益×平均·CTA'],
    ['③ 内部对账', P.magenta, '双边配对', 'matched=min\ndiff 差异检测'],
    ['④ 逐级聚合', P.green, 'aggregate', '下级×并入比例\n全额=1 权益=0'],
    ['⑤ 生成抵销', P.orange, '规则驱动', '资本·少数·债务\n购销·存货·减值'],
    ['⑥ 工作底稿', P.blue, '四栏组装', '个别+调整+抵销\n= 合并数'],
    ['⑦ 出表', P.violet, '复用 RPT', 'CG/IND/ELIM\n合并四表'],
  ]
  const n = steps.length, gap = 12, x0 = 34, bw = (W - 2 * x0 - (n - 1) * gap) / n, y = 82, bh = 118
  steps.forEach(([tag, hue, sub, desc], i) => {
    const cx = x0 + i * (bw + gap)
    b += R(cx, y, bw, bh, { rx: 11, fill: hue, fop: 0.09, stroke: hue, sop: 0.32, sw: 1 })
    b += R(cx, y, bw, 4, { rx: 2, fill: hue })
    b += T(cx + bw / 2, y + 28, tag, { size: 12, w: 800, anchor: 'middle' })
    b += T(cx + bw / 2, y + 48, sub, { size: 10, w: 700, anchor: 'middle', fill: hue })
    desc.split('\n').forEach((ln, j) => b += T(cx + bw / 2, y + 74 + j * 16, ln, { size: 9.5, anchor: 'middle', fill: P.ink2 }))
    if (i < n - 1) b += LINE(cx + bw + 1, y + bh / 2, cx + bw + gap - 1, y + bh / 2, { sw: 1.4 })
  })
  // LCA 说明
  b += R(x0, y + bh + 24, W - 2 * x0, 52, { rx: 10, fill: P.plane, stroke: P.border, sw: 1 })
  b += R(x0, y + bh + 24, 4, 52, { rx: 2, fill: P.aqua })
  b += T(x0 + 18, y + bh + 46, '多级合并要点 · 最低公共祖先（LCA）抵销', { size: 12.5, w: 800 })
  b += T(x0 + 18, y + bh + 64, '内部往来/存货利润在两端最低公共祖先节点抵销（same_child_subtree 判定），避免跨层重复抵销；3 级实测不重复。', { size: 11, fill: P.muted })
  // 幂等
  b += R(x0, y + bh + 86, W - 2 * x0, 40, { rx: 10, fill: P.green, fop: 0.08, stroke: P.green, sop: 0.28, sw: 1 })
  b += R(x0, y + bh + 86, 4, 40, { rx: 2, fill: P.green })
  b += T(x0 + 18, y + bh + 111, '幂等：每次 run 先 DELETE 该 scheme+period 派生数据再重算 → 重跑结果一致、无重复凭证（8 方案实测）。', { size: 11.5, fill: P.ink2 })
  return doc(W, H, b)
}

// ══════════════ 图3 · 能力演进（合并会计能力轨） ══════════════
function fig3 () {
  const W = 980
  const tracks = [
    ['骨架', 'C0', P.violet, ['3 consol crate', 'cg_* 元数据表', '方案/范围主数据 CRUD', 'model-center 部署']],
    ['逐级合并', 'C2/C3', P.blue, ['全额合并', '逐级 rollup', '资本抵销(长投↔权益)', '商誉/合并价差', '少数股东权益 NCI', '少数股东损益']],
    ['抵销引擎', 'C3', P.orange, ['规则驱动 cg_elim_rule', '债务抵销', '购销抵销', '合并分类账凭证式', '可追溯 source_rule']],
    ['采集/对账', 'C1/C4', P.aqua, ['CoA 科目映射 + sign', 'IC 双边对账', 'matched/diff/单边', '差异工作台']],
    ['外币/C6', 'C5/C6', P.magenta, ['IAS21 外币折算+CTA', '存货未实现利润+期初结转', '权益法确认', '商誉减值']],
    ['出表+范围', 'C7', P.blue, ['合并四表', '范围变动 diff_scope', '前端四区工作台', 'CG/IND/ELIM 函数']],
    ['Next 五项', '0822', P.green, ['现金流量表数据模型(CF流水)', '权益变动表数据模型(EQC流水)', 'CCF/CSE 真取数', 'cmx-flow 关账编排(env-gated)', '四表进工作台', '抵销分录反向下钻']],
    ['Later 七项', '0822', P.orange, ['同一控制下企业合并', '分步取得/处置损益', '固定资产内部未实现利润', '内部股利抵销', '现金流量·工作底稿法', '交叉持股(矩阵法有效持股)', '附注自动生成']],
  ]
  const x0 = 156, maxX = W - 34
  let rows = '', y = 78
  for (const [name, code, hue, items] of tracks) {
    const f = chipFlow(x0, y, maxX, items, hue, { size: 11, lh: 28 })
    const rowH = f.height
    rows += R(28, y - 4, 116, rowH - 4, { rx: 9, fill: hue, fop: 0.10, stroke: hue, sop: 0.30, sw: 1 })
    rows += R(28, y - 4, 4, rowH - 4, { rx: 2, fill: hue })
    rows += T(44, y + 14, name, { size: 13, w: 800 })
    if (code) rows += T(44, y + 31, code, { size: 10, fill: P.muted, mono: true })
    rows += f.svg
    y += rowH + 8
  }
  const H = y + 46
  let b = title(W, '能力演进 · 合并会计能力轨（对标 CAS 33 / IFRS 10 / LucaNet）', 'C0–C7 基线 + Next 五项 + Later 七项，全部交付并回归通过（借方正内核不变）')
  b += rows
  b += R(28, y + 2, W - 56, 34, { rx: 9, fill: P.plane, stroke: P.border, sw: 1 })
  b += T(44, y + 24, '状态：C0–C7 + Next 五项 + Later 七项全部落地并真机验证；八方案合并 BS 恒等=0、model 22 单测、后端/前端 E2E 全绿。', { size: 11.5, fill: P.ink2 })
  return doc(W, H, b)
}

// ══════════════ 图4 · 合并会计能力地图（覆盖矩阵） ══════════════
function fig4 () {
  const W = 980
  const cats = [
    ['合并方法', P.blue, [['全额合并', 'ok'], ['权益法', 'ok'], ['比例合并', 'ok'], ['成本法', 'ok'], ['同一控制下(权益结合法)', 'ok']]],
    ['资本抵销', P.orange, [['长投↔子公司权益', 'ok'], ['商誉/合并价差', 'ok'], ['少数股东权益', 'ok'], ['少数股东损益', 'ok'], ['分步取得(公允重估)', 'ok'], ['处置损益/权益交易', 'ok']]],
    ['内部交易', P.aqua, [['债务抵销', 'ok'], ['购销抵销', 'ok'], ['存货未实现利润', 'ok'], ['期初结转', 'ok'], ['固定资产未实现+折旧转回', 'ok'], ['内部股利', 'ok']]],
    ['内部对账', P.magenta, [['双边申报配对', 'ok'], ['匹配额 min(A,B)', 'ok'], ['差异检测', 'ok'], ['单边未达识别', 'ok'], ['差异工作台', 'ok'], ['自动调整建议', 'no']]],
    ['外币折算', P.green, [['期末汇率(资产负债)', 'ok'], ['平均汇率(损益)', 'ok'], ['历史汇率(权益)', 'ok'], ['CTA 折算差额', 'ok'], ['CTA 不参与抵销', 'ok'], ['净投资套期', 'no']]],
    ['权益法/减值', P.violet, [['份额确认投资收益', 'ok'], ['长投调整', 'ok'], ['盈亏反向', 'ok'], ['商誉减值', 'ok'], ['商誉减值测试模型', 'no']]],
    ['持股/关账', P.yellow, [['多级逐级合并', 'ok'], ['LCA 抵销', 'ok'], ['交叉持股(矩阵法有效持股)', 'ok'], ['关账流程编排(cmx-flow)', 'ok']]],
    ['范围/出表/披露', P.blue, [['范围变动对比', 'ok'], ['合并资产负债表', 'ok'], ['合并利润表', 'ok'], ['现金流量表(流水+工作底稿法)', 'ok'], ['权益变动表', 'ok'], ['附注自动生成', 'ok']]],
  ]
  const x0 = 152, maxX = W - 30
  let rows = '', y = 100
  for (const [name, hue, items] of cats) {
    const f = statusFlow(x0, y, maxX, items)
    const rowH = f.height
    rows += R(28, y - 4, 116, rowH - 4, { rx: 9, fill: hue, fop: 0.10, stroke: hue, sop: 0.30, sw: 1 })
    rows += R(28, y - 4, 4, rowH - 4, { rx: 2, fill: hue })
    rows += T(44, y + 13, name, { size: 12, w: 800 })
    rows += f.svg
    y += rowH + 8
  }
  const H = y + 16
  let b = title(W, '合并会计能力地图 · 覆盖矩阵', '核心抵销 + 高级合并全覆盖(同控/分步处置/固资/股利/交叉持股/关账/四表/附注);余下少数项为按需/择机的进阶场景')
  let lx = W / 2 - 220
  for (const [lab, st] of [['已交付', 'ok'], ['暂不支持（按需/择机）', 'no']]) {
    const c = statusCell(lx, 64, lab, st); b += c.svg; lx += c.w + 12
  }
  b += rows
  return doc(W, H, b)
}

// ══════════════ 图5 · 测试覆盖 stat tiles ══════════════
function fig5 () {
  const W = 920, H = 396
  let b = title(W, '测试覆盖 · 真机验证', 'Rust 纯引擎单测 + 后端 curl 回归 + Playwright/CDP 前端；八合并方案均借贷平衡（合并数合计=0）且幂等')
  const tiles = [
    ['22', 'model 纯引擎单测', 'cargo test -p cmx-consol-model'],
    ['8/8', '合并方案 BS 恒等 0', 'CAS_LEGAL/FX/INV/GW/EQ/MAP/RECON/IS'],
    ['9/9', 'Later 七项后端 E2E', 'e2e-consol-later.sh'],
    ['10/10', '四表+下钻 前端 CDP', 'e2e-consol-statements-frontend.mjs'],
    ['17/17', '工作台功能 CDP', 'e2e-consol-workbench.mjs'],
    ['4/4 · 1/1', '出表 / 范围变动 E2E', 'e2e-consol-statements/scope'],
    ['22', 'cg_* 元数据表', 'model-center 部署(additive)'],
    ['30+', 'consol API 端点', 'consol_routes::<S>()'],
  ]
  const cols = 4, gap = 16, x0 = 40, tw = (W - 80 - (cols - 1) * gap) / cols, th = 96, y0 = 78
  tiles.forEach((t, i) => {
    const cx = x0 + (i % cols) * (tw + gap), cy = y0 + Math.floor(i / cols) * (th + 16)
    b += R(cx, cy, tw, th, { rx: 12, fill: P.surface, stroke: P.border, sw: 1 })
    b += R(cx, cy, tw, 4, { rx: 2, fill: P.good })
    b += `<circle cx="${cx + 16}" cy="${cy + 26}" r="7" fill="${P.good}" fill-opacity="0.15"/><path d="M${cx + 12.5},${cy + 26} l2.5,2.5 l5,-5.5" stroke="${P.good}" stroke-width="1.8" fill="none" stroke-linecap="round" stroke-linejoin="round"/>`
    b += T(cx + tw / 2 + 8, cy + 48, t[0], { size: 26, w: 800, anchor: 'middle', fill: P.good, mono: true })
    b += T(cx + tw / 2, cy + 68, t[1], { size: 11, w: 700, anchor: 'middle', fill: P.ink })
    b += T(cx + tw / 2, cy + 85, t[2], { size: 9, anchor: 'middle', fill: P.muted, mono: true })
  })
  return doc(W, H, b)
}

// ══════════════ 图6 · 未来计划路线图 ══════════════
function fig6 () {
  const W = 940
  let b = ''
  const colW = (W - 80 - 2 * 18) / 3, x0 = 40, y0 = 74
  const cols = [
    ['已全部交付', P.good, ['借方正内核 + 逐级合并 + NCI', '资本抵销 / 债务 / 购销 / 存货利润', 'CoA 映射 + IC 双边对账', '外币折算 CTA（IAS21）', '权益法 + 商誉减值（C6 完整）', '合并四表真出数（CBS/CIS/CCF/CSE）', '范围变动 + 六区前端工作台', '关账编排（cmx-flow env-gated）', '四表内嵌工作台 + 抵销下钻（N4/N5）']],
    ['Later 七项（已交付）', P.blue, ['同一控制下企业合并（权益结合法）', '分步取得 / 处置损益', '固定资产内部未实现利润 + 折旧转回', '内部股利抵销', '现金流量表·工作底稿法（间接法）', '交叉持股（矩阵法有效持股迭代收敛）', '附注自动生成（NCI / 商誉 / 范围变动）']],
    ['正交 / 择机项', P.violet, ['净投资套期（外币）', '商誉减值测试模型', '自动 IC 调整建议', '完整 CCF/CSE 直接法流水', '完整现金流量 33 行科目', '有效持股接入 NCI 精算', 'cmx-container 一键部署集成']],
  ]
  const CH = 386
  cols.forEach(([hd, hue, items], ci) => {
    const cx = x0 + ci * (colW + 18)
    b += R(cx, y0, colW, CH, { rx: 12, fill: P.surface, stroke: P.border, sw: 1 })
    b += R(cx, y0, colW, 34, { rx: 12, fill: hue, fop: 0.14 })
    b += R(cx, y0 + 22, colW, 12, { fill: hue, fop: 0.14 })
    b += R(cx, y0, 4, CH, { rx: 2, fill: hue })
    b += T(cx + 16, y0 + 22, hd, { size: 13.5, w: 800 })
    items.forEach((it, i) => {
      const iy = y0 + 52 + i * 37
      b += `<circle cx="${cx + 18}" cy="${iy}" r="3" fill="${hue}"/>`
      const words = it, max = colW - 40
      if (wOf(words, 11.5) <= max) { b += T(cx + 30, iy + 4, words, { size: 11, fill: P.ink }) }
      else {
        let cut = words.length
        while (cut > 1 && wOf(words.slice(0, cut), 11.5) > max) cut--
        let br = words.slice(0, cut)
        const sp = Math.max(br.lastIndexOf('（'), br.lastIndexOf(' '), br.lastIndexOf('/'))
        if (sp > cut * 0.5) cut = sp
        b += T(cx + 30, iy - 3, words.slice(0, cut), { size: 11, fill: P.ink })
        b += T(cx + 30, iy + 11, words.slice(cut), { size: 11, fill: P.ink })
      }
    })
  })
  const H = y0 + CH + 62
  let hdr = title(W, '未来计划 · 全交付状态 / 择机项', '核心抵销体系全覆盖；高级合并 Later 七项已落地；余下项按业务诉求择机推进')
  b = hdr + b
  b += R(x0, y0 + CH + 8, W - 80, 46, { rx: 10, fill: P.plane, stroke: P.border, sw: 1 })
  b += R(x0, y0 + CH + 8, 4, 46, { rx: 2, fill: P.serious })
  b += T(x0 + 18, y0 + CH + 30, '正交 / 坚决不做（永久约束）', { size: 12, w: 800, fill: P.ink })
  b += T(x0 + 18, y0 + CH + 46, '引擎不认字典/组织/DB（维度经装载注入）· 出表复用报表 RPT 不另造计算引擎 · 表结构走元数据部署不在程序建表', { size: 11, fill: P.muted })
  return doc(W, H, b)
}

const figs = { 'fig-1-architecture': fig1(), 'fig-2-pipeline': fig2(), 'fig-3-timeline': fig3(), 'fig-4-capability-map': fig4(), 'fig-5-tests': fig5(), 'fig-6-roadmap': fig6() }
for (const [k, v] of Object.entries(figs)) { writeFileSync(join(DIR, k + '.svg'), v); console.log(`${k}.svg: ${v.length}B`) }
console.log(`\n${Object.keys(figs).length} figs written`)
