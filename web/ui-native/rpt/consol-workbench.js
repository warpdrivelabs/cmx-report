/**
 * 合并报表工作台 —— native_pages 四区工作台(集团合并数据消费/运营侧)。
 *
 * explorer：合并方案下拉 + 会计期间下拉 + 合并范围节点树(cg_scope);
 *           顶部「运行合并」「运行对账」按钮。点击节点 → content/property 联动该节点。
 * content ：三个 tab —— 合并工作底稿(科目|个别|调整|抵销|合并,末行平衡校验)、
 *           内部往来对账(A/B/双边额/匹配/差异/状态,差异行标红)、合并分类账(抵销/调整凭证)。
 * property：选中节点信息(方法/持股/币种/层级) + 合并平衡校验(全科目合并数合计=0 ⇒ 借贷平衡)。
 *
 * 数据源:report-server 的 /api/consol/*(独立运行直连;门户内嵌经 ReportProxy 反代)。
 * 借方正 signed 约定:金额借方为正、贷方为负;工作底稿按原值展示(负数红色)。
 */

const state = {
  schemes: [],
  selectedScheme: '',
  periods: [],
  selectedPeriod: '',
  nodes: [],
  nodeTree: [],
  nodeExpanded: new Set(),
  selectedNode: '',
  accounts: {}, // code -> {name, type}
  tab: 'worksheet', // worksheet | recon | journal | scope
  worksheet: [], // [{account_code, individual, adjust, elim, consolidated}]
  journal: [], // [{doc_no,line_no,elim_type,account_code,dr,cr,partner,is_opening,source_rule}]
  recon: [], // [{entity_a,entity_b,ic_type,a_amount,b_amount,matched,diff,recon_status}]
  scopeChange: [], // [{org_code,org_name,change_type,curr_method,prev_method,curr_ownership,prev_ownership,prev_period}]
  loading: false,
  running: false,
  message: '',
  hosts: new Set(),
}

const METHOD_LABEL = { full: '全额合并', equity: '权益法', proportional: '比例合并', cost: '成本法' }
const METHOD_COLOR = { full: '#1e88e5', equity: '#8e24aa', proportional: '#00acc1', cost: '#64748b' }
const RECON_BADGE = {
  matched: { label: '已匹配', color: '#43a047' },
  diff: { label: '有差异', color: '#e53935' },
  one_sided: { label: '单边未达', color: '#fb8c00' },
}
const ELIM_LABEL = {
  capital: '资本抵销', nci: '少数股东损益', debt: '债务抵销', sales: '购销抵销',
  inventory: '存货未实现利润', inventory_opening: '存货利润·期初结转',
  equity_method: '权益法确认', goodwill_impair: '商誉减值',
}

const SCOPE_BADGE = {
  first_time: { label: '新纳入', color: '#43a047' },
  disposal: { label: '处置', color: '#e53935' },
  ownership_up: { label: '增持', color: '#1e88e5' },
  ownership_down: { label: '减持', color: '#fb8c00' },
  method_change: { label: '方法变更', color: '#8e24aa' },
  unchanged: { label: '不变', color: '#64748b' },
}

const esc = (s) => String(s ?? '')
  .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
  .replace(/"/g, '&quot;').replace(/'/g, '&#39;')

const enc = (s) => encodeURIComponent(String(s ?? ''))

// 金额格式化(千分位 + 2 位小数;负数用括号并标红由 CSS 处理)。
function fmt (v) {
  const n = Number(v)
  if (!isFinite(n)) return String(v ?? '')
  const neg = n < 0
  const s = Math.abs(n).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })
  return neg ? `(${s})` : s
}

function num (v) {
  const n = Number(v)
  return isFinite(n) ? n : 0
}

function normalizeArray (v) {
  if (Array.isArray(v)) return v
  if (v && Array.isArray(v.rows)) return v.rows
  return []
}

async function apiJson (url, options = {}) {
  const res = await fetch(url, {
    ...options,
    headers: { Accept: 'application/json', ...(options.headers || {}) },
    credentials: 'same-origin',
  })
  let j = null
  try { j = await res.json() } catch {}
  if (!res.ok || (j && typeof j.code === 'number' && j.code !== 0)) {
    throw new Error((j && (j.msg || j.error)) || `HTTP ${res.status}`)
  }
  return j && typeof j === 'object' && 'data' in j ? j.data : j
}

// ============================================================================
// 数据装载
// ============================================================================

async function loadSchemes () {
  state.loading = true
  refreshAll()
  try {
    const d = await apiJson('/api/consol/schemes')
    state.schemes = normalizeArray(d.schemes)
    if (!state.selectedScheme && state.schemes[0]) {
      state.selectedScheme = state.schemes[0].scheme_code
    }
  } catch (err) {
    state.message = '合并方案加载失败：' + (err.message || err)
  } finally {
    state.loading = false
  }
  if (state.selectedScheme) { await loadScheme(state.selectedScheme, false) }
  refreshAll()
}

