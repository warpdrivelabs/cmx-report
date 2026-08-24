/**
 * 合并报表工作台 native page —— CDP 冒烟测试(Playwright + chromium)。
 *
 * 不依赖门户菜单树:直接 fetch 门户反代的 /api/native-pages/portal.consol.workbench 源码,
 * 以 blob module 动态 import,挂 explorer/content/property 三区到 shadow host,驱动其调
 * 门户白名单 + 反代的 /api/consol/*(→ report-server:8092),断言真实合并数据渲染正确。
 *
 * 运行:node cmx-report/scripts/e2e-consol-workbench.mjs
 * 前置:门户 cmx-portal-server(:8080) + report-server(:8092) 均在跑,fico 已 seed 8 方案。
 */
import { chromium } from 'playwright'

const BASE = process.env.CMX_E2E_BASE || 'http://localhost:8080'
const results = []
const check = (name, ok, extra = '') => {
  results.push({ name, ok: !!ok, extra })
  console.log(`${ok ? '✅' : '❌'} ${name}${extra ? '  — ' + extra : ''}`)
}

// 页面内注入:mount 三区 + 轮询等待。作为字符串传入 evaluate。
const HARNESS = `
window.__cg = window.__cg || {}
async function bootstrap() {
  const r = await fetch('/api/native-pages/portal.consol.workbench').then(r => r.json())
  const src = r.data.source
  const url = URL.createObjectURL(new Blob([src], { type: 'text/javascript' }))
  const mod = await import(url)
  const api = mod.default
  function makeHost() {
    const el = document.createElement('div')
    el.style.cssText = 'width:420px;height:520px;position:relative'
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
window.__cg.bootstrap = bootstrap
`

const readState = `(() => {
  const h = window.__cg.hosts
  const q = (host, sel) => host.renderRoot.querySelector(sel)
  const qa = (host, sel) => Array.from(host.renderRoot.querySelectorAll(sel))
  const sel = q(h.explorer, '[data-scheme-select]')
  const rows = qa(h.content, '.cg-ws tbody tr').map(tr => {
    const tds = tr.querySelectorAll('td')
    return { acc: tds[0]?.querySelector('b')?.textContent || '', con: tds[4]?.textContent?.trim() || '' }
  })
  return {
    schemeOpts: sel ? sel.options.length : 0,
    selectedScheme: sel ? sel.value : '',
    periodOpts: (q(h.explorer, '[data-period-select]')?.options.length) || 0,
    nodeCount: qa(h.explorer, '[data-node]').length,
    tabs: qa(h.content, '.cg-tab').length,
    wsRows: rows.length,
    rows,
    tieText: q(h.content, '.cg-tie-badge')?.textContent?.trim() || '',
    checkTitle: q(h.property, '.cg-check-title')?.textContent?.trim() || '',
    runBtn: !!q(h.explorer, '[data-act="run"]'),
    reconBtn: !!q(h.explorer, '[data-act="reconcile"]'),
  }
})()`

const pickScheme = (code) => `(() => {
  const sel = window.__cg.hosts.explorer.renderRoot.querySelector('[data-scheme-select]')
  sel.value = ${JSON.stringify(code)}
  sel.dispatchEvent(new Event('change', { bubbles: true }))
  return sel.value
})()`

const clickTab = (tab) => `(() => {
  const btn = window.__cg.hosts.content.renderRoot.querySelector('[data-tab="${tab}"]')
  if (btn) { btn.click(); return true } return false
})()`

const readTab = `(() => {
  const root = window.__cg.hosts.content.renderRoot
  return {
    reconRows: root.querySelectorAll('.cg-table tbody tr').length,
    hasDiffRow: !!root.querySelector('tr.row-diff'),
    hasSuggestPanel: !!root.querySelector('.cg-suggest'),
    suggestRows: root.querySelectorAll('.cg-suggest .cg-table tbody tr').length,
    journalRules: Array.from(root.querySelectorAll('.cg-journal tbody tr .cg-rule')).map(e => e.textContent.trim()).filter(Boolean),
    journalTypes: Array.from(root.querySelectorAll('.cg-journal tbody tr .cg-badge')).map(e => e.textContent.trim()),
    text: root.textContent,
  }
})()`

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

