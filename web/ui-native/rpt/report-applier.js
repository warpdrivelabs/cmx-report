/**
 * 报表应用器 —— native_pages 多实例页面（从报表应用工作台打开，数据消费侧）。
 *
 * 与报表设计器（portal.rpt.designer）互不影响：设计器只设计版式，应用器只跑数据。
 * 复用同一个 cmx-spreadjs-sheet 组件 + 同一批后端端点（layout 读 / data 读写），但不 import designer.js。
 *
 * props: { reportCode, reportName, version, orgCode, periodCode }
 * content ：SpreadJS 画布 + 顶部数据条（组织/期间徽标 + 取数 / 存数 / 导出）。
 * property：报表属性（只读）+ 数据状态。
 *
 * 打开即按版式端点渲染报表格式（BLOB→无损复原，无则初始骨架），用户点「取数」按 org+period
 * 装载单元格值（cr_cell_data），「存数」回写。多实例：每 (报表+版本+组织+期间) 一套实例。
 */

const instances = new Map()
const DEFAULT_REGION = '__default__'

const { escHtml: esc } = globalThis.__cmxDataComp // 共享转义（cmx-data-comp/lib/cmx-page-helpers.js；最严格五字符集合，文本/属性上下文皆安全）

const enc = (s) => encodeURIComponent(String(s ?? ''))

const { apiJson } = globalThis.__cmxDataComp // 共享 fetch 封装（cmx-data-comp/lib/cmx-page-helpers.js；信封解包+结构化错误）

function propsOf (ctx) {
  const p = ctx?.props || ctx?.host?.__props || {}
  return {
    reportCode: String(p.reportCode || p.code || '').trim(),
    reportName: String(p.reportName || p.name || '').trim(),
    version: String(p.version || '').trim(),
    orgCode: String(p.orgCode || '').trim(),
    periodCode: String(p.periodCode || '').trim(),
  }
}

function instanceKey (props) {
  // 每 (报表+版本+组织+期间) 一套实例，与 cr_cell_data 的键一致。
  return `${props.reportCode || 'UNKNOWN'}@@${props.version || ''}@@${props.orgCode || ''}@@${props.periodCode || ''}`
}

function getState (ctx) {
  const props = propsOf(ctx)
  const key = instanceKey(props)
  if (!instances.has(key)) {
    instances.set(key, {
      props,
      hosts: new Set(),
      report: null,
      reportLoading: false,
      contentHash: null,
      loadedCells: 0,
      dataLoaded: false,
      periods: [], // cr_acct_calendar（explorer 期间下拉）
      org: null, // 当前组织详情行（cr_consol_org）
      explorerLoading: false,
      explorerLoaded: false,
      // 当前选中期间：默认取传入的 periodCode，可在 explorer 下拉换
      curPeriod: props.periodCode || '',
      zoom: 1, // 顶栏缩放（会话级视图偏好；1=100%，范围 0.5~2）
      selectedCell: 'A1', // 公式栏名称框/坐标当前活动格
      selectedRange: 'A1', // 公式栏名称框当前选区（拖选时与活动格不同）
      // 设计器在版式里定义的「元素/取数公式/校验公式」映射（键=`sheet名!地址`），
      // 由 loadLayout 从 /layout 的 cellMap 回填。选中格时公式栏据此显公式+元素胶囊（对齐设计器续18）。
      cellMap: {},
      elements: [], // 数据元素目录（GET /elements）：元素胶囊按 code 显示名称
      elementsLoaded: false,
    })
  }
  const st = instances.get(key)
  st.props = props
  if (!st.curPeriod) st.curPeriod = props.periodCode || ''
  return st
}

function reportTitle (st) {
  const code = st.props.reportCode || ''
  const name = st.props.reportName || ''
  return name ? `${code}-${name}` : code || '未指定报表'
}

/** content tab 标签：报表名｜组织/期间（随期间切换更新）。 */
function tabLabelOf (st) {
  const code = String(st.props.reportCode || '').trim()
  const name = String(st.props.reportName || '').trim()
  const base = name ? `${code}-${name}` : code || '报表'
  const ctx = [st.props.orgCode, st.curPeriod || st.props.periodCode].filter(Boolean).join('/')
  return ctx ? `${base}｜${ctx}` : base
}

/** 深度穿透 shadow DOM 全局找 PORTAL-CONTENT-AREA（parent-walk 失败时兜底）。 */
function deepFindContentArea (root = document) {
  const stack = [root]
  for (let guard = 0; guard < 5000 && stack.length; guard++) {
    const node = stack.pop()
    if (!node) continue
    if (node.nodeType === 1) {
      const tag = node.tagName || ''
      if (tag === 'PORTAL-CONTENT-AREA' || (node._tabs && typeof node.getActiveTabId === 'function')) return node
      if (node.shadowRoot) stack.push(node.shadowRoot)
    }
    const kids = node.children
    if (kids) for (let i = 0; i < kids.length; i++) stack.push(kids[i])
  }
  return null
}

/** 从 native-page 宿主向上穿 shadow host 链，找到 PORTAL-CONTENT-AREA 组件（失败则全局兜底）。 */
function findContentArea (host) {
  let node = host
  for (let i = 0; i < 40 && node; i++) {
    const tag = node.tagName || ''
    if (tag === 'PORTAL-CONTENT-AREA' || (node._tabs && typeof node.getActiveTabId === 'function')) return node
    node = node.parentElement || (node.parentNode instanceof ShadowRoot ? node.parentNode.host : node.getRootNode?.()?.host) || null
  }
  return deepFindContentArea()
}

/** 本宿主所在 tab 的 id：向上找 dataset.cmxWorkspaceId="tab:<id>" 的挂载根，剥前缀。 */
function ownTabId (host) {
  let node = host
  for (let i = 0; i < 40 && node; i++) {
    const wsId = node.dataset?.cmxWorkspaceId || node.getAttribute?.('data-cmx-workspace-id')
    if (wsId && String(wsId).startsWith('tab:')) return String(wsId).slice(4)
    node = node.parentElement || (node.parentNode instanceof ShadowRoot ? node.parentNode.host : node.getRootNode?.()?.host) || null
  }
  return null
}

/** 设置/清除本报表 tab 的 dirty 标记（关闭时门户据此弹「是否保存」对话框）。 */
function markDirty (st, dirty) {
  st.dirty = !!dirty
  let done = false
  for (const host of Array.from(st.hosts)) {
    if (!host || !host.isConnected) continue
    const ca = findContentArea(host)
    if (!ca || typeof ca.setTabDirty !== 'function') continue
    const tabId = ownTabId(host) || (ca.getActiveTabId ? ca.getActiveTabId() : ca._activeTab)
    if (tabId) { try { ca.setTabDirty(tabId, !!dirty); done = true } catch (_) { /* 防御性忽略 */ } }
  }
  return done
}

/**
 * 期间切换后更新 content 区当前 tab 的显示标签。
 * 直接改 active .tab-item 的文本 span + 同步 _tabs[].text（不触发 renderTabs，避免销毁画布 CE）。
 */
function updateApplierTab (st) {
  const label = tabLabelOf(st)
  for (const host of Array.from(st.hosts)) {
    if (!host || !host.isConnected) continue
    if (host.__raView !== 'content') continue
    const ca = findContentArea(host)
    if (!ca) continue
    try {
      const activeId = ca.getActiveTabId ? ca.getActiveTabId() : ca._activeTab
      const items = ca.shadowRoot ? [...ca.shadowRoot.querySelectorAll('.tab-item')] : []
      const item = items.find((el) => el.dataset?.id === activeId)
      // .tab-item 内部：<span.tab-icon-stack>…</span><span>{text}</span><span.tab-close>…
      const textSpan = item ? [...item.querySelectorAll(':scope > span')].find((s) => !s.className) : null
      if (textSpan) textSpan.textContent = label
      const rec = ca._tabs?.find((t) => t.id === activeId)
      if (rec) rec.text = label
    } catch (_) { /* 防御性忽略 */ }
  }
}

function versionLabel (v) {
  return v || '默认版本'
}

function indexToCol (idx) {
  let n = Number(idx) + 1
  let s = ''
  while (n > 0) { const r = (n - 1) % 26; s = String.fromCharCode(65 + r) + s; n = Math.floor((n - 1) / 26) }
  return s || 'A'
}

function parseAddr (addr) {
  const m = /^([A-Z]+)(\d+)$/.exec(String(addr || '').toUpperCase())
  if (!m) return null
  let col = 0
  for (let i = 0; i < m[1].length; i++) col = col * 26 + (m[1].charCodeAt(i) - 64)
  return { col: col - 1, row: Number(m[2]) - 1 }
}

/** base64 SSJSON → 对象（UTF-8 安全） */
function decodeDoc (b64) {
  if (!b64) return null
  try {
    const bin = atob(b64)
    const bytes = new Uint8Array(bin.length)
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i)
    return JSON.parse(new TextDecoder().decode(bytes))
  } catch (_) { return null }
}

/** 当前在屏活动 sheet 名（切 sheet 后自然变）；取不到回退报表码/Sheet1。 */
function activeSheetName (st) {
  for (const host of Array.from(st.hosts || [])) {
    if (!host || !host.isConnected || host.__raView !== 'content') continue
    const root = host.renderRoot || host.shadowRoot?.querySelector('.native-page-root')
    const ws = root?.querySelector?.('[data-ra-spread]')?.getWorkbook?.()?.getActiveSheet?.()
    const nm = ws?.name ? ws.name() : ''
    if (nm) return nm
  }
  return st.props.reportCode || 'Sheet1'
}

/** cellMap 复合键 `sheet名!地址`（与设计器/后端 cr_cell_element_map 的 sheet_code|cell_ref 同构）。 */
function cellKey (st, addr, sheetCode) {
  const sc = sheetCode || activeSheetName(st)
  return `${sc}!${String(addr || '').toUpperCase()}`
}

/** 从 /layout 的 cellMap 回填 st.cellMap（键 `sheet名!地址`，各 sheet 同位不互覆）。对齐设计器 hydratePropsFromLayout。 */
function hydrateCellMap (st, data) {
  const raw = Array.isArray(data?.cellMap) ? data.cellMap : []
  const cm = {}
  for (const m of raw) {
    const ref = m.cell_ref
    if (!ref) continue
    const sc = m.sheet_code || st.props.reportCode || 'Sheet1'
    cm[`${sc}!${String(ref).toUpperCase()}`] = {
      elementCode: m.element_code || '',
      valueType: m.value_type || '',
      dataSource: m.data_source || '',
      calcFormula: m.calc_formula || '',
      checkFormula: m.check_formula || '',
      numberFormat: m.number_format || '',
    }
  }
  st.cellMap = cm
}

/** 懒加载数据元素目录（元素胶囊显示名称用；无则退化裸 code）。 */
async function loadElements (st) {
  if (st.elementsLoaded) return st.elements
  try {
    const data = await apiJson('/api/report-design/elements')
    st.elements = Array.isArray(data?.elements) ? data.elements : []
  } catch (_) { st.elements = [] }
  st.elementsLoaded = true
  return st.elements
}

/** 读某格已定义的表达式（供 fx 编辑器回显）：设计器取数公式优先，回退画布原生公式。去前导 =。 */
function readCellExpr (st, addr) {
  const cm = st.cellMap && st.cellMap[cellKey(st, addr)]
  if (cm && cm.calcFormula) return String(cm.calcFormula).replace(/^=+/, '')
  const ws = liveContentSheet(st)?.getWorkbook?.()?.getActiveSheet?.()
  const p = parseAddr(addr)
  if (ws && p) { try { const f = ws.getFormula(p.row, p.col); if (f) return String(f).replace(/^=+/, '') } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ } }
  return ''
}

/** 在屏 content 宿主的 sheet 组件（优先可见宿主，兜底首个连着的）。 */
function liveContentSheet (st) {
  let fallback = null
  for (const host of Array.from(st.hosts || [])) {
    if (!host || !host.isConnected || host.__raView !== 'content') continue
    const root = host.renderRoot || host.shadowRoot?.querySelector('.native-page-root')
    const el = root?.querySelector?.('[data-ra-spread]')
    if (!el) continue
    if (!fallback) fallback = el
    const visible = el.offsetParent !== null || (el.getClientRects && el.getClientRects().length > 0)
    if (visible) return el
  }
  return fallback
}

/** 初始骨架（无 BLOB 版式时兜底渲染）。 */
function skeletonModel (st) {
  return {
    meta: { reportCode: st.props.reportCode, reportName: st.props.reportName, version: st.props.version },
    sheets: [{
      id: 'sheet1',
      name: st.props.reportCode || 'Sheet1',
      grid: { rows: 60, cols: 18, colWidths: { A: 56, B: 160, C: 130 } },
      cells: {
        B1: { type: 'text', value: reportTitle(st), class: 'title' },
        B2: { type: 'text', value: '组织' }, C2: { type: 'text', value: st.props.orgCode || '' },
        B3: { type: 'text', value: '期间' }, C3: { type: 'text', value: st.props.periodCode || '' },
      },
    }],
  }
}

// ============================================================================
// 视图
// ============================================================================

function orgIcon (t) {
  return ({ group: 'company-view', subgroup: 'org-chart', entity: 'building', branch: 'building' })[t] || 'building'
}

/** 加载 explorer 所需：会计日历（期间下拉）+ 当前组织详情行。仅一次。 */
async function loadExplorer (st) {
  if (st.explorerLoading || st.explorerLoaded) return
  st.explorerLoading = true
  refreshInstance(st, (v) => v === 'explorer')
  try {
    const [cal, org] = await Promise.all([
      apiJson('/api/report-design/calendar'),
      apiJson('/api/report-design/consol-org'),
    ])
    st.periods = Array.isArray(cal?.periods) ? cal.periods : []
    const orgs = Array.isArray(org?.orgs) ? org.orgs : []
    st.org = orgs.find((o) => String(o.code) === String(st.props.orgCode)) || null
    st.explorerLoaded = true
  } catch (_) {
    // 静默：explorer 是辅助信息，取数仍可用传入的 period
  } finally {
    st.explorerLoading = false
    refreshInstance(st, (v) => v === 'explorer')
  }
}