// 选中方案 → 装载期间 + 科目表,并选中首期。
async function loadScheme (scheme, doRefresh = true) {
  state.selectedScheme = scheme
  state.selectedPeriod = ''
  state.periods = []
  state.nodes = []
  state.nodeTree = []
  state.worksheet = []; state.journal = []; state.recon = []
  try {
    const [p, a] = await Promise.all([
      apiJson(`/api/consol/periods?scheme=${enc(scheme)}`),
      apiJson(`/api/consol/accounts?scheme=${enc(scheme)}`),
    ])
    state.periods = normalizeArray(p.periods).map((r) => r.period_code).filter(Boolean)
    state.accounts = {}
    for (const r of normalizeArray(a.accounts)) {
      if (r.account_code) state.accounts[r.account_code] = { name: r.name || '', type: r.account_type || '' }
    }
    if (state.periods[0]) { await loadPeriod(state.periods[0], false) }
  } catch (err) {
    state.message = '方案数据加载失败：' + (err.message || err)
  }
  if (doRefresh) refreshAll()
}

// 选中期间 → 装载合并节点树,选中根节点。
async function loadPeriod (period, doRefresh = true) {
  state.selectedPeriod = period
  state.nodes = []; state.nodeTree = []
  state.worksheet = []; state.journal = []; state.recon = []
  try {
    const d = await apiJson(`/api/consol/nodes?scheme=${enc(state.selectedScheme)}&period=${enc(period)}`)
    state.nodes = normalizeArray(d.nodes)
    state.nodeTree = buildNodeTree(state.nodes)
    state.nodeExpanded = new Set(state.nodes.map((n) => String(n.org_code)))
    const root = state.nodes.find((n) => !n.parent_code) || state.nodes[0]
    state.selectedNode = root ? String(root.org_code) : ''
    if (state.selectedNode) { await loadNode(false) }
  } catch (err) {
    state.message = '合并范围加载失败：' + (err.message || err)
  }
  if (doRefresh) refreshAll()
}

// 装载选中节点的工作底稿 + 分类账 + 对账 + 范围变动(对账/范围变动是方案期级,非节点级)。
async function loadNode (doRefresh = true) {
  const { selectedScheme: s, selectedPeriod: p, selectedNode: n } = state
  if (!s || !p) return
  try {
    const tasks = [
      apiJson(`/api/consol/worksheet?scheme=${enc(s)}&period=${enc(p)}&node=${enc(n)}`),
      apiJson(`/api/consol/journal?scheme=${enc(s)}&period=${enc(p)}&node=${enc(n)}`),
      apiJson(`/api/consol/ic-recon?scheme=${enc(s)}&period=${enc(p)}`),
      apiJson(`/api/consol/scope-change?scheme=${enc(s)}&period=${enc(p)}`),
    ]
    const [ws, jn, rc, sc] = await Promise.all(tasks)
    state.worksheet = normalizeArray(ws.rows)
    state.journal = normalizeArray(jn.entries)
    state.recon = normalizeArray(rc.rows)
    state.scopeChange = normalizeArray(sc.rows)
  } catch (err) {
    state.message = '底稿/分类账加载失败：' + (err.message || err)
  }
  if (doRefresh) refreshAll()
}

// ============================================================================
// 运行动作
// ============================================================================

async function runConsolidation () {
  if (!state.selectedScheme || !state.selectedPeriod) { toast('请先选择方案与期间', 'warn'); return }
  state.running = true; refreshAll()
  try {
    const d = await apiJson('/api/consol/run', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ scheme: state.selectedScheme, period: state.selectedPeriod }),
    })
    toast(d.message || '合并完成', 'ok')
    await loadNode(false)
  } catch (err) {
    toast('合并失败：' + (err.message || err), 'error')
  } finally {
    state.running = false; refreshAll()
  }
}

async function runReconcile () {
  if (!state.selectedScheme || !state.selectedPeriod) { toast('请先选择方案与期间', 'warn'); return }
  state.running = true; refreshAll()
  try {
    const d = await apiJson('/api/consol/ic-reconcile', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ scheme: state.selectedScheme, period: state.selectedPeriod }),
    })
    toast(d.message || '对账完成', 'ok')
    state.tab = 'recon'
    await loadNode(false)
  } catch (err) {
    toast('对账失败：' + (err.message || err), 'error')
  } finally {
    state.running = false; refreshAll()
  }
}

async function runScopeChange () {
  if (!state.selectedScheme || !state.selectedPeriod) { toast('请先选择方案与期间', 'warn'); return }
  state.running = true; refreshAll()
  try {
    const d = await apiJson('/api/consol/scope-change', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ scheme: state.selectedScheme, period: state.selectedPeriod }),
    })
    toast(d.message || '范围变动对比完成', 'ok')
    state.tab = 'scope'
    await loadNode(false)
  } catch (err) {
    toast('范围变动失败：' + (err.message || err), 'error')
  } finally {
    state.running = false; refreshAll()
  }
}

// ============================================================================
// 节点树
// ============================================================================

function buildNodeTree (rows) {
  const byCode = new Map(rows.map((r) => [String(r.org_code), { ...r, children: [] }]))
  const roots = []
  for (const r of byCode.values()) {
    const pc = r.parent_code ? String(r.parent_code) : ''
    if (pc && byCode.has(pc)) byCode.get(pc).children.push(r)
    else roots.push(r)
  }
  const sortRec = (ns) => {
    ns.sort((a, b) => String(a.org_code).localeCompare(String(b.org_code)))
    ns.forEach((n) => sortRec(n.children))
  }
  sortRec(roots)
  return roots
}