async function main () {
  const browser = await chromium.launch({ headless: true })
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } })
  const page = await ctx.newPage()
  const errors = []
  page.on('pageerror', (e) => errors.push(String(e)))
  page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()) })

  try {
    await page.goto(`${BASE}/portal/login.html`, { waitUntil: 'domcontentloaded' })
    check('同源页加载(portal/login.html)', true)

    await page.addScriptTag({ content: HARNESS })
    const booted = await page.evaluate('window.__cg.bootstrap()')
    check('native page 动态 import + 三区挂载', booted === true)

    // 等自动装载(schemes → 首方案 → 底稿)
    let st
    for (let i = 0; i < 40; i++) {
      await sleep(300)
      st = await page.evaluate(readState)
      if (st.schemeOpts > 0 && st.wsRows > 0) break
    }
    check('explorer 方案下拉已填充', st.schemeOpts >= 7, `方案数=${st.schemeOpts}`)
    check('explorer 运行合并/对账按钮就位', st.runBtn && st.reconBtn)
    check('content 六 tab(底稿/对账/分类账/范围变动/合并报表/附注)', st.tabs === 6, `tabs=${st.tabs}`)
    check('首方案自动装载工作底稿', st.wsRows > 0, `行=${st.wsRows} 方案=${st.selectedScheme}`)

    // 切到 GW_TEST(商誉减值)—— 断言商誉1801=15.00、减值损失6701=5.00、借贷平衡
    await page.evaluate(pickScheme('GW_TEST'))
    let gw
    for (let i = 0; i < 30; i++) {
      await sleep(300)
      gw = await page.evaluate(readState)
      if (gw.selectedScheme === 'GW_TEST' && gw.wsRows > 0) break
    }
    const find = (rows, a) => rows.find((r) => r.acc === a)?.con || ''
    check('GW_TEST 节点树渲染', gw.nodeCount >= 3, `节点=${gw.nodeCount}`)
    check('GW_TEST 商誉(1801)合并数=15.00(减值后)', find(gw.rows, '1801') === '15.00', `实际=${find(gw.rows, '1801')}`)
    check('GW_TEST 资产减值损失(6701)=5.00', find(gw.rows, '6701') === '5.00', `实际=${find(gw.rows, '6701')}`)
    check('GW_TEST 借贷平衡校验(property)', gw.checkTitle.includes('借贷平衡'), gw.checkTitle)
    check('GW_TEST 底稿合计=✓平衡', gw.tieText.includes('平衡'), gw.tieText)

    // 分类账 tab —— 断言含 goodwill_impair 与 capital 凭证
    await page.evaluate(clickTab('journal'))
    await sleep(500)
    const jn = await page.evaluate(readTab)
    check('分类账含商誉减值凭证(R_GW)', jn.journalRules.includes('R_GW'), jn.journalRules.join(','))
    check('分类账含资本抵销凭证(R_CAP)', jn.journalRules.includes('R_CAP'))

    // 切到 RECON_TEST + 对账 tab —— 断言有差异行(A↔C matched180/diff20)
    await page.evaluate(pickScheme('RECON_TEST'))
    for (let i = 0; i < 25; i++) { await sleep(300); const s = await page.evaluate(readState); if (s.selectedScheme === 'RECON_TEST') break }
    await page.evaluate(clickTab('recon'))
    await sleep(600)
    const rc = await page.evaluate(readTab)
    check('RECON_TEST 对账表有行', rc.reconRows >= 2, `行=${rc.reconRows}`)
    check('RECON_TEST 差异行标红(diff)', rc.hasDiffRow)
    check('O3 自动调整建议面板渲染', rc.hasSuggestPanel && rc.suggestRows >= 1, `panel=${rc.hasSuggestPanel} rows=${rc.suggestRows}`)

    // 切到 CAS_LEGAL + 范围变动 tab —— 断言 2026-12 有处置/新纳入徽标
    await page.evaluate(pickScheme('CAS_LEGAL'))
    for (let i = 0; i < 25; i++) { await sleep(300); const s = await page.evaluate(readState); if (s.selectedScheme === 'CAS_LEGAL') break }
    // 选 2026-12 期间(范围变动的本期)
    await page.evaluate(`(() => {
      const sel = window.__cg.hosts.explorer.renderRoot.querySelector('[data-period-select]')
      if (sel) { sel.value = '2026-12'; sel.dispatchEvent(new Event('change', { bubbles: true })) }
    })()`)
    for (let i = 0; i < 25; i++) { await sleep(300); const s = await page.evaluate(readState); if (s.selectedPeriod === '2026-12') break }
    await page.evaluate(clickTab('scope'))
    await sleep(600)
    const sc = await page.evaluate(readTab)
    check('范围变动 tab 渲染(有徽标)', /处置|新纳入/.test(sc.text), sc.text.slice(0, 40))

    check('无页面级 JS 错误', errors.length === 0, errors.slice(0, 2).join(' | '))
  } catch (err) {
    check('运行异常', false, String(err))
  } finally {
    await browser.close()
  }

  const pass = results.filter((r) => r.ok).length
  console.log(`\n${pass}/${results.length} 通过`)
  process.exit(pass === results.length ? 0 : 1)
}

main()
