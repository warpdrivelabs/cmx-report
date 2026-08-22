/**
 * 合并报表工作台 —— 前端功能逐项截图(Playwright + chromium)。
 * 加载门户反代的 native page,把 explorer/content/property 三区挂到带真实尺寸+浅色背景的 host,
 * 逐个功能驱动(切方案/切 tab/切期间)并 screenshot,产出到 SHOTS 目录供人工确认。
 *
 * 运行:node cmx-report/scripts/shots-consol-workbench.mjs
 */
import { chromium } from 'playwright'

const BASE = process.env.CMX_E2E_BASE || 'http://localhost:8080'
const SHOTS = process.env.SHOTS || '/tmp/claude-1000/-Users-nanomesh-Workspace-presentation/1c203842-52c3-ec0b-d4b5-e52246f6a83f/scratchpad/shots'
const log = (m) => console.log(m)
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

// 页面内:动态 import native page,三区挂到带边框卡片的 host。
const HARNESS = `
window.__cg = window.__cg || {}
window.__cg.boot = async function() {
  // 清掉登录页自身 DOM(保留同源以带 cookie fetch),铺干净背景。
  document.querySelectorAll('body > *').forEach(el => el.remove())
  document.body.style.cssText = 'margin:0;background:#eef2f6;font-family:system-ui;padding:16px;min-height:100vh'
  const r = await fetch('/api/native-pages/portal.consol.workbench').then(x=>x.json())
  const mod = await import(URL.createObjectURL(new Blob([r.data.source],{type:'text/javascript'})))
  const api = mod.default
  function card(title, w, h) {
    const wrap = document.createElement('div')
    wrap.style.cssText = 'display:inline-block;vertical-align:top;margin:0 12px 12px 0;background:#fff;border:1px solid #d6dee6;border-radius:12px;box-shadow:0 2px 10px rgba(0,0,0,.06);overflow:hidden'
    const cap = document.createElement('div')
    cap.textContent = title
    cap.style.cssText = 'font:600 12px system-ui;padding:8px 12px;background:#f4f7fa;border-bottom:1px solid #e4e9ee;color:#5b6b7f'
    const host = document.createElement('div')
    host.style.cssText = 'width:'+w+'px;height:'+h+'px;position:relative;overflow:auto'
    const sh = host.attachShadow({mode:'open'})
    const root = document.createElement('div'); root.className='native-page-root'; root.style.cssText='width:100%;height:100%'
    sh.appendChild(root); host.renderRoot = root
    wrap.appendChild(cap); wrap.appendChild(host)
    document.body.appendChild(wrap)
    return host
  }
  const explorer = card('EXPLORER · 方案/期间/范围树+操作', 320, 620)
  const content  = card('CONTENT · 工作底稿/对账/分类账/范围变动', 720, 620)
  const property = card('PROPERTY · 节点属性+平衡校验', 360, 620)
  await api.views.explorer({ host: explorer })
  await api.views.content({ host: content })
  await api.views.property({ host: property })
  window.__cg.hosts = { explorer, content, property }
  return true
}
window.__cg.pick = function(scheme){ const s=window.__cg.hosts.explorer.renderRoot.querySelector('[data-scheme-select]'); s.value=scheme; s.dispatchEvent(new Event('change',{bubbles:true})) }
window.__cg.period = function(p){ const s=window.__cg.hosts.explorer.renderRoot.querySelector('[data-period-select]'); if(s){s.value=p; s.dispatchEvent(new Event('change',{bubbles:true}))} }
window.__cg.tab = function(t){ const b=window.__cg.hosts.content.renderRoot.querySelector('[data-tab="'+t+'"]'); if(b)b.click() }
window.__cg.scheme = function(){ const s=window.__cg.hosts.explorer.renderRoot.querySelector('[data-scheme-select]'); return s?s.value:'' }
window.__cg.period_val = function(){ const s=window.__cg.hosts.explorer.renderRoot.querySelector('[data-period-select]'); return s?s.value:'' }
`

async function main () {
  const browser = await chromium.launch({ headless: true })
  const ctx = await browser.newContext({ viewport: { width: 1480, height: 720 }, deviceScaleFactor: 2 })
  const page = await ctx.newPage()
  const shot = async (name) => { await page.screenshot({ path: `${SHOTS}/${name}.png` }); log(`  📸 ${name}.png`) }

  await page.goto(`${BASE}/portal/login.html`, { waitUntil: 'domcontentloaded' })
  await page.addScriptTag({ content: HARNESS })
  await page.evaluate('window.__cg.boot()')

  // 等首方案(CAS_LEGAL)自动装载出底稿
  for (let i = 0; i < 40; i++) {
    await sleep(300)
    const ready = await page.evaluate(`(()=>{const r=window.__cg.hosts.content.renderRoot;return r.querySelectorAll('.cg-ws tbody tr').length>0})()`)
    if (ready) break
  }

  log('1. 三区总览(默认 CAS_LEGAL 工作底稿)')
  await shot('01-overview')

  log('2. 切 GW_TEST(商誉减值)—— 工作底稿含 adjust 栏')
  await page.evaluate(`window.__cg.pick('GW_TEST')`); await sleep(1500)
  await page.evaluate(`window.__cg.tab('worksheet')`); await sleep(600)
  await shot('02-gw-worksheet')

  log('3. GW_TEST 合并分类账(资本抵销+商誉减值凭证)')
  await page.evaluate(`window.__cg.tab('journal')`); await sleep(700)
  await shot('03-gw-journal')

  log('4. GW_TEST property 平衡校验')
  await shot('04-gw-full') // 整屏含 property

  log('5. 切 RECON_TEST 内部往来对账(差异行标红)')
  await page.evaluate(`window.__cg.pick('RECON_TEST')`); await sleep(1500)
  await page.evaluate(`window.__cg.tab('recon')`); await sleep(700)
  await shot('05-recon')

  log('6. 切 CAS_LEGAL 2026-12 范围变动(处置/新纳入徽标)')
  await page.evaluate(`window.__cg.pick('CAS_LEGAL')`); await sleep(1500)
  await page.evaluate(`window.__cg.period('2026-12')`); await sleep(1500)
  await page.evaluate(`window.__cg.tab('scope')`); await sleep(700)
  await shot('06-scope-change')

  log('7. INV_TEST 存货未实现利润(工作底稿 elim 栏)')
  await page.evaluate(`window.__cg.pick('INV_TEST')`); await sleep(1500)
  await page.evaluate(`window.__cg.period('2026-03')`); await sleep(1200)
  await page.evaluate(`window.__cg.tab('worksheet')`); await sleep(600)
  await shot('07-inv-worksheet')

  log('8. FX_TEST 外币折算(CTA 科目)')
  await page.evaluate(`window.__cg.pick('FX_TEST')`); await sleep(1500)
  await page.evaluate(`window.__cg.tab('worksheet')`); await sleep(600)
  await shot('08-fx-worksheet')

  await browser.close()
  log(`\n截图完成 → ${SHOTS}`)
}

main().catch((e) => { console.error(e); process.exit(1) })