function selectedNodeRow () {
  return state.nodes.find((n) => String(n.org_code) === String(state.selectedNode)) || null
}

// 合并数合计(借方正下应=0 ⇒ 借贷平衡)。
function worksheetTotals () {
  let ind = 0, adj = 0, elm = 0, con = 0
  for (const r of state.worksheet) {
    ind += num(r.individual); adj += num(r.adjust); elm += num(r.elim); con += num(r.consolidated)
  }
  return { ind, adj, elm, con }
}

// ============================================================================
// mount / render
// ============================================================================

function mount (ctx, view) {
  const host = ctx.host
  state.hosts.add(host)
  if (host) host.__consolView = view
  const render = () => {
    const root = host?.renderRoot || host?.shadowRoot?.querySelector('.native-page-root')
    if (!root || !root.isConnected) return
    root.innerHTML = `<style>${styleCss()}</style>${viewHtml(view)}`
    bind(root, view)
  }
  requestAnimationFrame(() => { render(); if (!state.schemes.length) loadSchemes() })
  return `<style>${styleCss()}</style>${viewHtml(view)}`
}

function refreshAll () {
  for (const host of Array.from(state.hosts)) {
    if (!host || !host.isConnected) { state.hosts.delete(host); continue }
    const root = host.renderRoot || host.shadowRoot?.querySelector('.native-page-root')
    if (!root) continue
    const view = host.__consolView || host.getAttribute?.('view') || 'content'
    root.innerHTML = `<style>${styleCss()}</style>${viewHtml(view)}`
    bind(root, view)
  }
}

function toast (message, kind = 'info') {
  requestAnimationFrame(() => {
    let host = null
    for (const h of Array.from(state.hosts)) {
      if (!h || !h.isConnected) continue
      const root = h.renderRoot || h.shadowRoot?.querySelector('.native-page-root')
      const sec = root?.querySelector?.('.cg-content')
      if (sec) { host = sec; break }
    }
    if (!host) return
    if (getComputedStyle(host).position === 'static') host.style.position = 'relative'
    let box = host.querySelector(':scope > .cg-toast')
    if (!box) { box = document.createElement('div'); box.className = 'cg-toast'; host.appendChild(box) }
    box.setAttribute('data-kind', kind)
    box.textContent = message
    box.classList.remove('show'); void box.offsetWidth; box.classList.add('show')
    clearTimeout(box.__t)
    box.__t = setTimeout(() => box.classList.remove('show'), 3200)
  })
}

function viewHtml (view) {
  if (view === 'explorer') return explorerHtml()
  if (view === 'property') return propertyHtml()
  return contentHtml()
}

// ============================================================================
// explorer:方案 + 期间 + 节点树 + 运行按钮
// ============================================================================

function explorerHtml () {
  const schemeOpts = state.schemes.map((s) =>
    `<option value="${esc(s.scheme_code)}" ${state.selectedScheme === s.scheme_code ? 'selected' : ''}>${esc(s.name || s.scheme_code)}</option>`).join('')
  const periodOpts = state.periods.map((p) =>
    `<option value="${esc(p)}" ${state.selectedPeriod === p ? 'selected' : ''}>${esc(p)}</option>`).join('')
  const runningAttr = state.running ? 'disabled' : ''
  return `<section class="cg cg-explorer">
    <div class="cg-field">
      <span class="cg-lbl"><ui5-icon name="combine"></ui5-icon>合并方案</span>
      <div class="cg-select-wrap">
        <select data-scheme-select>${schemeOpts || '<option value="">（无方案）</option>'}</select>
        <ui5-icon class="cg-caret" name="slim-arrow-down"></ui5-icon>
      </div>
    </div>
    <div class="cg-field">
      <span class="cg-lbl"><ui5-icon name="calendar"></ui5-icon>会计期间</span>
      <div class="cg-select-wrap">
        <select data-period-select>${periodOpts || '<option value="">（无期间）</option>'}</select>
        <ui5-icon class="cg-caret" name="slim-arrow-down"></ui5-icon>
      </div>
    </div>
    <div class="cg-actions">
      <button class="cg-btn cg-btn-primary" data-act="run" ${runningAttr}>
        <ui5-icon name="begin"></ui5-icon>${state.running ? '运行中…' : '运行合并'}</button>
      <button class="cg-btn" data-act="reconcile" ${runningAttr}>
        <ui5-icon name="synchronize"></ui5-icon>运行对账</button>
      <button class="cg-btn" data-act="scope-change" ${runningAttr}>
        <ui5-icon name="org-chart"></ui5-icon>范围变动</button>
    </div>
    <div class="cg-tree-title"><ui5-icon name="org-chart"></ui5-icon>合并范围
      <span class="cg-tree-count">${state.nodes.length}</span></div>
    <div class="cg-tree">${renderNodes(state.nodeTree, 0)}</div>
  </section>`
}

