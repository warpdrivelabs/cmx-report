/**
 * N4+N5 合并四表出表 + 抵销下钻 —— 截图产出(Playwright,存 docs/screenshots/)。
 *
 * 运行:node cmx-report/scripts/shots-consol-statements.mjs
 * 前置:门户 :8080 + report-server :8092 在跑;CAS_LEGAL/2026-06 已合并 + 出表 + CF/EQC 有数。
 */
import { chromium } from 'playwright'

const BASE = process.env.CMX_E2E_BASE || 'http://localhost:8080'
const SHOTS = process.env.SHOTS || 'docs/screenshots'
const log = (m) => console.log(m)
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

const HARNESS = `
window.__cg = window.__cg || {}
window.__cg.boot = async function() {
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
  const explorer = card('EXPLORER · 方案/期间/范围树 + 出表', 320, 640)
  const content  = card('CONTENT · 合并报表 / 下钻', 760, 640)
  window.__cg.hosts = { explorer, content }
  await api.views.explorer({ host: explorer })
  await api.views.content({ host: content })
  return true
}
window.__cg.pick = function(scheme){ const s=window.__cg.hosts.explorer.renderRoot.querySelector('[data-scheme-select]'); s.value=scheme; s.dispatchEvent(new Event('change',{bubbles:true})) }
window.__cg.period = function(p){ const s=window.__cg.hosts.explorer.renderRoot.querySelector('[data-period-select]'); if(s){s.value=p; s.dispatchEvent(new Event('change',{bubbles:true}))} }
window.__cg.tab = function(t){ const b=window.__cg.hosts.content.renderRoot.querySelector('[data-tab="'+t+'"]'); if(b)b.click() }
window.__cg.click = function(sel){ const el=window.__cg.hosts.content.renderRoot.querySelector(sel); if(el)el.click() }
window.__cg.act = function(a){ const el=window.__cg.hosts.explorer.renderRoot.querySelector('[data-act="'+a+'"]'); if(el)el.click() }
window.__cg.node = function(code){ const nodes=window.__cg.hosts.explorer.renderRoot.querySelectorAll('[data-node]'); for(const n of nodes){ if(n.getAttribute('data-node')===code){ n.click(); break } } }
`

async function main () {
  const browser = await chromium.launch({ headless: true })
  const ctx = await browser.newContext({ viewport: { width: 1140, height: 720 }, deviceScaleFactor: 2 })
  const page = await ctx.newPage()
  const shot = async (name) => { await page.screenshot({ path: `${SHOTS}/${name}.png` }); log(`  📸 ${name}.png`) }

  await page.goto(BASE, { waitUntil: 'domcontentloaded' })
  await page.addScriptTag({ content: HARNESS })
  await page.evaluate('window.__cg.boot()')
  await sleep(2000)
  await page.evaluate(`window.__cg.pick('CAS_LEGAL')`); await sleep(1500)
  await page.evaluate(`window.__cg.period('2026-06')`); await sleep(1500)
  await page.evaluate(`window.__cg.node('CSCEC')`); await sleep(1200)

  // 09 出表 → 合并报表 tab(CBS 预览)。
  await page.evaluate(`window.__cg.act('statements')`); await sleep(3000)
  await shot('09-statements-cbs')

  // 10 现金流量表(CCF 真取数)。
  await page.evaluate(`window.__cg.click('[data-stmt="CCF"]')`); await sleep(1500)
  await shot('10-statements-ccf')

  // 11 权益变动表(CSE 真取数)。
  await page.evaluate(`window.__cg.click('[data-stmt="CSE"]')`); await sleep(1500)
  await shot('11-statements-cse')

  // 12 抵销下钻:工作底稿 → 点抵销栏 → 分类账过滤高亮。
  await page.evaluate(`window.__cg.node('CSCEC')`); await sleep(800)
  await page.evaluate(`window.__cg.tab('worksheet')`); await sleep(800)
  await page.evaluate(`window.__cg.click('[data-drill-src]')`); await sleep(1000)
  await shot('12-drilldown-journal')

  log(`\n✅ 截图完成,存 ${SHOTS}/`)
  await browser.close()
}

main().catch((e) => { console.error(e); process.exit(1) })
