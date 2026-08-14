/**
 * 报表设计工作台 —— native_pages 三栏工作台。
 *
 * explorer：报表类别列表。
 * content ：按类别过滤的报表列表，左侧按 cr_period_type 分期间 tab。
 * property：选中报表的详细信息。
 *
 * 数据源：fico-db 数据库中的 cr_report_* / cr_period_type 物理表。
 */

const state = {
  categories: [],
  periods: [],
  reports: [],
  details: {},
  selectedCategory: '',
  selectedPeriod: '',
  selectedCode: '',
  selectedVersion: {},
  query: '',
  loading: false,
  message: '',
  dialog: null,
  hosts: new Set(),
}

const FALLBACK_PERIODS = [
  { code: 'month', name: '月报', icon: 'calendar' },
  { code: 'quarter', name: '季报', icon: 'calendar' },
  { code: 'halfyear', name: '半年报', icon: 'business-card' },
  { code: 'year', name: '年报', icon: 'appointment-2' },
]

const CATEGORY_ICONS = {
  statutory: 'official-service',
  management: 'business-objects-experience',
  internal: 'dimension',
  tax: 'receipt',
  consolidation: 'combine',
  consol: 'combine',
}

const PERIOD_ICONS = {
  month: 'calendar',
  quarter: 'calendar',
  halfyear: 'business-card',
  year: 'appointment-2',
}

const PERIOD_RAINBOW_COLORS = [
  '#e53935',
  '#fb8c00',
  '#fdd835',
  '#43a047',
  '#00acc1',
  '#1e88e5',
  '#8e24aa',
]

const PERIOD_UNKNOWN_COLOR = '#64748b'

const esc = (s) => String(s ?? '')
  .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
  .replace(/"/g, '&quot;').replace(/'/g, '&#39;')

const enc = encodeURIComponent

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
  return idx >= 0 ? PERIOD_RAINBOW_COLORS[idx % PERIOD_RAINBOW_COLORS.length] : PERIOD_UNKNOWN_COLOR
}

function periodLabel (code) {
  return state.periods.find((p) => p.code === code)?.name || code || '未分期间'
}

function periodIcon (code) {
  return state.periods.find((p) => p.code === code)?.icon || PERIOD_ICONS[code] || 'calendar'
}

function periodColor (code) {
  const idx = state.periods.findIndex((p) => p.code === code)
  return idx >= 0 ? PERIOD_RAINBOW_COLORS[idx % PERIOD_RAINBOW_COLORS.length] : PERIOD_UNKNOWN_COLOR
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

function designerProps (report, version) {
  return {
    reportCode: report?.code || '',
    reportName: report?.name || '',
    version: version || '',
  }
}

function designerTabLabel (report) {
  const code = String(report?.code || '').trim()
  const name = String(report?.name || '').trim()
  return name ? `${code}-${name}` : code || '报表设计器'
}

function designerView (id, view, tabLabel, icon, props) {
  return {
    id,
    tabLabel,
    icon,
    type: 'native_pages',
    native_page: 'portal.rpt.designer',
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
  const candidates = [
    window,
    window.parent,
    window.top,
    globalThis,
  ].filter(Boolean)
  for (const target of candidates) {
    try {
      if (typeof target.openTab === 'function') {
        target.openTab(workNode)
        return true
      }
      if (typeof target.openWorkspaceNode === 'function') {
        target.openWorkspaceNode(workNode)
        return true
      }
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
        details: `动态报表设计器工作区：${workNode.id}`,
        workspace: workNode.workspace,
      }),
    })
    dispatchPortalAction(sourceEl, { kind: 'node', id: workNode.id, icon: workNode.icon || 'create', title: workNode.caption || workNode.name || workNode.id })
    return true
  } catch {}
  const detail = { workNode, menu: workNode }
  try {
    window.parent?.postMessage({ type: 'openTab', payload: workNode }, '*')
    window.parent?.postMessage({ type: 'openWorkspaceNode', payload: workNode }, '*')
    window.top?.postMessage({ type: 'openTab', payload: workNode }, '*')
    window.top?.postMessage({ type: 'openWorkspaceNode', payload: workNode }, '*')
  } catch {}
  try {
    document.dispatchEvent(new CustomEvent('cmx-open-workspace-node', { detail, bubbles: true, composed: true }))
    window.dispatchEvent(new CustomEvent('cmx-open-workspace-node', { detail }))
  } catch {}
  return true
}