function renderNodes (nodes, depth) {
  if (!nodes.length && depth === 0) {
    return `<cmx-empty-state icon="tree" title="暂无合并范围" size="sm"></cmx-empty-state>`
  }
  return nodes.map((n) => {
    const code = String(n.org_code)
    const pad = 8 + depth * 14
    const hasKids = n.children.length > 0
    const open = state.nodeExpanded.has(code)
    const active = String(state.selectedNode) === code
    const leaf = Number(n.is_leaf) === 1
    const method = String(n.consol_method || '').toLowerCase()
    const caret = hasKids
      ? `<ui5-icon class="cg-caret2" name="${open ? 'navigation-down-arrow' : 'navigation-right-arrow'}" data-node-expand="${esc(code)}"></ui5-icon>`
      : '<span class="cg-caret-empty"></span>'
    const kids = hasKids && open ? `<div class="cg-tree-children">${renderNodes(n.children, depth + 1)}</div>` : ''
    const badge = leaf ? '' : `<span class="cg-node-badge" style="--c:${METHOD_COLOR[method] || '#64748b'}">${esc(METHOD_LABEL[method] || method || '合并')}</span>`
    return `<div class="cg-branch">
      <div class="cg-node ${active ? 'active' : ''}" data-node="${esc(code)}" style="padding-left:${pad}px" title="${esc(n.org_name || code)}">
        ${caret}
        <ui5-icon class="cg-node-ic" name="${leaf ? 'building' : 'company-view'}"></ui5-icon>
        <span class="cg-node-name">${esc(n.org_name || code)}</span>
        ${badge}
      </div>${kids}
    </div>`
  }).join('')
}

// ============================================================================
// content:tab(工作底稿 / 对账 / 分类账)
// ============================================================================

function contentHtml () {
  const nodeRow = selectedNodeRow()
  const ctx = [state.selectedScheme, state.selectedPeriod, nodeRow?.org_name || state.selectedNode].filter(Boolean).join(' · ')
  const tabs = [
    { key: 'worksheet', label: '合并工作底稿', icon: 'table-chart' },
    { key: 'recon', label: '内部往来对账', icon: 'synchronize' },
    { key: 'journal', label: '合并分类账', icon: 'journey-arrive' },
    { key: 'scope', label: '范围变动', icon: 'org-chart' },
  ]
  const tabRow = tabs.map((t) =>
    `<button class="cg-tab ${state.tab === t.key ? 'active' : ''}" data-tab="${t.key}">
       <ui5-icon name="${t.icon}"></ui5-icon>${t.label}</button>`).join('')
  let body = ''
  if (state.tab === 'worksheet') body = worksheetTable()
  else if (state.tab === 'recon') body = reconTable()
  else if (state.tab === 'scope') body = scopeChangeTable()
  else body = journalTable()
  return `<section class="cg cg-content">
    <div class="cg-head">
      <div class="cg-head-ctx"><ui5-icon name="combine"></ui5-icon><span>${esc(ctx || '合并报表工作台')}</span></div>
      <div class="cg-tabs">${tabRow}</div>
    </div>
    <div class="cg-body">${body}</div>
  </section>`
}

function accName (code) {
  const a = state.accounts[code]
  return a && a.name ? a.name : ''
}

function worksheetTable () {
  if (!state.worksheet.length) {
    return `<cmx-empty-state icon="table-chart" title="暂无工作底稿" description="选择方案/期间/节点后点「运行合并」" size="md"></cmx-empty-state>`
  }
  const rows = state.worksheet.map((r) => {
    const nm = accName(r.account_code)
    return `<tr>
      <td class="cg-acc"><b>${esc(r.account_code)}</b>${nm ? `<span class="cg-acc-nm">${esc(nm)}</span>` : ''}</td>
      ${amtCell(r.individual)}${amtCell(r.adjust)}${amtCell(r.elim)}${amtCell(r.consolidated, true)}
    </tr>`
  }).join('')
  const t = worksheetTotals()
  const tie = Math.abs(t.con) < 0.005
  return `<div class="cg-table-wrap">
    <table class="cg-table cg-ws">
      <thead><tr>
        <th class="cg-acc-h">科目</th><th class="cg-amt-h">个别数</th><th class="cg-amt-h">调整</th>
        <th class="cg-amt-h">抵销</th><th class="cg-amt-h">合并数</th>
      </tr></thead>
      <tbody>${rows}</tbody>
      <tfoot><tr class="cg-total ${tie ? 'tie' : 'untie'}">
        <td>合计(借方正 ⇒ 应为 0)</td>
        ${amtCell(t.ind)}${amtCell(t.adj)}${amtCell(t.elm)}
        <td class="cg-amt"><span class="cg-tie-badge">${tie ? '✓ 平衡' : '✗ ' + fmt(t.con)}</span></td>
      </tr></tfoot>
    </table>
  </div>`
}

function amtCell (v, strong) {
  const n = num(v)
  const cls = n < 0 ? 'cg-amt neg' : 'cg-amt'
  const zero = Math.abs(n) < 0.005
  return `<td class="${cls}${zero ? ' zero' : ''}${strong ? ' strong' : ''}">${zero ? '–' : fmt(n)}</td>`
}