function styleCss () {
  return `
    .ra{--ra-blue:#0a6ed1;--ra-cyan:#00a6c8;--ra-green:#10a760;--ra-amber:#d98200;--ra-accent:#0d9488;--ra-accent2:#14b8a6;--ra-border:var(--sapGroup_TitleBorderColor,#d9e2ec);
      height:100%;min-height:0;box-sizing:border-box;display:flex;flex-direction:column;overflow:hidden;background:var(--sapBackgroundColor,#f5f6f7);color:var(--sapTextColor,#1d2d3e);font:13px/1.45 var(--sapFontFamily,Arial,sans-serif)}
    .ra-head{height:46px;flex:0 0 auto;display:flex;align-items:center;gap:9px;padding:0 12px;border-bottom:1px solid color-mix(in srgb,var(--ra-accent) 26%,var(--ra-border));background:linear-gradient(180deg,color-mix(in srgb,var(--ra-accent) 16%,var(--sapList_HeaderBackground,#f7f9fc)),color-mix(in srgb,var(--ra-accent) 9%,var(--sapList_HeaderBackground,#f7f9fc)))}
    /* content 顶部（标题+工具栏+公式栏）用青绿「数据」标识区别于设计器的蓝色，青绿由主题色 color-mix 得出以自适应 light/dark */
    .ra-head,.ra-fxbar{--ra-blue:var(--ra-accent)}
    .ra-head .ra-head-ic{background:linear-gradient(135deg,var(--ra-accent2),var(--ra-accent));color:#fff;box-shadow:0 1px 4px color-mix(in srgb,var(--ra-accent) 46%,transparent)}
    .ra-head .ra-title b{color:color-mix(in srgb,var(--ra-accent) 62%,var(--sapTextColor,#1d2d3e))}
    .ra-head .ra-btn.primary{background:linear-gradient(180deg,var(--ra-accent2),var(--ra-accent));box-shadow:0 1px 2px color-mix(in srgb,var(--ra-accent) 40%,transparent)}
    .ra-head .ra-btn.primary:hover{background:linear-gradient(180deg,color-mix(in srgb,var(--ra-accent2) 88%,#fff),color-mix(in srgb,var(--ra-accent) 86%,#000))}
    .ra-head .ra-zoom-range:hover::-webkit-slider-thumb,.ra-head .ra-zoom-range:active::-webkit-slider-thumb{box-shadow:0 2px 8px color-mix(in srgb,var(--ra-accent) 46%,transparent),0 0 0 4px color-mix(in srgb,var(--ra-accent) 20%,transparent)}
    .ra-head .ra-zoom-range:hover::-moz-range-thumb,.ra-head .ra-zoom-range:active::-moz-range-thumb{box-shadow:0 2px 8px color-mix(in srgb,var(--ra-accent) 46%,transparent),0 0 0 4px color-mix(in srgb,var(--ra-accent) 20%,transparent)}
    /* Excel 样式公式栏：名称框 | fx | 内容编辑器（复制自报表设计器） */
    .ra-fxbar{flex:0 0 auto;display:flex;align-items:center;height:32px;padding:0 10px;border-bottom:1px solid color-mix(in srgb,var(--ra-accent) 20%,var(--ra-border));background:color-mix(in srgb,var(--ra-accent) 6%,var(--sapTile_Background,#fff));box-shadow:inset 0 -1px 0 color-mix(in srgb,var(--ra-accent) 30%,transparent)}
    .ra-namebox{position:relative;flex:0 0 auto;width:124px;height:24px;display:flex;align-items:center;border:1px solid color-mix(in srgb,var(--ra-blue) 24%,var(--ra-border));border-radius:5px;background:var(--sapField_Background,#fff);box-shadow:inset 0 1px 2px rgba(10,31,68,.04)}
    .ra-namebox:focus-within{border-color:var(--ra-blue);box-shadow:0 0 0 2px color-mix(in srgb,var(--ra-blue) 18%,transparent)}
    .ra-namebox-input{flex:1;min-width:0;height:100%;border:0;outline:0;background:transparent;padding:0 4px 0 8px;font:700 12px/1 ui-monospace,Menlo,Consolas,monospace;letter-spacing:.02em;color:var(--ra-blue)}
    .ra-namebox-caret{flex:0 0 auto;padding:0 6px 0 2px;font-size:9px;color:color-mix(in srgb,var(--ra-blue) 60%,#888);pointer-events:none}
    .ra-fxbar-sep{flex:0 0 auto;width:1px;height:18px;background:var(--ra-border);margin:0 8px}
    .ra-fx-btn{flex:0 0 auto;height:24px;min-width:30px;padding:0 8px;border:1px solid transparent;border-radius:5px;background:transparent;color:var(--sapContent_LabelColor,#5b6b7b);cursor:pointer;display:inline-flex;align-items:center;justify-content:center;transition:background .12s,color .12s,box-shadow .12s}
    .ra-fx-btn i{font:italic 700 13px/1 "Times New Roman",Georgia,serif;letter-spacing:.02em}
    .ra-fx-btn:hover{background:color-mix(in srgb,var(--ra-blue) 12%,transparent);color:var(--ra-blue);box-shadow:0 1px 3px rgba(10,31,68,.12)}
    .ra-fx-btn:active{background:color-mix(in srgb,var(--ra-blue) 20%,transparent)}
    .ra-fxbar-input{flex:1;min-width:0;height:24px;border:0;outline:0;background:transparent;padding:0 6px;font:13px/1 var(--sapFontFamily,Arial,sans-serif);color:var(--sapTextColor,#1d2d3e)}
    .ra-fxbar-input::placeholder{color:var(--sapContent_LabelColor,#9aa4b0);font-style:italic}
    .ra-fxbar-input:focus{background:color-mix(in srgb,var(--ra-blue) 5%,transparent);border-radius:5px}
    /* 内容框左侧数据元素只读胶囊（绿，有绑定才显）——公式在右可编辑，元素在左只读（对齐设计器）。 */
    .ra-fxbar-elem{flex:0 0 auto;display:inline-flex;align-items:center;gap:5px;max-width:230px;height:22px;margin-right:8px;padding:0 9px;border:1px solid color-mix(in srgb,var(--ra-green) 40%,var(--ra-border));border-radius:999px;background:color-mix(in srgb,var(--ra-green) 12%,var(--sapTile_Background,#fff));color:var(--ra-green);font:700 12px/1 var(--sapFontFamily,Arial,sans-serif);white-space:nowrap;cursor:default}
    .ra-fxbar-elem[hidden]{display:none}
    .ra-fxbar-elem ui5-icon{flex:0 0 auto;width:.82rem;height:.82rem;color:var(--ra-green)}
    .ra-fxbar-elem [data-ra-fxelem-text]{min-width:0;overflow:hidden;text-overflow:ellipsis}
    .ra-head-ic{width:30px;height:30px;border-radius:8px;display:flex;align-items:center;justify-content:center;background:color-mix(in srgb,var(--ra-blue) 12%,transparent);color:var(--ra-blue)}.ra-head-ic ui5-icon{width:1rem;height:1rem}
    .ra-title{min-width:0}.ra-title>b,.ra-title>span{display:block;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.ra-title b{font-size:14px}.ra-title>span{font-size:11px;color:var(--sapContent_LabelColor,#6a6d70)}
    /* 第二行：版本 / 组织 / 期间 三气泡（自适应 light/dark，青绿数据标识）
       ★ 用 .ra-title>span.ra-subrow 提高优先级，压过上一行 .ra-title>span{display:block}——
         否则 subrow 退化成 block、align-items 失效，三气泡按基线对齐、版本气泡偏下。 */
    .ra-title>span.ra-subrow{display:flex;align-items:center;gap:6px;min-width:0;flex-wrap:wrap;overflow:visible}
    .ra-ctx-chip{flex:0 0 auto;display:inline-flex;align-items:center;justify-content:center;gap:3px;height:18px;box-sizing:border-box;padding:0 7px;border-radius:999px;border:1px solid color-mix(in srgb,var(--ra-accent) 34%,var(--ra-border));background:color-mix(in srgb,var(--ra-accent) 12%,var(--sapTile_Background,#fff));color:color-mix(in srgb,var(--ra-accent) 72%,var(--sapTextColor,#1d2d3e));font-size:10.5px;font-weight:700;line-height:1;letter-spacing:.01em;white-space:nowrap;vertical-align:middle}
    .ra-ctx-chip ui5-icon{width:.72rem;height:.72rem;color:var(--ra-accent)}
    .ra-ctx-chip.period{border-color:color-mix(in srgb,var(--ra-cyan) 38%,var(--ra-border));background:color-mix(in srgb,var(--ra-cyan) 12%,var(--sapTile_Background,#fff));color:color-mix(in srgb,var(--ra-cyan) 76%,var(--sapTextColor,#1d2d3e))}.ra-ctx-chip.period ui5-icon{color:var(--ra-cyan)}
    .ra-ctx-chip.version{border-color:var(--ra-border);background:color-mix(in srgb,var(--sapContent_LabelColor,#6a6d70) 12%,var(--sapTile_Background,#fff));color:var(--sapContent_LabelColor,#6a6d70)}.ra-ctx-chip.version ui5-icon{color:var(--sapContent_LabelColor,#6a6d70)}
    .ra-tools{margin-left:auto;display:flex;align-items:center;gap:8px;min-width:0}
    .ra-ctx{display:inline-flex;align-items:center;gap:5px}
    .ra-badge{display:inline-flex;align-items:center;gap:4px;height:26px;padding:0 9px;border-radius:6px;background:var(--sapField_Background,#fff);border:1px solid color-mix(in srgb,var(--ra-blue) 24%,var(--ra-border));color:var(--ra-blue);font-size:11.5px;font-weight:700}.ra-badge ui5-icon{width:.85rem;height:.85rem}
    .ra-hgroup{display:inline-flex;align-items:center;gap:2px;height:32px;padding:2px;border-radius:8px;background:color-mix(in srgb,var(--ra-border) 26%,transparent)}
    .ra-btn{height:28px;border:0;border-radius:6px;background:transparent;color:var(--sapContent_IconColor,#475059);display:inline-flex;align-items:center;justify-content:center;gap:6px;padding:0 10px;font:inherit;font-size:12px;font-weight:600;cursor:pointer;transition:background .12s,color .12s,box-shadow .12s;white-space:nowrap}
    .ra-btn ui5-icon{width:1rem;height:1rem}.ra-btn:hover{background:var(--sapTile_Background,#fff);color:var(--ra-blue);box-shadow:0 1px 4px rgba(10,31,68,.12)}
    .ra-btn.primary{background:linear-gradient(180deg,#1a7ee0,var(--ra-blue));color:#fff;box-shadow:0 1px 2px rgba(10,110,209,.36)}.ra-btn.primary:hover{background:linear-gradient(180deg,#248ceb,#0a63bd);color:#fff}
    .ra-btn:disabled{opacity:.4;cursor:not-allowed;background:transparent!important;color:var(--sapContent_IconColor,#475059)!important;box-shadow:none!important}
    .ra-btn svg{width:1.02rem;height:1.02rem;fill:none;stroke:currentColor;stroke-width:1.85;stroke-linecap:round;stroke-linejoin:round}
    /* 存数▾ 分裂按钮 */
    .ra-rpt{position:relative;display:inline-flex;align-items:center}
    .ra-rpt-main{border-radius:6px 0 0 6px;padding:0 10px}
    .ra-rpt-caret{border-radius:0 6px 6px 0;min-width:22px;padding:0 4px;margin-left:1px}
    .ra-rpt-caret svg{width:.62rem;height:.62rem;stroke-width:2.4}
    .ra-rpt-menu{position:fixed;z-index:1000;display:none;flex-direction:column;gap:1px;width:186px;padding:6px;border:1px solid var(--ra-border);border-radius:9px;background:var(--sapPopover_Background,#fff);box-shadow:0 14px 36px rgba(10,31,68,.2)}
    .ra-rpt.open .ra-rpt-menu{display:flex}
    .ra-rpt-item{display:flex;align-items:center;gap:8px;width:100%;height:32px;padding:0 10px;border:0;border-radius:6px;background:transparent;color:var(--sapTextColor,#1d2d3e);font:inherit;font-size:12.5px;cursor:pointer;text-align:left}
    .ra-rpt-item ui5-icon{width:1rem;height:1rem;flex:0 0 auto;color:var(--sapContent_IconColor,#475059)}
    .ra-rpt-item:hover{background:color-mix(in srgb,var(--ra-blue) 10%,var(--sapTile_Background,#fff));color:var(--ra-blue)}.ra-rpt-item:hover ui5-icon{color:var(--ra-blue)}
    .ra-rpt-sep{height:1px;margin:4px 6px;background:var(--ra-border)}
    /* 撤销/重做（分裂：动作 + caret 下拉历史） */
    .ra-history{position:relative;display:inline-flex;align-items:center}
    .ra-hist-action{min-width:26px;padding:0 6px;border-radius:6px 0 0 6px}
    .ra-hist-caret{height:28px;min-width:16px;padding:0 3px;margin-left:1px;border:0;border-radius:0 6px 6px 0;background:transparent;color:var(--sapContent_IconColor,#475059);display:inline-flex;align-items:center;justify-content:center;cursor:pointer;transition:background .12s,color .12s}
    .ra-hist-caret svg{width:.6rem;height:.6rem;fill:none;stroke:currentColor;stroke-width:2.4;stroke-linecap:round;stroke-linejoin:round}
    .ra-hist-caret:hover:not(:disabled){background:var(--sapTile_Background,#fff);color:var(--ra-blue)}
    .ra-hist-caret:disabled{opacity:.4;cursor:not-allowed}
    .ra-hist-menu{position:absolute;right:0;top:34px;z-index:1000;display:none;width:230px;max-height:288px;overflow:auto;padding:6px;border:1px solid var(--ra-border);border-radius:9px;background:var(--sapPopover_Background,#fff);box-shadow:0 14px 36px rgba(10,31,68,.2)}
    .ra-history.open .ra-hist-menu{display:block}
    .ra-hist-title{padding:4px 8px 6px;font-size:11px;font-weight:700;color:var(--sapContent_LabelColor,#6a6d70)}
    .ra-hist-item{width:100%;height:30px;border:0;border-radius:6px;background:transparent;color:inherit;font:inherit;font-size:12px;display:flex;align-items:center;gap:8px;padding:0 8px;text-align:left;cursor:pointer}
    .ra-hist-item:hover,.ra-hist-item.hot{background:color-mix(in srgb,var(--ra-blue) 10%,transparent);color:var(--ra-blue)}
    .ra-hist-item span{min-width:0;flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
    .ra-hist-item small{margin-left:auto;color:var(--sapContent_LabelColor,#8a9099);font:700 10px/1 ui-monospace,Menlo,monospace}
    .ra-hist-item:hover small,.ra-hist-item.hot small{color:var(--ra-blue)}
    .ra-hist-empty{padding:12px 8px;color:var(--sapContent_LabelColor,#6a6d70);font-size:12px;text-align:center;display:block}
    /* 表格缩放滑杆 */
    .ra-zoom{display:inline-flex;align-items:center;gap:5px;height:32px;padding:2px 6px;border-radius:8px;background:color-mix(in srgb,var(--ra-border) 26%,transparent)}
    .ra-zoom-step{flex:0 0 auto;width:22px;height:22px;padding:0;border:0;border-radius:6px;background:transparent;color:var(--sapContent_IconColor,#475059);display:inline-flex;align-items:center;justify-content:center;cursor:pointer;transition:background .12s,color .12s}
    .ra-zoom-step svg{width:.95rem;height:.95rem;fill:none;stroke:currentColor;stroke-width:2;stroke-linecap:round;stroke-linejoin:round}
    .ra-zoom-step:hover{background:var(--sapTile_Background,#fff);color:var(--ra-blue);box-shadow:0 1px 4px rgba(10,31,68,.12)}
    .ra-zoom-range{-webkit-appearance:none;appearance:none;width:104px;height:20px;background:transparent;cursor:pointer;margin:0}
    .ra-zoom-range:focus{outline:none}
    .ra-zoom-range::-webkit-slider-runnable-track{height:4px;border-radius:999px;background:linear-gradient(90deg,var(--ra-blue) 0,var(--ra-cyan) var(--ra-zoom-fill,50%),color-mix(in srgb,var(--ra-border) 70%,transparent) var(--ra-zoom-fill,50%))}
    .ra-zoom-range::-moz-range-track{height:4px;border-radius:999px;background:color-mix(in srgb,var(--ra-border) 70%,transparent)}
    .ra-zoom-range::-moz-range-progress{height:4px;border-radius:999px;background:linear-gradient(90deg,var(--ra-blue),var(--ra-cyan))}
    .ra-zoom-range::-webkit-slider-thumb{-webkit-appearance:none;appearance:none;width:14px;height:14px;margin-top:-5px;border-radius:50%;background:#fff;border:1px solid color-mix(in srgb,var(--ra-blue) 40%,var(--ra-border));box-shadow:0 1px 3px rgba(10,31,68,.32);transition:transform .12s,box-shadow .12s}
    .ra-zoom-range::-moz-range-thumb{width:14px;height:14px;border-radius:50%;background:#fff;border:1px solid color-mix(in srgb,var(--ra-blue) 40%,var(--ra-border));box-shadow:0 1px 3px rgba(10,31,68,.32);transition:transform .12s,box-shadow .12s}
    .ra-zoom-range:hover::-webkit-slider-thumb,.ra-zoom-range:active::-webkit-slider-thumb{transform:scale(1.18);box-shadow:0 2px 8px rgba(10,110,209,.4),0 0 0 4px color-mix(in srgb,var(--ra-blue) 20%,transparent)}
    .ra-zoom-range:hover::-moz-range-thumb,.ra-zoom-range:active::-moz-range-thumb{transform:scale(1.18);box-shadow:0 2px 8px rgba(10,110,209,.4),0 0 0 4px color-mix(in srgb,var(--ra-blue) 20%,transparent)}
    .ra-zoom-pct{flex:0 0 auto;min-width:46px;height:22px;padding:0 8px;border:1px solid color-mix(in srgb,var(--ra-blue) 24%,var(--ra-border));border-radius:999px;background:color-mix(in srgb,var(--ra-blue) 8%,var(--sapField_Background,#fff));color:var(--ra-blue);font:800 11.5px/1 ui-monospace,Menlo,Consolas,monospace;letter-spacing:.02em;cursor:pointer;transition:background .12s,border-color .12s}
    .ra-zoom-pct:hover{background:color-mix(in srgb,var(--ra-blue) 16%,var(--sapField_Background,#fff));border-color:color-mix(in srgb,var(--ra-blue) 46%,var(--ra-border))}
    .ra-stage{flex:1;min-height:0;overflow:hidden;padding:0;background:linear-gradient(180deg,color-mix(in srgb,var(--ra-blue) 4%,var(--sapBackgroundColor,#f5f6f7)),var(--sapBackgroundColor,#f5f6f7))}
    .ra-host{height:100%;min-height:460px;border:0;border-radius:0;background:var(--sapTile_Background,#fff);box-shadow:none;overflow:hidden}
    .ra-spread{display:block;width:100%;height:100%;min-height:460px}
    .ra-prop{flex:1;min-height:0;overflow:auto;padding:10px;display:flex;flex-direction:column;gap:10px}
    .ra-hero{display:flex;gap:10px;align-items:center;border:1px solid var(--ra-border);border-radius:8px;background:linear-gradient(135deg,color-mix(in srgb,var(--ra-blue) 12%,var(--sapTile_Background,#fff)),var(--sapTile_Background,#fff));padding:12px}.ra-hero-ic{width:40px;height:40px;border-radius:9px;display:flex;align-items:center;justify-content:center;background:var(--ra-blue);color: #fff}.ra-hero-ic ui5-icon{width:1.35rem;height:1.35rem}.ra-hero b{display:block;font-size:15px}.ra-hero span{font-size:11px;color:var(--sapContent_LabelColor,#6a6d70)}
    .ra-grid{display:grid;grid-template-columns:1fr;gap:6px}.ra-kv{border:1px solid var(--ra-border);border-radius:7px;background:var(--sapTile_Background,#fff);padding:7px 9px}.ra-kv span{display:block;font-size:10px;color:var(--sapContent_LabelColor,#6a6d70)}.ra-kv b{display:block;font-size:12px;word-break:break-word}
    .ra-sec{border:1px solid var(--ra-border);border-radius:8px;background:var(--sapTile_Background,#fff);padding:10px}.ra-sec>b{display:block;margin-bottom:7px;color:var(--ra-blue)}.ra-sec p{margin:0;color:var(--sapContent_LabelColor,#6a6d70);font-size:12px}
    .ra-empty{padding:18px;border:1px dashed var(--ra-border);border-radius:8px;background:var(--sapTile_Background,#fff);color:var(--sapContent_LabelColor,#6a6d70);text-align:center}
    .ra-note{margin:10px;border:1px dashed var(--ra-border);border-radius:8px;padding:12px;background:var(--sapList_HeaderBackground,#f7f9fc);color:var(--sapContent_LabelColor,#6a6d70)}
    .ra-fp{margin-top:10px}.ra-fp>b{display:flex;align-items:center;gap:5px;color:var(--ra-accent,#0d9488)}.ra-fp-count{margin-left:auto;font-size:10px;background:color-mix(in srgb,var(--ra-accent,#0d9488) 16%,transparent);color:var(--ra-accent,#0d9488);border-radius:999px;padding:1px 7px;font-weight:700}
    .ra-fp-hint{margin:2px 0 8px!important;font-size:11px}
    .ra-fp-tablewrap{max-height:220px;overflow:auto;border:1px solid var(--ra-border);border-radius:7px}
    .ra-fp-table{width:100%;border-collapse:collapse;font-size:11.5px}.ra-fp-table th{position:sticky;top:0;background:var(--sapList_HeaderBackground,#f2f5f9);color:var(--sapContent_LabelColor,#5b6b7b);font-weight:600;text-align:left;padding:5px 7px;border-bottom:1px solid var(--ra-border)}.ra-fp-table td{padding:3px 7px;border-bottom:1px solid color-mix(in srgb,var(--ra-border) 60%,transparent);vertical-align:middle}
    .ra-fp-in{width:100%;box-sizing:border-box;height:24px;border:1px solid transparent;border-radius:4px;background:transparent;font:inherit;font-size:11.5px;padding:0 5px;color:var(--sapTextColor,#1d2d3e)}.ra-fp-in:hover{border-color:var(--ra-border)}.ra-fp-in:focus{outline:none;border-color:var(--ra-accent,#0d9488);background:var(--sapTile_Background,#fff)}.ra-fp-num{font-variant-numeric:tabular-nums}
    .ra-fp-tag{font-size:9px;font-weight:800;padding:1px 5px;border-radius:4px}.ra-fp-tag.manual{background:color-mix(in srgb,#a855f7 16%,transparent);color:#a855f7}.ra-fp-tag.seed{background:color-mix(in srgb,var(--ra-accent,#0d9488) 14%,transparent);color:var(--ra-accent,#0d9488)}
    .ra-fp-del{width:24px;height:24px;border:0;border-radius:5px;background:transparent;color:var(--sapContent_IconColor,#8a94a0);cursor:pointer;display:inline-flex;align-items:center;justify-content:center}.ra-fp-del:hover{background:color-mix(in srgb,#ef4444 12%,transparent);color:#ef4444}
    .ra-fp-empty{text-align:center;color:var(--sapContent_LabelColor,#6a6d70);padding:14px!important;font-size:11px}
    .ra-fp-acts{display:flex;gap:6px;margin-top:8px;flex-wrap:wrap}.ra-fp-acts .ra-btn{border:1px solid var(--ra-border)}.ra-fp-acts .ra-btn.primary{background:var(--ra-accent,#0d9488);color: #fff;border-color:var(--ra-accent,#0d9488)}
    .ra-toast{position:absolute;left:50%;bottom:22px;transform:translate(-50%,14px);z-index:60;max-width:min(560px,88%);padding:10px 16px;border-radius:9px;background:var(--sapInformationElementColor, #1d2d3e);color:var(--sapGroup_ContentBorderColor, #ffffff);font-size:12.5px;font-weight:600;box-shadow:0 12px 32px rgba(10,31,68,.34);opacity:0;pointer-events:none;transition:opacity .22s,transform .22s;display:flex;align-items:center;gap:8px}.ra-toast.show{opacity:1;transform:translate(-50%,0)}.ra-toast[data-kind="success"]{background:linear-gradient(180deg,var(--sapPositiveElementColor, #12b56b),var(--sapPositiveElementColor, #0f9d5c))}.ra-toast[data-kind="warn"]{background:linear-gradient(180deg,var(--sapCriticalElementColor, #e0a336),var(--sapCriticalElementColor, #d98200))}.ra-toast[data-kind="error"]{background:linear-gradient(180deg,var(--sapNegativeElementColor, #e5544b),var(--sapNegativeElementColor, #c0392b))}
    /* explorer：期间下拉（顶部标题区，高度与 content .ra-head 一致 46px）+ 组织详情 */
    .ra-explorer{overflow:hidden}
    .ra-period-row{height:46px;flex:0 0 auto;box-sizing:border-box;display:flex;align-items:center;gap:8px;padding:0 12px;border-bottom:1px solid var(--ra-border);background:var(--sapList_HeaderBackground,#f7f9fc)}
    .ra-period-lbl{flex:0 0 auto;display:inline-flex;align-items:center;gap:4px;font-size:12px;font-weight:700;color:var(--sapContent_LabelColor,#6a6d70)}.ra-period-lbl ui5-icon{width:.95rem;height:.95rem;color:var(--ra-cyan)}
    .ra-select-wrap{position:relative;flex:1;min-width:0}
    .ra-select-wrap select{width:100%;height:32px;border:1px solid color-mix(in srgb,var(--ra-blue) 20%,var(--ra-border));border-radius:8px;background:var(--sapField_Background,#fff);color:inherit;font:inherit;font-size:12.5px;font-weight:600;padding:0 30px 0 11px;cursor:pointer;-webkit-appearance:none;appearance:none;box-shadow:0 1px 2px rgba(10,31,68,.05);transition:border-color .15s,box-shadow .15s}
    .ra-select-wrap select:hover{border-color:color-mix(in srgb,var(--ra-blue) 42%,var(--ra-border))}
    .ra-select-wrap select:focus{outline:0;border-color:var(--ra-blue);box-shadow:0 0 0 3px color-mix(in srgb,var(--ra-blue) 14%,transparent)}
    .ra-select-caret{position:absolute;right:9px;top:50%;transform:translateY(-50%);width:.85rem;height:.85rem;color:var(--ra-blue);pointer-events:none}
    .ra-org-scroll{flex:1;min-height:0;overflow:auto}
    .ra-org-head{display:flex;align-items:center;gap:6px;padding:9px 10px 5px;font-size:11px;font-weight:700;color:var(--sapContent_LabelColor,#6a6d70);text-transform:uppercase;letter-spacing:.03em}.ra-org-head ui5-icon{width:.9rem;height:.9rem;color:var(--ra-blue)}
    .ra-org-hero{display:flex;gap:9px;align-items:center;margin:0 10px 8px;border:1px solid var(--ra-border);border-radius:8px;background:linear-gradient(135deg,color-mix(in srgb,var(--ra-blue) 12%,var(--sapTile_Background,#fff)),var(--sapTile_Background,#fff));padding:10px}.ra-org-ic{width:34px;height:34px;flex:0 0 auto;border-radius:8px;display:flex;align-items:center;justify-content:center;background:var(--ra-blue);color: #fff}.ra-org-ic ui5-icon{width:1.15rem;height:1.15rem}.ra-org-hero b{display:block;font-size:13px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.ra-org-hero span{font-size:11px;color:var(--sapContent_LabelColor,#6a6d70)}
    .ra-org-grid{display:grid;grid-template-columns:1fr;gap:6px;padding:0 10px 10px}
  `
}

