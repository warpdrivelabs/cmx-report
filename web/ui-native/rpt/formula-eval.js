/**
 * formula-eval.js —— 报表公式的前端镜像引擎（对齐后端 cmx-rpt-formula）。
 *
 * 方案 §9「前端计算态（读缓存）」：同一套 DSL 文法 + 同一函数语义，但**取数函数改读缓存**——
 * QM/QC/FS/JE 从 applier 已按 org+period 取回的 cr_cell_data 缓存命中，不发计算请求、不碰 DB；
 * SUM/IF/ROUND/… 与后端逐位一致的纯 JS 实现，本地即算。用途：①应用态联动（改一格→依赖它的
 * 合计格前端即时重算）②设计态预览（输入公式→用缓存/样例值给预览数）。
 *
 * REF 跨表递归求值**只在后端做**（前端只读缓存、不递归），从源头避免两端递归语义漂移。
 *
 * 与后端 ast.rs/eval.rs 的对拍点：负号紧邻数字为负数字面量、函数名大写化、单元格引用 A1、
 * 组织记号 @current、除零按 0、缺失字段按 0、错误值 #REF!/#NAME? 逐层传播。
 */

// ─────────────────────── 词法 ───────────────────────

function lex (s) {
  const cs = String(s || '')
  const out = []
  let i = 0
  const prevAllowsUnary = () => {
    if (out.length === 0) return true
    const t = out[out.length - 1]
    return t.t === 'op' || t.t === '(' || t.t === ',' || t.t === ':'
  }
  while (i < cs.length) {
    const c = cs[i]
    if (/\s/.test(c)) { i++; continue }
    if (c === '(') { out.push({ t: '(' }); i++; continue }
    if (c === ')') { out.push({ t: ')' }); i++; continue }
    if (c === ',') { out.push({ t: ',' }); i++; continue }
    if (c === ':') { out.push({ t: ':' }); i++; continue }
    if (c === '+' || c === '*' || c === '/') { out.push({ t: 'op', v: c }); i++; continue }
    if (c === '-') {
      if (prevAllowsUnary() && i + 1 < cs.length && /[0-9.]/.test(cs[i + 1])) {
        let buf = '-'; i++
        while (i < cs.length && /[0-9.]/.test(cs[i])) { buf += cs[i]; i++ }
        out.push({ t: 'num', v: Number(buf) })
      } else { out.push({ t: 'op', v: '-' }); i++ }
      continue
    }
    if (c === '>' || c === '<' || c === '=' || c === '!') {
      if (i + 1 < cs.length && cs[i + 1] === '=') { out.push({ t: 'op', v: c + '=' }); i += 2 }
      else if (c === '!') { out.push({ t: 'op', v: '!' }); i++ }
      else if (c === '=') { throw new Error('单个 = 非法（用 ==）') }
      else { out.push({ t: 'op', v: c }); i++ }
      continue
    }
    if (c === '&' || c === '|') {
      if (i + 1 < cs.length && cs[i + 1] === c) { out.push({ t: 'op', v: c + c }); i += 2 }
      else throw new Error('非法运算符 ' + c)
      continue
    }
    if (c === '@') {
      i++; let buf = ''
      while (i < cs.length && /[A-Za-z0-9_]/.test(cs[i])) { buf += cs[i]; i++ }
      if (!buf) throw new Error('@ 后需跟组织记号')
      out.push({ t: 'org', v: buf.toLowerCase() })
      continue
    }
    if (c === '\'' || c === '"') {
      const q = c; i++; let buf = ''
      while (i < cs.length && cs[i] !== q) { buf += cs[i]; i++ }
      if (i >= cs.length) throw new Error('字符串未闭合')
      i++; out.push({ t: 'str', v: buf })
      continue
    }
    if (/[0-9.]/.test(c)) {
      let buf = ''
      while (i < cs.length && /[0-9.]/.test(cs[i])) { buf += cs[i]; i++ }
      out.push({ t: 'num', v: Number(buf) })
      continue
    }
    if (/[A-Za-z_]/.test(c)) {
      let buf = ''
      while (i < cs.length && /[A-Za-z0-9_.]/.test(cs[i])) { buf += cs[i]; i++ }
      const low = buf.toLowerCase()
      if (low === 'true') out.push({ t: 'bool', v: true })
      else if (low === 'false') out.push({ t: 'bool', v: false })
      else if (low === 'null') out.push({ t: 'null' })
      else out.push({ t: 'ident', v: buf })
      continue
    }
    throw new Error('非法字符 ' + c)
  }
  return out
}

// ─────────────────────── 语法 ───────────────────────