function reconTable () {
  if (!state.recon.length) {
    return `<cmx-empty-state icon="synchronize" title="暂无对账结果" description="录入双边申报后点「运行对账」" size="md"></cmx-empty-state>`
  }
  const rows = state.recon.map((r) => {
    const st = RECON_BADGE[r.recon_status] || { label: r.recon_status, color: '#64748b' }
    const diffN = num(r.diff)
    return `<tr class="${r.recon_status === 'diff' ? 'row-diff' : ''}">
      <td><b>${esc(r.entity_a)}</b> → <b>${esc(r.entity_b)}</b></td>
      <td class="cg-mid">${esc(ELIM_LABEL[r.ic_type] || r.ic_type)}</td>
      ${amtCell(r.a_amount)}${amtCell(r.b_amount)}${amtCell(r.matched, true)}
      <td class="cg-amt ${Math.abs(diffN) > 0.005 ? 'neg' : 'zero'}">${Math.abs(diffN) < 0.005 ? '–' : fmt(diffN)}</td>
      <td class="cg-mid"><span class="cg-badge" style="--c:${st.color}">${esc(st.label)}</span></td>
    </tr>`
  }).join('')
  return `<div class="cg-table-wrap">
    <table class="cg-table">
      <thead><tr>
        <th>往来对(债权方→债务方)</th><th class="cg-mid-h">类型</th>
        <th class="cg-amt-h">A侧申报</th><th class="cg-amt-h">B侧申报</th>
        <th class="cg-amt-h">匹配额</th><th class="cg-amt-h">差异</th><th class="cg-mid-h">状态</th>
      </tr></thead>
      <tbody>${rows}</tbody>
    </table>
  </div>`
}

function pct (v) {
  const n = Number(v)
  return isFinite(n) && n !== 0 ? (n * 100).toFixed(1) + '%' : '–'
}

function scopeChangeTable () {
  if (!state.scopeChange.length) {
    return `<cmx-empty-state icon="org-chart" title="暂无范围变动" description="点「范围变动」对比本期与上期合并范围" size="md"></cmx-empty-state>`
  }
  const rows = state.scopeChange.map((r) => {
    const st = SCOPE_BADGE[r.change_type] || { label: r.change_type, color: '#64748b' }
    const pm = r.prev_method ? (METHOD_LABEL[r.prev_method] || r.prev_method) : '–'
    const cm = r.curr_method ? (METHOD_LABEL[r.curr_method] || r.curr_method) : '–'
    const mChanged = r.change_type === 'method_change'
    return `<tr>
      <td class="cg-acc"><b>${esc(r.org_code)}</b><span class="cg-acc-nm">${esc(r.org_name || '')}</span></td>
      <td class="cg-mid"><span class="cg-badge" style="--c:${st.color}">${esc(st.label)}</span></td>
      <td class="cg-mid">${esc(pm)} ${mChanged ? '→ ' + esc(cm) : ''}</td>
      <td class="cg-amt">${pct(r.prev_ownership)}</td>
      <td class="cg-amt strong">${pct(r.curr_ownership)}</td>
    </tr>`
  }).join('')
  const prev = state.scopeChange[0]?.prev_period || ''
  return `<div class="cg-table-wrap">
    <table class="cg-table">
      <thead><tr>
        <th class="cg-acc-h">主体</th><th class="cg-mid-h">变动</th><th class="cg-mid-h">合并方法</th>
        <th class="cg-amt-h">上期持股(${esc(prev)})</th><th class="cg-amt-h">本期持股</th>
      </tr></thead>
      <tbody>${rows}</tbody>
    </table>
  </div>`
}

function journalTable () {
  if (!state.journal.length) {
    return `<cmx-empty-state icon="journey-arrive" title="暂无抵销/调整凭证" description="运行合并后在此查看合并分类账" size="md"></cmx-empty-state>`
  }
  let lastDoc = ''
  const rows = state.journal.map((e) => {
    const firstOfDoc = e.doc_no !== lastDoc
    lastDoc = e.doc_no
    const opening = Number(e.is_opening) === 1
    return `<tr class="${firstOfDoc ? 'doc-start' : ''}">
      <td class="cg-doc">${firstOfDoc ? esc(e.doc_no) : ''}</td>
      <td class="cg-mid">${firstOfDoc ? `<span class="cg-badge" style="--c:#5b6b7f">${esc(ELIM_LABEL[e.elim_type] || e.elim_type)}</span>${opening ? '<span class="cg-open">期初</span>' : ''}` : ''}</td>
      <td class="cg-acc"><b>${esc(e.account_code)}</b>${accName(e.account_code) ? `<span class="cg-acc-nm">${esc(accName(e.account_code))}</span>` : ''}</td>
      ${amtCell(e.dr)}${amtCell(e.cr)}
      <td class="cg-mid">${esc(e.partner || '')}</td>
      <td class="cg-mid cg-rule">${esc(e.source_rule || '')}</td>
    </tr>`
  }).join('')
  return `<div class="cg-table-wrap">
    <table class="cg-table cg-journal">
      <thead><tr>
        <th class="cg-doc-h">凭证号</th><th class="cg-mid-h">类型</th><th class="cg-acc-h">科目</th>
        <th class="cg-amt-h">借方</th><th class="cg-amt-h">贷方</th><th class="cg-mid-h">对手</th><th class="cg-mid-h">规则</th>
      </tr></thead>
      <tbody>${rows}</tbody>
    </table>
  </div>`
}