// ============================================================================
// explorer：期间下拉 + 当前组织详情
// ============================================================================

function explorerPeriodSelect (st) {
  const years = st.periods.filter((p) => Number(p.level_no) === 1)
  const groups = years.map((y) => {
    const months = st.periods.filter((p) => p.parent_code === y.code && Number(p.is_leaf) === 1)
    const opts = months.map((m) =>
      `<option value="${esc(m.code)}" ${st.curPeriod === m.code ? 'selected' : ''}>${esc(m.name)}</option>`).join('')
    return `<optgroup label="${esc(y.name)}">${opts}</optgroup>`
  }).join('')
  // 若日历未加载但有传入期间，至少给一个当前项
  const fallback = st.curPeriod ? `<option value="${esc(st.curPeriod)}" selected>${esc(st.curPeriod)}</option>` : '<option value="">（无期间）</option>'
  return `<div class="ra-period-row">
    <span class="ra-period-lbl"><ui5-icon name="calendar"></ui5-icon>期间</span>
    <div class="ra-select-wrap">
      <select data-ra-period>${groups || fallback}</select>
      <ui5-icon class="ra-select-caret" name="slim-arrow-down"></ui5-icon>
    </div>
  </div>`
}

function orgDetailHtml (st) {
  const o = st.org
  if (st.explorerLoading && !o) return '<cmx-empty-state icon="synchronize" title="正在加载组织详情..." size="sm"></cmx-empty-state>'
  if (!o) {
    return `<div class="ra-org-head"><ui5-icon name="tree"></ui5-icon><span>组织机构</span></div>
      <cmx-empty-state icon="message-error" title="未找到组织 ${esc(st.props.orgCode || '')} 的详情" size="sm"></cmx-empty-state>`
  }
  return `<div class="ra-org-head"><ui5-icon name="${orgIcon(o.org_type)}"></ui5-icon><span>组织机构</span></div>
    <div class="ra-org-hero">
      <span class="ra-org-ic"><ui5-icon name="${orgIcon(o.org_type)}"></ui5-icon></span>
      <div><b>${esc(o.name)}</b><span>${esc(o.code)} · ${esc(o.org_type || '')}</span></div>
    </div>
    <div class="ra-org-grid">
      ${kv('组织编码', o.code)}
      ${kv('核算实体', o.entity_code)}
      ${kv('合并方案', o.consol_scheme)}
      ${kv('合并方法', o.consol_method)}
      ${kv('持股比例', o.ownership_pct != null ? `${o.ownership_pct}%` : '-')}
      ${kv('表决权比例', o.voting_pct != null ? `${o.voting_pct}%` : '-')}
      ${kv('合并币种', o.consol_currency)}
      ${kv('是否母公司', Number(o.is_parent) === 1 ? '是' : '否')}
      ${kv('内部抵消', Number(o.offset_flag) === 1 ? '参与抵消' : '不抵消')}
      ${kv('层级深度', o.level_no)}
      ${kv('全路径', o.full_path)}
    </div>
    ${o.remark ? `<div class="ra-sec" style="margin:0 10px 10px"><b>备注</b><p>${esc(o.remark)}</p></div>` : ''}`
}