const isCellRef = (s) => /^[A-Za-z]+[0-9]+$/.test(s)

function parse (expr) {
  const toks = lex(expr)
  let pos = 0
  const peek = () => toks[pos]
  const next = () => toks[pos++]

  const parseExpr = () => parseOr()
  function parseOr () {
    let l = parseAnd()
    while (peek() && peek().t === 'op' && peek().v === '||') { next(); l = { n: 'bin', op: '||', l, r: parseAnd() } }
    return l
  }
  function parseAnd () {
    let l = parseCmp()
    while (peek() && peek().t === 'op' && peek().v === '&&') { next(); l = { n: 'bin', op: '&&', l, r: parseCmp() } }
    return l
  }
  function parseCmp () {
    let l = parseAdd()
    while (peek() && peek().t === 'op' && ['>', '<', '>=', '<=', '==', '!='].includes(peek().v)) {
      const op = next().v; l = { n: 'bin', op, l, r: parseAdd() }
    }
    return l
  }
  function parseAdd () {
    let l = parseMul()
    while (peek() && peek().t === 'op' && (peek().v === '+' || peek().v === '-')) {
      const op = next().v; l = { n: 'bin', op, l, r: parseMul() }
    }
    return l
  }
  function parseMul () {
    let l = parseUnary()
    while (peek() && peek().t === 'op' && (peek().v === '*' || peek().v === '/')) {
      const op = next().v; l = { n: 'bin', op, l, r: parseUnary() }
    }
    return l
  }
  function parseUnary () {
    if (peek() && peek().t === 'op' && (peek().v === '-' || peek().v === '!')) {
      const op = next().v; return { n: 'un', op, x: parseUnary() }
    }
    return parsePrimary()
  }
  function parsePrimary () {
    const t = next()
    if (!t) throw new Error('意外结束')
    if (t.t === 'num') return { n: 'num', v: t.v }
    if (t.t === 'str') return { n: 'str', v: t.v }
    if (t.t === 'bool') return { n: 'bool', v: t.v }
    if (t.t === 'null') return { n: 'null' }
    if (t.t === 'org') return { n: 'org', v: t.v }
    if (t.t === '(') { const e = parseExpr(); if (!next() || toks[pos - 1].t !== ')') throw new Error('缺少 )'); return e }
    if (t.t === 'ident') {
      if (peek() && peek().t === '(') {
        next(); const args = []
        if (!(peek() && peek().t === ')')) {
          for (;;) { args.push(parseExpr()); if (peek() && peek().t === ',') next(); else break }
        }
        if (!next() || toks[pos - 1].t !== ')') throw new Error('函数缺少 )')
        return { n: 'call', name: t.v.toUpperCase(), args }
      }
      if (isCellRef(t.v)) {
        if (peek() && peek().t === ':') {
          next(); const end = next()
          if (!end || end.t !== 'ident' || !isCellRef(end.v)) throw new Error('区间右端非法单元格')
          return { n: 'range', a: t.v.toUpperCase(), b: end.v.toUpperCase() }
        }
        return { n: 'cell', v: t.v.toUpperCase() }
      }
      return { n: 'ident', v: t.v }
    }
    throw new Error('意外 token')
  }

  const node = parseExpr()
  if (pos !== toks.length) throw new Error('表达式有多余 token')
  return node
}

// ─────────────────────── A1 区间展开 ───────────────────────

const colToIndex = (col) => { let n = 0; for (const ch of col.toUpperCase()) if (/[A-Z]/.test(ch)) n = n * 26 + (ch.charCodeAt(0) - 64); return n - 1 }
const indexToCol = (idx) => { let n = idx + 1; let s = ''; while (n > 0) { const r = (n - 1) % 26; s = String.fromCharCode(65 + r) + s; n = Math.floor((n - 1) / 26) } return s }
function splitCell (s) { const m = /^([A-Za-z]+)([0-9]+)$/.exec(s); return m ? { col: m[1].toUpperCase(), row: Number(m[2]) } : null }
function expandRange (a, b) {
  const p = splitCell(a); const q = splitCell(b)
  if (!p || !q) return []
  const c1 = Math.min(colToIndex(p.col), colToIndex(q.col)); const c2 = Math.max(colToIndex(p.col), colToIndex(q.col))
  const r1 = Math.min(p.row, q.row); const r2 = Math.max(p.row, q.row)
  const out = []
  for (let r = r1; r <= r2; r++) for (let c = c1; c <= c2; c++) out.push(indexToCol(c) + r)
  return out
}

// ─────────────────────── 值语义 ───────────────────────