// ============================================================================
// property:节点信息 + 平衡校验
// ============================================================================

function propertyHtml () {
  const n = selectedNodeRow()
  if (!n) {
    return `<section class="cg cg-prop"><cmx-empty-state icon="detail-view" title="未选中节点" size="sm"></cmx-empty-state></section>`
  }
  const method = String(n.consol_method || '').toLowerCase()
  const t = worksheetTotals()
  const tie = Math.abs(t.con) < 0.005
  const rowsN = state.worksheet.length
  const leaf = Number(n.is_leaf) === 1
  const kv = (label, val, extra = '') => `<div class="cg-kv"><span class="k">${esc(label)}</span><span class="v ${extra}">${val}</span></div>`
  return `<section class="cg cg-prop">
    <div class="cg-prop-head">
      <ui5-icon name="${leaf ? 'building' : 'company-view'}"></ui5-icon>
      <div><div class="cg-prop-name">${esc(n.org_name || n.org_code)}</div>
      <div class="cg-prop-sub">${esc(n.org_code)}</div></div>
    </div>
    <div class="cg-prop-sec">合并属性</div>
    ${kv('节点类型', leaf ? '<span class="cg-badge" style="--c:#5b6b7f">叶子主体</span>' : '<span class="cg-badge" style="--c:#1e88e5">合并节点</span>')}
    ${kv('合并方法', `<span class="cg-badge" style="--c:${METHOD_COLOR[method] || '#64748b'}">${esc(METHOD_LABEL[method] || method || '—')}</span>`)}
    ${kv('持股比例', n.ownership_pct != null ? (num(n.ownership_pct) * 100).toFixed(1) + '%' : '—')}
    ${kv('功能币种', esc(n.currency || '（随集团）'))}
    ${kv('层级', esc(n.level_no ?? '—'))}
    ${kv('投资额', num(n.investment_amount) ? fmt(n.investment_amount) : '—')}
    <div class="cg-prop-sec">合并平衡校验</div>
    <div class="cg-check ${tie ? 'ok' : 'bad'}">
      <ui5-icon name="${tie ? 'sys-enter-2' : 'error'}"></ui5-icon>
      <div>
        <div class="cg-check-title">${tie ? '借贷平衡' : '借贷不平衡'}</div>
        <div class="cg-check-sub">全 ${rowsN} 科目合并数合计 = ${fmt(t.con)}${tie ? '（借方正下应为 0）' : ''}</div>
      </div>
    </div>
    ${kv('个别数合计', fmt(t.ind))}
    ${kv('调整合计', fmt(t.adj))}
    ${kv('抵销合计', fmt(t.elm))}
    ${kv('合并数合计', fmt(t.con), tie ? 'good' : 'warn')}
  </section>`
}

// ============================================================================
// 事件绑定
// ============================================================================

function bind (root, view) {
  root.querySelector('[data-scheme-select]')?.addEventListener('change', (ev) => {
    loadScheme(ev.target.value || '')
  })
  root.querySelector('[data-period-select]')?.addEventListener('change', (ev) => {
    loadPeriod(ev.target.value || '')
  })
  root.querySelectorAll('[data-node-expand]').forEach((el) => el.addEventListener('click', (ev) => {
    ev.stopPropagation()
    const code = el.getAttribute('data-node-expand')
    if (state.nodeExpanded.has(code)) state.nodeExpanded.delete(code)
    else state.nodeExpanded.add(code)
    refreshAll()
  }))
  root.querySelectorAll('[data-node]').forEach((el) => el.addEventListener('click', (ev) => {
    if (ev.target.closest('[data-node-expand]')) return
    state.selectedNode = el.getAttribute('data-node') || ''
    loadNode()
  }))
  root.querySelectorAll('[data-tab]').forEach((btn) => btn.addEventListener('click', () => {
    state.tab = btn.getAttribute('data-tab') || 'worksheet'
    refreshAll()
  }))
  root.querySelector('[data-act="run"]')?.addEventListener('click', () => runConsolidation())
  root.querySelector('[data-act="reconcile"]')?.addEventListener('click', () => runReconcile())
  root.querySelector('[data-act="scope-change"]')?.addEventListener('click', () => runScopeChange())
}

// ============================================================================
// 样式(锚定 UI5 --sap* 令牌,穿透明暗主题;裸 hex 兜底)
// ============================================================================