function explorerHtml (st) {
  return `<section class="ra ra-explorer">
    ${explorerPeriodSelect(st)}
    <div class="ra-org-scroll">${orgDetailHtml(st)}</div>
  </section>`
}

/** 顶栏「表格视图缩放」滑杆：−/+ 微调 + range 拖动 + 百分数胶囊（点击回 100%）。范围 50~200%。 */
function raZoomSlider (st) {
  const pct = Math.round((st.zoom || 1) * 100)
  const fill = ((pct - 50) / 150) * 100
  const minus = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12h14"/></svg>'
  const plus = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 5v14"/><path d="M5 12h14"/></svg>'
  return `<span class="ra-zoom" data-ra-zoom style="--ra-zoom-fill:${fill}%">
    <button class="ra-zoom-step" type="button" data-ra-zoom-step="-1" title="缩小" aria-label="缩小">${minus}</button>
    <input class="ra-zoom-range" type="range" min="50" max="200" step="10" value="${pct}" data-ra-zoom-range aria-label="表格缩放百分比" title="拖动调整表格缩放">
    <button class="ra-zoom-step" type="button" data-ra-zoom-step="1" title="放大" aria-label="放大">${plus}</button>
    <button class="ra-zoom-pct" type="button" data-ra-zoom-reset title="点击重置为 100%" aria-label="缩放百分比，点击重置"><span data-ra-zoom-pct-text>${pct}%</span></button>
  </span>`
}

/** 顶栏撤销/重做（分裂：动作按钮 + caret 下拉历史列表，仿报表设计器）。初始禁用，编辑后启用。 */
function raHistoryButtons () {
  const chevron = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="m7 10 5 5 5-5"/></svg>'
  const one = (kind, icon, title) => `<span class="ra-history" data-ra-history="${kind}">
      <button class="ra-btn ra-hist-action" type="button" data-ra-cmd="${kind}" title="${esc(title)}" aria-label="${esc(title)}" disabled><ui5-icon name="${icon}"></ui5-icon></button>
      <button class="ra-hist-caret" type="button" data-ra-hist-toggle="${kind}" title="${esc(title)}历史" aria-label="${esc(title)}历史" disabled>${chevron}</button>
      <span class="ra-hist-menu" data-ra-hist-menu="${kind}"><span class="ra-hist-empty">暂无${esc(title)}记录</span></span>
    </span>`
  return `<span class="ra-hgroup ra-hgroup-history">${one('undo', 'undo', '撤销')}${one('redo', 'redo', '重做')}</span>`
}

/** 顶栏「存数 ▾」分裂按钮：主按钮=存数；下拉=存数/计算/校验/取数/导出。 */
function raSaveSplit () {
  const item = (cmd, icon, label) => `<button class="ra-rpt-item" type="button" data-ra-cmd="${cmd}"><ui5-icon name="${icon}"></ui5-icon><span>${label}</span></button>`
  const chevron = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="m7 10 5 5 5-5"/></svg>'
  return `<span class="ra-rpt" data-ra-rpt>
    <button class="ra-btn primary ra-rpt-main" type="button" data-ra-cmd="save" title="保存数据到 cr_cell_data" aria-label="存数"><ui5-icon name="save"></ui5-icon><span>存数</span></button>
    <button class="ra-btn primary ra-rpt-caret" type="button" data-ra-rpt-toggle title="数据操作" aria-label="数据操作" aria-haspopup="true" aria-expanded="false">${chevron}</button>
    <span class="ra-rpt-menu" data-ra-rpt-menu>
      ${item('save', 'save', '存数')}
      <span class="ra-rpt-sep"></span>
      ${item('compute', 'sum', '计算')}
      ${item('verify', 'validate', '校验')}
      ${item('load', 'download-from-cloud', '取数')}
      <span class="ra-rpt-sep"></span>
      ${item('export', 'excel-attachment', '导出 Excel')}
    </span>
  </span>`
}

/** Excel 样式公式栏（复制自报表设计器）：名称框(坐标) | fx 按钮 | 内容编辑器。
 *  fx 函数对话框未复制——按钮暂弹占位（公式编辑引擎另案，避免跨页复制设计器专属状态）。 */
function raFormulaBar (st) {
  return `<div class="ra-fxbar">
    <div class="ra-namebox" title="名称框：输入单元格(如 B4)或区域(如 A1:C5)，回车跳转/选中">
      <input class="ra-namebox-input" data-ra-namebox spellcheck="false" autocomplete="off"
             value="${esc(st.selectedRange || st.selectedCell || 'A1')}" aria-label="名称框">
      <span class="ra-namebox-caret" aria-hidden="true">▾</span>
    </div>
    <span class="ra-fxbar-sep"></span>
    <button class="ra-fx-btn" type="button" data-ra-fxbtn title="插入函数（fx）：进入公式编辑" aria-label="插入函数"><i>fx</i></button>
    <span class="ra-fxbar-sep"></span>
    <span class="ra-fxbar-elem" data-ra-fxelem hidden title="当前单元格绑定的数据元素"><ui5-icon name="database"></ui5-icon><span data-ra-fxelem-text></span></span>
    <input class="ra-fxbar-input" data-ra-fxinput spellcheck="false" autocomplete="off"
           placeholder="输入内容，或以 = 开头输入公式" aria-label="公式输入框" value="">
  </div>`
}

/** 标题第二行：版本 / 组织编码 / 期间 三个气泡（有值才显）。 */
function titleContextChips (st) {
  const ver = String(versionLabel(st.props.version) || '').trim()
  const org = String(st.props.orgCode || '').trim()
  const period = String(st.curPeriod || st.props.periodCode || '').trim()
  const chips = []
  if (ver) chips.push(`<span class="ra-ctx-chip version" title="版本"><ui5-icon name="version-1"></ui5-icon>${esc(ver)}</span>`)
  if (org) chips.push(`<span class="ra-ctx-chip" title="组织机构编码"><ui5-icon name="org-chart"></ui5-icon>${esc(org)}</span>`)
  if (period) chips.push(`<span class="ra-ctx-chip period" title="会计期间"><ui5-icon name="calendar"></ui5-icon>${esc(period)}</span>`)
  return chips.join('')
}

function contentHtml (st) {
  const model = skeletonModel(st)
  return `<section class="ra">
    <div class="ra-head">
      <span class="ra-head-ic"><ui5-icon name="table-chart"></ui5-icon></span>
      <span class="ra-title"><b>${esc(reportTitle(st))}</b><span class="ra-subrow">${titleContextChips(st)}</span></span>
      <span class="ra-tools">
        ${raZoomSlider(st)}
        ${raHistoryButtons()}
        ${raSaveSplit()}
      </span>
    </div>
    ${raFormulaBar(st)}
    <div class="ra-stage"><div class="ra-host"><cmx-spreadjs-sheet class="ra-spread" data-ra-spread data-cmx-formula-bar="false" data-cmx-report="${esc(JSON.stringify(model))}"></cmx-spreadjs-sheet></div></div>
  </section>`
}

function propertyHtml (st) {
  const r = st.report || {}
  return `<section class="ra ra-prop">
    <div class="ra-hero">
      <span class="ra-hero-ic"><ui5-icon name="detail-view"></ui5-icon></span>
      <div><b>${esc(r.name || st.props.reportName || st.props.reportCode)}</b><span>${esc(st.props.reportCode)} · ${esc(versionLabel(st.props.version))}</span></div>
    </div>
    <div class="ra-grid">
      ${kv('报表编码', r.code || st.props.reportCode)}
      ${kv('报表名称', r.name || st.props.reportName)}
      ${kv('报表类型', r.report_type)}
      ${kv('报表类别', r.report_category)}
      ${kv('期间类型', r.period_type)}
      ${kv('币种 / 单位', `${r.currency_code || '-'} / ${r.amount_unit || '-'}`)}
      ${kv('取数来源', r.data_source || '未指定')}
      ${kv('状态', r.status == null ? '-' : (Number(r.status) === 0 ? '停用' : '启用'))}
    </div>
    <div class="ra-sec"><b>说明</b><p>${esc(r.remark || '暂无备注')}</p></div>
  </section>`
}

function propertyStatusHtml (st) {
  return `<section class="ra ra-prop">
    <div class="ra-hero">
      <span class="ra-hero-ic"><ui5-icon name="status-positive"></ui5-icon></span>
      <div><b>数据状态</b><span>${esc(reportTitle(st))}</span></div>
    </div>
    <div class="ra-grid">
      ${kv('组织', st.props.orgCode || '未指定')}
      ${kv('会计期间', st.props.periodCode || '未指定')}
      ${kv('版式已加载', st.contentHash ? '是' : '首次/骨架')}
      ${kv('已装载单元格', st.dataLoaded ? String(st.loadedCells) : '尚未取数')}
    </div>
    <div class="ra-sec"><b>说明</b><p>「取数」按组织+期间从 cr_cell_data 装载单元格值并覆盖到版式画布（保留格式与公式）；「存数」把画布上的手工/非公式值回写 cr_cell_data。公式计算另案。</p></div>
    ${floatPanelHtml(st)}
  </section>`
}

/** 浮动明细维护面板：列出/增删改当前 org+period 的浮动行（cr_report_float_row）。 */
function floatPanelHtml (st) {
  const fp = st.__floatPanel || { loaded: false, items: [], kind: 'row' }
  const rows = fp.items || []
  const listRows = rows.length
    ? rows.map((it, i) => `<tr data-fp-row="${i}">
        <td>${esc(it.dimKey || '')}</td>
        <td><input class="ra-fp-in" data-fp-field="label" data-fp-i="${i}" value="${esc(it.label || '')}"></td>
        <td><input class="ra-fp-in ra-fp-num" data-fp-field="cellB" data-fp-i="${i}" value="${esc(cellVal(it, 'B'))}" placeholder="B列值/公式"></td>
        <td>${Number(it.isManual) === 1 ? '<span class="ra-fp-tag manual">手工</span>' : '<span class="ra-fp-tag seed">取数</span>'}</td>
        <td><button class="ra-fp-del" data-fp-del="${esc(String(it.id || ''))}" data-fp-i="${i}" title="删除"><ui5-icon name="delete"></ui5-icon></button></td>
      </tr>`).join('')
    : '<tr><td colspan="5" class="ra-fp-empty"><cmx-empty-state icon="table-row" title="尚无浮动明细" description="点「从取数初始化」拉取，或「新增一行」手工录入。" size="sm"></cmx-empty-state></td></tr>'
  return `<div class="ra-sec ra-fp">
    <b><ui5-icon name="multiselect-all"></ui5-icon> 浮动明细维护 ${rows.length ? `<span class="ra-fp-count">${rows.length}</span>` : ''}</b>
    <p class="ra-fp-hint">按 <b>${esc(st.props.orgCode || '?')}</b> + <b>${esc(st.curPeriod || st.props.periodCode || '?')}</b> 维护浮动行；改动保存后「取数」刷新画布。</p>
    <div class="ra-fp-tablewrap">
      <table class="ra-fp-table">
        <thead><tr><th>维度键</th><th>名称</th><th>B列值</th><th>来源</th><th></th></tr></thead>
        <tbody>${listRows}</tbody>
      </table>
    </div>
    <div class="ra-fp-acts">
      <button class="ra-btn" type="button" data-fp-cmd="reload"><ui5-icon name="refresh"></ui5-icon>刷新</button>
      <button class="ra-btn" type="button" data-fp-cmd="seed"><ui5-icon name="download"></ui5-icon>从取数初始化</button>
      <button class="ra-btn" type="button" data-fp-cmd="add"><ui5-icon name="add"></ui5-icon>新增一行</button>
      <button class="ra-btn primary" type="button" data-fp-cmd="save"><ui5-icon name="save"></ui5-icon>保存全部</button>
    </div>
  </div>`
}

/** 取浮动行某列的值（cells JSONB）。 */
function cellVal (item, col) {
  const c = item && item.cells
  return (c && (c[col] != null)) ? String(c[col]) : ''
}

function kv (label, value) {
  return `<div class="ra-kv"><span>${esc(label)}</span><b>${esc(value == null || value === '' ? '-' : value)}</b></div>`
}

function viewHtml (view, st) {
  if (view === 'explorer') return explorerHtml(st)
  if (view === 'property') return propertyHtml(st)
  if (view === 'propertyStatus') return propertyStatusHtml(st)
  return contentHtml(st)
}

// ============================================================================
// toast
// ============================================================================

const { showCmxToast } = globalThis.__cmxDataComp // 共享 toast（cmx-data-comp/lib/cmx-toast.js；治理清单 B-05）

// ============================================================================
// 版式加载 + 数据取/存（复用后端端点，逻辑本地实现，不 import designer.js）
// ============================================================================

/** 打开报表：一次后端调用取全集（版式+cellMap+元素+函数+数据+浮动展开）。分发进各缓存，返回 bundle。
 *  走 /expand（/open 的超集：额外带 float.regions[] 浮动实例行）。失败返回 null，调用方回退旧多调用路径（保底不白屏）。 */
async function openReportBundle (st) {
  try {
    const body = { version: st.props.version || '' }
    if (st.props.orgCode && st.props.periodCode) { body.orgCode = st.props.orgCode; body.periodCode = st.props.periodCode }
    const data = await apiJson(`/api/report-design/reports/${enc(st.props.reportCode)}/expand`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
    })
    if (!data) return null
    st.contentHash = data?.fmt?.contentHash || null
    hydrateCellMap(st, data)                       // 元素/取数/校验公式 → 公式栏 + applyCellFormulas
    st.elements = Array.isArray(data?.elements) ? data.elements : []
    st.elementsLoaded = true                       // 预置：元素胶囊显名称，不再单独请求 /elements
    st.__functions = Array.isArray(data?.functions) ? data.functions : []
    st.__fnLoaded = true                           // 预置：fx 编辑器取数区，不再单独请求 /functions
    st.__float = data?.float || null               // 浮动展开段（规则见 applyFloatExpansion）
    return data
  } catch (_) {
    return null
  }
}

/**
 * 把报表 DSL 公式转成 SpreadJS 语法安全串（仅供画布 setFormula）。移植自设计器：
 * 取数函数(QM/FS/…)按格取值不解析参数，故参数只需语法合法——@current/@parent、绝对期间(2026-06)、
 * 单引号字符串 转成双引号字符串字面量，避免 SpreadJS 拒绝整条公式。
 * ★ 幂等：**已在双引号内**的 @token / YYYY-MM 不再二次加引号（否则 "@current" → ""@current"" 语法非法 → #VALUE!）。
 *   做法：按双引号切段，只对引号外的段做 @/期间 替换；引号内段原样保留。
 */
