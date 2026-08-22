/**
 * N4+N5 合并工作台前端增强 —— CDP 冒烟(Playwright)。
 *
 * 验证:①「出表」按钮存在 ②第5 tab「合并报表」可切 ③CBS/CIS/CCF/CSE chip 可选并内嵌预览计算结果
 *       ④工作底稿抵销栏可下钻到分类账并高亮 ⑤分类账科目可回底稿。
 *
 * 运行:node cmx-report/scripts/e2e-consol-statements-frontend.mjs
 * 前置:门户 :8080 + report-server :8092 在跑;CAS_LEGAL/2026-06 已合并 + 出表 + CF/EQC 有数。
 */
import { chromium } from 'playwright'

const BASE = process.env.CMX_E2E_BASE || 'http://localhost:8080'
const results = []
const check = (name, ok, extra = '') => {
  results.push({ name, ok: !!ok, extra })
  console.log(`${ok ? '✅' : '❌'} ${name}${extra ? '  — ' + extra : ''}`)
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

const HARNESS = `
window.__cg = window.__cg || {}
window.__cg.bootstrap = async function() {
  const r = await fetch('/api/native-pages/portal.consol.workbench').then(r => r.json())
  const src = r.data.source
  const url = URL.createObjectURL(new Blob([src], { type: 'text/javascript' }))
  const mod = await import(url)
  const api = mod.default
  function makeHost() {
    const el = document.createElement('div')
    el.style.cssText = 'width:640px;height:560px;position:relative'
    document.body.appendChild(el)
    const sh = el.attachShadow({ mode: 'open' })
    const root = document.createElement('div'); root.className = 'native-page-root'
    root.style.cssText = 'width:100%;height:100%'
    sh.appendChild(root); el.renderRoot = root
    return el
  }
  const hosts = { explorer: makeHost(), content: makeHost(), property: makeHost() }
  await api.views.explorer({ host: hosts.explorer })
  await api.views.content({ host: hosts.content })
  await api.views.property({ host: hosts.property })
  window.__cg.hosts = hosts
  return true
}
`

async function main () {
  const browser = await chromium.launch()
  const page = await browser.newPage()
  page.on('console', (m) => { if (m.type() === 'error') console.log('  [page error]', m.text()) })
  await page.goto(BASE, { waitUntil: 'domcontentloaded' })
  await page.addScriptTag({ content: HARNESS })
  await page.evaluate('window.__cg.bootstrap()')

  // 等自动装载(schemes → 首方案 → 底稿)。
  for (let i = 0; i < 40; i++) {
    const ready = await page.evaluate(`(() => {
      const h = window.__cg.hosts
      const sel = h.explorer.renderRoot.querySelector('[data-scheme-select]')
      const rows = h.content.renderRoot.querySelectorAll('.cg-ws tbody tr').length
      return sel && sel.options.length > 0 && rows > 0
    })()`)
    if (ready) break
    await sleep(250)
  }

  // 选中 CAS_LEGAL 方案(有出表数据的方案)。
  await page.evaluate(`(async () => {
    const sel = window.__cg.hosts.explorer.renderRoot.querySelector('[data-scheme-select]')
    if (sel && sel.value !== 'CAS_LEGAL') { sel.value = 'CAS_LEGAL'; sel.dispatchEvent(new Event('change')) }
  })()`)
  await sleep(1500)
  // 选中期间 2026-06(本测试的 CF/EQC + 合并数据所在期)。
  await page.evaluate(`(async () => {
    const sel = window.__cg.hosts.explorer.renderRoot.querySelector('[data-period-select]')
    if (sel && sel.value !== '2026-06') { sel.value = '2026-06'; sel.dispatchEvent(new Event('change')) }
  })()`)
  await sleep(1500)

  // ① 出表按钮存在。
  const hasStmtBtn = await page.evaluate(`!!window.__cg.hosts.explorer.renderRoot.querySelector('[data-act="statements"]')`)
  check('explorer 有「出表」按钮', hasStmtBtn)

  // ② 第5 tab「合并报表」存在。
  const tabCount = await page.evaluate(`window.__cg.hosts.content.renderRoot.querySelectorAll('.cg-tab').length`)
  check('content 有 6 个 tab(含合并报表+附注)', tabCount === 6, `tabs=${tabCount}`)

  // 点「出表」→ seed + 聚合 + 切到合并报表 tab。
  await page.evaluate(`window.__cg.hosts.explorer.renderRoot.querySelector('[data-act="statements"]').click()`)
  await sleep(3000)

  // ③ 合并报表 tab 激活,四个 chip 存在,CBS 预览有数据。
  const st = await page.evaluate(`(() => {
    const root = window.__cg.hosts.content.renderRoot
    const chips = Array.from(root.querySelectorAll('[data-stmt]')).map(b => b.getAttribute('data-stmt'))
    const gridRows = root.querySelectorAll('.cg-stmt-table tbody tr').length
    const msg = root.querySelector('.cg-stmt-msg')?.textContent || ''
    const activeTab = root.querySelector('.cg-tab.active')?.textContent?.trim() || ''
    return { chips, gridRows, msg, activeTab }
  })()`)
  check('合并报表 tab 已激活', st.activeTab.includes('合并报表'), st.activeTab)
  check('四表 chip 齐全(CBS/CIS/CCF/CSE)', ['CBS','CIS','CCF','CSE'].every(c => st.chips.includes(c)), st.chips.join(','))
  check('CBS 内嵌预览有数据行', st.gridRows > 5, `rows=${st.gridRows} | ${st.msg}`)

  // ④ 切到 CCF chip → 预览现金流量表(有真数)。
  await page.evaluate(`window.__cg.hosts.content.renderRoot.querySelector('[data-stmt="CCF"]').click()`)
  await sleep(1500)
  const ccf = await page.evaluate(`(() => {
    const root = window.__cg.hosts.content.renderRoot
    const rows = Array.from(root.querySelectorAll('.cg-stmt-table tbody tr')).map(tr => Array.from(tr.querySelectorAll('td')).map(td => td.textContent.trim()))
    const msg = root.querySelector('.cg-stmt-msg')?.textContent || ''
    return { rows, msg }
  })()`)
  const ccfHasNum = ccf.rows.some(r => /\d/.test(r[1] || ''))
  check('CCF 预览含金额(真取数)', ccfHasNum, ccf.msg)

  // ⑤ N5:回工作底稿 tab,选中根节点(CSCEC 有抵销),找抵销栏可下钻的科目。
  await page.evaluate(`(() => {
    // 显式点根节点确保 worksheet 装载合并节点(有 elim)。
    const nodes = window.__cg.hosts.explorer.renderRoot.querySelectorAll('[data-node]')
    for (const n of nodes) { if (n.getAttribute('data-node') === 'CSCEC') { n.click(); break } }
  })()`)
  await sleep(1500)
  await page.evaluate(`window.__cg.hosts.content.renderRoot.querySelector('[data-tab="worksheet"]').click()`)
  await sleep(800)
  const drillSrc = await page.evaluate(`(() => {
    const el = window.__cg.hosts.content.renderRoot.querySelector('[data-drill-src]')
    return el ? el.getAttribute('data-drill-src') : ''
  })()`)
  check('工作底稿抵销栏有可下钻科目', !!drillSrc, `acc=${drillSrc}`)

  if (drillSrc) {
    // 点下钻 → 切到分类账 + 过滤条 + 高亮行。
    await page.evaluate(`window.__cg.hosts.content.renderRoot.querySelector('[data-drill-src]').click()`)
    await sleep(800)
    const drilled = await page.evaluate(`(() => {
      const root = window.__cg.hosts.content.renderRoot
      const bar = root.querySelector('.cg-drill-bar')
      const hl = root.querySelectorAll('.cg-journal tr.drill-hl').length
      const activeTab = root.querySelector('.cg-tab.active')?.textContent?.trim() || ''
      return { hasBar: !!bar, hl, activeTab }
    })()`)
    check('下钻切到分类账并过滤', drilled.hasBar && drilled.activeTab.includes('分类账'), `hl=${drilled.hl} tab=${drilled.activeTab}`)
    check('分类账高亮该科目行', drilled.hl > 0, `hl=${drilled.hl}`)

    // 从分类账科目点回底稿。
    await page.evaluate(`(() => { const el = window.__cg.hosts.content.renderRoot.querySelector('.cg-journal [data-drill-src]'); if (el) el.click() })()`)
    await sleep(800)
    const back = await page.evaluate(`window.__cg.hosts.content.renderRoot.querySelector('.cg-tab.active')?.textContent?.trim() || ''`)
    check('分类账科目可回工作底稿', back.includes('工作底稿'), back)
  }

  const pass = results.filter(r => r.ok).length
  console.log(`\n=== ${pass}/${results.length} passed ===`)
  await browser.close()
  process.exit(pass === results.length ? 0 : 1)
}

main().catch((e) => { console.error(e); process.exit(1) })