function styleCss () {
  return `
  :host, .native-page-root { height: 100%; }
  .cg { box-sizing: border-box; font-family: var(--sapFontFamily, "72", system-ui, sans-serif);
    color: var(--sapTextColor, #1c2b36); font-size: 13px; height: 100%; display: flex; flex-direction: column; }
  .cg * { box-sizing: border-box; }
  .cg ui5-icon { color: currentColor; }

  /* explorer */
  .cg-explorer { padding: 12px; gap: 12px; overflow: auto; }
  .cg-field { display: flex; flex-direction: column; gap: 4px; }
  .cg-lbl { display: flex; align-items: center; gap: 6px; font-size: 12px; font-weight: 600;
    color: var(--sapContent_LabelColor, #5b6b7f); }
  .cg-lbl ui5-icon { width: 14px; height: 14px; }
  .cg-select-wrap { position: relative; }
  .cg-select-wrap select { width: 100%; appearance: none; padding: 7px 28px 7px 10px;
    border: 1px solid var(--sapField_BorderColor, #b3c2cf); border-radius: 8px;
    background: var(--sapField_Background, #fff); color: var(--sapField_TextColor, #1c2b36);
    font-size: 13px; cursor: pointer; }
  .cg-caret { position: absolute; right: 8px; top: 50%; transform: translateY(-50%);
    width: 14px; height: 14px; pointer-events: none; color: var(--sapContent_LabelColor, #5b6b7f); }
  .cg-actions { display: flex; gap: 8px; }
  .cg-btn { flex: 1; display: inline-flex; align-items: center; justify-content: center; gap: 6px;
    padding: 8px 10px; border-radius: 8px; border: 1px solid var(--sapButton_BorderColor, #b3c2cf);
    background: var(--sapButton_Background, #fff); color: var(--sapButton_TextColor, #0a6ed1);
    font-size: 13px; font-weight: 600; cursor: pointer; transition: filter .15s; }
  .cg-btn:hover { filter: brightness(0.97); }
  .cg-btn[disabled] { opacity: .55; cursor: default; }
  .cg-btn ui5-icon { width: 15px; height: 15px; }
  .cg-btn-primary { background: var(--sapButton_Emphasized_Background, #0a6ed1);
    color: var(--sapButton_Emphasized_TextColor, #fff); border-color: transparent; }

  .cg-tree-title { display: flex; align-items: center; gap: 6px; font-size: 12px; font-weight: 700;
    color: var(--sapContent_LabelColor, #5b6b7f); margin-top: 4px; padding-bottom: 4px;
    border-bottom: 1px solid var(--sapList_BorderColor, #e4e9ee); }
  .cg-tree-title ui5-icon { width: 14px; height: 14px; }
  .cg-tree-count { margin-left: auto; background: var(--sapObjectHeader_Background, #eef2f6);
    border-radius: 10px; padding: 1px 8px; font-size: 11px; }
  .cg-tree { display: flex; flex-direction: column; }
  .cg-node { display: flex; align-items: center; gap: 6px; padding: 6px 8px; border-radius: 6px;
    cursor: pointer; }
  .cg-node:hover { background: var(--sapList_Hover_Background, #f2f6fa); }
  .cg-node.active { background: var(--sapList_SelectionBackgroundColor, #e0eefb); }
  .cg-caret2 { width: 14px; height: 14px; cursor: pointer; color: var(--sapContent_LabelColor, #5b6b7f); }
  .cg-caret-empty { width: 14px; display: inline-block; }
  .cg-node-ic { width: 15px; height: 15px; color: var(--sapContent_NonInteractiveIconColor, #64748b); }
  .cg-node-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cg-node-badge, .cg-badge { font-size: 10.5px; font-weight: 700; color: #fff; background: var(--c, #64748b);
    border-radius: 5px; padding: 1px 6px; white-space: nowrap; }
  .cg-open { font-size: 10px; font-weight: 700; color: #fff; background: #fb8c00; border-radius: 4px;
    padding: 0 5px; margin-left: 5px; }

  /* content */
  .cg-content { overflow: hidden; }
  .cg-head { padding: 10px 14px 0; border-bottom: 1px solid var(--sapList_BorderColor, #e4e9ee); }
  .cg-head-ctx { display: flex; align-items: center; gap: 8px; font-size: 14px; font-weight: 700;
    margin-bottom: 8px; }
  .cg-head-ctx ui5-icon { width: 18px; height: 18px; color: var(--sapContent_IconColor, #0a6ed1); }
  .cg-tabs { display: flex; gap: 4px; }
  .cg-tab { display: inline-flex; align-items: center; gap: 6px; padding: 8px 14px; border: none;
    background: transparent; color: var(--sapContent_LabelColor, #5b6b7f); font-size: 13px;
    font-weight: 600; cursor: pointer; border-bottom: 2px solid transparent; }
  .cg-tab ui5-icon { width: 15px; height: 15px; }
  .cg-tab:hover { color: var(--sapTextColor, #1c2b36); }
  .cg-tab.active { color: var(--sapSelectedColor, #0a6ed1); border-bottom-color: var(--sapSelectedColor, #0a6ed1); }
  .cg-body { flex: 1; overflow: auto; padding: 14px; }
  .cg-table-wrap { border: 1px solid var(--sapList_BorderColor, #e4e9ee); border-radius: 10px; overflow: hidden; }
  .cg-table { width: 100%; border-collapse: collapse; font-size: 12.5px; }
  .cg-table thead th { position: sticky; top: 0; background: var(--sapList_HeaderBackground, #f4f7fa);
    color: var(--sapContent_LabelColor, #5b6b7f); font-weight: 700; text-align: right;
    padding: 9px 12px; border-bottom: 1px solid var(--sapList_BorderColor, #e4e9ee); white-space: nowrap; }
  .cg-table th.cg-acc-h, .cg-table th.cg-mid-h, .cg-table th.cg-doc-h { text-align: left; }
  .cg-table tbody td { padding: 7px 12px; border-bottom: 1px solid var(--sapList_BorderColor, #eef2f6); }
  .cg-table tbody tr:hover { background: var(--sapList_Hover_Background, #f7fafc); }
  .cg-acc { white-space: nowrap; }
  .cg-acc b { font-variant-numeric: tabular-nums; }
  .cg-acc-nm { color: var(--sapContent_LabelColor, #5b6b7f); margin-left: 8px; }
  .cg-amt { text-align: right; font-variant-numeric: tabular-nums; white-space: nowrap; }
  .cg-amt.neg { color: var(--sapNegativeColor, #d32030); }
  .cg-amt.zero { color: var(--sapContent_DisabledTextColor, #b0bcc7); }
  .cg-amt.strong { font-weight: 700; }
  .cg-mid { text-align: center; }
  .cg-rule { font-size: 11px; color: var(--sapContent_LabelColor, #5b6b7f); font-family: monospace; }
  .cg-ws tfoot .cg-total td { padding: 9px 12px; font-weight: 700; border-top: 2px solid var(--sapList_BorderColor, #cfd8e0);
    background: var(--sapList_HeaderBackground, #f4f7fa); }
  .cg-tie-badge { font-weight: 700; }
  .cg-total.tie .cg-tie-badge { color: var(--sapPositiveColor, #107e3e); }
  .cg-total.untie .cg-tie-badge { color: var(--sapNegativeColor, #d32030); }
  .cg-journal .doc-start td { border-top: 1px solid var(--sapList_BorderColor, #d6dee6); }
  .cg-doc { font-weight: 700; font-family: monospace; font-size: 11.5px; white-space: nowrap; }
  .row-diff { background: color-mix(in srgb, var(--sapNegativeColor, #d32030) 8%, transparent); }

  /* property */
  .cg-prop { padding: 14px; gap: 2px; overflow: auto; }
  .cg-prop-head { display: flex; align-items: center; gap: 10px; padding-bottom: 12px;
    border-bottom: 1px solid var(--sapList_BorderColor, #e4e9ee); margin-bottom: 10px; }
  .cg-prop-head ui5-icon { width: 26px; height: 26px; color: var(--sapContent_IconColor, #0a6ed1); }
  .cg-prop-name { font-size: 15px; font-weight: 700; }
  .cg-prop-sub { font-size: 12px; color: var(--sapContent_LabelColor, #5b6b7f); font-family: monospace; }
  .cg-prop-sec { font-size: 11px; font-weight: 800; letter-spacing: .04em; text-transform: uppercase;
    color: var(--sapContent_LabelColor, #5b6b7f); margin: 14px 0 6px; }
  .cg-kv { display: flex; align-items: center; justify-content: space-between; gap: 12px;
    padding: 6px 0; border-bottom: 1px solid var(--sapList_BorderColor, #f0f3f6); }
  .cg-kv .k { color: var(--sapContent_LabelColor, #5b6b7f); }
  .cg-kv .v { font-weight: 600; text-align: right; font-variant-numeric: tabular-nums; }
  .cg-kv .v.good { color: var(--sapPositiveColor, #107e3e); }
  .cg-kv .v.warn { color: var(--sapNegativeColor, #d32030); }
  .cg-check { display: flex; align-items: center; gap: 10px; padding: 12px; border-radius: 10px; margin: 4px 0 6px; }
  .cg-check ui5-icon { width: 22px; height: 22px; }
  .cg-check.ok { background: color-mix(in srgb, var(--sapPositiveColor, #107e3e) 10%, transparent); color: var(--sapPositiveColor, #107e3e); }
  .cg-check.bad { background: color-mix(in srgb, var(--sapNegativeColor, #d32030) 10%, transparent); color: var(--sapNegativeColor, #d32030); }
  .cg-check-title { font-weight: 700; font-size: 13px; }
  .cg-check-sub { font-size: 11.5px; opacity: .85; color: var(--sapTextColor, #1c2b36); }

  /* toast */
  .cg-toast { position: absolute; left: 50%; bottom: 18px; transform: translateX(-50%) translateY(8px);
    background: var(--sapButton_Emphasized_Background, #0a6ed1); color: #fff; padding: 9px 16px;
    border-radius: 8px; font-size: 13px; font-weight: 600; opacity: 0; pointer-events: none;
    transition: opacity .2s, transform .2s; box-shadow: 0 6px 20px rgba(0,0,0,.18); z-index: 20; max-width: 80%; }
  .cg-toast.show { opacity: 1; transform: translateX(-50%) translateY(0); }
  .cg-toast[data-kind="error"] { background: var(--sapNegativeColor, #d32030); }
  .cg-toast[data-kind="warn"] { background: var(--sapCriticalColor, #e9730c); }
  .cg-toast[data-kind="ok"] { background: var(--sapPositiveColor, #107e3e); }
  `
}

// native_pages 契约:导出 default { defaultView, views:{ view: async ctx => mount(ctx, view) } }。
export default {
  defaultView: 'content',
  views: {
    async explorer (ctx) { return mount(ctx, 'explorer') },
    async content (ctx) { return mount(ctx, 'content') },
    async property (ctx) { return mount(ctx, 'property') },
  },
}