function sanitizeExprForSpreadjs (expr) {
  let s = String(expr || '')
  s = s.replace(/'([^']*)'/g, (mm, inner) => `"${inner.replace(/"/g, '')}"`)   // '...' → "..."（单引号先转双引号）
  // 按 "..." 双引号串切段：偶数下标=引号外（需处理），奇数下标=引号内（原样，含开合引号本身重组）
  const parts = s.split('"')
  for (let i = 0; i < parts.length; i += 2) {
    parts[i] = parts[i]
      .replace(/@[a-zA-Z]+/g, (m) => `"${m}"`)                                   // 裸 @current → "@current"
      .replace(/(^|[(,\s])(\d{4}-\d{2})(?=[),\s]|$)/g, (mm, pre, ym) => `${pre}"${ym}"`) // 裸 2026-06 → "2026-06"
  }
  return parts.join('"')
}

/**
 * 把设计器定义的取数/计算公式（cellMap.calcFormula）落到 SpreadJS 单元格公式上，
 * 使其**参与自动计算**（QM/FS 等已由组件注册为自定义函数，按格取值；上层 SUM 等聚合可级联重算）。
 * 只处理当前在屏活动 sheet 的格；calcFormula 为空的格不动（保留版式 BLOB 里的原生公式）。
 */
function applyCellFormulas (sheet, st) {
  const wb = sheet?.getWorkbook?.()
  if (!wb) return 0
  let n = 0
  const cnt = wb.getSheetCount ? wb.getSheetCount() : 1
  for (let si = 0; si < cnt; si++) {
    const ws = wb.getSheet ? wb.getSheet(si) : wb.getActiveSheet?.()
    if (!ws) continue
    const sn = ws.name ? ws.name() : ''
    for (const key of Object.keys(st.cellMap || {})) {
      const cm = st.cellMap[key]
      if (!cm || !cm.calcFormula) continue
      // 浮动模板公式（含 {{占位符}}）由 applyFloatExpansion 逐实例行落格，这里跳过——
      // 否则会把 QM(0,@current,'{{cust_code}}') 字面量写到模板行位置，污染表头/画布。
      if (String(cm.calcFormula).includes('{{')) continue
      // key = `sheet名!地址`，只落本 sheet 的
      const bang = key.indexOf('!')
      if (bang < 0) continue
      if (key.slice(0, bang) !== sn) continue
      const addr = key.slice(bang + 1)
      const p = parseAddr(addr)
      if (!p) continue
      try {
        ws.setFormula(p.row, p.col, sanitizeExprForSpreadjs(String(cm.calcFormula).replace(/^=+/, '')))
        n++
      } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ }
    }
  }
  return n
}

/**
 * 浮动行展开落地：把后端 /expand 返回的 float.regions[].instances 逐行写到画布。
 * 每条实例行 = 一行画布数据：col A 写行标题(name)、其余列 setFormula（已由后端替换 {{维度}}/{{r}}/{{total}}）。
 * 实例行物理行号 physRow 为 1-based（后端按模板行起点顺序展开），转 0-based 落 setValue/setFormula。
 * 展开的是「模板行 × 数据源 → N 实例行」——设计态只有 1 条模板行，运行态在此铺成 N 行。
 * 分级浮动(rowType=subtotal/total、levelNo 层级)：小计/合计行加粗，明细/小计按 levelNo 缩进 col A。
 * 返回落地的实例行数。空/无浮动段则 0。
 */
function applyFloatExpansion (sheet, st, bundle) {
  const fl = bundle?.float || st.__float
  const regions = fl && Array.isArray(fl.regions) ? fl.regions : []
  if (!regions.length) return 0
  const wb = sheet?.getWorkbook?.()
  const ws = wb && wb.getActiveSheet && wb.getActiveSheet()
  if (!ws) return 0
  const GC = (typeof globalThis !== 'undefined' && globalThis.GC) || null
  let n = 0
  st.__floatIndex = {}   // physRow-1 → { rowId, dimKeyPath, regionCode, rowType }（存数时回填 8 元键用）
  st.__floatColIndex = {} // colIndex → { colId, dimKeyPath, regionCode }（列浮动存数用）
  for (const reg of regions) {
    // ── 列浮动（P3）：axis='col'，横向铺列。每实例列写列头(第1行)+各行公式。
    if (reg.axis === 'col') {
      const colIdxs = []
      for (const ci of (reg.colInstances || [])) {
        const c0 = Number(ci.colIndex)
        if (!(c0 >= 0)) continue
        colIdxs.push(c0)
        // 列头写到第 1 行（row 0）
        try { ws.setValue(0, c0, ci.header != null ? String(ci.header) : '') } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ }
        for (const cell of (ci.cells || [])) {
          const r0 = Number(cell.row) - 1
          if (!(r0 >= 0)) continue
          const f = String(cell.formula || '')
          try {
            if (f.startsWith('=')) ws.setFormula(r0, c0, sanitizeExprForSpreadjs(f.replace(/^=+/, '')))
            else ws.setFormula(r0, c0, sanitizeExprForSpreadjs(f))
          } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ }
        }
        st.__floatColIndex[c0] = { colId: ci.colId, dimKeyPath: ci.dimKeyPath, regionCode: reg.regionCode, sheetCode: reg.sheetCode }
        n++
      }
      // 列大纲：把连续的浮动列成组（可折叠整块浮动月份列）。
      applyColGrouping(ws, colIdxs)
      continue
    }
    for (const inst of (reg.instances || [])) {
      const r0 = Number(inst.physRow) - 1
      if (!(r0 >= 0)) continue
      const rowType = inst.rowType || 'float'
      const level = Number(inst.levelNo) || 1
      // col A：行标题
      try { ws.setValue(r0, 0, (inst.name != null ? String(inst.name) : '')) } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ }
      // 其余列：公式（后端已重定位坐标/替换维度，前端只需 sanitize 成 SpreadJS 语法）
      for (const c of (inst.cells || [])) {
        const p = parseAddr(`${c.col}${inst.physRow}`)
        if (!p) continue
        const f = String(c.formula || '')
        try {
          if (f.startsWith('=')) ws.setFormula(p.row, p.col, sanitizeExprForSpreadjs(f.replace(/^=+/, '')))
          else ws.setFormula(p.row, p.col, sanitizeExprForSpreadjs(f))
        } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ }
      }
      // 分级视觉：小计/合计加粗；A 列缩进（用 cellPadding，退化则前缀空格）
      try {
        if (rowType === 'subtotal' || rowType === 'total') {
          const style = ws.getStyle ? ws.getStyle(r0, 0) : null
          if (GC && GC.Spread && GC.Spread.Sheets) {
            const s = new GC.Spread.Sheets.Style(); s.font = 'bold 12px Arial'
            for (let c = 0; c <= 3; c++) { try { ws.setStyle(r0, c, s) } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ } }
          } else if (style && ws.setStyle) { /* 无 GC 全局：跳过样式，不阻断 */ }
        }
        if (level > 1 && ws.getCell) { try { ws.getCell(r0, 0).textIndent(level - 1) } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ } }
      } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ }
      st.__floatIndex[r0] = { rowId: inst.rowId, dimKeyPath: inst.dimKeyPath, regionCode: reg.regionCode, sheetCode: reg.sheetCode, rowType }
      n++
    }
    // 分级折叠：按 parentRow 把子行分组成 SpreadJS 行大纲（可折叠 [−]，如图）。
    applyRowGrouping(ws, reg.instances || [])
  }
  st.floatExpanded = n
  return n
}

/**
 * 分级折叠：把浮动实例行按层级建成 SpreadJS 行大纲（可折叠 [−]，如图）。
 * 正确嵌套：先分内层（明细收在小计下），再分外层（小计+明细整体收在合计下）——
 * 用「每个父行 → 其全部后代的物理行跨度[min,max]」分组，深层(level 大)先 group。
 * ★ 必须 suspendPaint/resumePaint 包裹，否则 group() 静默不生效。
 */
function applyRowGrouping (ws, instances) {
  try {
    const wb = ws.getParent ? ws.getParent() : null
    if (!ws.rowOutlines || !ws.rowOutlines.group) return

    // physRow → instance；按 parentRow 建父→子映射（子=直接下级）。
    const byPhys = {}
    instances.forEach((i) => { byPhys[Number(i.physRow)] = i })
    const directChildren = {}
    instances.forEach((i) => {
      const p = i.parentRow
      if (p != null) { (directChildren[p] = directChildren[p] || []).push(Number(i.physRow)) }
    })

    // 递归求某父行的全部后代物理行（含各级子孙），用于算折叠跨度。
    const descendants = (row) => {
      const out = []
      const kids = directChildren[row] || []
      kids.forEach((k) => { out.push(k); out.push(...descendants(k)) })
      return out
    }

    // 每个「有子行的父」→ 一个分组 [start0, count]；跨度 = 父行(小计/合计,即 summary) + 其全部后代明细。
    // 按父行的层级深度排序：level 大（深）的先分组，保证内层先于外层（正确嵌套）。
    const parents = Object.keys(directChildren)
      .map(Number)
      .filter((p) => byPhys[p]) // 父必须是真实实例行（合计/小计）
      .sort((a, b) => (byPhys[b].levelNo || 0) - (byPhys[a].levelNo || 0))

    const segs = []
    parents.forEach((p) => {
      const desc = descendants(p)
      if (!desc.length) return
      // ★ 组范围必须**含父行 p 本身**（summaryBelow=false 时组首行=summary，按钮画在此行）。
      //   只取后代 min..max 会把明细首行当 summary → 按钮错位一行（华北小计的按钮画到北京D明细上）。
      const min = Math.min(p, ...desc); const max = Math.max(p, ...desc)
      segs.push([min - 1, max - min + 1]) // [start0, count]（0-based）
    })
    if (!segs.length) return

    if (wb && wb.suspendPaint) wb.suspendPaint()
    try {
      // 折叠按钮放在组的「上方」（我们的小计/合计 summary 行在明细之前）。
      // 内核方向：SpreadJS 用 rowOutlines.direction(0)；cmx-megasheet 用 sheet.summaryBelow=false
      //（汇总在首行，折叠隐藏其后明细）。两者都试，命中即生效。
      try { ws.rowOutlines.direction && ws.rowOutlines.direction(0) } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ }
      try { ws.summaryBelow = false } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ }
      segs.forEach(([start0, count]) => {
        if (count > 0 && start0 >= 0) { try { ws.rowOutlines.group(start0, count) } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ } }
      })
    } finally {
      if (wb && wb.resumePaint) wb.resumePaint()
    }
    // ★ 置 showRowOutline **在 group 之后**——它经 spread-compat 活代理触发 element.refreshOutlines()，
    //   而后者按 rowOutlines.maxLevel() 算大纲带宽度；分组前触发则带宽=0 不画（换 cmx-megasheet 后的顺序坑）。
    try { if (wb && wb.options) wb.options.showRowOutline = true } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ }
    // 兜底：直呼 element.refreshOutlines()（spread-compat 的 ws._el 是宿主 element；SpreadJS 内核无此属性，跳过）。
    try { if (ws._el && typeof ws._el.refreshOutlines === 'function') ws._el.refreshOutlines() } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ }
  } catch (_) { /* 大纲失败不阻断渲染 */ }
}

/**
 * 列大纲：把连续的浮动列成组（SpreadJS columnOutlines.group），呈现列头上方的折叠 [−]，
 * 可整块收起浮动月份列。折叠按钮放在组「左侧」（第一列，与行大纲上方对称）。
 */
function applyColGrouping (ws, colIdxs) {
  try {
    const wb = ws.getParent ? ws.getParent() : null
    if (!ws.columnOutlines || !ws.columnOutlines.group || !colIdxs || !colIdxs.length) return
    // 连续段分组（浮动列一般连续，如 C..H）。
    const sorted = colIdxs.slice().sort((a, b) => a - b)
    const segs = []
    let segStart = null; let prev = null
    const flush = (end) => { if (segStart != null) segs.push([segStart, end - segStart + 1]); segStart = null }
    sorted.forEach((c) => {
      if (segStart == null) { segStart = c; prev = c; return }
      if (c === prev + 1) { prev = c; return }
      flush(prev); segStart = c; prev = c
    })
    flush(prev)
    if (!segs.length) return
    if (wb && wb.suspendPaint) wb.suspendPaint()
    try {
      // 折叠按钮放列组「左侧」（summary 在前）。SpreadJS 用 columnOutlines.direction(0)；
      // cmx-megasheet 用 sheet.summaryRight=false（汇总在首列）。两者都试。
      try { ws.columnOutlines.direction && ws.columnOutlines.direction(0) } catch (_) { /* 防御性忽略 */ }
      try { ws.summaryRight = false } catch (_) { /* 防御性忽略 */ }
      segs.forEach(([start, count]) => {
        if (count > 0 && start >= 0) { try { ws.columnOutlines.group(start, count) } catch (_) { /* 防御性忽略 */ } }
      })
    } finally {
      if (wb && wb.resumePaint) wb.resumePaint()
    }
    // ★ group 之后再置 showColumnOutline（触发 refreshOutlines 按 maxLevel 算带宽），同行大纲顺序坑。
    try { if (wb && wb.options) wb.options.showColumnOutline = true } catch (_) { /* 防御性忽略 */ }
    try { if (ws._el && typeof ws._el.refreshOutlines === 'function') ws._el.refreshOutlines() } catch (_) { /* 防御性忽略 */ }
  } catch (_) { /* 列大纲失败不阻断渲染 */ }
}

/**
 * 把已取到的 cells 灌进画布（不再单独请求）：
 *  ① 公式格(画布 getFormula 命中)——灌 setReportValueMap，取数函数按格取值→自动算，不覆盖公式。
 *  ② 非公式格(手工值)——setCellValues 直填。
 * 供 openReportBundle 打开时复用（cells 来自 bundle），也供手动「取数」（cells 来自 data/query）。
 */
function applyCellsToCanvas (sheet, st, cells) {
  const list = Array.isArray(cells) ? cells : []
  const wb = sheet.getWorkbook && sheet.getWorkbook()
  const ws = wb && wb.getActiveSheet && wb.getActiveSheet()
  const sheetName = (ws && ws.name && ws.name()) || ''
  const valueMap = {}     // sheetName!CELLREF -> value（供公式格取数）
  const plainValues = {}  // CELLREF -> value（非公式格直填）
  for (const r of list) {
    if (!r.cellRef) continue
    const v = r.valueType === 'number' ? r.numValue : r.textValue
    valueMap[`${sheetName}!${String(r.cellRef).toUpperCase()}`] = v
    let hasFormula = false
    const p = parseAddr(r.cellRef)
    if (ws && p) { try { hasFormula = !!ws.getFormula(p.row, p.col) } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ } }
    if (!hasFormula) plainValues[r.cellRef] = v
  }
  if (sheet.setReportValueMap) sheet.setReportValueMap(valueMap)                         // 公式格显真值 + 触发重算
  if (sheet.setCellValues && Object.keys(plainValues).length) sheet.setCellValues(plainValues)
  st.dataLoaded = true
  st.loadedCells = list.length
}

