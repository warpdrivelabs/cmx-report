/**
 * 报表应用工作台 —— native_pages 三栏工作台（数据消费侧，对标报表设计工作台）。
 *
 * explorer：顶部会计期间下拉（cr_acct_calendar 年度→月度）+ 下方合并组织树（cr_consol_org）。
 *           点击组织节点 → property 区激活「组织机构」详情页。
 * content ：顶部报表类别按钮行（图标+文字）+ 报表列表（同设计工作台，卡片不变）。
 * property：报表属性（不变）+ 组织机构（新增，展示组织全字段）。
 *
 * 打开报表 → portal.rpt.report-applier（新应用器页），传入 org+period 上下文驱动取数/存数。
 * 数据源：fico-db 的 cr_report_* / cr_acct_calendar / cr_consol_org 物理表。
 */

const state = {
  categories: [],
  periods: [], // cr_acct_calendar 行（年度 level_no=1 + 月度 is_leaf=1）
  reports: [],
  details: {},
  orgs: [], // cr_consol_org 扁平行
  orgTree: [], // buildOrgTree(orgs) 结果
  orgExpanded: new Set(), // 展开的组织节点 id（String）
  orgLoading: false, // 组织树刷新中
  periodTypes: [], // cr_period_type（日报/周报…），content 区 tab
  selectedPeriodType: '', // 选中期间类型（报表列表 tab 过滤）
  categoryOpen: false, // content 标题区类别下拉展开态
  selectedCategory: '',
  selectedPeriod: '', // 会计期间叶子 code，如 '2026-07'（explorer 下拉，数据上下文）
  selectedOrg: '', // 选中组织 code
  selectedCode: '', // 当前高亮报表（单选，用于卡片高亮/属性联动）
  selectedCodes: new Set(), // 多选报表（计算/校验批量操作对象）
  selectedVersion: {},
  query: '',
  loading: false,
  message: '',
  hosts: new Set(),
}

const CATEGORY_ICONS = {
  statutory: 'official-service',
  management: 'business-objects-experience',
  internal: 'dimension',
  tax: 'receipt',
  consolidation: 'combine',
  consol: 'combine',
}

const PERIOD_TYPE_ICONS = {
  day: 'calendar',
  week: 'calendar',
  month: 'calendar',
  quarter: 'business-card',
  halfyear: 'business-card',
  year: 'appointment-2',
  adhoc: 'document',
}

const RAINBOW = ['#e53935', '#fb8c00', '#43a047', '#00acc1', '#1e88e5', '#8e24aa', '#d98200']
const UNKNOWN_COLOR = '#64748b'

const esc = (s) => String(s ?? '')
  .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
  .replace(/"/g, '&quot;').replace(/'/g, '&#39;')

const enc = encodeURIComponent

// 组织树视图 id —— 必须与 report-menu.json 的第二个 property view id 完全一致（激活联动靠它）。
const PROP_ORG_VIEW_ID = 'rpt-app-workbench-prop-org'

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

function normalizeArray (v) {
  if (Array.isArray(v)) return v
  if (typeof v === 'string') {
    const s = v.trim()
    if (!s) return []
    try {
      const parsed = JSON.parse(s)
      return Array.isArray(parsed) ? parsed : []
    } catch {
      return []
    }
  }
  return []
}

function categoryLabel (code) {
  return state.categories.find((c) => c.code === code)?.name || code || '未分类'
}

function categoryColor (code) {
  const idx = state.categories.findIndex((c) => c.code === code)
  return idx >= 0 ? RAINBOW[idx % RAINBOW.length] : UNKNOWN_COLOR
}

function periodTypeLabel (code) {
  return state.periodTypes.find((p) => p.code === code)?.name || code || '未分期间'
}

function periodTypeIcon (code) {
  return state.periodTypes.find((p) => p.code === code)?.icon || PERIOD_TYPE_ICONS[code] || 'calendar'
}

function periodTypeColor (code) {
  const idx = state.periodTypes.findIndex((p) => p.code === code)
  return idx >= 0 ? RAINBOW[idx % RAINBOW.length] : UNKNOWN_COLOR
}

function reportVersion (r) {
  const versions = normalizeArray(r.versions)
  return state.selectedVersion[r.code] || r.current_version_code || versions[0]?.code || ''
}

function versionLabel (code) {
  return code || '默认版本'
}

function slug (s) {
  return String(s || 'default').trim().replace(/[^A-Za-z0-9_-]+/g, '_') || 'default'
}

function applierProps (report, version) {
  return {
    reportCode: report?.code || '',
    reportName: report?.name || '',
    version: version || '',
    orgCode: state.selectedOrg || '',
    periodCode: state.selectedPeriod || '',
  }
}

function applierTabLabel (report) {
  const code = String(report?.code || '').trim()
  const name = String(report?.name || '').trim()
  const base = name ? `${code}-${name}` : code || '报表'
  const ctx = [state.selectedOrg, state.selectedPeriod].filter(Boolean).join('/')
  return ctx ? `${base}｜${ctx}` : base
}

function applierView (id, view, tabLabel, icon, props) {
  return {
    id,
    tabLabel,
    icon,
    type: 'native_pages',
    native_page: 'portal.rpt.report-applier',
    view,
    props,
  }
}

function dispatchPortalAction (sourceEl, detail) {
  const ev = new CustomEvent('portal-help-action', { detail, bubbles: true, composed: true })
  try {
    if (sourceEl?.dispatchEvent) sourceEl.dispatchEvent(ev)
    else document.dispatchEvent(ev)
    return true
  } catch {
    try {
      document.dispatchEvent(new CustomEvent('portal-help-action', { detail, bubbles: true, composed: true }))
      return true
    } catch {}
  }
  return false
}

async function openWorkNode (workNode, sourceEl) {
  const candidates = [window, window.parent, window.top, globalThis].filter(Boolean)
  for (const target of candidates) {
    try {
      if (typeof target.openTab === 'function') { target.openTab(workNode); return true }
      if (typeof target.openWorkspaceNode === 'function') { target.openWorkspaceNode(workNode); return true }
    } catch {}
  }
  const inlineDetail = { kind: 'inlineNode', node: workNode, icon: workNode.icon || 'workflow-tasks', title: workNode.caption || workNode.name || workNode.id }
  if (dispatchPortalAction(sourceEl, inlineDetail)) return true
  try {
    await apiJson('/api/workspace-nodes', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        id: workNode.id,
        name: workNode.caption || workNode.menuName || workNode.name || workNode.id,
        icon: workNode.icon || 'table-chart',
        details: `报表应用器工作区：${workNode.id}`,
        workspace: workNode.workspace,
      }),
    })
    dispatchPortalAction(sourceEl, { kind: 'node', id: workNode.id, icon: workNode.icon || 'create', title: workNode.caption || workNode.name || workNode.id })
    return true
  } catch {}
  try {
    window.parent?.postMessage({ type: 'openTab', payload: workNode }, '*')
    window.top?.postMessage({ type: 'openTab', payload: workNode }, '*')
  } catch {}
  try {
    document.dispatchEvent(new CustomEvent('cmx-open-workspace-node', { detail: { workNode, menu: workNode }, bubbles: true, composed: true }))
  } catch {}
  return true
}

async function openReportApplier (code, sourceEl) {
  const report = state.reports.find((r) => r.code === code)
  if (!report) return
  const version = reportVersion(report) || ''
  const props = applierProps(report, version)
  const sid = `${slug(report.code)}-${slug(version || 'default')}-${slug(state.selectedOrg || 'org')}-${slug(state.selectedPeriod || 'period')}`
  const title = `报表应用-${report.name || report.code}`
  const sheetTitle = applierTabLabel(report)
  const menu = {
    id: `rpt-applier-${sid}`,
    name: `rpt-applier-${sid}`,
    menuName: title,
    caption: title,
    type: 'workspace-node',
    icon: 'table-chart',
    openType: 0,
    status: 1,
    workspace: {
      id: `rpt_applier_${sid}`,
      params: props,
      explorerWidth: 280,
      propertyWidth: 340,
      model: {
        id: `rpt-applier-${sid}-model`,
        type: 'native_pages',
        native_page: 'portal.rpt.report-applier',
        view: 'content',
        props,
      },
      explorer: {
        caption: '期间与组织',
        icon: 'calendar',
        views: [
          applierView(`rpt-applier-${sid}-explorer`, 'explorer', '期间/组织', 'calendar', props),
        ],
      },
      content: {
        caption: sheetTitle,
        icon: 'table-chart',
        views: [
          applierView(`rpt-applier-${sid}-sheet`, 'content', sheetTitle, 'table-chart', props),
        ],
      },
      property: {
        caption: '属性',
        icon: 'detail-view',
        views: [
          applierView(`rpt-applier-${sid}-prop`, 'property', '报表属性', 'detail-view', props),
          applierView(`rpt-applier-${sid}-status`, 'propertyStatus', '数据状态', 'status-positive', props),
        ],
      },
    },
  }
  await openWorkNode(menu, sourceEl)
}