const isErr = (v) => typeof v === 'object' && v !== null && v.__err
const err = (e) => ({ __err: true, e })
function asNum (v) {
  if (isErr(v)) return 0
  if (typeof v === 'number') return v
  if (typeof v === 'boolean') return v ? 1 : 0
  if (typeof v === 'string') { const n = Number(v.trim()); return isNaN(n) ? 0 : n }
  return 0
}
function asBool (v) {
  if (isErr(v)) return false
  if (typeof v === 'boolean') return v
  if (typeof v === 'number') return v !== 0
  if (typeof v === 'string') return v.length > 0
  return false
}
function asText (v) {
  if (isErr(v)) return v.e
  if (v == null) return ''
  return String(v)
}
const isEmpty = (v) => v == null || v === '' || (isErr(v))
function valuesEq (a, b) {
  if (typeof a === 'string' && typeof b === 'string') return a === b
  if (a == null && b == null) return true
  if (typeof a === 'boolean' && typeof b === 'boolean') return a === b
  return Math.abs(asNum(a) - asNum(b)) < 1e-9
}

// ─────────────────────── 求值 ───────────────────────

/**
 * ctx: {
 *   org, period,                     // 当前上下文
 *   periods: [...升序期间码],          // 相对期间偏移
 *   orgParent: { code: parentCode },  // @parent 解析
 *   fetch(kind, period, org, object), // 取数缓存读取（返回 number|null）——QM/QC/FS/JE
 *   cell(ref),                        // 本表单元格缓存读取（返回值|null）
 * }
 */
function resolvePeriodOffset (ctx, offset) {
  if (!offset || !ctx.periods || !ctx.periods.length) return ctx.period
  const idx = ctx.periods.indexOf(ctx.period)
  if (idx < 0) return ctx.period
  const t = idx + offset
  return (t >= 0 && t < ctx.periods.length) ? ctx.periods[t] : ctx.period
}
function resolveOrgRef (ctx, name) {
  if (name === 'current' || name === 'self') return ctx.org
  if (name === 'parent') return (ctx.orgParent && ctx.orgParent[ctx.org]) || ctx.org
  if (name === 'root') { let cur = ctx.org; const seen = new Set(); while (ctx.orgParent && ctx.orgParent[cur] && !seen.has(cur)) { seen.add(cur); cur = ctx.orgParent[cur] } return cur }
  return name
}

function evalNode (node, ctx) {
  switch (node.n) {
    case 'num': return node.v
    case 'str': return node.v
    case 'bool': return node.v
    case 'null': return null
    case 'ident': { const v = ctx.fields ? ctx.fields[node.v] : undefined; return v == null ? 0 : v }
    case 'org': return resolveOrgRef(ctx, node.v)
    case 'cell': { const v = ctx.cell ? ctx.cell(node.v) : null; return v == null ? 0 : v }
    case 'range': return err('#RANGE!')
    case 'un': {
      const v = evalNode(node.x, ctx); if (isErr(v)) return v
      return node.op === '-' ? -asNum(v) : !asBool(v)
    }
    case 'bin': return evalBinary(node, ctx)
    case 'call': return evalCall(node.name, node.args, ctx)
    default: return err('#NODE!')
  }
}

function evalBinary (node, ctx) {
  const { op } = node
  if (op === '&&') { const l = evalNode(node.l, ctx); if (isErr(l)) return l; if (!asBool(l)) return false; const r = evalNode(node.r, ctx); return isErr(r) ? r : asBool(r) }
  if (op === '||') { const l = evalNode(node.l, ctx); if (isErr(l)) return l; if (asBool(l)) return true; const r = evalNode(node.r, ctx); return isErr(r) ? r : asBool(r) }
  const l = evalNode(node.l, ctx); const r = evalNode(node.r, ctx)
  if (isErr(l)) return l; if (isErr(r)) return r
  switch (op) {
    case '+': return (typeof l === 'string' || typeof r === 'string') ? asText(l) + asText(r) : asNum(l) + asNum(r)
    case '-': return asNum(l) - asNum(r)
    case '*': return asNum(l) * asNum(r)
    case '/': { const d = asNum(r); return d === 0 ? 0 : asNum(l) / d }
    case '>': return asNum(l) > asNum(r)
    case '<': return asNum(l) < asNum(r)
    case '>=': return asNum(l) >= asNum(r)
    case '<=': return asNum(l) <= asNum(r)
    case '==': return valuesEq(l, r)
    case '!=': return !valuesEq(l, r)
    default: return err('#OP!')
  }
}