/** 打开即加载版式：GET layout → 有 BLOB 用 setWorkbookJson 无损复原，无则初始骨架。 */
async function loadLayout (sheet, st, root) {
  try {
    const data = await apiJson(`/api/report-design/reports/${enc(st.props.reportCode)}/layout?version=${enc(st.props.version || '')}`)
    st.contentHash = data?.fmt?.contentHash || null
    hydrateCellMap(st, data) // 回填设计器定义的元素/取数/校验公式 → 选中格时公式栏据此显示
    const wbJson = decodeDoc(data?.fmt?.docContent)
    if (wbJson && sheet.setWorkbookJson) {
      await sheet.setWorkbookJson(wbJson)
    } else if (sheet.setReportModel) {
      sheet.setReportModel(skeletonModel(st))
    }
    refreshInstance(st, (v) => v === 'propertyStatus')
    return true
  } catch (_) {
    if (sheet.setReportModel) sheet.setReportModel(skeletonModel(st))
    return false
  }
}

/** 取数：POST data/query → setCellValues 覆盖画布值（保留版式与公式）。
 *  silent=true（打开报表自动取数用）：不弹成功 toast、缺组织/期间时静默跳过（不算错误，骨架期常见）。 */
async function loadData (sheet, st, root, silent = false) {
  const { orgCode, periodCode } = st.props
  if (!orgCode || !periodCode) { if (!silent) showCmxToast('缺少组织或期间上下文', { level: 'error' }); return }
  try {
    const res = await apiJson(`/api/report-design/reports/${enc(st.props.reportCode)}/data/query`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ version: st.props.version || '', orgCode, periodCode }),
    })
    const cells = res?.cells || []
    st.__loading = true
    applyCellsToCanvas(sheet, st, cells) // 双路灌值（公式格 setReportValueMap 自动算 / 非公式格直填）
    setTimeout(() => { st.__loading = false }, 200)
    markDirty(st, false) // 取数=从DB装载，画布与DB一致，清除未保存标记
    if (!silent) showCmxToast(`已装载 ${cells.length} 个单元格数据`, { level: 'success' })
    refreshInstance(st, (v) => v === 'propertyStatus')
  } catch (err) {
    if (!silent) showCmxToast(`取数失败：${String(err?.message || err)}`, { level: 'error' })
  }
}

/** 计算：POST compute → 后端装载公式递归求值（QM/QC/REF…）落 cr_cell_data → 再取数刷新画布。 */
async function computeData (sheet, st, root) {
  const orgCode = st.props.orgCode
  const periodCode = st.curPeriod || st.props.periodCode
  if (!orgCode || !periodCode) { showCmxToast('缺少组织或期间上下文', { level: 'error' }); return }
  try {
    showCmxToast('正在按公式计算…', { level: 'info' })
    const res = await apiJson(`/api/report-design/reports/${enc(st.props.reportCode)}/compute`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ version: st.props.version || '', orgCode, periodCode }),
    })
    const computed = res?.computed || 0
    const errs = res?.errorCount || 0
    if (errs > 0) {
      const detail = (res?.errors || []).slice(0, 3).join('；')
      showCmxToast(`计算完成：${computed} 格已算，${errs} 格异常（${detail}）`, { level: 'warning' })
    } else {
      showCmxToast(`计算完成：${computed} 个单元格已算并落库`, { level: 'success' })
    }
    // 计算已落 cr_cell_data，取数把算好的值刷回画布
    await loadData(sheet, st, root)
  } catch (err) {
    showCmxToast(`计算失败：${String(err?.message || err)}`, { level: 'error' })
  }
}

/** 存数：收集画布非公式有值单元格 → POST data（按 org+period UPSERT cr_cell_data）。 */
async function saveData (sheet, st, root) {
  const { orgCode, periodCode } = st.props
  if (!orgCode || !periodCode) { showCmxToast('缺少组织或期间上下文', { level: 'error' }); return }
  const wb = sheet.getWorkbook?.()
  const ws = wb?.getActiveSheet?.()
  if (!ws) { showCmxToast('工作簿未就绪', { level: 'error' }); return }
  const sheetCode = ws.name ? ws.name() : 'Sheet1'
  const cells = []
  const rc = Math.min(ws.getRowCount ? ws.getRowCount() : 0, 500)
  const cc = Math.min(ws.getColumnCount ? ws.getColumnCount() : 0, 100)
  for (let r = 0; r < rc; r++) {
    for (let c = 0; c < cc; c++) {
      const formula = ws.getFormula ? ws.getFormula(r, c) : null
      if (formula) continue // 公式格不落数据（由取数/计算产生）
      const val = ws.getValue ? ws.getValue(r, c) : null
      if (val === null || val === undefined || val === '') continue
      const isNum = typeof val === 'number' && Number.isFinite(val)
      cells.push({
        sheetCode, regionCode: DEFAULT_REGION,
        // row_id/col_id 用画布网格位置(1基)作稳定唯一键——cr_cell_data 唯一键含 row_id+col_id，
        // 若全用 0 会让所有单元格撞同一键、互相覆盖。装载按 cellRef 回填，故此处只需保证每格唯一。
        rowId: r + 1, colId: c + 1, cellRef: `${indexToCol(c)}${r + 1}`,
        valueType: isNum ? 'number' : 'text',
        textValue: isNum ? null : String(val),
        numValue: isNum ? String(val) : null,
        isManual: 1,
      })
    }
  }
  try {
    await apiJson(`/api/report-design/reports/${enc(st.props.reportCode)}/data`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ version: st.props.version || '', orgCode, periodCode, cells }),
    })
    markDirty(st, false) // 存数成功 → 清除未保存标记
    showCmxToast(`已保存 ${cells.length} 个单元格数据`, { level: 'success' })
    return true
  } catch (err) {
    showCmxToast(`存数失败：${String(err?.message || err)}`, { level: 'error' })
    return false
  }
}

async function loadReportMeta (st) {
  if (st.report || st.reportLoading || !st.props.reportCode) return
  st.reportLoading = true
  try {
    const url = `/api/report-design/reports/${enc(st.props.reportCode)}${st.props.version ? `?version=${enc(st.props.version)}` : ''}`
    const data = await apiJson(url)
    st.report = data?.report || null
  } catch (_) { /* 防御性忽略 */ } finally {
    st.reportLoading = false
    refreshInstance(st, (v) => v === 'property')
  }
}

// ============================================================================
// 组件初始化 + 绑定
// ============================================================================

function ensureSpreadElementRegistered () {
  if (customElements.get('cmx-spreadjs-sheet')) return true
  const C = (typeof globalThis !== 'undefined' && globalThis.__cmxDataComp) || {}
  if (C.CmxSpreadjsSheet) {
    try { customElements.define('cmx-spreadjs-sheet', C.CmxSpreadjsSheet); return true } catch { /* 组件重复注册/预载：忽略 */ }
  }
  // 懒加载靠 index.js 的 MutationObserver（document.querySelector，穿不透 Shadow DOM）监听 sheet 标签首现。
  // 本页 <cmx-spreadjs-sheet> 挂在 native-page shadowRoot 内 → observer 看不到 → 永不注册。主动触发 preload。
  try { C.preloadSheetComponents?.() } catch { /* 组件重复注册/预载：忽略 */ }
  return false
}

/**
 * 监听门户「关闭含未保存修改的 tab → 点保存」派发的 portal-content-tab-save-request。
 * 仅当 tabId 命中本实例的 content 宿主时执行存数。每实例只挂一次。
 */
function setupSaveRequestListener (st) {
  if (st.__saveReqBound) return
  st.__saveReqBound = true
  document.addEventListener('portal-content-tab-save-request', (ev) => {
    const tabId = ev.detail?.tabId
    if (!tabId) return
    for (const host of Array.from(st.hosts)) {
      if (!host || !host.isConnected || host.__raView !== 'content') continue
      if (String(ownTabId(host)) !== String(tabId)) continue
      const root = host.renderRoot || host.shadowRoot?.querySelector('.native-page-root')
      const sheet = root?.querySelector?.('[data-ra-spread]')
      if (sheet) saveData(sheet, st, root)
      break
    }
  })
}

function initSpread (root, st) {
  const sheet = root.querySelector('[data-ra-spread]')
  if (!sheet || sheet.__raBound) return
  sheet.__raBound = true
  // 组件派发的 cmx-cell-edited（编程改值时）→ 置 dirty
  sheet.addEventListener('cmx-cell-edited', () => {
    if (st.__loading) return
    markDirty(st, true)
    updateApplierToolbarAll(st)
  })
  setupSaveRequestListener(st)
  // 组件的 async connectedCallback 里先 await ensureSpreadJs() 才 new Workbook()——首次进门户
  // SpreadJS 懒加载慢，此时 getWorkbook() 仍为 null。whenDefined 只保证「类已注册」，不保证
  // 「本实例工作簿已构建」。必须等 getWorkbook() 非空再 loadLayout，否则 setWorkbookJson/showHeaders
  // 均命中组件内 `if(!this._spread)return` 静默丢弃已存版式 → 显示初始骨架（关闭重开才好，因组件已热）。
  const whenWorkbookReady = () => new Promise((resolve) => {
    const t0 = Date.now()
    const tick = () => {
      let wb = null
      try { wb = sheet.getWorkbook && sheet.getWorkbook() } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ }
      if (wb) { resolve(wb); return }
      if (!sheet.isConnected || Date.now() - t0 > 20000) { resolve(null); return } // 宿主断开/超时兜底，不永久挂起
      setTimeout(tick, 60)
    }
    tick()
  })
  const apply = async () => {
    try {
      const wb = await whenWorkbookReady()
      if (!wb) { if (sheet.isConnected) throw new Error('SpreadJS 工作簿在 20s 内未就绪'); return }
      if (!sheet.isConnected) return // 等待期间切走了 tab：放弃装载
      // 隐藏组件自带的极简公式栏（名称框 + 裸 input，浮在列头上方）——应用器用自建 .ra-fxbar。
      // （HTML 属性 data-cmx-formula-bar="false" 已设，此处显式再关一次，避免 bootstrap 时序把它置回。）
      if (typeof sheet.showFormulaBar === 'function') sheet.showFormulaBar(false)
      if (typeof sheet.showHeaders === 'function') sheet.showHeaders(true)
      if (typeof sheet.showGridlines === 'function') sheet.showGridlines(true)
      // 应用器只跑数据，画布默认只读（避免误改版式）；存数收集的是画布当前值。
      if (typeof sheet.setEditable === 'function') sheet.setEditable(true)
      st.__loading = true
      // 一次后端调用取全集（版式+cellMap+元素+函数+数据）。失败回退旧多调用路径（保底）。
      const bundle = await openReportBundle(st)
      if (bundle) {
        const wbJson = decodeDoc(bundle?.fmt?.docContent)
        if (wbJson && sheet.setWorkbookJson) await sheet.setWorkbookJson(wbJson)   // ① 复原版式（含原生公式）
        else if (sheet.setReportModel) sheet.setReportModel(skeletonModel(st))
        applyCellFormulas(sheet, st)                                               // ② 设计器取数/计算公式落格 → 自动计算
        applyFloatExpansion(sheet, st, bundle)                                     // ②b 浮动区：模板行 × 数据源 → N 实例行落格
        if (bundle.hasData) applyCellsToCanvas(sheet, st, bundle.cells)            // ③ 灌数据（公式格取值自动算 / 非公式格直填）
        markDirty(st, false)
        refreshInstance(st, (v) => v === 'propertyStatus')
      } else {
        // 回退：旧顺序多调用（版式 → 自动取数 → 元素）
        await loadLayout(sheet, st, root).catch(() => { if (typeof sheet.setReportModel === 'function') sheet.setReportModel(skeletonModel(st)) })
        applyCellFormulas(sheet, st)
        if (st.props.orgCode && st.props.periodCode) await loadData(sheet, st, root, true)
        loadElements(st).catch(() => {})
      }
      setTimeout(() => { st.__loading = false }, 300)
      bindWorkbookEditEvents(sheet, st) // 用户键盘编辑靠这个（组件只绑了 CellChanged，用户输入不触发）
      bindFxSelectionSync(sheet, st, root) // 公式栏名称框/内容框随选区联动
      if ((st.zoom || 1) !== 1) { try { sheet.getWorkbook?.()?.getActiveSheet?.()?.zoom?.(st.zoom) } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ } } // 会话级缩放：加载后保持手感
      updateApplierToolbar(root, st) // 撤销/重做初始可用态 + 缩放控件同步
      fxSyncFromSelection(root, st) // 公式栏初次回填（A1）：元素胶囊 + 公式（从 cellMap，元素已随 bundle 到位）
    } catch (err) {
      st.__loading = false
      sheet.insertAdjacentHTML('afterend', `<div class="ra-note">SpreadJS 初始化失败：${esc(err instanceof Error ? err.message : String(err))}</div>`)
    }
  }
  if (ensureSpreadElementRegistered()) { apply(); return }
  customElements.whenDefined('cmx-spreadjs-sheet').then(apply)
  setTimeout(() => {
    if (!customElements.get('cmx-spreadjs-sheet')) {
      sheet.insertAdjacentHTML('afterend', '<div class="ra-note">cmx-spreadjs-sheet 组件尚未注册，请确认 cmx-data-comp 已预加载。</div>')
    }
  }, 1200)
}

/**
 * 直接绑 SpreadJS 的用户编辑事件 → markDirty。
 * ★ 组件内部只绑了 Events.CellChanged（编程改值触发），**用户键盘输入提交走 ValueChanged/EditEnded，
 * 不一定触发 CellChanged**，故在此补绑。SpreadJS 的编辑事件既可绑在 workbook 也可绑在 worksheet，
 * 为稳妥两者都绑。workbook.bind/sheet.bind 接受事件名字符串。工作簿未就绪则重试。
 */
function bindWorkbookEditEvents (sheet, st, tries = 0) {
  const wb = sheet.getWorkbook?.()
  if (!wb) {
    if (tries < 20) setTimeout(() => bindWorkbookEditEvents(sheet, st, tries + 1), 300)
    return
  }
  if (wb.__raEditBound) return
  wb.__raEditBound = true
  const onEdit = () => { if (!st.__loading) { markDirty(st, true); updateApplierToolbarAll(st) } }
  const EVENTS = ['ValueChanged', 'EditEnded', 'ClipboardPasted', 'RangeChanged', 'CellChanged', 'DragDropBlockCompleted', 'DragFillBlockCompleted']
  // workbook 级
  for (const name of EVENTS) { try { wb.bind(name, onEdit) } catch (_) { /* 事件名不被当前表格内核支持：跳过 */ } }
  // worksheet 级（部分编辑事件只在 sheet 上派发）——绑当前 + 后续所有 sheet
  const bindSheet = (ws) => {
    if (!ws || ws.__raEditBound) return
    ws.__raEditBound = true
    for (const name of EVENTS) { try { ws.bind(name, onEdit) } catch (_) { /* 事件名不被当前表格内核支持：跳过 */ } }
  }
  try { const cnt = wb.getSheetCount?.() || 1; for (let i = 0; i < cnt; i++) bindSheet(wb.getSheet?.(i)) } catch (_) { bindSheet(wb.getActiveSheet?.()) }
  try { wb.bind('ActiveSheetChanged', () => bindSheet(wb.getActiveSheet?.())) } catch (_) { /* 事件名不被当前表格内核支持：跳过 */ }
}