function selectedReport () {
  return state.reports.find((r) => r.code === state.selectedCode) || null
}

function detailKey (code, version) {
  return `${code || ''}@@${version || ''}`
}

function selectedDetail () {
  const r = selectedReport()
  if (!r) return null
  return state.details[detailKey(r.code, reportVersion(r))] || null
}

// 报表列表过滤：类别 + 期间类型 tab（cr_period_type，如 day/week）+ 关键字。
// 注意：会计期间(state.selectedPeriod=cr_acct_calendar code，如 2026-07)只作数据上下文，不过滤列表。
function filteredReports () {
  const q = state.query.trim().toLowerCase()
  return state.reports.filter((r) => {
    if (state.selectedCategory && r.report_category !== state.selectedCategory) return false
    if (state.selectedPeriodType && r.period_type !== state.selectedPeriodType) return false
    if (!q) return true
    const hay = [
      r.code, r.name, r.report_type, r.report_category, r.period_type,
      r.currency_code, r.entity_scope, r.data_source, r.remark,
    ].join(' ').toLowerCase()
    return q.split(/\s+/).filter(Boolean).every((x) => hay.includes(x))
  })
}

// 类别维度 + 期间类型维度（cr_period_type，content 区 tab）。
function rebuildCategories (raw) {
  const cats = normalizeArray(raw.categories).map((c) => ({
    code: c.code,
    name: c.name || c.code,
    remark: c.remark,
    sort_no: Number(c.sort_no ?? 999999),
    icon: CATEGORY_ICONS[c.code] || 'folder',
  })).filter((c) => c.code)
  const catMap = new Map(cats.map((c) => [c.code, { ...c, count: 0 }]))
  for (const r of state.reports) {
    const code = r.report_category || 'other'
    if (!catMap.has(code)) {
      catMap.set(code, { code, name: code, sort_no: 999999, icon: CATEGORY_ICONS[code] || 'folder', count: 0 })
    }
    catMap.get(code).count += 1
  }
  state.categories = [...catMap.values()].sort((a, b) => (a.sort_no - b.sort_no) || String(a.name).localeCompare(String(b.name), 'zh-CN'))
}

// 期间类型（cr_period_type：日报/周报/月报…）——content 区报表列表 tab。
function rebuildPeriodTypes (raw) {
  const rows = normalizeArray(raw.periods).map((p) => ({
    code: p.code,
    name: p.name || p.code,
    sort_no: Number(p.sort_no ?? 999999),
    icon: PERIOD_TYPE_ICONS[p.code] || 'calendar',
  })).filter((p) => p.code)
  const map = new Map(rows.map((p) => [p.code, p]))
  // 补齐报表里出现但字典未列的期间类型
  for (const r of state.reports) {
    const code = r.period_type || ''
    if (code && !map.has(code)) map.set(code, { code, name: code, sort_no: 999999, icon: PERIOD_TYPE_ICONS[code] || 'calendar' })
  }
  state.periodTypes = [...map.values()].sort((a, b) => (a.sort_no - b.sort_no) || String(a.name).localeCompare(String(b.name), 'zh-CN'))
}

// 默认会计期间：状态 open 的最后一个叶子月（否则最后一个叶子）。
function defaultPeriodLeaf (periods) {
  const leaves = periods.filter((p) => Number(p.is_leaf) === 1)
  const open = leaves.filter((p) => p.period_status === 'open')
  const pick = (open.length ? open : leaves).slice(-1)[0]
  return pick ? String(pick.code) : ''
}

function buildOrgTree (rows) {
  const byId = new Map(rows.map((r) => [String(r.id), { ...r, children: [] }]))
  const roots = []
  for (const r of rows) {
    const node = byId.get(String(r.id))
    const pid = r.parent_id == null ? null : String(r.parent_id)
    if (pid && byId.has(pid)) byId.get(pid).children.push(node)
    else roots.push(node)
  }
  return roots
}

function selectedOrgRow () {
  return state.orgs.find((o) => String(o.code) === String(state.selectedOrg)) || null
}

async function loadData (force = false) {
  if (state.loading) return
  if (!force && state.reports.length) return
  state.loading = true
  state.message = ''
  refreshAll()
  try {
    const [overview, cal, org] = await Promise.all([
      apiJson('/api/report-design/overview'),
      apiJson('/api/report-design/calendar'),
      apiJson('/api/report-design/consol-org'),
    ])
    state.reports = normalizeArray(overview.reports)
    rebuildCategories(overview)
    rebuildPeriodTypes(overview)
    state.periods = normalizeArray(cal.periods)
    state.orgs = normalizeArray(org.orgs)
    state.orgTree = buildOrgTree(state.orgs)
    if (!state.selectedCategory && state.categories[0]) state.selectedCategory = state.categories[0].code
    if (!state.selectedPeriodType && state.periodTypes[0]) state.selectedPeriodType = state.periodTypes[0].code
    if (!state.selectedPeriod) state.selectedPeriod = defaultPeriodLeaf(state.periods)
    if (!state.selectedOrg && state.orgs[0]) state.selectedOrg = String(state.orgs[0].code)
    // 默认展开根组织
    state.orgTree.forEach((r) => state.orgExpanded.add(String(r.id)))
    const first = filteredReports()[0] || state.reports[0]
    if (!state.selectedCode && first) state.selectedCode = first.code
    const chosen = selectedReport()
    if (chosen) await ensureDetail(chosen.code, reportVersion(chosen), false)
  } catch (err) {
    state.message = '报表应用工作台加载失败：' + (err.message || err)
  } finally {
    state.loading = false
    refreshAll()
  }
}

// 仅刷新合并组织树（explorer 组织标题栏刷新按钮）。
async function refreshOrgTree () {
  if (state.orgLoading) return
  state.orgLoading = true
  refreshAll()
  try {
    const org = await apiJson('/api/report-design/consol-org')
    state.orgs = normalizeArray(org.orgs)
    state.orgTree = buildOrgTree(state.orgs)
    state.orgTree.forEach((r) => state.orgExpanded.add(String(r.id)))
    if (state.selectedOrg && !state.orgs.some((o) => String(o.code) === String(state.selectedOrg))) {
      state.selectedOrg = state.orgs[0] ? String(state.orgs[0].code) : ''
    }
  } catch (err) {
    state.message = '组织架构刷新失败：' + (err.message || err)
  } finally {
    state.orgLoading = false
    refreshAll()
  }
}

async function ensureDetail (code, version, shouldRefresh = true) {
  if (!code) return
  const key = detailKey(code, version)
  if (state.details[key]) return
  try {
    const url = `/api/report-design/reports/${enc(code)}${version ? `?version=${enc(version)}` : ''}`
    state.details[key] = await apiJson(url)
    if (shouldRefresh) refreshAll()
  } catch (err) {
    state.message = '报表详情加载失败：' + (err.message || err)
    if (shouldRefresh) refreshAll()
  }
}

function mount (ctx, view) {
  const host = ctx.host
  state.hosts.add(host)
  if (host) host.__rptAppView = view
  const render = () => {
    const root = host?.renderRoot || host?.shadowRoot?.querySelector('.native-page-root')
    if (!root || !root.isConnected) return
    root.innerHTML = `<style>${styleCss()}</style>${viewHtml(view)}`
    bind(root, view)
  }
  requestAnimationFrame(() => { render(); loadData(false) })
  return `<style>${styleCss()}</style>${viewHtml(view)}`
}

function refreshAll () {
  for (const host of Array.from(state.hosts)) {
    if (!host || !host.isConnected) { state.hosts.delete(host); continue }
    const root = host.renderRoot || host.shadowRoot?.querySelector('.native-page-root')
    if (!root) continue
    const view = host.__rptAppView || host.getAttribute?.('view') || 'content'
    root.innerHTML = `<style>${styleCss()}</style>${viewHtml(view)}`
    bind(root, view)
  }
}