/** 取数键派生（与后端 derive_balance_key 对齐）。 */
function deriveBalance (name, args, ctx) {
  const kind = { QM: 'end', QC: 'begin', JE: 'net', FS: 'fs' }[name]
  let period
  const a0 = args[0] ? evalNode(args[0], ctx) : null
  if (typeof a0 === 'number') period = resolvePeriodOffset(ctx, Math.trunc(a0))
  else { const t = asText(a0); period = t || ctx.period }
  const org = args[1] ? (asText(evalNode(args[1], ctx)) || ctx.org) : ctx.org
  const object = args[2] ? asText(evalNode(args[2], ctx)) : ''
  let k = kind
  if (name === 'FS') {
    const dir = args[3] ? asText(evalNode(args[3], ctx)).toLowerCase() : 'net'
    k = (dir === 'debit' || dir === 'dr' || dir === '借') ? 'debit'
      : (dir === 'credit' || dir === 'cr' || dir === '贷') ? 'credit' : 'net'
  }
  return { kind: k, period, org, object }
}

function argNums (arg, ctx) {
  if (arg.n === 'range') {
    const out = []
    for (const c of expandRange(arg.a, arg.b)) { const v = ctx.cell ? ctx.cell(c) : null; if (isErr(v)) return { err: v }; out.push(asNum(v == null ? 0 : v)) }
    return { nums: out }
  }
  const v = evalNode(arg, ctx); if (isErr(v)) return { err: v }
  return { nums: [asNum(v)] }
}

function evalCall (name, args, ctx) {
  switch (name) {
    case 'QM': case 'QC': case 'FS': case 'JE': {
      const b = deriveBalance(name, args, ctx)
      const v = ctx.fetch ? ctx.fetch(b.kind, b.period, b.org, b.object) : null
      return v == null ? 0 : v
    }
    case 'REF': {
      // 前端只读缓存、不递归；缓存未命中 → 回退（返 0，后端算权威值）
      const v = ctx.ref ? ctx.ref(args.map((a) => (a.n === 'cell' ? a.v : asText(evalNode(a, ctx))))) : null
      return v == null ? 0 : v
    }
    case 'SUM': { let s = 0; for (const a of args) { const r = argNums(a, ctx); if (r.err) return r.err; s += r.nums.reduce((x, y) => x + y, 0) } return s }
    case 'MIN': case 'MAX': {
      const vals = []; for (const a of args) { const r = argNums(a, ctx); if (r.err) return r.err; vals.push(...r.nums) }
      if (!vals.length) return 0
      return name === 'MIN' ? Math.min(...vals) : Math.max(...vals)
    }
    case 'ABS': { const v = evalNode(args[0], ctx); if (isErr(v)) return v; return Math.abs(asNum(v)) }
    case 'ROUND': {
      const v = evalNode(args[0], ctx); if (isErr(v)) return v
      const d = args[1] ? Math.trunc(asNum(evalNode(args[1], ctx))) : 0
      const f = Math.pow(10, d); return Math.round(asNum(v) * f) / f
    }
    case 'IF': {
      if (args.length < 2) return err('#ARG!')
      const c = evalNode(args[0], ctx); if (isErr(c)) return c
      return asBool(c) ? evalNode(args[1], ctx) : (args[2] ? evalNode(args[2], ctx) : null)
    }
    case 'AND': { for (const a of args) { const v = evalNode(a, ctx); if (isErr(v)) return v; if (!asBool(v)) return false } return true }
    case 'OR': { for (const a of args) { const v = evalNode(a, ctx); if (isErr(v)) return v; if (asBool(v)) return true } return false }
    case 'NOT': { const v = evalNode(args[0], ctx); if (isErr(v)) return v; return !asBool(v) }
    case 'ISEMPTY': return isEmpty(evalNode(args[0], ctx))
    case 'COALESCE': { for (const a of args) { const v = evalNode(a, ctx); if (isErr(v)) return v; if (!isEmpty(v)) return v } return null }
    default: return err('#NAME?' + name)
  }
}

/** 求值一个公式串（去掉可选前导 =）。返回 number|string|boolean|null 或 {__err,e}。 */
export function evalFormula (expr, ctx) {
  const src = String(expr || '').replace(/^\s*=/, '')
  if (!src.trim()) return null
  const ast = parse(src)
  return evalNode(ast, ctx || {})
}

/** 便捷：求值为数值（预览/联动用），错误/非数 → fallback。 */
export function evalNumber (expr, ctx, fallback = null) {
  try { const v = evalFormula(expr, ctx); return isErr(v) ? fallback : (typeof v === 'number' ? v : (isEmpty(v) ? fallback : asNum(v))) } catch { return fallback }
}

export { parse, expandRange, isErr, asNum, asText }