function bind (root, st, view) {
  if (view === 'content') {
    const sheet = root.querySelector('[data-ra-spread]')
    root.querySelectorAll('[data-ra-cmd]').forEach((btn) => btn.addEventListener('click', () => {
      const cmd = btn.getAttribute('data-ra-cmd')
      if (!sheet) { showCmxToast('画布未就绪', { level: 'error' }); return }
      closeRptMenu(root) // 菜单项点击后收起下拉
      if (cmd === 'load') loadData(sheet, st, root)
      else if (cmd === 'compute') computeData(sheet, st, root)
      else if (cmd === 'verify') verifyData(sheet, st, root)
      else if (cmd === 'save') saveData(sheet, st, root)
      else if (cmd === 'export') sheet.exportXlsx?.(`${st.props.reportCode || 'report'}-${st.props.orgCode || ''}-${st.curPeriod || st.props.periodCode || ''}`)
      else if (cmd === 'undo') { sheet.undo?.(); setTimeout(() => updateApplierToolbar(root, st), 30) }
      else if (cmd === 'redo') { sheet.redo?.(); setTimeout(() => updateApplierToolbar(root, st), 30) }
    }))
    bindZoomControls(root, st, sheet)
    bindSaveSplitMenu(root, st)
    bindHistoryMenus(root, st, sheet)
    if (sheet) bindFormulaBar(root, st, sheet)
    initSpread(root, st)
  } else if (view === 'explorer') {
    root.querySelector('[data-ra-period]')?.addEventListener('change', (ev) => {
      const val = ev.target.value || ''
      st.curPeriod = val
      st.props.periodCode = val // 后续取数/存数按新期间
      // content 页顶部徽标同步 + 更新 content 区 tab 标签
      refreshInstance(st, (v) => v === 'content' || v === 'propertyStatus')
      updateApplierTab(st)
      if (st.dataLoaded) showCmxToast(`期间已切到 ${val}，请在报表页点「取数」刷新数据`, { level: 'info' })
    })
  } else if (view === 'propertyStatus') {
    bindFloatPanel(root, st)
  }
}

/** 浮动明细维护：当前 org+period 的浮动行 CRUD（调 /float/rows/* 端点）。 */
function floatRegionCode (st) {
  // 目标浮动区：取上次 expand 的第一个行浮动区（axis!=col）。
  const regs = (st.__float && st.__float.regions) || []
  const r = regs.find((x) => x.axis !== 'col') || regs[0]
  return r ? { regionCode: r.regionCode, sheetCode: r.sheetCode } : { regionCode: '', sheetCode: '' }
}

function floatCtxBody (st, extra) {
  const { regionCode, sheetCode } = floatRegionCode(st)
  return Object.assign({
    version: st.props.version || '',
    sheetCode: sheetCode || 'Sheet1',
    regionCode: regionCode || '',
    orgCode: st.props.orgCode || '',
    periodCode: st.curPeriod || st.props.periodCode || '',
  }, extra || {})
}

async function loadFloatItems (st, root) {
  if (!st.props.orgCode || !(st.curPeriod || st.props.periodCode)) {
    st.__floatPanel = { loaded: true, items: [], kind: 'row' }
    refreshInstance(st, (v) => v === 'propertyStatus'); return
  }
  try {
    const data = await apiJson(`/api/report-design/reports/${enc(st.props.reportCode)}/float/rows/query`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(floatCtxBody(st)),
    })
    st.__floatPanel = { loaded: true, items: (data && data.items) || [], kind: 'row' }
  } catch (e) {
    st.__floatPanel = { loaded: true, items: [], kind: 'row' }
    showCmxToast(`浮动明细加载失败：${e instanceof Error ? e.message : String(e)}`, { level: 'error' })
  }
  refreshInstance(st, (v) => v === 'propertyStatus')
}

/** 保存全部：把当前面板行批量 UPSERT（手工 is_manual=1）。 */
async function saveFloatItems (st, root) {
  const fp = st.__floatPanel || { items: [] }
  const items = (fp.items || []).map((it, i) => ({
    id: it.id || 0,
    dimKey: it.dimKey,
    label: it.label || '',
    parentDimKey: it.parentDimKey || '',
    seq: it.seq != null ? it.seq : i,
    cells: it.cells || {},
  })).filter((it) => it.dimKey)
  try {
    await apiJson(`/api/report-design/reports/${enc(st.props.reportCode)}/float/rows`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(floatCtxBody(st, { items })),
    })
    showCmxToast(`已保存 ${items.length} 条浮动明细`, { level: 'success' })
    await loadFloatItems(st, root)
    reExpandCanvas(st, root)
  } catch (e) { showCmxToast(`保存失败：${e instanceof Error ? e.message : String(e)}`, { level: 'error' }) }
}

async function seedFloatItems (st, root) {
  try {
    const r = await apiJson(`/api/report-design/reports/${enc(st.props.reportCode)}/float/seed`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(floatCtxBody(st)),
    })
    showCmxToast(`已从取数初始化 ${r?.seeded ?? 0} 条（源:${r?.dataSource || '-'}）`, { level: 'success' })
    await loadFloatItems(st, root)
    reExpandCanvas(st, root)
  } catch (e) { showCmxToast(`初始化失败：${e instanceof Error ? e.message : String(e)}`, { level: 'error' }) }
}

async function deleteFloatItem (st, root, id, idx) {
  // 无 id（未保存的新行）→ 仅从面板移除
  if (!id) { (st.__floatPanel.items || []).splice(idx, 1); refreshInstance(st, (v) => v === 'propertyStatus'); return }
  try {
    await apiJson(`/api/report-design/reports/${enc(st.props.reportCode)}/float/rows/${enc(String(id))}`, { method: 'DELETE' })
    showCmxToast('已删除', { level: 'success' })
    await loadFloatItems(st, root)
    reExpandCanvas(st, root)
  } catch (e) { showCmxToast(`删除失败：${e instanceof Error ? e.message : String(e)}`, { level: 'error' }) }
}

/** 重展开画布：重新调 /expand（读存储表）并把结果落画布。 */
function reExpandCanvas (st, root) {
  // content 宿主里的 sheet：跨宿主取在屏画布重灌。
  for (const host of Array.from(st.hosts || [])) {
    const r = host.renderRoot || host.shadowRoot?.querySelector('.native-page-root')
    const sheet = r && r.querySelector && r.querySelector('[data-ra-spread]')
    if (sheet) { openReportBundle(st).then((b) => { if (b) { applyFloatExpansion(sheet, st, b); if (b.hasData) applyCellsToCanvas(sheet, st, b.cells) } }).catch(() => {}); break }
  }
}

function bindFloatPanel (root, st) {
  if (!st.__floatPanel || !st.__floatPanel.loaded) { loadFloatItems(st, root); return }
  root.querySelectorAll('[data-fp-cmd]').forEach((b) => b.addEventListener('click', () => {
    const cmd = b.getAttribute('data-fp-cmd')
    if (cmd === 'reload') loadFloatItems(st, root)
    else if (cmd === 'seed') seedFloatItems(st, root)
    else if (cmd === 'save') saveFloatItems(st, root)
    else if (cmd === 'add') {
      st.__floatPanel.items = st.__floatPanel.items || []
      st.__floatPanel.items.push({ id: 0, dimKey: `manual_${Date.now()}`, label: '新客户', cells: {}, isManual: 1, seq: st.__floatPanel.items.length })
      refreshInstance(st, (v) => v === 'propertyStatus')
    }
  }))
  root.querySelectorAll('[data-fp-del]').forEach((b) => b.addEventListener('click', () =>
    deleteFloatItem(st, root, b.getAttribute('data-fp-del'), Number(b.getAttribute('data-fp-i')))))
  // 行内编辑：label / B列值 写回面板 items（保存时批量提交）
  root.querySelectorAll('[data-fp-field]').forEach((el) => el.addEventListener('input', () => {
    const i = Number(el.getAttribute('data-fp-i'))
    const field = el.getAttribute('data-fp-field')
    const it = (st.__floatPanel.items || [])[i]
    if (!it) return
    if (field === 'label') it.label = el.value
    else if (field === 'cellB') { it.cells = it.cells || {}; it.cells.B = el.value }
  }))
}

/** 缩放：拖动 range 实时 / −+ 步进 / 点胶囊回 100%。 */
function applyApplierZoom (root, st, pct) {
  const p = Math.max(50, Math.min(200, Math.round(Number(pct) || 100)))
  st.zoom = p / 100
  const sheet = root.querySelector('[data-ra-spread]')
  try { sheet?.getWorkbook?.()?.getActiveSheet?.()?.zoom?.(p / 100) } catch (_) { /* 表格内核差异导致该调用被拒：降级跳过 */ }
  updateZoomControl(root, st)
}

/** 原地同步缩放控件（range 值 + 已选段渐变 + 百分数胶囊）到 st.zoom。 */
function updateZoomControl (root, st) {
  const box = root.querySelector('[data-ra-zoom]')
  if (!box) return
  const pct = Math.round((st.zoom || 1) * 100)
  box.style.setProperty('--ra-zoom-fill', `${((pct - 50) / 150) * 100}%`)
  const range = box.querySelector('[data-ra-zoom-range]')
  const focused = box.getRootNode?.()?.activeElement
  if (range && range !== focused) range.value = String(pct)
  const txt = box.querySelector('[data-ra-zoom-pct-text]')
  if (txt) txt.textContent = `${pct}%`
}

function bindZoomControls (root, st, sheet) {
  const range = root.querySelector('[data-ra-zoom-range]')
  range?.addEventListener('input', () => applyApplierZoom(root, st, range.value))
  root.querySelectorAll('[data-ra-zoom-step]').forEach((btn) => {
    btn.addEventListener('click', () => {
      const dir = Number(btn.getAttribute('data-ra-zoom-step')) || 0
      applyApplierZoom(root, st, Math.round((st.zoom || 1) * 100) + dir * 10)
    })
  })
  root.querySelector('[data-ra-zoom-reset]')?.addEventListener('click', () => applyApplierZoom(root, st, 100))
}

/** 收起存数▾下拉。 */
function closeRptMenu (root) {
  const wrap = root.querySelector('[data-ra-rpt]')
  if (wrap) { wrap.classList.remove('open'); wrap.querySelector('[data-ra-rpt-toggle]')?.setAttribute('aria-expanded', 'false') }
}

/** 存数▾分裂按钮：caret 开合 + fixed 菜单跟随定位 + 外点/resize 关闭。 */
function bindSaveSplitMenu (root, st) {
  const wrap = root.querySelector('[data-ra-rpt]')
  const toggle = root.querySelector('[data-ra-rpt-toggle]')
  const menu = root.querySelector('[data-ra-rpt-menu]')
  if (!wrap || !toggle || !menu) return
  const place = () => {
    const r = toggle.getBoundingClientRect()
    menu.style.top = `${Math.round(r.bottom + 4)}px`
    menu.style.left = `${Math.round(Math.min(r.right - 186, window.innerWidth - 194))}px`
  }
  toggle.addEventListener('click', (ev) => {
    ev.stopPropagation()
    const open = !wrap.classList.contains('open')
    wrap.classList.toggle('open', open)
    toggle.setAttribute('aria-expanded', open ? 'true' : 'false')
    if (open) place()
  })
  document.addEventListener('click', (ev) => { if (!wrap.contains(ev.target)) closeRptMenu(root) })
  window.addEventListener('resize', () => closeRptMenu(root))
  window.addEventListener('scroll', () => closeRptMenu(root), true)
}

/** 刷新撤销/重做按钮可用态（读组件 getHistoryState）。 */
function updateApplierToolbar (root, st) {
  const sheet = root.querySelector('[data-ra-spread]')
  const h = sheet?.getHistoryState?.() || {}
  root.querySelectorAll('[data-ra-cmd="undo"]').forEach((b) => { b.disabled = h.canUndo !== true })
  root.querySelectorAll('[data-ra-cmd="redo"]').forEach((b) => { b.disabled = h.canRedo !== true })
  root.querySelectorAll('[data-ra-hist-toggle="undo"]').forEach((b) => { b.disabled = h.canUndo !== true })
  root.querySelectorAll('[data-ra-hist-toggle="redo"]').forEach((b) => { b.disabled = h.canRedo !== true })
  updateZoomControl(root, st)
}

/** 渲染某一侧（undo|redo）的历史下拉列表（复用组件 getHistoryState 的堆栈）。 */
function renderHistoryMenu (root, sheet, kind) {
  const menu = root.querySelector(`[data-ra-hist-menu="${kind}"]`)
  if (!menu) return
  const h = sheet?.getHistoryState?.() || {}
  const items = kind === 'redo' ? (h.redo || []) : (h.undo || [])
  if (!items.length) {
    menu.innerHTML = `<span class="ra-hist-empty">暂无${kind === 'redo' ? '重做' : '撤销'}记录</span>`
    return
  }
  const title = kind === 'redo' ? '重做至此' : '撤销至此'
  menu.innerHTML = `<div class="ra-hist-title">${title}</div>` + items.slice(0, 30).map((it) => `<button class="ra-hist-item" type="button" data-ra-hist-step="${kind}" data-ra-hist-count="${Number(it.steps) || 1}"><span>${esc(it.label || (kind === 'redo' ? '重做' : '撤销'))}</span><small>${Number(it.steps) || 1}</small></button>`).join('')
}

function closeHistoryMenus (root) {
  root.querySelectorAll('[data-ra-history].open').forEach((el) => el.classList.remove('open'))
}

/** 撤销/重做 caret 下拉：开合 + 渲染历史列表 + 点「至此」批量撤销/重做（仿设计器）。 */
function bindHistoryMenus (root, st, sheet) {
  if (!sheet || root.__raHistoryBound) return
  root.__raHistoryBound = true
  root.querySelectorAll('[data-ra-hist-toggle]').forEach((btn) => {
    btn.addEventListener('click', (ev) => {
      ev.stopPropagation()
      if (btn.disabled) return
      const kind = btn.getAttribute('data-ra-hist-toggle') || 'undo'
      const wrap = btn.closest('[data-ra-history]')
      const willOpen = !wrap?.classList.contains('open')
      closeHistoryMenus(root)
      if (!willOpen || !wrap) return
      renderHistoryMenu(root, sheet, kind)
      wrap.classList.add('open')
    })
  })
  root.addEventListener('click', (ev) => {
    const item = ev.target.closest?.('[data-ra-hist-step]')
    if (!item) return
    const kind = item.getAttribute('data-ra-hist-step') || 'undo'
    const count = Math.max(1, Number(item.getAttribute('data-ra-hist-count')) || 1)
    if (kind === 'redo') sheet.redoSteps?.(count)
    else sheet.undoSteps?.(count)
    closeHistoryMenus(root)
    setTimeout(() => updateApplierToolbar(root, st), 30)
  })
  document.addEventListener('click', () => closeHistoryMenus(root))
}