// 轻量 toast（占位操作/操作反馈用）。延后一帧挂载，避免被紧随的 refreshAll 重渲抹掉。
function toast (message, kind = 'info') {
  requestAnimationFrame(() => {
    let host = null
    for (const h of Array.from(state.hosts)) {
      if (!h || !h.isConnected) continue
      const root = h.renderRoot || h.shadowRoot?.querySelector('.native-page-root')
      const sec = root?.querySelector?.('.rpt-content')
      if (sec) { host = sec; break }
    }
    if (!host) return
    if (getComputedStyle(host).position === 'static') host.style.position = 'relative'
    let box = host.querySelector(':scope > .rpt-toast')
    if (!box) { box = document.createElement('div'); box.className = 'rpt-toast'; host.appendChild(box) }
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
  if (view === 'propertyOrg') return propertyOrgHtml()
  return contentHtml()
}

// ============================================================================
// explorer：期间下拉 + 组织树
// ============================================================================

function loadingBlock () {
  return `<cmx-empty-state icon="busy" title="加载中..." size="sm"></cmx-empty-state>`
}

function periodSelectHtml () {
  const years = state.periods.filter((p) => Number(p.level_no) === 1)
  const groups = years.map((y) => {
    const months = state.periods.filter((p) => p.parent_code === y.code && Number(p.is_leaf) === 1)
    const opts = months.map((m) =>
      `<option value="${esc(m.code)}" ${state.selectedPeriod === m.code ? 'selected' : ''}>${esc(m.name)}</option>`).join('')
    return `<optgroup label="${esc(y.name)}">${opts}</optgroup>`
  }).join('')
  return `<div class="rpt-period-row">
    <span class="rpt-period-lbl"><ui5-icon name="calendar"></ui5-icon>期间</span>
    <div class="rpt-select-wrap">
      <select data-period-select>${groups || '<option value="">（无期间）</option>'}</select>
      <ui5-icon class="rpt-select-caret" name="slim-arrow-down"></ui5-icon>
    </div>
  </div>`
}

function orgIcon (t) {
  return ({ group: 'company-view', subgroup: 'org-chart', entity: 'building', branch: 'building' })[t] || 'building'
}

function renderOrgNodes (nodes, depth) {
  if (!nodes.length && depth === 0) return `<cmx-empty-state icon="tree" title="暂无合并组织" size="sm"></cmx-empty-state>`
  return nodes.map((n) => {
    const pad = 8 + depth * 14
    const hasKids = n.children.length > 0
    const open = state.orgExpanded.has(String(n.id))
    const active = String(state.selectedOrg) === String(n.code)
    const caret = hasKids
      ? `<ui5-icon class="rpt-org-caret" name="${open ? 'navigation-down-arrow' : 'navigation-right-arrow'}" data-org-expand="${esc(n.id)}"></ui5-icon>`
      : '<span class="rpt-org-caret-empty"></span>'
    const kids = hasKids && open ? `<div class="rpt-org-children">${renderOrgNodes(n.children, depth + 1)}</div>` : ''
    return `<div class="rpt-org-branch">
      <div class="rpt-org-node ${active ? 'active' : ''}" data-org="${esc(n.code)}" style="padding-left:${pad}px" title="${esc(n.remark || n.name)}">
        ${caret}
        <ui5-icon class="rpt-org-ic" name="${orgIcon(n.org_type)}"></ui5-icon>
        <span class="rpt-org-text"><b>${esc(n.name)}</b><small>${esc(n.code)}${n.ownership_pct != null ? ` · ${esc(n.ownership_pct)}%` : ''}</small></span>
      </div>${kids}
    </div>`
  }).join('')
}

function explorerHtml () {
  return `<section class="rpt rpt-explorer">
    ${periodSelectHtml()}
    <div class="rpt-org-head">
      <ui5-icon name="tree"></ui5-icon><span>合并组织架构</span>
      <button class="rpt-icon-btn ${state.orgLoading ? 'spin' : ''}" data-act="refresh-org" title="刷新组织机构"><ui5-icon name="refresh"></ui5-icon></button>
    </div>
    <div class="rpt-org-tree">${(state.loading || state.orgLoading) ? loadingBlock() : renderOrgNodes(state.orgTree, 0)}</div>
  </section>`
}

// ============================================================================
// content：类别下拉 + 期间类型 tab + 报表列表
// ============================================================================

// content 标题区的报表类别下拉（点击展开菜单，非按钮组）。
function categoryDropdownHtml () {
  const cur = state.categories.find((c) => c.code === state.selectedCategory)
  const menu = state.categories.map((c) => {
    const active = state.selectedCategory === c.code
    return `<button class="rpt-cat-item ${active ? 'active' : ''}" data-cat="${esc(c.code)}" style="--cat-color:${esc(categoryColor(c.code))}">
      <ui5-icon name="${esc(c.icon)}"></ui5-icon><span>${esc(c.name)}</span><b>${c.count}</b>
    </button>`
  }).join('') || '<cmx-empty-state icon="folder" title="暂无报表类别" size="sm"></cmx-empty-state>'
  return `<div class="rpt-cat-dd ${state.categoryOpen ? 'open' : ''}" data-cat-dd>
    <button class="rpt-cat-trigger" data-cat-toggle style="--cat-color:${esc(categoryColor(state.selectedCategory))}">
      <ui5-icon name="${esc(cur?.icon || 'folder')}"></ui5-icon>
      <span>${esc(cur?.name || '报表类别')}</span>
      <ui5-icon class="rpt-cat-tri-caret" name="slim-arrow-down"></ui5-icon>
    </button>
    <div class="rpt-cat-menu">${menu}</div>
  </div>`
}

// 期间类型 tab（日报/周报…），参考报表设计工作台。
function periodTabsHtml () {
  const tabs = state.periodTypes.map((p) => {
    const n = state.reports.filter((r) => (!state.selectedCategory || r.report_category === state.selectedCategory) && r.period_type === p.code).length
    return `<button class="rpt-period ${state.selectedPeriodType === p.code ? 'active' : ''}" style="--tab-color:${esc(periodTypeColor(p.code))}" data-period-type="${esc(p.code)}">
      <ui5-icon name="${esc(periodTypeIcon(p.code))}"></ui5-icon><span>${esc(p.name)}</span><b>${n}</b>
    </button>`
  }).join('')
  return `<div class="rpt-periods">${tabs}</div>`
}

// 标题区批量操作按钮（刷新右侧）：计算/校验/校验报告，作用于多选报表。
function batchActionsHtml () {
  const n = state.selectedCodes.size
  const dis = n ? '' : 'disabled'
  const badge = n ? `<b class="rpt-batch-n">${n}</b>` : ''
  return `<span class="rpt-batch">
    <button class="rpt-batch-btn" data-batch="compute" ${dis} title="计算选中的 ${n} 张报表"><ui5-icon name="sum"></ui5-icon><span>计算</span>${badge}</button>
    <button class="rpt-batch-btn" data-batch="validate" ${dis} title="校验选中的 ${n} 张报表"><ui5-icon name="validate"></ui5-icon><span>校验</span></button>
    <button class="rpt-batch-btn" data-batch="check-report" ${dis} title="生成选中 ${n} 张报表的校验报告"><ui5-icon name="document-text"></ui5-icon><span>校验报告</span></button>
  </span>`
}

function contentHtml () {
  const list = filteredReports()
  const org = selectedOrgRow()
  const orgName = org?.name || state.selectedOrg || '未选择组织'
  const periodName = state.periods.find((p) => p.code === state.selectedPeriod)?.name || state.selectedPeriod || '-'
  const tabColor = periodTypeColor(state.selectedPeriodType)
  return `<section class="rpt rpt-content" style="--period-color:${esc(tabColor)}">
    <div class="rpt-head">
      <div class="rpt-title one-line">
        <span class="rpt-title-ic"><ui5-icon name="${orgIcon(org?.org_type)}"></ui5-icon></span>
        <b class="rpt-title-name">${esc(orgName)}</b>
        <span class="rpt-title-code">${esc(state.selectedOrg || '-')}</span>
        <span class="rpt-title-period"><ui5-icon name="calendar"></ui5-icon>${esc(periodName)}</span>
      </div>
      ${categoryDropdownHtml()}
      <div class="rpt-toolbar">
        <input class="rpt-search" data-query placeholder="搜索编码 / 名称 / 口径..." value="${esc(state.query)}">
        <button class="rpt-icon-btn" data-act="refresh" title="刷新"><ui5-icon name="refresh"></ui5-icon></button>
        ${batchActionsHtml()}
      </div>
    </div>
    ${state.message ? `<div class="rpt-msg">${esc(state.message)}</div>` : ''}
    <div class="rpt-main">
      ${periodTabsHtml()}
      <div class="rpt-list">
        ${state.loading ? `<cmx-empty-state icon="busy" title="加载报表主档..." size="sm"></cmx-empty-state>` : (list.length ? list.map(reportCardHtml).join('') : emptyReportsHtml())}
      </div>
    </div>
  </section>`
}

function emptyReportsHtml () {
  return `<cmx-empty-state icon="document" title="当前类别与期间下暂无报表" description="切换类别或期间 tab，或到报表设计工作台新增报表。" size="sm"></cmx-empty-state>`
}

function reportCardHtml (r) {
  const active = state.selectedCode === r.code
  const checked = state.selectedCodes.has(r.code)
  const versions = normalizeArray(r.versions)
  const current = reportVersion(r)
  const versionOptions = versions.length
    ? versions.map((v) => `<option value="${esc(v.code)}" ${v.code === current ? 'selected' : ''}>${esc(v.code)} · ${esc(v.name || v.version_status || '')}</option>`).join('')
    : `<option value="">默认版本</option>`
  return `<article class="rpt-card ${active ? 'active' : ''} ${checked ? 'checked' : ''}" data-code="${esc(r.code)}">
    <label class="rpt-check" title="选择用于批量计算/校验"><input type="checkbox" data-pick="${esc(r.code)}" ${checked ? 'checked' : ''}><span class="rpt-check-box"><ui5-icon name="accept"></ui5-icon></span></label>
    <span class="rpt-card-bar"></span>
    <span class="rpt-card-ic"><ui5-icon name="document-text"></ui5-icon></span>
    <div class="rpt-card-main">
      <div class="rpt-card-title"><b>${esc(r.name)}</b><span>CODE ${esc(r.code)}</span></div>
      <div class="rpt-card-sub">${esc(r.report_type || 'CUSTOM')} · ${esc(categoryLabel(r.report_category))}</div>
      <div class="rpt-card-tags">
        <span><ui5-icon name="factory"></ui5-icon>${esc(r.entity_scope || 'single')}</span>
        <span><ui5-icon name="money-bills"></ui5-icon>${esc(r.currency_code || '-')} / ${esc(r.amount_unit || '-')}</span>
        <span><ui5-icon name="layers"></ui5-icon>${Number(r.version_count || versions.length || 0)} 个版本</span>
      </div>
    </div>
    <div class="rpt-card-actions">
      <select class="rpt-version" data-version="${esc(r.code)}">${versionOptions}</select>
      <button class="rpt-mini" data-act="open" data-code="${esc(r.code)}" title="打开报表"><ui5-icon name="table-view"></ui5-icon></button>
      <button class="rpt-mini" data-act="compute" data-code="${esc(r.code)}" title="计算报表"><ui5-icon name="sum"></ui5-icon></button>
      <button class="rpt-mini" data-act="validate" data-code="${esc(r.code)}" title="校验报表"><ui5-icon name="validate"></ui5-icon></button>
      <button class="rpt-mini" data-act="check-report" data-code="${esc(r.code)}" title="校验报告"><ui5-icon name="document-text"></ui5-icon></button>
    </div>
  </article>`
}

// ============================================================================
// property：报表属性（不变） + 组织机构（新增）
// ============================================================================

function propertyHtml () {
  const r = selectedReport()
  if (!r) {
    return `<section class="rpt rpt-prop"><cmx-empty-state icon="detail-view" title="请选择一张报表" size="sm"></cmx-empty-state></section>`
  }
  const version = reportVersion(r)
  const detail = selectedDetail()
  if (!detail) ensureDetail(r.code, version)
  const versions = normalizeArray(detail?.versions?.length ? detail.versions : r.versions)
  const stats = detail?.stats || {}
  const current = versions.find((v) => v.code === version) || versions[0] || {}
  return `<section class="rpt rpt-prop">
    <div class="rpt-prop-hero">
      <span class="rpt-prop-ic"><ui5-icon name="detail-view"></ui5-icon></span>
      <div><b>${esc(r.name)}</b><span>${esc(r.code)} · ${esc(versionLabel(version))}</span></div>
    </div>
    <div class="rpt-prop-grid">
      ${kv('报表类别', categoryLabel(r.report_category))}
      ${kv('期间类型', r.period_type)}
      ${kv('报表类型', r.report_type)}
      ${kv('编制口径', r.entity_scope)}
      ${kv('币种 / 单位', `${r.currency_code || '-'} / ${r.amount_unit || '-'}`)}
      ${kv('取数来源', r.data_source || '未指定')}
      ${kv('状态', Number(r.status) === 0 ? '停用' : '启用')}
      ${kv('更新时间', r.update_time || '-')}
    </div>
    <div class="rpt-prop-sec">
      <b>当前版本</b>
      <cmx-status-tag tone="info" variant="subtle" size="sm">${esc(versionLabel(current.code || version))}</cmx-status-tag>
      <cmx-status-tag tone="neutral" variant="subtle" size="sm">${esc(current.version_status || 'draft')}</cmx-status-tag>
      ${Number(current.is_current || 0) === 1 ? '<cmx-status-tag tone="success" variant="solid" size="sm">当前生效</cmx-status-tag>' : ''}
      <p>${esc(current.change_summary || current.remark || '暂无版本说明')}</p>
    </div>
    <div class="rpt-prop-sec">
      <b>设计资产</b>
      <div class="rpt-stat-grid">
        ${stat('Sheet', stats.sheet_count)}
        ${stat('区域', stats.region_count)}
        ${stat('行', stats.row_count)}
        ${stat('列', stats.col_count)}
        ${stat('格式', stats.format_count)}
      </div>
    </div>
  </section>`
}

function propertyOrgHtml () {
  const o = selectedOrgRow()
  if (!o) {
    return `<section class="rpt rpt-prop"><cmx-empty-state icon="tree" title="请在左侧选择组织节点" size="sm"></cmx-empty-state></section>`
  }
  return `<section class="rpt rpt-prop">
    <div class="rpt-prop-hero">
      <span class="rpt-prop-ic"><ui5-icon name="${orgIcon(o.org_type)}"></ui5-icon></span>
      <div><b>${esc(o.name)}</b><span>${esc(o.code)} · ${esc(o.org_type || '')}</span></div>
    </div>
    <div class="rpt-prop-grid">
      ${kv('组织编码', o.code)}
      ${kv('组织名称', o.name)}
      ${kv('核算实体', o.entity_code)}
      ${kv('组织类型', o.org_type)}
      ${kv('合并方案', o.consol_scheme)}
      ${kv('合并方法', o.consol_method)}
      ${kv('持股比例', o.ownership_pct != null ? `${o.ownership_pct}%` : '-')}
      ${kv('表决权比例', o.voting_pct != null ? `${o.voting_pct}%` : '-')}
      ${kv('合并币种', o.consol_currency)}
      ${kv('是否母公司', Number(o.is_parent) === 1 ? '是' : '否')}
      ${kv('内部抵消', Number(o.offset_flag) === 1 ? '参与抵消' : '不抵消')}
      ${kv('层级深度', o.level_no)}
      ${kv('是否末级', Number(o.is_leaf) === 1 ? '是' : '否')}
      ${kv('全路径', o.full_path)}
      ${kv('状态', Number(o.status) === 0 ? '停用' : '启用')}
    </div>
    <div class="rpt-prop-sec"><b>备注</b><p>${esc(o.remark || '暂无备注')}</p></div>
  </section>`
}

function kv (label, value) {
  return `<div class="rpt-kv"><span>${esc(label)}</span><b>${esc(value == null || value === '' ? '-' : value)}</b></div>`
}

function stat (label, value) {
  return `<div class="rpt-stat"><b>${Number(value || 0)}</b><span>${esc(label)}</span></div>`
}

// ============================================================================
// 跨区激活：点组织节点 → property 区切到「组织机构」视图（复用 designer.js 同款机制）
// ============================================================================

function collectDeep (root, selector, out = []) {
  if (!root) return out
  try {
    if (root.querySelectorAll) {
      root.querySelectorAll(selector).forEach((el) => out.push(el))
      root.querySelectorAll('*').forEach((el) => {
        if (el.shadowRoot) collectDeep(el.shadowRoot, selector, out)
      })
    }
  } catch {}
  return out
}

function findWorkspaceFromDom (source) {
  let node = source instanceof Element ? source : source?.host
  while (node) {
    if (node.workspace) return node.workspace
    if (node.dataset?.cmxWorkspaceId) {
      const wsId = node.dataset.cmxWorkspaceId
      const ma = globalThis.mainapp
      return ma?.workspaces?.[wsId] || ma?.activityScopes?.[wsId] || null
    }
    node = node.parentElement || (node.parentNode instanceof ShadowRoot ? node.parentNode.host : null)
  }
  const ma = globalThis.mainapp
  const wsId = ma?.activeWorkspaceId
  return wsId ? ma?.workspaces?.[wsId] : null
}

function activateWorkspaceView (workspace, detail) {
  if (!workspace) return false
  const attempts = [
    ['activateView', [{ region: detail.area, area: detail.area, view: detail.view, viewId: detail.viewId }]],
    ['activateView', [detail.area, detail.viewId]],
    ['activateView', [detail.viewId]],
    ['selectView', [{ region: detail.area, area: detail.area, view: detail.view, viewId: detail.viewId }]],
    ['selectView', [detail.area, detail.viewId]],
    ['selectView', [detail.viewId]],
    ['setActiveView', [{ region: detail.area, area: detail.area, view: detail.view, viewId: detail.viewId }]],
    ['setActiveView', [detail.area, detail.viewId]],
    ['setActiveView', [detail.viewId]],
    ['activateRegionView', [{ region: detail.area, area: detail.area, view: detail.view, viewId: detail.viewId }]],
    ['activateRegionView', [detail.area, detail.viewId]],
    ['selectRegionView', [detail.area, detail.viewId]],
    ['setActiveRegionView', [detail.area, detail.viewId]],
    ['viewManager.activate', [{ region: detail.area, area: detail.area, view: detail.view, viewId: detail.viewId }]],
    ['viewManager.activate', [detail.area, detail.viewId]],
    ['viewManager.select', [detail.area, detail.viewId]],
    ['viewManager.setActive', [detail.area, detail.viewId]],
  ]
  for (const [path, args] of attempts) {
    const fn = path.split('.').reduce((obj, key) => obj?.[key], workspace)
    if (typeof fn !== 'function') continue
    try {
      fn.apply(path.includes('.') ? workspace.viewManager : workspace, args)
      return true
    } catch {}
  }
  try { workspace.dispatchEvent?.(new CustomEvent('cmx-workspace-activate-view', { detail })) } catch {}
  return false
}

function activatePropertyOrgView (source) {
  const viewId = PROP_ORG_VIEW_ID
  const detail = { area: 'property', view: 'propertyOrg', viewId }
  const workspace = source?.workspace || source?.host?.workspace || findWorkspaceFromDom(source)
  if (activateWorkspaceView(workspace, detail)) return
  try { source?.dispatchEvent?.(new CustomEvent('cmx-workspace-activate-view', { detail, bubbles: true, composed: true })) } catch {}

  const tryActivate = () => {
    const embeds = collectDeep(document, 'cmx-embed-page')
    for (const embed of embeds) {
      const pages = String(embed.getAttribute?.('pages') || '')
      if (!pages.split(/[,\s]+/).includes(viewId)) continue
      try {
        embed.setAttribute('initial-view', viewId)
        if (typeof embed._activate === 'function') embed._activate(viewId)
        return true
      } catch {}
    }
    const btns = collectDeep(document, `[data-view-id="${viewId}"],[data-view="${viewId}"]`)
    const btn = btns.find((el) => typeof el.click === 'function')
    if (btn) { try { btn.click(); return true } catch {} }
    return false
  }
  if (tryActivate()) return
  setTimeout(tryActivate, 60)
  setTimeout(tryActivate, 180)
}

// ============================================================================
// 绑定
// ============================================================================

function selectReport (code) {
  if (!code) return
  state.selectedCode = code
  const r = selectedReport()
  if (r) ensureDetail(r.code, reportVersion(r))
  refreshAll()
}

function bind (root) {
  root.querySelectorAll('[data-act="refresh"]').forEach((btn) => btn.addEventListener('click', () => loadData(true)))
  root.querySelectorAll('[data-act="refresh-org"]').forEach((btn) => btn.addEventListener('click', () => refreshOrgTree()))

  // 会计期间下拉（数据上下文）
  root.querySelector('[data-period-select]')?.addEventListener('change', (ev) => {
    state.selectedPeriod = ev.target.value || ''
    refreshAll()
  })

  // 组织树：展开/折叠 + 选中
  root.querySelectorAll('[data-org-expand]').forEach((el) => el.addEventListener('click', (ev) => {
    ev.stopPropagation()
    const id = el.getAttribute('data-org-expand')
    if (state.orgExpanded.has(id)) state.orgExpanded.delete(id)
    else state.orgExpanded.add(id)
    refreshAll()
  }))
  root.querySelectorAll('[data-org]').forEach((el) => el.addEventListener('click', (ev) => {
    if (ev.target.closest('[data-org-expand]')) return
    state.selectedOrg = el.getAttribute('data-org') || ''
    refreshAll()
    activatePropertyOrgView(el)
  }))

  // 报表类别下拉：触发器 toggle + 菜单项选择 + 点外部关闭
  root.querySelector('[data-cat-toggle]')?.addEventListener('click', (ev) => {
    ev.stopPropagation()
    state.categoryOpen = !state.categoryOpen
    refreshAll()
  })
  root.querySelectorAll('[data-cat]').forEach((btn) => btn.addEventListener('click', () => {
    state.selectedCategory = btn.getAttribute('data-cat') || ''
    state.categoryOpen = false
    const first = filteredReports()[0]
    state.selectedCode = first ? first.code : ''
    refreshAll()
  }))
  if (state.categoryOpen && !root.__catOutside) {
    root.__catOutside = true
    document.addEventListener('click', (ev) => {
      if (!state.categoryOpen) return
      const dd = root.querySelector('[data-cat-dd]')
      if (dd && !dd.contains(ev.target)) { state.categoryOpen = false; refreshAll() }
    })
  }

  // 期间类型 tab
  root.querySelectorAll('[data-period-type]').forEach((btn) => btn.addEventListener('click', () => {
    state.selectedPeriodType = btn.getAttribute('data-period-type') || ''
    const first = filteredReports()[0]
    state.selectedCode = first ? first.code : ''
    refreshAll()
  }))

  // 搜索
  root.querySelector('[data-query]')?.addEventListener('input', (ev) => {
    state.query = ev.target.value || ''
    refreshAll()
  })

  // 报表卡片选中 + 版本 + 打开
  root.querySelectorAll('.rpt-card').forEach((card) => card.addEventListener('click', (ev) => {
    if (ev.target.closest('button') || ev.target.closest('select') || ev.target.closest('.rpt-check')) return
    selectReport(card.getAttribute('data-code'))
  }))
  // 多选勾选（批量计算/校验对象）
  root.querySelectorAll('[data-pick]').forEach((cb) => cb.addEventListener('change', (ev) => {
    ev.stopPropagation()
    const code = cb.getAttribute('data-pick')
    if (cb.checked) state.selectedCodes.add(code)
    else state.selectedCodes.delete(code)
    refreshAll()
  }))
  // 标题区批量操作（占位：计算/校验引擎待接入）
  root.querySelectorAll('[data-batch]').forEach((btn) => btn.addEventListener('click', () => {
    if (btn.disabled) return
    const act = btn.getAttribute('data-batch')
    const codes = [...state.selectedCodes]
    if (!codes.length) { toast('请先勾选报表', 'error'); return }
    const label = ({ compute: '计算', validate: '校验', 'check-report': '校验报告' })[act] || '操作'
    const ctx = [state.selectedOrg, state.selectedPeriod].filter(Boolean).join(' / ') || '未选组织/期间'
    toast(`批量${label} ${codes.length} 张报表（${ctx}）—— 计算/校验引擎待接入`, 'info')
  }))
  root.querySelectorAll('[data-version]').forEach((sel) => sel.addEventListener('change', () => {
    const code = sel.getAttribute('data-version')
    state.selectedVersion[code] = sel.value
    if (state.selectedCode === code) ensureDetail(code, sel.value)
    refreshAll()
  }))
  root.querySelectorAll('[data-act="open"]').forEach((btn) => btn.addEventListener('click', (ev) => {
    ev.stopPropagation()
    openReportApplier(btn.getAttribute('data-code'), btn).catch((err) => {
      state.message = '打开报表应用失败：' + (err.message || err)
      refreshAll()
    })
  }))
  // 计算 / 校验 / 校验报告：占位（计算与校验引擎待接入）。选中该报表并给专业提示。
  root.querySelectorAll('[data-act="compute"],[data-act="validate"],[data-act="check-report"]').forEach((btn) => btn.addEventListener('click', (ev) => {
    ev.stopPropagation()
    const code = btn.getAttribute('data-code') || ''
    const act = btn.getAttribute('data-act')
    state.selectedCode = code
    const r = state.reports.find((x) => x.code === code)
    const label = ({ compute: '计算报表', validate: '校验报表', 'check-report': '校验报告' })[act] || '操作'
    const ctx = [state.selectedOrg, state.selectedPeriod].filter(Boolean).join(' / ') || '未选组织/期间'
    refreshAll()
    toast(`${label}「${r?.name || code}」（${ctx}）—— 计算/校验引擎待接入`, 'info')
  }))
}

function styleCss () {
  return `
    .rpt{--rpt-blue:#0d9488;--rpt-blue2:#14b8a6;--rpt-cyan:#00a6c8;--rpt-green:#10a760;--rpt-amber:#d98200;--rpt-red:#bb0000;--rpt-border:var(--sapGroup_TitleBorderColor,#d9e2ec);
      height:100%;min-height:0;box-sizing:border-box;display:flex;flex-direction:column;background:var(--sapBackgroundColor,#f5f6f7);color:var(--sapTextColor,#1d2d3e);font:13px/1.45 var(--sapFontFamily,Arial,sans-serif);overflow:hidden}
    .rpt-head{height:46px;flex:0 0 auto;display:flex;align-items:center;gap:10px;padding:0 12px;border-bottom:1px solid color-mix(in srgb,var(--rpt-blue) 26%,var(--rpt-border));background:linear-gradient(180deg,color-mix(in srgb,var(--rpt-blue) 15%,var(--sapList_HeaderBackground,#f7f9fc)),color-mix(in srgb,var(--rpt-blue) 8%,var(--sapList_HeaderBackground,#f7f9fc)))}
    .rpt-head.compact{height:46px}.rpt-head b{display:block;font-size:14px}.rpt-head span{display:block;font-size:11px;color:var(--sapContent_LabelColor,#6a6d70)}.rpt-head-action{margin-left:auto}
    .rpt-title{min-width:0;display:grid;grid-template-columns:34px minmax(0,1fr);align-items:center;gap:9px}.rpt-title-ic{width:32px;height:32px;border-radius:8px;background:linear-gradient(135deg,var(--rpt-blue2,var(--rpt-blue)),var(--rpt-blue));color:#fff;display:flex!important;align-items:center;justify-content:center;box-shadow:0 1px 4px color-mix(in srgb,var(--rpt-blue) 46%,transparent)}.rpt-title-ic ui5-icon{width:1.08rem;height:1.08rem}.rpt-title-main{min-width:0;display:block!important}.rpt-title-main b,.rpt-title-main small{display:block;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.rpt-title-main small{font-size:11px;color:var(--sapContent_LabelColor,#6a6d70)}
    .rpt-toolbar{margin-left:auto;display:flex;align-items:center;gap:8px;min-width:0}.rpt-search{width:220px;max-width:36vw;height:30px;border:1px solid var(--rpt-border);border-radius:6px;padding:0 10px;background:var(--sapField_Background,#fff);color:inherit}
    .rpt-batch{display:inline-flex;align-items:center;gap:5px;padding-left:8px;margin-left:2px;border-left:1px solid var(--rpt-border)}
    .rpt-batch-btn{position:relative;height:30px;display:inline-flex;align-items:center;gap:5px;border:1px solid var(--rpt-border);border-radius:6px;background:var(--sapButton_Background,#fff);color:var(--sapButton_TextColor,#0a6ed1);cursor:pointer;padding:0 10px;font:inherit;font-size:12px;font-weight:600;transition:background .12s,color .12s,border-color .12s,box-shadow .12s}
    .rpt-batch-btn ui5-icon{width:.95rem;height:.95rem}
    .rpt-batch-btn:hover:not(:disabled){border-color:color-mix(in srgb,var(--rpt-blue) 48%,var(--rpt-border));background:color-mix(in srgb,var(--rpt-blue) 6%,#fff);box-shadow:0 1px 5px color-mix(in srgb,var(--rpt-blue) 16%,transparent)}
    .rpt-batch-btn:disabled{opacity:.42;cursor:not-allowed;color:var(--sapContent_LabelColor,#6a6d70)}
    .rpt-batch-n{min-width:16px;height:16px;padding:0 4px;border-radius:999px;display:inline-flex;align-items:center;justify-content:center;font-size:10px;font-weight:800;color:#fff;background:var(--rpt-blue)}
    .rpt-btn,.rpt-icon-btn,.rpt-mini{border:1px solid var(--rpt-border);background:var(--sapButton_Background,#fff);color:var(--sapButton_TextColor,#0a6ed1);border-radius:6px;cursor:pointer;display:inline-flex;align-items:center;justify-content:center;gap:6px;height:30px;padding:0 10px;font:inherit;font-size:12px}
    .rpt-icon-btn,.rpt-mini{width:30px;padding:0}.rpt-mini.primary{background:var(--rpt-blue);border-color:var(--rpt-blue);color:#fff}.rpt-btn ui5-icon,.rpt-mini ui5-icon,.rpt-icon-btn ui5-icon{width:1rem;height:1rem}
    .rpt-icon-btn.spin ui5-icon{animation:rpt-spin .8s linear infinite}@keyframes rpt-spin{to{transform:rotate(360deg)}}
    /* explorer 期间：一行 期间 + 精致下拉；高度与 content 标题区 .rpt-head 一致(46px) */
    .rpt-period-row{height:46px;flex:0 0 auto;box-sizing:border-box;display:flex;align-items:center;gap:8px;padding:0 12px;border-bottom:1px solid var(--rpt-border);background:var(--sapList_HeaderBackground,#f7f9fc)}
    .rpt-period-lbl{flex:0 0 auto;display:inline-flex;align-items:center;gap:4px;font-size:12px;font-weight:700;color:var(--sapContent_LabelColor,#6a6d70)}.rpt-period-lbl ui5-icon{width:.95rem;height:.95rem;color:var(--rpt-cyan)}
    .rpt-select-wrap{position:relative;flex:1;min-width:0}
    .rpt-select-wrap select{width:100%;height:32px;border:1px solid color-mix(in srgb,var(--rpt-blue) 20%,var(--rpt-border));border-radius:8px;background:var(--sapField_Background,#fff);color:inherit;font:inherit;font-size:12.5px;font-weight:600;padding:0 30px 0 11px;cursor:pointer;-webkit-appearance:none;appearance:none;box-shadow:0 1px 2px rgba(10,31,68,.05);transition:border-color .15s,box-shadow .15s}
    .rpt-select-wrap select:hover{border-color:color-mix(in srgb,var(--rpt-blue) 42%,var(--rpt-border))}
    .rpt-select-wrap select:focus{outline:0;border-color:var(--rpt-blue);box-shadow:0 0 0 3px color-mix(in srgb,var(--rpt-blue) 14%,transparent)}
    .rpt-select-caret{position:absolute;right:9px;top:50%;transform:translateY(-50%);width:.85rem;height:.85rem;color:var(--rpt-blue);pointer-events:none}
    /* explorer 组织标题 + 刷新 */
    .rpt-org-head{flex:0 0 auto;display:flex;align-items:center;gap:6px;padding:9px 8px 5px 10px;font-size:11px;font-weight:700;color:var(--sapContent_LabelColor,#6a6d70);text-transform:uppercase;letter-spacing:.03em}.rpt-org-head>ui5-icon{width:.9rem;height:.9rem;color:var(--rpt-blue)}.rpt-org-head .rpt-icon-btn{margin-left:auto;width:26px;height:26px}.rpt-org-head .rpt-icon-btn ui5-icon{width:.9rem;height:.9rem}
    .rpt-org-tree{flex:1;min-height:0;overflow:auto;padding:2px 6px 10px}
    .rpt-org-node{display:flex;align-items:center;gap:6px;min-height:38px;border-radius:7px;cursor:pointer;padding-right:8px}.rpt-org-node:hover{background:color-mix(in srgb,var(--rpt-blue) 6%,transparent)}.rpt-org-node.active{background:color-mix(in srgb,var(--rpt-blue) 12%,var(--sapTile_Background,#fff));box-shadow:inset 3px 0 0 var(--rpt-blue)}
    .rpt-org-caret,.rpt-org-caret-empty{width:16px;height:16px;flex:0 0 auto;display:inline-flex;align-items:center;justify-content:center;color:var(--sapContent_LabelColor,#6a6d70)}.rpt-org-caret{cursor:pointer;border-radius:4px}.rpt-org-caret:hover{background:color-mix(in srgb,var(--rpt-blue) 14%,transparent);color:var(--rpt-blue)}.rpt-org-caret ui5-icon{width:.72rem;height:.72rem}
    .rpt-org-ic{width:1rem;height:1rem;flex:0 0 auto;color:var(--rpt-blue)}.rpt-org-node.active .rpt-org-ic{color:var(--rpt-blue)}
    .rpt-org-text{min-width:0;display:block}.rpt-org-text b{display:block;font-size:12px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.rpt-org-text small{display:block;font-size:10px;color:var(--sapContent_LabelColor,#6a6d70);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;font-family:ui-monospace,Menlo,Consolas,monospace}
    .rpt-org-node.active .rpt-org-text b{color:var(--rpt-blue);font-weight:800}
    /* content 标题：组织图标+名称+编码+期间 单行展示 */
    .rpt-title.one-line{display:flex;align-items:center;gap:8px;min-width:0}
    .rpt-title.one-line .rpt-title-ic{flex:0 0 auto}
    .rpt-title-name{min-width:0;font-size:14px;font-weight:700;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;max-width:min(40vw,320px)}
    .rpt-title-code{flex:0 0 auto;font:800 10px/1 ui-monospace,Menlo,Consolas,monospace;color:#fff;background:var(--rpt-blue);border-radius:4px;padding:3px 6px}
    .rpt-title-period{flex:0 0 auto;display:inline-flex;align-items:center;gap:3px;font-size:12px;font-weight:600;color:var(--sapContent_LabelColor,#6a6d70);border:1px solid var(--rpt-border);border-radius:5px;padding:2px 7px;background:var(--sapTile_Background,#fff)}.rpt-title-period ui5-icon{width:.82rem;height:.82rem;color:var(--rpt-cyan)}
    /* content 类别下拉 */
    .rpt-cat-dd{position:relative;flex:0 0 auto}
    .rpt-cat-trigger{--cat-color:var(--rpt-blue);display:inline-flex;align-items:center;gap:7px;height:32px;border:1px solid color-mix(in srgb,var(--cat-color) 30%,var(--rpt-border));border-radius:8px;background:var(--sapTile_Background,#fff);color:inherit;cursor:pointer;padding:0 9px 0 11px;font:inherit;font-size:12.5px;font-weight:700;box-shadow:0 1px 2px rgba(10,31,68,.05)}
    .rpt-cat-trigger>ui5-icon:first-child{width:1rem;height:1rem;color:var(--cat-color)}.rpt-cat-trigger>span{max-width:120px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.rpt-cat-tri-caret{width:.8rem;height:.8rem;color:var(--cat-color);transition:transform .15s}.rpt-cat-dd.open .rpt-cat-tri-caret{transform:rotate(180deg)}
    .rpt-cat-trigger:hover{border-color:color-mix(in srgb,var(--cat-color) 52%,var(--rpt-border))}
    .rpt-cat-menu{position:absolute;left:0;top:38px;z-index:30;min-width:220px;display:none;flex-direction:column;gap:3px;padding:6px;border:1px solid var(--rpt-border);border-radius:9px;background:var(--sapTile_Background,#fff);box-shadow:0 14px 36px rgba(10,31,68,.2)}.rpt-cat-dd.open .rpt-cat-menu{display:flex}
    .rpt-cat-item{--cat-color:var(--rpt-blue);display:flex;align-items:center;gap:8px;height:34px;border:0;border-radius:7px;background:transparent;color:inherit;cursor:pointer;padding:0 9px;font:inherit;font-size:12.5px;text-align:left}.rpt-cat-item ui5-icon{width:1rem;height:1rem;color:var(--cat-color)}.rpt-cat-item span{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.rpt-cat-item b{min-width:20px;height:20px;border-radius:999px;display:inline-flex;align-items:center;justify-content:center;font-size:11px;color:var(--cat-color);background:color-mix(in srgb,var(--cat-color) 10%,transparent)}
    .rpt-cat-item:hover{background:color-mix(in srgb,var(--cat-color) 8%,transparent)}.rpt-cat-item.active{background:color-mix(in srgb,var(--cat-color) 14%,transparent);font-weight:800}.rpt-cat-item.active b{background:var(--cat-color);color:#fff}
    .rpt-cat-empty{color:var(--sapContent_LabelColor,#6a6d70);font-size:12px;padding:8px}
    /* content 期间类型 tab（参考设计工作台）+ 报表列表 */
    .rpt-content{overflow:hidden}
    /* 期间 tab 改左侧竖排纵向文字：main 从「上 tab 下列表」改「左 rail 右列表」 */
    .rpt-main{flex:1;min-height:0;display:grid;grid-template-columns:auto minmax(0,1fr);overflow:hidden;background:linear-gradient(180deg,color-mix(in srgb,var(--period-color,var(--rpt-blue)) 6%,var(--sapList_HeaderBackground,#f7f9fc)),var(--sapBackgroundColor,#f5f6f7))}
    .rpt-periods{border-right:1px solid color-mix(in srgb,var(--period-color,var(--rpt-blue)) 24%,var(--rpt-border));background:color-mix(in srgb,var(--sapTile_Background,#fff) 78%,var(--period-color,var(--rpt-blue)));padding:12px 0 12px 8px;display:flex;flex-direction:column;align-items:flex-end;gap:7px;overflow-y:auto;overflow-x:hidden}
    .rpt-period{width:46px;min-height:94px;border:1px solid color-mix(in srgb,var(--tab-color) 20%,var(--rpt-border));border-right:0;border-radius:9px 0 0 9px;background:linear-gradient(90deg,color-mix(in srgb,var(--tab-color) 8%,var(--sapTile_Background,#fff)),color-mix(in srgb,var(--tab-color) 2%,var(--sapTile_Background,#fff)));color:inherit;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:7px;padding:12px 0;cursor:pointer;position:relative;transform:translateX(1px)}
    .rpt-period::before{content:"";position:absolute;top:0;bottom:0;left:0;width:3px;border-radius:9px 0 0 9px;background:var(--tab-color);opacity:.72}.rpt-period ui5-icon{width:.95rem;height:.95rem;color:var(--tab-color);flex:0 0 auto}.rpt-period span{writing-mode:vertical-rl;text-orientation:upright;font-weight:700;letter-spacing:.08em;white-space:nowrap}.rpt-period b{min-width:20px;height:20px;border-radius:999px;display:inline-flex;align-items:center;justify-content:center;font-size:11px;color:var(--tab-color);background:color-mix(in srgb,var(--tab-color) 10%,var(--sapTile_Background,#fff));flex:0 0 auto}
    .rpt-period:hover{background:linear-gradient(90deg,color-mix(in srgb,var(--tab-color) 14%,var(--sapTile_Background,#fff)),var(--sapTile_Background,#fff))}
    .rpt-period.active{z-index:1;width:50px;background:var(--sapBackgroundColor,#f5f6f7);border-color:color-mix(in srgb,var(--tab-color) 52%,var(--rpt-border));box-shadow:-2px 0 0 var(--tab-color),-6px 0 16px color-mix(in srgb,var(--tab-color) 12%,transparent)}.rpt-period.active::before{width:4px;opacity:1}.rpt-period.active::after{content:"";position:absolute;top:0;bottom:0;right:-1px;width:1px;background:var(--sapBackgroundColor,#f5f6f7)}.rpt-period.active b{background:var(--tab-color);color:#fff}
    .rpt-list{min-height:0;overflow:auto;padding:10px 12px 18px;display:flex;flex-direction:column;gap:8px}
    /* 报表卡片选择样式：参考报表设计工作台，选中态用 --period-color（跟随期间 tab 配色）*/
    .rpt-card{position:relative;flex:0 0 auto;display:grid;grid-template-columns:22px 5px 38px minmax(0,1fr) auto;gap:10px;align-items:center;border:1px solid color-mix(in srgb,var(--period-color,var(--rpt-blue)) 16%,var(--rpt-border));border-radius:8px;background:var(--sapTile_Background,#fff);padding:10px;cursor:pointer;overflow:hidden}
    .rpt-card.checked{border-color:color-mix(in srgb,var(--period-color,var(--rpt-blue)) 60%,var(--rpt-border));background:color-mix(in srgb,var(--period-color,var(--rpt-blue)) 6%,var(--sapTile_Background,#fff))}
    .rpt-check{width:20px;height:20px;flex:0 0 auto;display:inline-flex;align-items:center;justify-content:center;cursor:pointer;position:relative}
    .rpt-check input{position:absolute;opacity:0;width:100%;height:100%;margin:0;cursor:pointer}
    .rpt-check-box{width:18px;height:18px;border:1.5px solid color-mix(in srgb,var(--period-color,var(--rpt-blue)) 40%,var(--rpt-border));border-radius:5px;background:var(--sapField_Background,#fff);display:inline-flex;align-items:center;justify-content:center;transition:background .12s,border-color .12s}
    .rpt-check-box ui5-icon{width:.72rem;height:.72rem;color:#fff;opacity:0;transition:opacity .12s}
    .rpt-check input:checked + .rpt-check-box{background:var(--period-color,var(--rpt-blue));border-color:var(--period-color,var(--rpt-blue))}.rpt-check input:checked + .rpt-check-box ui5-icon{opacity:1}
    .rpt-check:hover .rpt-check-box{border-color:var(--period-color,var(--rpt-blue))}
    .rpt-card:hover{border-color:color-mix(in srgb,var(--period-color,var(--rpt-blue)) 48%,var(--rpt-border));box-shadow:0 2px 10px color-mix(in srgb,var(--period-color,var(--rpt-blue)) 18%,transparent)}
    .rpt-card.active{border-color:color-mix(in srgb,var(--period-color,var(--rpt-blue)) 72%,var(--rpt-border));background:linear-gradient(90deg,color-mix(in srgb,var(--period-color,var(--rpt-blue)) 16%,var(--sapTile_Background,#fff)),color-mix(in srgb,var(--period-color,var(--rpt-blue)) 5%,var(--sapTile_Background,#fff)) 56%,var(--sapTile_Background,#fff));box-shadow:inset 0 0 0 1px color-mix(in srgb,var(--period-color,var(--rpt-blue)) 38%,transparent),0 6px 18px color-mix(in srgb,var(--period-color,var(--rpt-blue)) 20%,transparent)}
    .rpt-card.active::after{content:"";position:absolute;right:0;top:0;border-top:22px solid var(--period-color,var(--rpt-blue));border-left:22px solid transparent}
    .rpt-card-bar{align-self:stretch;border-radius:5px;background:linear-gradient(180deg,var(--period-color,var(--rpt-blue)),color-mix(in srgb,var(--period-color,var(--rpt-blue)) 62%,#ffffff))}.rpt-card.active .rpt-card-bar{width:7px;box-shadow:0 0 14px color-mix(in srgb,var(--period-color,var(--rpt-blue)) 48%,transparent)}
    .rpt-card-ic{width:36px;height:36px;border-radius:8px;background:color-mix(in srgb,var(--period-color,var(--rpt-blue)) 13%,transparent);color:var(--period-color,var(--rpt-blue));display:flex;align-items:center;justify-content:center}.rpt-card-ic ui5-icon{width:1.2rem;height:1.2rem}.rpt-card.active .rpt-card-ic{background:var(--period-color,var(--rpt-blue));color:#fff;box-shadow:0 6px 14px color-mix(in srgb,var(--period-color,var(--rpt-blue)) 26%,transparent)}
    .rpt-card-main{min-width:0}.rpt-card-title{display:flex;align-items:center;gap:8px;min-width:0}.rpt-card-title b{font-size:14px;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.rpt-card.active .rpt-card-title b{color:var(--period-color,var(--rpt-blue));font-weight:800}
    .rpt-card-title span{flex:0 1 auto;min-width:0;max-width:48%;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font:800 11px/1 ui-monospace,Menlo,Consolas,monospace;color:#fff;background:var(--period-color,var(--rpt-blue));border-radius:5px;padding:3px 6px}.rpt-card.active .rpt-card-title span{box-shadow:0 3px 9px color-mix(in srgb,var(--period-color,var(--rpt-blue)) 20%,transparent)}
    .rpt-card-sub{font-size:11px;color:var(--sapContent_LabelColor,#6a6d70);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;margin-top:3px}.rpt-card-tags{display:flex;flex-wrap:wrap;gap:5px;margin-top:6px}.rpt-card-tags span{display:inline-flex;align-items:center;gap:4px;font-size:10px;border:1px solid color-mix(in srgb,var(--period-color,var(--rpt-blue)) 15%,var(--rpt-border));border-radius:5px;padding:2px 6px;background:color-mix(in srgb,var(--period-color,var(--rpt-blue)) 5%,var(--sapList_HeaderBackground,#f7f9fc))}.rpt-card-tags ui5-icon{width:.78rem;height:.78rem;color:var(--period-color,var(--rpt-blue))}
    .rpt-card-actions{display:flex;align-items:center;gap:5px}.rpt-version{height:30px;max-width:130px;border:1px solid var(--rpt-border);border-radius:6px;background:var(--sapField_Background,#fff);color:inherit;font-size:12px}
    .rpt-card-actions .rpt-mini{transition:background .12s,color .12s,border-color .12s,box-shadow .12s}.rpt-card-actions .rpt-mini:hover{border-color:color-mix(in srgb,var(--period-color,var(--rpt-blue)) 48%,var(--rpt-border));color:var(--period-color,var(--rpt-blue));box-shadow:0 1px 5px color-mix(in srgb,var(--period-color,var(--rpt-blue)) 18%,transparent)}.rpt-card-actions .rpt-mini.primary:hover{color:#fff}
    .rpt-prop{padding:10px;gap:10px;overflow:auto}.rpt-prop-hero{display:flex;gap:10px;align-items:center;border:1px solid var(--rpt-border);border-radius:8px;background:linear-gradient(135deg,color-mix(in srgb,var(--rpt-blue) 12%,var(--sapTile_Background,#fff)),var(--sapTile_Background,#fff));padding:12px}.rpt-prop-ic{width:40px;height:40px;border-radius:9px;display:flex;align-items:center;justify-content:center;background:var(--rpt-blue);color:#fff}.rpt-prop-ic ui5-icon{width:1.35rem;height:1.35rem}.rpt-prop-hero b{display:block;font-size:15px}.rpt-prop-hero span{font-size:11px;color:var(--sapContent_LabelColor,#6a6d70)}
    .rpt-prop-grid{display:grid;grid-template-columns:1fr;gap:6px}.rpt-kv{border:1px solid var(--rpt-border);border-radius:7px;background:var(--sapTile_Background,#fff);padding:7px 9px}.rpt-kv span{display:block;font-size:10px;color:var(--sapContent_LabelColor,#6a6d70)}.rpt-kv b{display:block;font-size:12px;word-break:break-word}
    .rpt-prop-sec{border:1px solid var(--rpt-border);border-radius:8px;background:var(--sapTile_Background,#fff);padding:10px}.rpt-prop-sec>b{display:block;margin-bottom:7px}.rpt-prop-sec p{margin:0;color:var(--sapContent_LabelColor,#6a6d70);font-size:12px}.rpt-chip{display:inline-flex;margin:0 5px 5px 0;border:1px solid var(--rpt-border);border-radius:6px;padding:3px 7px;font-size:11px;background:var(--sapList_HeaderBackground,#f7f9fc)}.rpt-chip.strong{color:#fff;background:var(--rpt-green);border-color:var(--rpt-green)}
    .rpt-stat-grid{display:grid;grid-template-columns:repeat(5,minmax(0,1fr));gap:6px}.rpt-stat{border:1px solid var(--rpt-border);border-radius:7px;padding:7px 5px;text-align:center;background:var(--sapList_HeaderBackground,#f7f9fc)}.rpt-stat b{display:block;font-size:15px;color:var(--rpt-blue)}.rpt-stat span{font-size:10px;color:var(--sapContent_LabelColor,#6a6d70)}
    .rpt-msg{margin:8px 12px 0;border:1px solid #ffd7a8;background:#fff8ed;color:#8a4b00;border-radius:7px;padding:7px 9px;font-size:12px}.rpt-empty{flex:1;min-height:0;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:7px;color:var(--sapContent_LabelColor,#6a6d70);text-align:center;padding:18px}.rpt-empty.large{min-height:260px}.rpt-empty ui5-icon{width:1.8rem;height:1.8rem;color:var(--rpt-blue)}.rpt-empty b{color:var(--sapTextColor,#1d2d3e)}
    .rpt-toast{position:absolute;left:50%;bottom:22px;transform:translate(-50%,14px);z-index:60;max-width:min(560px,88%);padding:10px 16px;border-radius:9px;background:#1d2d3e;color:#fff;font-size:12.5px;font-weight:600;box-shadow:0 12px 32px rgba(10,31,68,.34);opacity:0;pointer-events:none;transition:opacity .22s,transform .22s;display:flex;align-items:center;gap:8px}.rpt-toast.show{opacity:1;transform:translate(-50%,0)}.rpt-toast[data-kind="success"]{background:linear-gradient(180deg,#12b56b,#0f9d5c)}.rpt-toast[data-kind="error"]{background:linear-gradient(180deg,#e5544b,#c0392b)}.rpt-toast::before{content:"";width:7px;height:7px;border-radius:50%;background:currentColor;opacity:.8;flex:0 0 auto}
  `
}

export default {
  defaultView: 'content',
  views: {
    async explorer (ctx) { return mount(ctx, 'explorer') },
    async content (ctx) { return mount(ctx, 'content') },
    async property (ctx) { return mount(ctx, 'property') },
    async propertyOrg (ctx) { return mount(ctx, 'propertyOrg') },
  },
}