async function openReportDesigner (code, sourceEl) {
  const report = state.reports.find((r) => r.code === code)
  if (!report) return
  const version = reportVersion(report) || ''
  const props = designerProps(report, version)
  const sid = `${slug(report.code)}-${slug(version || 'default')}`
  const title = `报表设计器-${report.name || report.code}`
  const sheetTitle = designerTabLabel(report)
  const menu = {
    id: `rpt-designer-${sid}`,
    name: `rpt-designer-${sid}`,
    menuName: title,
    caption: title,
    type: 'workspace-node',
    icon: 'table-chart',
    openType: 0,
    status: 1,
    workspace: {
      id: `rpt_designer_${sid}`,
      params: props,
      explorerWidth: 300,
      propertyWidth: 340,
      model: {
        id: `rpt-designer-${sid}-model`,
        type: 'native_pages',
        native_page: 'portal.rpt.designer',
        view: 'content',
        props,
      },
      explorer: {
        caption: '设计资源',
        icon: 'database',
        views: [
          designerView(`rpt-designer-${sid}-elements`, 'explorerData', '数据元素', 'database', props),
          designerView(`rpt-designer-${sid}-models`, 'explorerModel', '多维模型', 'dimension', props),
        ],
      },
      content: {
        caption: sheetTitle,
        icon: 'table-chart',
        views: [
          designerView(`rpt-designer-${sid}-sheet`, 'content', sheetTitle, 'table-chart', props),
        ],
      },
      property: {
        caption: '属性',
        icon: 'detail-view',
        views: [
          designerView(`rpt-designer-${sid}-prop-meta`, 'propertyMeta', '报表属性', 'detail-view', props),
          designerView(`rpt-designer-${sid}-prop-cell`, 'propertyCell', '单元格', 'target-group', props),
          designerView(`rpt-designer-${sid}-prop-element`, 'propertyElement', '元素属性', 'database', props),
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

function filteredReports () {
  const q = state.query.trim().toLowerCase()
  return state.reports.filter((r) => {
    if (state.selectedCategory && r.report_category !== state.selectedCategory) return false
    if (state.selectedPeriod && r.period_type !== state.selectedPeriod) return false
    if (!q) return true
    const hay = [
      r.code, r.name, r.report_type, r.report_category, r.period_type,
      r.currency_code, r.entity_scope, r.data_source, r.remark,
    ].join(' ').toLowerCase()
    return q.split(/\s+/).filter(Boolean).every((x) => hay.includes(x))
  })
}

function rebuildDimensions (raw) {
  const cats = normalizeArray(raw.categories).map((c) => ({
    code: c.code,
    name: c.name || c.code,
    remark: c.remark,
    sort_no: Number(c.sort_no ?? 999999),
    icon: CATEGORY_ICONS[c.code] || 'folder',
  })).filter((c) => c.code)
  const catMap = new Map(cats.map((c) => [c.code, { ...c, count: 0, periods: {} }]))
  for (const r of state.reports) {
    const code = r.report_category || 'other'
    if (!catMap.has(code)) {
      catMap.set(code, { code, name: code, sort_no: 999999, icon: CATEGORY_ICONS[code] || 'folder', count: 0, periods: {} })
    }
    const c = catMap.get(code)
    c.count += 1
    const p = r.period_type || 'unknown'
    c.periods[p] = (c.periods[p] || 0) + 1
  }
  state.categories = [...catMap.values()].sort((a, b) => (a.sort_no - b.sort_no) || String(a.name).localeCompare(String(b.name), 'zh-CN'))

  const periodRows = normalizeArray(raw.periods).map((p) => ({
    code: p.code,
    name: p.name || p.code,
    remark: p.remark,
    sort_no: Number(p.sort_no ?? 999999),
    icon: PERIOD_ICONS[p.code] || 'calendar',
  })).filter((p) => p.code)
  const periodMap = new Map((periodRows.length ? periodRows : FALLBACK_PERIODS).map((p, idx) => [p.code, { ...p, sort_no: p.sort_no ?? idx }]))
  for (const r of state.reports) {
    const code = r.period_type || 'unknown'
    if (!periodMap.has(code)) periodMap.set(code, { code, name: code, sort_no: 999999, icon: PERIOD_ICONS[code] || 'calendar' })
  }
  state.periods = [...periodMap.values()].sort((a, b) => (a.sort_no - b.sort_no) || String(a.name).localeCompare(String(b.name), 'zh-CN'))
}

async function loadData (force = false) {
  if (state.loading) return
  if (!force && state.reports.length) return
  state.loading = true
  state.message = ''
  refreshAll()
  try {
    const raw = await apiJson('/api/report-design/overview')
    state.reports = normalizeArray(raw.reports)
    rebuildDimensions(raw)
    if (!state.selectedCategory && state.categories[0]) state.selectedCategory = state.categories[0].code
    if (!state.selectedPeriod && state.periods[0]) state.selectedPeriod = state.periods[0].code
    const first = filteredReports()[0] || state.reports[0]
    if (!state.selectedCode && first) state.selectedCode = first.code
    const chosen = selectedReport()
    if (chosen) await ensureDetail(chosen.code, reportVersion(chosen), false)
  } catch (err) {
    state.message = '报表工作台加载失败：' + (err.message || err)
  } finally {
    state.loading = false
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
  if (host) host.__rptDesignerView = view
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
    const view = host.__rptDesignerView || host.getAttribute?.('view') || 'content'
    root.innerHTML = `<style>${styleCss()}</style>${viewHtml(view)}`
    bind(root, view)
  }
}

function viewHtml (view) {
  if (view === 'explorer') return explorerHtml()
  if (view === 'property') return propertyHtml()
  return contentHtml() + dialogHtml()
}

function explorerHtml () {
  const body = state.loading
    ? `<cmx-empty-state icon="busy" title="加载报表类别..." size="sm"></cmx-empty-state>`
    : (state.categories.length ? state.categories.map(categoryItemHtml).join('') : `<cmx-empty-state icon="folder" title="fico-db 中暂无报表类别" size="sm"></cmx-empty-state>`)
  return `<section class="rpt rpt-explorer">
    <div class="rpt-head compact">
      <div><b>报表类别</b><span>fico-db / cr_report_category</span></div>
      <button class="rpt-icon-btn rpt-head-action" data-act="refresh" title="刷新"><ui5-icon name="refresh"></ui5-icon></button>
    </div>
    <div class="rpt-cat-list">${body}</div>
  </section>`
}

function categoryItemHtml (c) {
  const active = state.selectedCategory === c.code
  return `<button class="rpt-cat ${active ? 'active' : ''}" style="--cat-color:${esc(categoryColor(c.code))}" data-cat="${esc(c.code)}">
    <span class="rpt-cat-ic"><ui5-icon name="${esc(c.icon)}"></ui5-icon></span>
    <span class="rpt-cat-main"><b>${esc(c.name)}</b><small>${esc(c.code)}</small></span>
    <span class="rpt-cat-count">${c.count}</span>
  </button>`
}

function contentHtml () {
  const cat = state.categories.find((c) => c.code === state.selectedCategory)
  const list = filteredReports()
  const selectedColor = periodColor(state.selectedPeriod)
  const tabs = state.periods.map((p) => {
    const n = state.reports.filter((r) => r.report_category === state.selectedCategory && r.period_type === p.code).length
    return `<button class="rpt-period ${state.selectedPeriod === p.code ? 'active' : ''}" style="--tab-color:${esc(periodColor(p.code))}" data-period="${esc(p.code)}">
      <ui5-icon name="${esc(periodIcon(p.code))}"></ui5-icon><span>${esc(p.name)}</span><b>${n}</b>
    </button>`
  }).join('')
  return `<section class="rpt rpt-content" style="--period-color:${esc(selectedColor)}">
    <div class="rpt-head">
      <div class="rpt-title">
        <span class="rpt-title-ic"><ui5-icon name="${esc(cat?.icon || 'folder')}"></ui5-icon></span>
        <span class="rpt-title-main"><b>${esc(cat?.name || '报表设计工作台')}</b><small>${esc(cat?.code || 'fico-db report design')}</small></span>
      </div>
      <div class="rpt-toolbar">
        <input class="rpt-search" data-query placeholder="搜索编码 / 名称 / 口径..." value="${esc(state.query)}">
        <button class="rpt-btn primary" data-act="new"><ui5-icon name="add"></ui5-icon>新增报表</button>
        <button class="rpt-icon-btn" data-act="refresh" title="刷新"><ui5-icon name="refresh"></ui5-icon></button>
      </div>
    </div>
    ${state.message ? `<div class="rpt-msg">${esc(state.message)}</div>` : ''}
    <div class="rpt-main">
      <div class="rpt-periods">${tabs}</div>
      <div class="rpt-list">
        ${state.loading ? `<cmx-empty-state icon="busy" title="加载报表主档..." size="sm"></cmx-empty-state>` : (list.length ? list.map(reportCardHtml).join('') : emptyReportsHtml())}
      </div>
    </div>
  </section>`
}

function emptyReportsHtml () {
  return `<cmx-empty-state icon="document" title="当前类别与期间下暂无报表" description="点击右上角新增报表，数据会直接写入 fico-db。" size="sm"></cmx-empty-state>`
}

function reportCardHtml (r) {
  const active = state.selectedCode === r.code
  const versions = normalizeArray(r.versions)
  const current = reportVersion(r)
  const versionOptions = versions.length
    ? versions.map((v) => `<option value="${esc(v.code)}" ${v.code === current ? 'selected' : ''}>${esc(v.code)} · ${esc(v.name || v.version_status || '')}</option>`).join('')
    : `<option value="">默认版本</option>`
  return `<article class="rpt-card ${active ? 'active' : ''}" data-code="${esc(r.code)}">
    <span class="rpt-card-bar"></span>
    <span class="rpt-card-ic"><ui5-icon name="document-text"></ui5-icon></span>
    <div class="rpt-card-main">
      <div class="rpt-card-title"><b>${esc(r.name)}</b><span>CODE ${esc(r.code)}</span></div>
      <div class="rpt-card-sub">${esc(r.report_type || 'CUSTOM')} · ${esc(categoryLabel(r.report_category))} · ${esc(periodLabel(r.period_type))}</div>
      <div class="rpt-card-tags">
        <span><ui5-icon name="factory"></ui5-icon>${esc(r.entity_scope || 'single')}</span>
        <span><ui5-icon name="money-bills"></ui5-icon>${esc(r.currency_code || '-')} / ${esc(r.amount_unit || '-')}</span>
        <span><ui5-icon name="layers"></ui5-icon>${Number(r.version_count || versions.length || 0)} 个版本</span>
        <span><ui5-icon name="database"></ui5-icon>${esc(r.data_source || '未指定')}</span>
      </div>
    </div>
    <div class="rpt-card-actions">
      <select class="rpt-version" data-version="${esc(r.code)}">${versionOptions}</select>
      <button class="rpt-mini" data-act="open" data-code="${esc(r.code)}" title="打开报表"><ui5-icon name="inspect"></ui5-icon></button>
      <button class="rpt-mini" data-act="version" data-code="${esc(r.code)}" title="版本管理"><ui5-icon name="add-document"></ui5-icon></button>
      <button class="rpt-mini danger" data-act="delete" data-code="${esc(r.code)}" title="删除报表"><ui5-icon name="delete"></ui5-icon></button>
    </div>
  </article>`
}

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
      ${kv('期间类型', periodLabel(r.period_type))}
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
    <div class="rpt-prop-sec">
      <b>版本列表</b>
      ${versions.length ? versions.map((v) => `<button class="rpt-version-row ${v.code === version ? 'active' : ''}" data-set-version="${esc(v.code)}">
        <span>${esc(v.code)}</span><b>${esc(v.name || v.version_status || '')}</b><em>${esc(v.change_summary || '')}</em>
      </button>`).join('') : '<p>默认版本。</p>'}
    </div>
    <div class="rpt-prop-sec">
      <b>备注</b>
      <p>${esc(r.remark || '暂无备注')}</p>
    </div>
  </section>`
}

function kv (label, value) {
  return `<div class="rpt-kv"><span>${esc(label)}</span><b>${esc(value || '-')}</b></div>`
}

function stat (label, value) {
  return `<div class="rpt-stat"><b>${Number(value || 0)}</b><span>${esc(label)}</span></div>`
}

function dialogHtml () {
  if (!state.dialog) return ''
  if (state.dialog.type === 'report') return reportDialogHtml()
  if (state.dialog.type === 'version') return versionDialogHtml()
  return ''
}

function dialogErrorHtml () {
  return state.dialog?.error ? `<div class="rpt-dialog-error">${esc(state.dialog.error)}</div>` : ''
}

function reportDialogHtml () {
  const category = state.dialog.category || state.selectedCategory || state.categories[0]?.code || 'management'
  const period = state.dialog.period || state.selectedPeriod || state.periods[0]?.code || 'month'
  const catOpts = state.categories.map((c) => `<option value="${esc(c.code)}" ${c.code === category ? 'selected' : ''}>${esc(c.name)} · ${esc(c.code)}</option>`).join('')
  const periodOpts = state.periods.map((p) => `<option value="${esc(p.code)}" ${p.code === period ? 'selected' : ''}>${esc(p.name)} · ${esc(p.code)}</option>`).join('')
  return `<div class="rpt-dialog-mask" data-dialog-mask>
    <section class="rpt-dialog report">
      <div class="rpt-dialog-head">
        <span><ui5-icon name="add-document"></ui5-icon></span>
        <div><b>新建报表</b><em>创建报表主档、分类口径与初始版本</em></div>
        <button class="rpt-mini" data-dialog-close title="关闭"><ui5-icon name="decline"></ui5-icon></button>
      </div>
      ${dialogErrorHtml()}
      <div class="rpt-dialog-body">
        <div class="rpt-form-section" style="--sec-color:var(--rpt-blue)">
          <div class="rpt-section-title"><span>01</span><b>基础信息</b><em>报表身份与显示名称</em></div>
          <div class="rpt-form-grid">
            ${fieldHtml('报表编码', 'code', 'input', 'BS_001', true)}
            ${fieldHtml('报表名称', 'name', 'input', '资产负债表', true)}
            ${selectFieldHtml('报表类型', 'report_type', `
              <option value="CUSTOM">自定义</option><option value="BS">资产负债表</option><option value="PL">利润表</option><option value="CF">现金流量表</option><option value="EQ">权益变动表</option>
            `, true)}
            ${fieldHtml('取数来源', 'data_source', 'input', 'gl_balance / manual', false)}
          </div>
        </div>
        <div class="rpt-form-section" style="--sec-color:var(--rpt-cyan)">
          <div class="rpt-section-title"><span>02</span><b>分类口径</b><em>决定列表归属与期间 tab</em></div>
          <div class="rpt-form-grid">
            ${selectFieldHtml('报表类别', 'report_category', catOpts, true)}
            ${selectFieldHtml('期间类型', 'period_type', periodOpts, true)}
            ${selectFieldHtml('编制口径', 'entity_scope', `
              <option value="single">单体</option><option value="consol">合并</option><option value="combined">汇总</option>
            `, false)}
            ${fieldHtml('币种', 'currency_code', 'input', 'CNY', false, 'CNY')}
            ${selectFieldHtml('金额单位', 'amount_unit', `
              <option value="yuan">元</option><option value="kyuan">千元</option><option value="wyuan">万元</option><option value="myuan">百万元</option>
            `, false)}
          </div>
        </div>
        <div class="rpt-form-section" style="--sec-color:var(--rpt-green)">
          <div class="rpt-section-title"><span>03</span><b>初始版本</b><em>生成第一版设计基线</em></div>
          <div class="rpt-form-grid">
            ${fieldHtml('初始版本', 'version_code', 'input', 'V1', false, 'V1')}
            ${fieldHtml('版本名称', 'version_name', 'input', '初始版本', false, '初始版本')}
            ${fieldHtml('备注', 'remark', 'textarea', '报表说明', false)}
          </div>
        </div>
      </div>
      <div class="rpt-dialog-actions">
        <button class="rpt-btn" data-dialog-close>取消</button>
        <button class="rpt-btn primary" data-dialog-submit="report"><ui5-icon name="accept"></ui5-icon>创建报表</button>
      </div>
    </section>
  </div>`
}

function versionDialogHtml () {
  const r = state.reports.find((x) => x.code === state.dialog.reportCode)
  if (!r) return ''
  const versions = normalizeArray(r.versions)
  const current = reportVersion(r)
  const next = Number(r.version_count || versions.length || 0) + 1
  const baseOpts = versions.length
    ? versions.map((v) => `<option value="${esc(v.code)}" ${v.code === current ? 'selected' : ''}>${esc(v.code)} · ${esc(v.name || '')}</option>`).join('')
    : '<option value="">默认版本</option>'
  const list = versions.length
    ? versions.map((v) => `<div class="rpt-version-manage-row ${v.code === current ? 'active' : ''}">
        <div><b class="rpt-version-code">${esc(v.code)}</b><span>${esc(v.name || v.version_status || '')}</span><em>${esc(v.change_summary || '暂无说明')}</em></div>
        <button class="rpt-btn slim ${Number(v.is_current || 0) === 1 ? 'is-default' : ''}" data-set-default="${esc(v.code)}">${Number(v.is_current || 0) === 1 ? '默认版本' : '设为默认'}</button>
      </div>`).join('')
    : `<div class="rpt-version-manage-empty"><cmx-empty-state icon="flag" title="默认版本" description="该报表尚未创建显式版本，系统按默认版本展示。" size="sm"></cmx-empty-state></div>`
  return `<div class="rpt-dialog-mask" data-dialog-mask>
    <section class="rpt-dialog version">
      <div class="rpt-dialog-head">
        <span><ui5-icon name="add-document"></ui5-icon></span>
        <div><b>版本管理</b><em>${esc(r.name)} · ${esc(r.code)}</em></div>
        <button class="rpt-mini" data-dialog-close title="关闭"><ui5-icon name="decline"></ui5-icon></button>
      </div>
      ${dialogErrorHtml()}
      <div class="rpt-version-manage">
        <div class="rpt-version-panel">
          <div class="rpt-version-panel-title"><div><b>版本序列</b><span>${versions.length ? `${versions.length} 个版本` : '默认版本'}</span></div><em>${esc(versionLabel(current))}</em></div>
          <div class="rpt-version-list">${list}</div>
        </div>
        <div class="rpt-version-form">
          <div class="rpt-section-title"><span>+</span><b>新建版本</b><em>从现有版本复制派生</em></div>
          <div class="rpt-dialog-grid compact">
            ${fieldHtml('版本编码', 'code', 'input', `V${next}`, true, `V${next}`)}
            ${fieldHtml('版本名称', 'name', 'input', `版本 ${next}`, true, `版本 ${next}`)}
            ${selectFieldHtml('源版本', 'base_version_code', baseOpts, false)}
            ${fieldHtml('变更说明', 'change_summary', 'textarea', '说明本版本调整内容', false)}
          </div>
          <label class="rpt-check"><input type="checkbox" data-field="is_current"> 创建后设为默认版本</label>
        </div>
      </div>
      <div class="rpt-dialog-actions">
        <button class="rpt-btn" data-dialog-close>关闭</button>
        <button class="rpt-btn primary" data-dialog-submit="version"><ui5-icon name="accept"></ui5-icon>新建版本</button>
      </div>
    </section>
  </div>`
}

function fieldHtml (label, name, type, placeholder, required, value = '') {
  const req = required ? '<i>*</i>' : ''
  if (type === 'textarea') {
    return `<label class="rpt-field wide"><span>${esc(label)}${req}</span><textarea data-field="${esc(name)}" placeholder="${esc(placeholder)}">${esc(value)}</textarea></label>`
  }
  return `<label class="rpt-field"><span>${esc(label)}${req}</span><input data-field="${esc(name)}" value="${esc(value)}" placeholder="${esc(placeholder)}"></label>`
}

function selectFieldHtml (label, name, options, required) {
  const req = required ? '<i>*</i>' : ''
  return `<label class="rpt-field"><span>${esc(label)}${req}</span><select data-field="${esc(name)}">${options}</select></label>`
}

function bind (root) {
  root.querySelectorAll('[data-act="refresh"]').forEach((btn) => btn.addEventListener('click', () => loadData(true)))
  root.querySelectorAll('[data-cat]').forEach((btn) => btn.addEventListener('click', () => {
    state.selectedCategory = btn.getAttribute('data-cat') || ''
    const first = filteredReports()[0]
    if (first) state.selectedCode = first.code
    refreshAll()
  }))
  root.querySelectorAll('[data-period]').forEach((btn) => btn.addEventListener('click', () => {
    state.selectedPeriod = btn.getAttribute('data-period') || ''
    const first = filteredReports()[0]
    if (first) state.selectedCode = first.code
    refreshAll()
  }))
  root.querySelector('[data-query]')?.addEventListener('input', (ev) => {
    state.query = ev.target.value || ''
    refreshAll()
  })
  root.querySelectorAll('.rpt-card').forEach((card) => card.addEventListener('click', (ev) => {
    if (ev.target.closest('button') || ev.target.closest('select')) return
    selectReport(card.getAttribute('data-code'))
  }))
  root.querySelectorAll('[data-version]').forEach((sel) => sel.addEventListener('change', () => {
    const code = sel.getAttribute('data-version')
    state.selectedVersion[code] = sel.value
    if (state.selectedCode === code) ensureDetail(code, sel.value)
    refreshAll()
  }))
  root.querySelectorAll('[data-set-version]').forEach((btn) => btn.addEventListener('click', () => {
    const r = selectedReport()
    if (!r) return
    state.selectedVersion[r.code] = btn.getAttribute('data-set-version') || ''
    ensureDetail(r.code, state.selectedVersion[r.code])
    refreshAll()
  }))
  root.querySelectorAll('[data-act="new"]').forEach((btn) => btn.addEventListener('click', createReport))
  root.querySelectorAll('[data-act="open"]').forEach((btn) => btn.addEventListener('click', (ev) => {
    ev.stopPropagation()
    openReportDesigner(btn.getAttribute('data-code'), btn).catch((err) => {
      state.message = '打开报表设计器失败：' + (err.message || err)
      refreshAll()
    })
  }))
  root.querySelectorAll('[data-act="version"]').forEach((btn) => btn.addEventListener('click', (ev) => {
    ev.stopPropagation()
    createVersion(btn.getAttribute('data-code'))
  }))
  root.querySelectorAll('[data-act="delete"]').forEach((btn) => btn.addEventListener('click', (ev) => {
    ev.stopPropagation()
    deleteReport(btn.getAttribute('data-code'))
  }))
  root.querySelectorAll('[data-dialog-close]').forEach((btn) => btn.addEventListener('click', closeDialog))
  root.querySelectorAll('[data-dialog-submit]').forEach((btn) => btn.addEventListener('click', () => {
    const kind = btn.getAttribute('data-dialog-submit')
    if (kind === 'report') submitReportDialog(root)
    if (kind === 'version') submitVersionDialog(root)
  }))
  root.querySelectorAll('[data-set-default]').forEach((btn) => btn.addEventListener('click', () => setDefaultVersion(btn.getAttribute('data-set-default'))))
}

function selectReport (code) {
  if (!code) return
  state.selectedCode = code
  const r = selectedReport()
  if (r) ensureDetail(r.code, reportVersion(r))
  refreshAll()
}

async function createReport () {
  state.dialog = {
    type: 'report',
    category: state.selectedCategory || state.categories[0]?.code || 'management',
    period: state.selectedPeriod || state.periods[0]?.code || 'month',
    error: '',
  }
  refreshAll()
}

function closeDialog () {
  state.dialog = null
  refreshAll()
}

function dialogValues (root) {
  const values = {}
  root.querySelectorAll('[data-field]').forEach((el) => {
    const key = el.getAttribute('data-field')
    values[key] = el.type === 'checkbox' ? !!el.checked : String(el.value || '').trim()
  })
  return values
}

function setDialogError (message) {
  state.dialog = { ...(state.dialog || {}), error: message }
  refreshAll()
}

async function submitReportDialog (root) {
  const values = dialogValues(root)
  if (!values.code) return setDialogError('请填写报表编码。')
  if (!values.name) return setDialogError('请填写报表名称。')
  if (!values.report_category) return setDialogError('请选择报表类别。')
  if (!values.period_type) return setDialogError('请选择期间类型。')
  try {
    await apiJson('/api/report-design/reports', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        code: values.code,
        name: values.name,
        report_category: values.report_category,
        period_type: values.period_type,
        report_type: values.report_type || 'CUSTOM',
        entity_scope: values.entity_scope || 'single',
        currency_code: values.currency_code || 'CNY',
        amount_unit: values.amount_unit || 'yuan',
        version_code: values.version_code || 'V1',
        version_name: values.version_name || '初始版本',
        data_source: values.data_source || null,
        remark: values.remark || null,
      }),
    })
    state.selectedCode = values.code
    state.selectedCategory = values.report_category
    state.selectedPeriod = values.period_type
    state.selectedVersion[values.code] = values.version_code || 'V1'
    state.dialog = null
    state.details = {}
    await loadData(true)
  } catch (err) {
    setDialogError('新增报表失败：' + (err.message || err))
  }
}

async function createVersion (code) {
  const r = state.reports.find((x) => x.code === code)
  if (!r) return
  state.selectedCode = code
  state.dialog = { type: 'version', reportCode: code, error: '' }
  refreshAll()
}

async function submitVersionDialog (root) {
  const code = state.dialog?.reportCode
  const r = state.reports.find((x) => x.code === code)
  if (!r) return setDialogError('未找到当前报表。')
  const values = dialogValues(root)
  if (!values.code) return setDialogError('请填写版本编码。')
  if (!values.name) return setDialogError('请填写版本名称。')
  try {
    await apiJson(`/api/report-design/reports/${enc(code)}/versions`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        code: values.code,
        name: values.name,
        base_version_code: values.base_version_code || null,
        change_summary: values.change_summary || null,
        is_current: !!values.is_current,
      }),
    })
    state.selectedCode = code
    state.selectedVersion[code] = values.code
    state.dialog = { type: 'version', reportCode: code, error: '' }
    state.details = {}
    await loadData(true)
  } catch (err) {
    setDialogError('创建版本失败：' + (err.message || err))
  }
}

async function setDefaultVersion (version) {
  const code = state.dialog?.reportCode
  if (!code || !version) return
  try {
    await apiJson(`/api/report-design/reports/${enc(code)}/versions/${enc(version)}/default`, { method: 'POST' })
    state.selectedVersion[code] = version
    state.details = {}
    state.dialog = { type: 'version', reportCode: code, error: '' }
    await loadData(true)
  } catch (err) {
    setDialogError('设置默认版本失败：' + (err.message || err))
  }
}

async function deleteReport (code) {
  const r = state.reports.find((x) => x.code === code)
  if (!r) return
  const C = (typeof globalThis !== 'undefined' && globalThis.__cmxDataComp) || {}
  const ok = typeof C.cmxConfirm === 'function'
    ? await C.cmxConfirm({ message: `确认删除报表 ${r.name}（${r.code}）及其版本、Sheet、区域、行列和格式定义？`, intent: 'danger', confirmText: '删除' })
    : window.confirm(`确认删除报表 ${r.name}（${r.code}）及其版本、Sheet、区域、行列和格式定义？`)
  if (!ok) return
  try {
    await apiJson(`/api/report-design/reports/${enc(code)}`, { method: 'DELETE' })
    delete state.selectedVersion[code]
    state.details = {}
    if (state.selectedCode === code) state.selectedCode = ''
    await loadData(true)
  } catch (err) {
    state.message = '删除报表失败：' + (err.message || err)
    refreshAll()
  }
}

function styleCss () {
  return `
    .rpt{--rpt-blue:#0a6ed1;--rpt-cyan:#00a6c8;--rpt-green:#10a760;--rpt-amber:#d98200;--rpt-red:#bb0000;--rpt-border:var(--sapGroup_TitleBorderColor,#d9e2ec);
      height:100%;min-height:0;box-sizing:border-box;display:flex;flex-direction:column;background:var(--sapBackgroundColor,#f5f6f7);color:var(--sapTextColor,#1d2d3e);font:13px/1.45 var(--sapFontFamily,Arial,sans-serif);overflow:hidden}
    .rpt-head{height:46px;flex:0 0 auto;display:flex;align-items:center;gap:10px;padding:0 12px;border-bottom:1px solid var(--rpt-border);background:var(--sapList_HeaderBackground,#f7f9fc)}
    .rpt-head.compact{height:46px}.rpt-head b{display:block;font-size:14px}.rpt-head span{display:block;font-size:11px;color:var(--sapContent_LabelColor,#6a6d70)}.rpt-head-action{margin-left:auto}
    .rpt-title{min-width:0;display:grid;grid-template-columns:34px minmax(0,1fr);align-items:center;gap:9px}.rpt-title-ic{width:32px;height:32px;border-radius:8px;background:color-mix(in srgb,var(--period-color,var(--rpt-blue)) 13%,transparent);color:var(--period-color,var(--rpt-blue));display:flex!important;align-items:center;justify-content:center}.rpt-title-ic ui5-icon{width:1.08rem;height:1.08rem}.rpt-title-main{min-width:0;display:block!important}.rpt-title-main b,.rpt-title-main small{display:block;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.rpt-title-main small{font-size:11px;color:var(--sapContent_LabelColor,#6a6d70)}
    .rpt-toolbar{margin-left:auto;display:flex;align-items:center;gap:8px;min-width:0}.rpt-search{width:240px;max-width:38vw;height:30px;border:1px solid var(--rpt-border);border-radius:6px;padding:0 10px;background:var(--sapField_Background,#fff);color:inherit}
    .rpt-btn,.rpt-icon-btn,.rpt-mini{border:1px solid var(--rpt-border);background:var(--sapButton_Background,#fff);color:var(--sapButton_TextColor,#0a6ed1);border-radius:6px;cursor:pointer;display:inline-flex;align-items:center;justify-content:center;gap:6px;height:30px;padding:0 10px;font:inherit;font-size:12px}
    .rpt-btn.primary{background:linear-gradient(135deg,var(--rpt-blue),color-mix(in srgb,var(--rpt-blue) 70%,var(--rpt-cyan)));border-color:var(--rpt-blue);color:#fff;box-shadow:0 4px 14px color-mix(in srgb,var(--rpt-blue) 38%,transparent),inset 0 1px 0 rgba(255,255,255,.2);font-weight:700}.rpt-btn.primary:hover{filter:brightness(1.08);transform:translateY(-1px)}.rpt-btn:not(.primary):hover{border-color:color-mix(in srgb,var(--rpt-blue) 50%,var(--rpt-border));color:var(--rpt-blue)}.rpt-icon-btn,.rpt-mini{width:30px;padding:0}.rpt-mini.danger{color:var(--rpt-red)}.rpt-btn ui5-icon,.rpt-mini ui5-icon,.rpt-icon-btn ui5-icon{width:1rem;height:1rem}
    .rpt-cat-list{flex:1;min-height:0;overflow:auto;padding:8px;display:flex;flex-direction:column;gap:7px}.rpt-cat{position:relative;width:100%;border:1px solid color-mix(in srgb,var(--cat-color) 18%,var(--rpt-border));border-radius:8px;background:var(--sapTile_Background,#fff);display:grid;grid-template-columns:34px minmax(0,1fr) auto;gap:8px;align-items:center;text-align:left;padding:9px 10px;cursor:pointer;color:inherit}
    .rpt-cat:hover,.rpt-cat.active{border-color:color-mix(in srgb,var(--cat-color) 54%,var(--rpt-border));box-shadow:0 0 0 1px color-mix(in srgb,var(--cat-color) 14%,transparent)}.rpt-cat:hover{background:color-mix(in srgb,var(--cat-color) 5%,var(--sapTile_Background,#fff))}
    .rpt-cat-ic{width:32px;height:32px;border-radius:8px;display:flex;align-items:center;justify-content:center;background:color-mix(in srgb,var(--cat-color) 12%,transparent);color:var(--cat-color)}.rpt-cat-ic ui5-icon{width:1.1rem;height:1.1rem}.rpt-cat-main{min-width:0}.rpt-cat-main b{display:block;font-size:13px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.rpt-cat-main small{display:block;font-size:10px;color:var(--sapContent_LabelColor,#6a6d70)}.rpt-cat-count{font-weight:800;color:var(--cat-color)}
    .rpt-cat.active{border-color:color-mix(in srgb,var(--cat-color) 72%,var(--rpt-border));background:linear-gradient(90deg,color-mix(in srgb,var(--cat-color) 16%,var(--sapTile_Background,#fff)),var(--sapTile_Background,#fff));box-shadow:inset 4px 0 0 var(--cat-color),0 4px 14px color-mix(in srgb,var(--cat-color) 18%,transparent)}.rpt-cat.active .rpt-cat-ic{background:var(--cat-color);color:#fff;box-shadow:0 6px 14px color-mix(in srgb,var(--cat-color) 24%,transparent)}.rpt-cat.active .rpt-cat-main b{color:var(--cat-color);font-weight:800}.rpt-cat.active .rpt-cat-main small{color:color-mix(in srgb,var(--cat-color) 62%,var(--sapContent_LabelColor,#6a6d70))}.rpt-cat.active .rpt-cat-count{min-width:22px;height:22px;border-radius:999px;display:inline-flex;align-items:center;justify-content:center;background:var(--cat-color);color:#fff;padding:0 6px;box-shadow:0 5px 12px color-mix(in srgb,var(--cat-color) 22%,transparent)}
    .rpt-main{flex:1;min-height:0;display:grid;grid-template-rows:auto minmax(0,1fr);overflow:hidden;background:linear-gradient(180deg,color-mix(in srgb,var(--period-color) 6%,var(--sapList_HeaderBackground,#f7f9fc)),var(--sapBackgroundColor,#f5f6f7))}.rpt-periods{border-bottom:1px solid color-mix(in srgb,var(--period-color) 24%,var(--rpt-border));background:color-mix(in srgb,var(--sapTile_Background,#fff) 78%,var(--period-color));padding:8px 12px 0;display:flex;align-items:flex-end;gap:7px;overflow-x:auto;overflow-y:hidden}.rpt-period{height:40px;min-width:104px;border:1px solid color-mix(in srgb,var(--tab-color) 20%,var(--rpt-border));border-bottom:0;border-radius:9px 9px 0 0;background:linear-gradient(180deg,color-mix(in srgb,var(--tab-color) 8%,var(--sapTile_Background,#fff)),color-mix(in srgb,var(--tab-color) 2%,var(--sapTile_Background,#fff)));color:inherit;display:grid;grid-template-columns:18px minmax(0,1fr) auto;align-items:center;gap:6px;padding:0 10px;cursor:pointer;text-align:left;position:relative;transform:translateY(1px)}.rpt-period::before{content:"";position:absolute;left:0;right:0;top:0;height:3px;border-radius:9px 9px 0 0;background:var(--tab-color);opacity:.72}.rpt-period ui5-icon{width:.95rem;height:.95rem;color:var(--tab-color)}.rpt-period span{font-weight:800;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.rpt-period b{min-width:20px;height:20px;border-radius:999px;display:inline-flex;align-items:center;justify-content:center;font-size:11px;color:var(--tab-color);background:color-mix(in srgb,var(--tab-color) 10%,var(--sapTile_Background,#fff))}.rpt-period:hover{background:linear-gradient(180deg,color-mix(in srgb,var(--tab-color) 14%,var(--sapTile_Background,#fff)),var(--sapTile_Background,#fff))}.rpt-period.active{z-index:1;height:44px;background:var(--sapBackgroundColor,#f5f6f7);border-color:color-mix(in srgb,var(--tab-color) 52%,var(--rpt-border));box-shadow:0 -2px 0 var(--tab-color),0 -6px 16px color-mix(in srgb,var(--tab-color) 12%,transparent)}.rpt-period.active::before{height:4px;opacity:1}.rpt-period.active::after{content:"";position:absolute;left:0;right:0;bottom:-1px;height:1px;background:var(--sapBackgroundColor,#f5f6f7)}.rpt-period.active b{background:var(--tab-color);color:#fff}
    .rpt-list{min-height:0;overflow:auto;padding:10px 12px 18px;display:flex;flex-direction:column;gap:8px;background:linear-gradient(180deg,color-mix(in srgb,var(--period-color) 7%,var(--sapBackgroundColor,#f5f6f7)),var(--sapBackgroundColor,#f5f6f7))}.rpt-card{position:relative;flex:0 0 auto;display:grid;grid-template-columns:5px 38px minmax(0,1fr) auto;gap:10px;align-items:center;border:1px solid color-mix(in srgb,var(--period-color) 16%,var(--rpt-border));border-radius:8px;background:var(--sapTile_Background,#fff);padding:10px;cursor:pointer;overflow:hidden}.rpt-card:hover{border-color:color-mix(in srgb,var(--period-color) 48%,var(--rpt-border));box-shadow:0 2px 10px color-mix(in srgb,var(--period-color) 18%,transparent)}.rpt-card.active{border-color:color-mix(in srgb,var(--period-color) 72%,var(--rpt-border));background:linear-gradient(90deg,color-mix(in srgb,var(--period-color) 16%,var(--sapTile_Background,#fff)),color-mix(in srgb,var(--period-color) 5%,var(--sapTile_Background,#fff)) 56%,var(--sapTile_Background,#fff));box-shadow:inset 0 0 0 1px color-mix(in srgb,var(--period-color) 38%,transparent),0 6px 18px color-mix(in srgb,var(--period-color) 20%,transparent)}.rpt-card.active::after{content:"";position:absolute;right:0;top:0;border-top:22px solid var(--period-color);border-left:22px solid transparent}.rpt-card.active .rpt-card-bar{width:7px;box-shadow:0 0 14px color-mix(in srgb,var(--period-color) 48%,transparent)}.rpt-card.active .rpt-card-ic{background:var(--period-color);color:#fff;box-shadow:0 6px 14px color-mix(in srgb,var(--period-color) 26%,transparent)}.rpt-card.active .rpt-card-title b{color:var(--period-color);font-weight:800}.rpt-card.active .rpt-card-code{border-color:color-mix(in srgb,var(--period-color) 32%,var(--rpt-border));background:color-mix(in srgb,var(--period-color) 10%,var(--sapTile_Background,#fff))}.rpt-card-bar{align-self:stretch;border-radius:5px;background:linear-gradient(180deg,var(--period-color),color-mix(in srgb,var(--period-color) 62%,#ffffff))}.rpt-card-ic{width:36px;height:36px;border-radius:8px;background:color-mix(in srgb,var(--period-color) 13%,transparent);color:var(--period-color);display:flex;align-items:center;justify-content:center}.rpt-card-ic ui5-icon{width:1.2rem;height:1.2rem}.rpt-card-main{min-width:0}.rpt-card-title{display:flex;align-items:center;gap:8px;min-width:0}.rpt-card-title b{font-size:14px;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.rpt-card-code{width:max-content;max-width:100%;display:flex;align-items:center;gap:6px;margin-top:3px;border:1px solid color-mix(in srgb,var(--period-color) 18%,var(--rpt-border));border-radius:5px;background:color-mix(in srgb,var(--period-color) 5%,var(--sapList_HeaderBackground,#f7f9fc));padding:2px 6px}.rpt-card-code span{font:800 9px/1 ui-monospace,Menlo,Consolas,monospace;color:var(--period-color)}.rpt-card-code b{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font:800 11px/1 ui-monospace,Menlo,Consolas,monospace;color:var(--sapTextColor,#1d2d3e)}.rpt-card-sub{font-size:11px;color:var(--sapContent_LabelColor,#6a6d70);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;margin-top:3px}.rpt-card-tags{display:flex;flex-wrap:wrap;gap:5px;margin-top:6px}.rpt-card-tags span{display:inline-flex;align-items:center;gap:4px;font-size:10px;border:1px solid color-mix(in srgb,var(--period-color) 15%,var(--rpt-border));border-radius:5px;padding:2px 6px;background:color-mix(in srgb,var(--period-color) 5%,var(--sapList_HeaderBackground,#f7f9fc))}.rpt-card-tags ui5-icon{width:.78rem;height:.78rem;color:var(--period-color)}.rpt-card-actions{display:flex;align-items:center;gap:6px}.rpt-version{height:30px;max-width:150px;border:1px solid var(--rpt-border);border-radius:6px;background:var(--sapField_Background,#fff);color:inherit;font-size:12px}
    .rpt-card-title span{flex:0 1 auto;min-width:0;max-width:48%;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font:800 11px/1 ui-monospace,Menlo,Consolas,monospace;color:#fff;background:var(--period-color);border-radius:5px;padding:3px 6px}.rpt-card.active .rpt-card-title span{box-shadow:0 3px 9px color-mix(in srgb,var(--period-color) 20%,transparent)}
    .rpt-prop{padding:10px;gap:10px;overflow:auto}.rpt-prop-hero{display:flex;gap:10px;align-items:center;border:1px solid var(--rpt-border);border-radius:8px;background:linear-gradient(135deg,color-mix(in srgb,var(--rpt-blue) 12%,var(--sapTile_Background,#fff)),var(--sapTile_Background,#fff));padding:12px}.rpt-prop-ic{width:40px;height:40px;border-radius:9px;display:flex;align-items:center;justify-content:center;background:var(--rpt-blue);color:#fff}.rpt-prop-ic ui5-icon{width:1.35rem;height:1.35rem}.rpt-prop-hero b{display:block;font-size:15px}.rpt-prop-hero span{font-size:11px;color:var(--sapContent_LabelColor,#6a6d70)}
    .rpt-prop-grid{display:grid;grid-template-columns:1fr;gap:6px}.rpt-kv{border:1px solid var(--rpt-border);border-radius:7px;background:var(--sapTile_Background,#fff);padding:7px 9px}.rpt-kv span{display:block;font-size:10px;color:var(--sapContent_LabelColor,#6a6d70)}.rpt-kv b{display:block;font-size:12px;word-break:break-word}.rpt-prop-sec{border:1px solid var(--rpt-border);border-radius:8px;background:var(--sapTile_Background,#fff);padding:10px}.rpt-prop-sec>b{display:block;margin-bottom:7px}.rpt-prop-sec p{margin:0;color:var(--sapContent_LabelColor,#6a6d70);font-size:12px}.rpt-chip{display:inline-flex;margin:0 5px 5px 0;border:1px solid var(--rpt-border);border-radius:6px;padding:3px 7px;font-size:11px;background:var(--sapList_HeaderBackground,#f7f9fc)}.rpt-chip.strong{color:#fff;background:var(--rpt-green);border-color:var(--rpt-green)}
    .rpt-stat-grid{display:grid;grid-template-columns:repeat(5,minmax(0,1fr));gap:6px}.rpt-stat{border:1px solid var(--rpt-border);border-radius:7px;padding:7px 5px;text-align:center;background:var(--sapList_HeaderBackground,#f7f9fc)}.rpt-stat b{display:block;font-size:15px;color:var(--rpt-blue)}.rpt-stat span{font-size:10px;color:var(--sapContent_LabelColor,#6a6d70)}.rpt-version-row{width:100%;border:1px solid var(--rpt-border);border-radius:7px;background:var(--sapList_HeaderBackground,#f7f9fc);color:inherit;text-align:left;padding:7px 8px;margin-bottom:6px;cursor:pointer;display:grid;grid-template-columns:54px minmax(0,1fr);gap:6px}.rpt-version-row.active{border-color:var(--rpt-blue);background:color-mix(in srgb,var(--rpt-blue) 8%,var(--sapTile_Background,#fff))}.rpt-version-row span{font:700 11px/1.4 ui-monospace,Menlo,Consolas,monospace;color:var(--rpt-blue)}.rpt-version-row b{font-size:12px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.rpt-version-row em{grid-column:1 / -1;font-style:normal;font-size:11px;color:var(--sapContent_LabelColor,#6a6d70);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
    .rpt-dialog-mask{position:absolute;inset:0;z-index:20;display:flex;align-items:center;justify-content:center;background:rgba(6,12,22,.55);backdrop-filter:blur(4px);padding:18px;box-sizing:border-box;--rpt-blue:#0a6ed1;--rpt-cyan:#00a6c8;--rpt-green:#10a760;--rpt-amber:#d98200;--rpt-red:#e5484d;--rpt-border:var(--sapGroup_TitleBorderColor,#d9e2ec)}.rpt-dialog{width:min(760px,calc(100vw - 48px));max-height:calc(100vh - 64px);overflow:auto;border:1px solid color-mix(in srgb,var(--rpt-blue) 28%,var(--rpt-border));border-radius:8px;background:var(--sapTile_Background,#fff);box-shadow:0 18px 46px rgba(0,0,0,.22)}.rpt-dialog.version{width:min(860px,calc(100vw - 48px))}.rpt-dialog-head{height:56px;display:grid;grid-template-columns:38px minmax(0,1fr) 30px;gap:10px;align-items:center;padding:0 12px;border-bottom:1px solid var(--rpt-border);background:linear-gradient(135deg,color-mix(in srgb,var(--rpt-blue) 10%,var(--sapList_HeaderBackground,#f7f9fc)),var(--sapList_HeaderBackground,#f7f9fc))}.rpt-dialog-head>span{width:34px;height:34px;border-radius:8px;display:flex;align-items:center;justify-content:center;background:var(--rpt-blue);color:#fff}.rpt-dialog-head ui5-icon{width:1rem;height:1rem}.rpt-dialog-head b{display:block;font-size:15px}.rpt-dialog-head em{display:block;font-style:normal;font-size:11px;color:var(--sapContent_LabelColor,#6a6d70);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.rpt-dialog-error{margin:10px 14px 0;border:1px solid color-mix(in srgb,var(--rpt-red) 45%,var(--rpt-border));background:color-mix(in srgb,var(--rpt-red) 14%,var(--sapTile_Background,#1d232a));color:color-mix(in srgb,var(--rpt-red) 78%,var(--sapTextColor,#f5f6f7));border-radius:8px;padding:8px 11px;font-size:12px;display:flex;align-items:center;gap:7px}.rpt-dialog-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:10px;padding:12px}.rpt-dialog-grid.compact{grid-template-columns:1fr;gap:8px;padding:8px 0 0}.rpt-field{min-width:0;display:flex;flex-direction:column;gap:4px}.rpt-field.wide{grid-column:1 / -1}.rpt-field span{font-size:11px;color:var(--sapContent_LabelColor,#6a6d70)}.rpt-field i{font-style:normal;color:var(--rpt-red);margin-left:3px}.rpt-field input,.rpt-field select,.rpt-field textarea{width:100%;box-sizing:border-box;border:1px solid var(--rpt-border);border-radius:7px;background:var(--sapField_Background,#fff);color:inherit;min-height:32px;padding:6px 8px;font:inherit;font-size:12px}.rpt-field textarea{min-height:72px;resize:vertical}.rpt-dialog-actions{position:sticky;bottom:0;display:flex;justify-content:flex-end;gap:8px;padding:10px 12px;border-top:1px solid var(--rpt-border);background:var(--sapTile_Background,#fff)}.rpt-version-manage{display:grid;grid-template-columns:minmax(0,1.1fr) minmax(260px,.9fr);gap:12px;padding:12px}.rpt-version-list{display:flex;flex-direction:column;gap:8px;min-height:0}.rpt-version-manage-row{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:8px;align-items:center;border:1px solid var(--rpt-border);border-radius:8px;background:var(--sapList_HeaderBackground,#f7f9fc);padding:8px}.rpt-version-manage-row.active{border-color:var(--rpt-blue);background:color-mix(in srgb,var(--rpt-blue) 8%,var(--sapTile_Background,#fff))}.rpt-version-manage-row b{display:block;color:var(--rpt-blue);font:700 12px/1.3 ui-monospace,Menlo,Consolas,monospace}.rpt-version-manage-row span{display:block;font-size:12px}.rpt-version-manage-row em{display:block;font-style:normal;font-size:11px;color:var(--sapContent_LabelColor,#6a6d70);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.rpt-btn.slim{height:28px;padding:0 8px;font-size:11px}.rpt-version-form{border:1px solid var(--rpt-border);border-radius:8px;background:var(--sapTile_Background,#fff);padding:10px}.rpt-version-form>b{display:block;margin-bottom:4px}.rpt-version-manage-empty{border:1px dashed var(--rpt-border);border-radius:8px;background:var(--sapList_HeaderBackground,#f7f9fc);padding:16px;text-align:center;color:var(--sapContent_LabelColor,#6a6d70)}.rpt-version-manage-empty ui5-icon{width:1.5rem;height:1.5rem;color:var(--rpt-blue)}.rpt-version-manage-empty b{display:block;color:var(--sapTextColor,#1d2d3e);margin-top:5px}.rpt-check{display:flex;align-items:center;gap:7px;margin-top:8px;font-size:12px;color:var(--sapContent_LabelColor,#6a6d70)}
    .rpt-dialog{border-radius:14px;overflow:hidden;background:linear-gradient(180deg,color-mix(in srgb,var(--rpt-blue) 7%,var(--sapTile_Background,#1d232a)),var(--sapTile_Background,#1d232a));border:1px solid color-mix(in srgb,var(--rpt-blue) 30%,var(--rpt-border));box-shadow:0 30px 80px rgba(0,0,0,.5),inset 0 1px 0 rgba(255,255,255,.06)}.rpt-dialog.report{width:min(820px,calc(100vw - 48px))}.rpt-dialog.version{width:min(900px,calc(100vw - 48px))}.rpt-dialog-head{height:64px;padding:0 16px;background:linear-gradient(135deg,color-mix(in srgb,var(--rpt-blue) 32%,var(--sapTile_Background,#1d232a)),color-mix(in srgb,var(--rpt-cyan) 20%,var(--sapTile_Background,#1d232a)));border-bottom:1px solid color-mix(in srgb,var(--rpt-blue) 40%,var(--rpt-border));box-shadow:0 1px 0 color-mix(in srgb,var(--rpt-cyan) 30%,transparent)}.rpt-dialog-head>span{border-radius:10px;background:linear-gradient(135deg,var(--rpt-blue),var(--rpt-cyan));box-shadow:inset 0 0 0 1px rgba(255,255,255,.3),0 8px 20px rgba(10,110,209,.4)}.rpt-dialog-head b{font-size:16px;color:var(--sapTextColor,#f5f6f7);letter-spacing:.01em}.rpt-dialog-head em{color:color-mix(in srgb,var(--sapTextColor,#f5f6f7) 60%,transparent)}.rpt-dialog-body{padding:12px;display:flex;flex-direction:column;gap:10px}.rpt-form-section,.rpt-version-panel,.rpt-version-form{border:1px solid color-mix(in srgb,var(--sec-color,var(--rpt-blue)) 22%,var(--rpt-border));border-left:3px solid var(--sec-color,var(--rpt-blue));border-radius:10px;background:linear-gradient(180deg,color-mix(in srgb,var(--sec-color,var(--rpt-blue)) 7%,var(--sapTile_Background,#1d232a)),var(--sapTile_Background,#1d232a));box-shadow:inset 0 1px 0 rgba(255,255,255,.04),0 2px 10px rgba(0,0,0,.18)}.rpt-form-section{padding:12px 13px}.rpt-section-title{display:grid;grid-template-columns:28px minmax(0,auto) minmax(0,1fr);gap:7px;align-items:center;margin-bottom:8px}.rpt-section-title span{width:26px;height:24px;border-radius:7px;display:flex;align-items:center;justify-content:center;background:linear-gradient(135deg,var(--sec-color,var(--rpt-blue)),color-mix(in srgb,var(--sec-color,var(--rpt-blue)) 62%,#000));color:#fff;font:800 11px/1 ui-monospace,Menlo,Consolas,monospace;box-shadow:0 3px 8px color-mix(in srgb,var(--sec-color,var(--rpt-blue)) 40%,transparent)}.rpt-section-title b{font-size:13px;color:var(--sapTextColor,#f5f6f7)}.rpt-section-title em{color:color-mix(in srgb,var(--sec-color,var(--rpt-blue)) 62%,var(--sapContent_LabelColor,#8a94a0))}.rpt-section-title em{font-style:normal;font-size:11px;color:var(--sapContent_LabelColor,#6a6d70);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.rpt-form-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:9px 10px}.rpt-field span{font-weight:700;color:color-mix(in srgb,var(--sapTextColor,#f5f6f7) 82%,transparent);font-size:11px;letter-spacing:.02em}.rpt-field input,.rpt-field select,.rpt-field textarea{min-height:36px;border-radius:8px;border-color:color-mix(in srgb,var(--rpt-border) 70%,var(--rpt-blue));background:var(--sapField_Background,#161c22);color:var(--sapTextColor,#f5f6f7);transition:border-color .15s,box-shadow .15s,background .15s}.rpt-field input::placeholder,.rpt-field textarea::placeholder{color:color-mix(in srgb,var(--sapTextColor,#f5f6f7) 42%,transparent)}.rpt-field select{-webkit-appearance:none;appearance:none;background-image:url("data:image/svg+xml;charset=utf-8,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 12 12'%3E%3Cpath d='M2.5 4.5L6 8l3.5-3.5' fill='none' stroke='%238a94a0' stroke-width='1.6' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E");background-repeat:no-repeat;background-position:right 10px center;background-size:12px;padding-right:28px}.rpt-field input:hover,.rpt-field select:hover,.rpt-field textarea:hover{border-color:color-mix(in srgb,var(--rpt-blue) 45%,var(--rpt-border))}.rpt-field input:focus,.rpt-field select:focus,.rpt-field textarea:focus{outline:0;border-color:var(--rpt-blue);box-shadow:0 0 0 3px color-mix(in srgb,var(--rpt-blue) 22%,transparent);background:color-mix(in srgb,var(--rpt-blue) 6%,var(--sapField_Background,#161c22))}.rpt-dialog-actions{padding:12px 16px;border-top:1px solid color-mix(in srgb,var(--rpt-blue) 16%,var(--rpt-border));background:linear-gradient(180deg,transparent,color-mix(in srgb,var(--rpt-blue) 6%,var(--sapTile_Background,#1d232a)))}.rpt-version-manage{grid-template-columns:minmax(0,1.05fr) minmax(300px,.95fr);gap:12px;padding:12px}.rpt-version-panel{padding:10px;min-height:330px}.rpt-version-panel-title{display:flex;align-items:center;justify-content:space-between;gap:10px;margin-bottom:9px}.rpt-version-panel-title b{display:block;font-size:13px}.rpt-version-panel-title span{display:block;font-size:11px;color:var(--sapContent_LabelColor,#6a6d70)}.rpt-version-panel-title em{font-style:normal;border:1px solid color-mix(in srgb,var(--rpt-green) 42%,var(--rpt-border));border-radius:6px;padding:3px 7px;background:color-mix(in srgb,var(--rpt-green) 10%,var(--sapTile_Background,#fff));color:var(--rpt-green);font-size:11px;font-weight:700}.rpt-version-list{max-height:340px;overflow:auto;padding-right:2px}.rpt-version-manage-row{border-radius:9px;background:var(--sapTile_Background,#fff);padding:9px 10px;box-shadow:0 1px 5px rgba(10,31,68,.05)}.rpt-version-manage-row.active{box-shadow:inset 3px 0 0 var(--rpt-blue),0 2px 10px rgba(10,110,209,.10)}.rpt-version-code{letter-spacing:0}.rpt-btn.slim.is-default{border-color:color-mix(in srgb,var(--rpt-green) 46%,var(--rpt-border));background:color-mix(in srgb,var(--rpt-green) 12%,var(--sapTile_Background,#fff));color:var(--rpt-green);font-weight:700}.rpt-version-form{padding:11px}.rpt-dialog-grid.compact{padding:0;margin-top:4px}.rpt-check{border:1px solid var(--rpt-border);border-radius:8px;padding:8px;margin-top:10px;background:var(--sapList_HeaderBackground,#f7f9fc)}.rpt-version-manage-empty{min-height:210px;display:flex;flex-direction:column;align-items:center;justify-content:center}
    .rpt-msg{margin:8px 12px 0;border:1px solid #ffd7a8;background:#fff8ed;color:#8a4b00;border-radius:7px;padding:7px 9px;font-size:12px}.rpt-empty{flex:1;min-height:0;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:7px;color:var(--sapContent_LabelColor,#6a6d70);text-align:center}.rpt-empty.large{min-height:260px}.rpt-empty ui5-icon{width:1.8rem;height:1.8rem;color:var(--rpt-blue)}.rpt-empty b{color:var(--sapTextColor,#1d2d3e)}
  `
}

export default {
  defaultView: 'content',
  views: {
    async explorer (ctx) { return mount(ctx, 'explorer') },
    async content (ctx) { return mount(ctx, 'content') },
    async property (ctx) { return mount(ctx, 'property') },
  },
}