/** 校验：占位——后端无同步单报表校验端点（rpt.verify 是异步批量作业，另案接入）。 */
function verifyData (sheet, st, root) {
  showCmxToast('校验引擎待接入（后续对接 rpt.verify 作业）', { level: 'info' })
}

// ── Excel 样式公式栏（名称框 + fx + 内容编辑器）：绑定 + 选区联动 ─────────────

/** 展开单格/区域地址（B4 / A1:C5）→ {r1,c1,r2,c2}；非法返回 null。 */
function expandRangeAddr (addr) {
  const s = String(addr || '').trim().toUpperCase()
  if (!s) return null
  const parts = s.split(':')
  const a = parseAddr(parts[0])
  if (!a) return null
  if (parts.length === 1) return { r1: a.row, c1: a.col, r2: a.row, c2: a.col }
  const b = parseAddr(parts[1])
  if (!b) return null
  return { r1: Math.min(a.row, b.row), c1: Math.min(a.col, b.col), r2: Math.max(a.row, b.row), c2: Math.max(a.col, b.col) }
}

/** 名称框跳转/选中当前 sheet 的单元格或区域。成功返回 true。 */
function gotoCellOrRange (sheet, st, addr) {
  const box = expandRangeAddr(addr)
  if (!box) return false
  const ws = sheet?.getWorkbook?.()?.getActiveSheet?.()
  if (!ws) return false
  const rows = box.r2 - box.r1 + 1
  const cols = box.c2 - box.c1 + 1
  try {
    ws.setActiveCell?.(box.r1, box.c1)
    ws.setSelection?.(box.r1, box.c1, rows, cols)
    try { ws.showCell?.(box.r1, box.c1, 3, 3) } catch { try { ws.showCell?.(box.r1, box.c1) } catch { /* 表格内核差异导致该调用被拒：降级跳过 */ } }
  } catch { return false }
  st.selectedCell = `${indexToCol(box.c1)}${box.r1 + 1}`
  st.selectedRange = rows === 1 && cols === 1 ? st.selectedCell : `${indexToCol(box.c1)}${box.r1 + 1}:${indexToCol(box.c2)}${box.r2 + 1}`
  return true
}

/** 选中格 → 刷新公式栏：左侧元素胶囊 + 内容框（设计器定义的取数/校验公式优先，回退画布原生公式/值）。 */
function fxSyncFromSelection (root, st) {
  const nb = root.querySelector('[data-ra-namebox]')
  const fx = root.querySelector('[data-ra-fxinput]')
  const focused = root.getRootNode?.()?.activeElement
  if (nb && nb !== focused) nb.value = st.selectedRange || st.selectedCell || 'A1'
  const addr = st.selectedCell || 'A1'
  const cm = (st.cellMap && st.cellMap[cellKey(st, addr)]) || {}
  // —— 左侧元素胶囊：有绑定才显，名称取自 st.elements（回退裸 code）——
  const chip = root.querySelector('[data-ra-fxelem]')
  if (chip) {
    const code = String(cm.elementCode || '').trim()
    if (code) {
      const el = (st.elements || []).find((x) => String(x.code) === code)
      const label = el ? (el.name ? `${el.name} (${code})` : code) : code
      const txt = chip.querySelector('[data-ra-fxelem-text]')
      if (txt) txt.textContent = label
      chip.title = `当前单元格绑定的数据元素：${label}`
      chip.hidden = false
    } else {
      chip.hidden = true
    }
  }
  // —— 内容框：设计器取数/校验公式优先，回退画布原生公式，再回退值（正在编辑不覆盖）——
  if (!fx || fx === focused) return
  const calc = String(cm.calcFormula || '').trim()
  const check = String(cm.checkFormula || '').trim()
  if (calc) { fx.value = /^=/.test(calc) ? calc : `=${calc}`; return }
  if (check) { fx.value = /^=/.test(check) ? check : `=${check}`; return }
  const ws = root.querySelector('[data-ra-spread]')?.getWorkbook?.()?.getActiveSheet?.()
  const p = parseAddr(addr)
  if (!ws || !p) { fx.value = ''; return }
  let formula = null
  try { formula = ws.getFormula ? ws.getFormula(p.row, p.col) : null } catch { /* 表格内核差异导致该调用被拒：降级跳过 */ }
  if (formula) { fx.value = `=${formula}`; return }
  let val = ''
  try { val = ws.getValue ? ws.getValue(p.row, p.col) : '' } catch { /* 表格内核差异导致该调用被拒：降级跳过 */ }
  fx.value = val == null ? '' : String(val)
}

/** 提交内容框：= 开头写公式，否则写值（数值自动转 number）。走组件 undo 栈。 */
function applyFxInput (sheet, st, root, raw) {
  const ws = sheet?.getWorkbook?.()?.getActiveSheet?.()
  const p = parseAddr(st.selectedCell || 'A1')
  if (!ws || !p) { showCmxToast('请先选中单元格', { level: 'error' }); return }
  const value = String(raw ?? '')
  const run = () => {
    if (value.startsWith('=')) ws.setFormula(p.row, p.col, value.slice(1))
    else {
      ws.setFormula(p.row, p.col, null)
      const num = value !== '' && !Number.isNaN(Number(value)) ? Number(value) : value
      ws.setValue(p.row, p.col, num)
    }
  }
  if (sheet._runUndoable) sheet._runUndoable('cmxFormulaBarEdit', run)
  else run()
  if (!st.__loading) markDirty(st, true)
  updateApplierToolbarAll(st)
}

/** 绑定公式栏：名称框跳转 + 内容框写回 + fx 按钮占位。 */
function bindFormulaBar (root, st, sheet) {
  if (root.__raFxbarBound) return
  root.__raFxbarBound = true
  const nb = root.querySelector('[data-ra-namebox]')
  const fx = root.querySelector('[data-ra-fxinput]')
  const fxBtn = root.querySelector('[data-ra-fxbtn]')
  const gotoNb = () => {
    const v = String(nb?.value || '').trim()
    if (!gotoCellOrRange(sheet, st, v)) {
      if (nb) nb.value = st.selectedRange || st.selectedCell || 'A1'
      showCmxToast('无效的单元格/区域地址（示例：B4 或 A1:C5）', { level: 'error' })
      return
    }
    fxSyncFromSelection(root, st)
    nb?.blur()
  }
  nb?.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter') { ev.preventDefault(); gotoNb() }
    else if (ev.key === 'Escape') { ev.preventDefault(); nb.value = st.selectedRange || st.selectedCell || 'A1'; nb.blur() }
  })
  nb?.addEventListener('focus', () => { try { nb.select() } catch { /* 焦点/选区 API 兼容性差异：忽略 */ } })
  nb?.addEventListener('change', gotoNb)
  const submitFx = () => applyFxInput(sheet, st, root, fx?.value ?? '')
  fx?.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter') { ev.preventDefault(); submitFx(); fx.blur() }
    else if (ev.key === 'Escape') { ev.preventDefault(); fxSyncFromSelection(root, st); fx.blur() }
  })
  fx?.addEventListener('change', submitFx)
  // fx 函数/公式编辑器：打开通用组件 cmx-fx-editor（内置函数内建、取数函数注入）。
  fxBtn?.addEventListener('click', () => openFxEditor(sheet, st, root, fxBtn))
}

/** 懒加载取数函数目录（GET /report-design/functions），供 fx 编辑器取数区。 */
async function loadApplierFunctions (st) {
  if (st.__fnLoaded) return st.__functions || []
  try {
    const data = await apiJson('/api/report-design/functions')
    st.__functions = Array.isArray(data?.functions) ? data.functions : []
  } catch (_) { st.__functions = [] }
  st.__fnLoaded = true
  return st.__functions
}

/** 报表专属参数控件（注入 cmx-fx-editor）：period/org/object/direction。应用器无 elements，object 退化文本框。 */
function raParamControls (st) {
  return {
    period: ({ value: val, attr: A, esc: e }) => {
      const opts = [['0', '本期(0)'], ['-1', '上期(-1)'], ['-2', '上两期(-2)'], ['-12', '上年同期(-12)']]
      const isAbs = !opts.some(([v]) => v === String(val)) && !!val
      const list = opts.map(([v, l]) => `<option value="${v}" ${String(val) === v ? 'selected' : ''}>${l}</option>`).join('')
      return `<select ${A}>${list}<option value="__abs" ${isAbs ? 'selected' : ''}>绝对期间…</option></select>
        <input ${A}-abs placeholder="或输入 2026-06" value="${e(isAbs ? val : '')}" class="fxe-abs">`
    },
    org: ({ value: val, attr: A, esc: e }) => {
      const isCode = val && val[0] !== '@'
      return `<select ${A}><option value="@current" ${val === '@current' || !val ? 'selected' : ''}>@当前组织</option>
        <option value="@parent" ${val === '@parent' ? 'selected' : ''}>@上级组织</option>
        <option value="__code" ${isCode ? 'selected' : ''}>指定组织码…</option></select>
        <input ${A}-code placeholder="组织码" value="${e(isCode ? val : '')}" class="fxe-abs">`
    },
    direction: ({ value: val, attr: A }) => `<select ${A}><option value="net" ${val === 'net' || !val ? 'selected' : ''}>净额</option>
      <option value="debit" ${val === 'debit' ? 'selected' : ''}>借方</option>
      <option value="credit" ${val === 'credit' ? 'selected' : ''}>贷方</option></select>`,
  }
}

/** 打开 fx 编辑器组件：注入取数函数 + 参数控件 + 初值；commit → 写画布公式。 */
function openFxEditor (sheet, st, root, anchorEl) {
  let el = root.querySelector('cmx-fx-editor[data-ra-fx]')
  if (!el) {
    el = document.createElement('cmx-fx-editor')
    el.setAttribute('data-ra-fx', '')
    root.appendChild(el)
    el.addEventListener('cmx-fx-commit', (ev) => {
      const expr = String(ev.detail?.expr || '').trim().replace(/^=+/, '')
      if (!expr) { showCmxToast('表达式为空', { level: 'error' }); return }
      const addr = ev.detail?.target || st.selectedCell || 'A1'
      if (addr !== st.selectedCell) gotoCellOrRange(sheet, st, addr)
      applyFxInput(sheet, st, root, '=' + expr) // 应用器无 cellMap，只写画布公式
      showCmxToast(`已写入 ${addr}：=${expr}`, { level: 'success' })
    })
  }
  el.configure({
    fetchFunctions: () => loadApplierFunctions(st),
    fetchTabLabel: '取数函数',
    paramControls: raParamControls(st),
    getInitialExpr: (addr) => readCellExpr(st, addr), // 设计器取数公式优先，回退画布原生公式（去前导 =）
    initialTarget: st.selectedCell || 'A1',
  })
  el.setCurrentCell(st.selectedCell || 'A1')
  el.open(anchorEl)
}

/** 选区联动：绑组件选区事件 + 250ms 兜底轮询，实时刷新名称框/内容框。 */
function bindFxSelectionSync (sheet, st, root, tries = 0) {
  const wb = sheet.getWorkbook?.()
  if (!wb) { if (tries < 20) setTimeout(() => bindFxSelectionSync(sheet, st, root, tries + 1), 300); return }
  if (wb.__raFxSelBound) return
  wb.__raFxSelBound = true
  const onSelect = () => {
    const addr = (typeof sheet.getActiveAddr === 'function') ? sheet.getActiveAddr() : null
    if (!addr) return
    const range = (typeof sheet.readSelection === 'function') ? sheet.readSelection() : addr
    // 切 sheet 时活动格地址可能不变（如都停 A1），但 cellKey 按 sheet 分——须强制重刷元素/公式。
    const sn = activeSheetName(st)
    if (addr === st.selectedCell && range === st.selectedRange && sn === st.__raSheetName) return
    st.selectedCell = addr
    st.selectedRange = range
    st.__raSheetName = sn
    fxSyncFromSelection(root, st)
  }
  const EVENTS = ['SelectionChanged', 'LeaveCell', 'EnterCell']
  for (const name of EVENTS) { try { wb.bind(name, onSelect) } catch (_) { /* 事件名不被当前表格内核支持：跳过 */ } }
  try { const cnt = wb.getSheetCount?.() || 1; for (let i = 0; i < cnt; i++) { const ws = wb.getSheet?.(i); for (const name of EVENTS) { try { ws?.bind?.(name, onSelect) } catch (_) { /* 事件名不被当前表格内核支持：跳过 */ } } } } catch (_) { /* 事件名不被当前表格内核支持：跳过 */ }
  try { wb.bind('ActiveSheetChanged', onSelect) } catch (_) { /* 事件名不被当前表格内核支持：跳过 */ }
  if (!st.__raFxSelPoll) {
    st.__raFxSelPoll = setInterval(() => {
      const alive = Array.from(st.hosts || []).some((h) => h && h.isConnected && h.__raView === 'content')
      if (!alive) { clearInterval(st.__raFxSelPoll); st.__raFxSelPoll = null; return }
      onSelect()
    }, 250)
  }
}

/** 跨所有在屏 content 宿主刷新工具栏（编辑事件里无 root 引用时用）。 */
function updateApplierToolbarAll (st) {
  for (const host of Array.from(st.hosts || [])) {
    if (!host || !host.isConnected || host.__raView !== 'content') continue
    const root = host.renderRoot || host.shadowRoot?.querySelector('.native-page-root')
    if (root) updateApplierToolbar(root, st)
  }
}

function mount (ctx, view) {
  const st = getState(ctx)
  const host = ctx.host
  st.hosts.add(host)
  if (host) host.__raView = view
  const render = () => {
    const root = host?.renderRoot || host?.shadowRoot?.querySelector('.native-page-root')
    if (!root || !root.isConnected) return
    root.innerHTML = `<style>${styleCss()}</style>${viewHtml(view, st)}`
    bind(root, st, view)
  }
  requestAnimationFrame(render)
  if (view === 'property') loadReportMeta(st)
  if (view === 'explorer') loadExplorer(st)
  return `<style>${styleCss()}</style>${viewHtml(view, st)}`
}

function refreshInstance (st, predicate) {
  for (const host of Array.from(st.hosts)) {
    if (!host || !host.isConnected) { st.hosts.delete(host); continue }
    const root = host.renderRoot || host.shadowRoot?.querySelector('.native-page-root')
    if (!root) continue
    const view = host.__raView || 'content'
    if (predicate && !predicate(view)) continue
    root.innerHTML = `<style>${styleCss()}</style>${viewHtml(view, st)}`
    bind(root, st, view)
  }
}

export default {
  defaultView: 'content',
  views: {
    async explorer (ctx) { return mount(ctx, 'explorer') },
    async content (ctx) { return mount(ctx, 'content') },
    async property (ctx) { return mount(ctx, 'property') },
    async propertyStatus (ctx) { return mount(ctx, 'propertyStatus') },
  },
}
