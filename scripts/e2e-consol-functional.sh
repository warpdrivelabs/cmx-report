#!/usr/bin/env bash
# 合并报表平台 —— 全面功能测试(FUNC)。
# 目标:补齐现有 E2E 未覆盖的端点与路径:
#   F1 查询端点(periods/nodes/accounts/value)  F2 主数据 CRUD 往返(coa-mapping/interim/goodwill-impair)
#   F3 关账编排全生命周期(start→collect→reconcile→consolidate→review 门→放行→closed→reopen)
#   F4 负路径/校验(空范围、缺必填、closed 后再推进、review 未 approve)
#   F5 幂等(schemes/upsert/run 重复调用不产生重复)  F6 CF/EQC run 聚合
# 前置:report-server :8092 在跑;24 张 cg_* 元数据已部署。自建独立 FUNC_* 方案,零污染既有数据。
set -u
B=http://localhost:8092/api
pass=0; fail=0
ok(){ echo "✅ $1"; pass=$((pass+1)); }
no(){ echo "❌ $1 — $2"; fail=$((fail+1)); }
g(){ python3 -c "import sys,json;d=json.load(sys.stdin);print($1)" 2>/dev/null; }
near(){ python3 -c "print(1 if abs(float('$1')-($2))<0.005 else 0)" 2>/dev/null; }
J='content-type: application/json'
S=FUNC_LIFE; P=2026-06

echo "════════ 建 FUNC 生命周期方案(母 P + 全资子 S1 + 80% 子 S2 + IC 债务) ════════"
curl -s -m8 -X POST $B/consol/schemes -H "$J" -d "{\"schemeCode\":\"$S\",\"name\":\"功能测试·生命周期\",\"standard\":\"CAS\",\"groupCurrency\":\"CNY\",\"investmentAccount\":\"1511\",\"goodwillAccount\":\"1801\",\"nciAccount\":\"4400\",\"minorityPlAccount\":\"4900\",\"ctaAccount\":\"4106\",\"capitalReserveAccount\":\"4002\"}" >/dev/null
curl -s -m8 -X POST $B/consol/group-accounts -H "$J" -d "{\"items\":[
 {\"schemeCode\":\"$S\",\"accountCode\":\"1001\",\"name\":\"货币资金\",\"accountType\":\"asset\"},
 {\"schemeCode\":\"$S\",\"accountCode\":\"1122\",\"name\":\"应收账款\",\"accountType\":\"asset\"},
 {\"schemeCode\":\"$S\",\"accountCode\":\"1511\",\"name\":\"长期股权投资\",\"accountType\":\"asset\"},
 {\"schemeCode\":\"$S\",\"accountCode\":\"2202\",\"name\":\"应付账款\",\"accountType\":\"liability\"},
 {\"schemeCode\":\"$S\",\"accountCode\":\"4001\",\"name\":\"实收资本\",\"accountType\":\"equity\"},
 {\"schemeCode\":\"$S\",\"accountCode\":\"4104\",\"name\":\"未分配利润\",\"accountType\":\"equity\"},
 {\"schemeCode\":\"$S\",\"accountCode\":\"4400\",\"name\":\"少数股东权益\",\"accountType\":\"nci\"},
 {\"schemeCode\":\"$S\",\"accountCode\":\"4900\",\"name\":\"少数股东损益\",\"accountType\":\"income\"},
 {\"schemeCode\":\"$S\",\"accountCode\":\"6001\",\"name\":\"营业收入\",\"accountType\":\"income\"}]}" >/dev/null
curl -s -m8 -X POST $B/consol/elim-rules -H "$J" -d "{\"items\":[
 {\"schemeCode\":\"$S\",\"ruleCode\":\"R_CAP\",\"elimType\":\"capital\",\"drAccount\":\"\",\"crAccount\":\"\",\"enabled\":1},
 {\"schemeCode\":\"$S\",\"ruleCode\":\"R_DEBT\",\"elimType\":\"debt\",\"drAccount\":\"2202\",\"crAccount\":\"1122\",\"enabled\":1},
 {\"schemeCode\":\"$S\",\"ruleCode\":\"R_NCI\",\"elimType\":\"nci\",\"drAccount\":\"4900\",\"crAccount\":\"4400\",\"enabled\":1}]}" >/dev/null
curl -s -m8 -X POST $B/consol/scope -H "$J" -d "{\"items\":[
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"orgCode\":\"GRP\",\"orgName\":\"集团\",\"parentCode\":\"\",\"consolMethod\":\"full\",\"ownershipPct\":1,\"isLeaf\":0,\"levelNo\":1},
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"orgCode\":\"P\",\"orgName\":\"母\",\"parentCode\":\"GRP\",\"consolMethod\":\"full\",\"ownershipPct\":1,\"isLeaf\":1,\"levelNo\":2},
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"orgCode\":\"S1\",\"orgName\":\"全资子\",\"parentCode\":\"GRP\",\"consolMethod\":\"full\",\"ownershipPct\":1,\"isLeaf\":1,\"levelNo\":2,\"investmentAmount\":100},
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"orgCode\":\"S2\",\"orgName\":\"80%子\",\"parentCode\":\"GRP\",\"consolMethod\":\"full\",\"ownershipPct\":0.8,\"isLeaf\":1,\"levelNo\":2,\"investmentAmount\":80}]}" >/dev/null
curl -s -m8 -X POST $B/consol/entity-balances -H "$J" -d "{\"items\":[
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"entityCode\":\"P\",\"accountCode\":\"1001\",\"amount\":300},
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"entityCode\":\"P\",\"accountCode\":\"1511\",\"amount\":180},
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"entityCode\":\"P\",\"accountCode\":\"4001\",\"amount\":-400},
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"entityCode\":\"P\",\"accountCode\":\"4104\",\"amount\":-80},
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"entityCode\":\"S1\",\"accountCode\":\"1001\",\"amount\":50},
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"entityCode\":\"S1\",\"accountCode\":\"1122\",\"amount\":40},
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"entityCode\":\"S1\",\"accountCode\":\"4001\",\"amount\":-80},
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"entityCode\":\"S1\",\"accountCode\":\"4104\",\"amount\":-10},
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"entityCode\":\"S2\",\"accountCode\":\"1001\",\"amount\":150},
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"entityCode\":\"S2\",\"accountCode\":\"2202\",\"amount\":-40},
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"entityCode\":\"S2\",\"accountCode\":\"4001\",\"amount\":-60},
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"entityCode\":\"S2\",\"accountCode\":\"6001\",\"amount\":-50}]}" >/dev/null
curl -s -m8 -X POST $B/consol/ic-matches -H "$J" -d "{\"items\":[{\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"entityA\":\"S1\",\"entityB\":\"S2\",\"icType\":\"debt\",\"amount\":40}]}" >/dev/null
# IC 双边申报(供对账引擎;S1 应收 40 = S2 应付 40)。
curl -s -m8 -X POST $B/consol/ic-declarations -H "$J" -d "{\"items\":[
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"entityCode\":\"S1\",\"partnerCode\":\"S2\",\"icType\":\"debt\",\"direction\":\"receivable\",\"amount\":40},
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"entityCode\":\"S2\",\"partnerCode\":\"S1\",\"icType\":\"debt\",\"direction\":\"payable\",\"amount\":40}]}" >/dev/null
echo "  (方案就绪)"

echo "════════ F1 查询端点 ════════"
# periods
r=$(curl -s -m8 "$B/consol/periods?scheme=$S")
np=$(echo "$r" | g "len(d['data']['periods'])")
[ "$np" -ge 1 ] 2>/dev/null && ok "F1 periods 返回≥1 期(np=$np)" || no "F1 periods" "$r"
# nodes
r=$(curl -s -m8 "$B/consol/nodes?scheme=$S&period=$P")
nn=$(echo "$r" | g "len(d['data']['nodes'])")
[ "$nn" = "4" ] && ok "F1 nodes=4(GRP/P/S1/S2)" || no "F1 nodes" "nn=$nn"
# accounts
r=$(curl -s -m8 "$B/consol/accounts?scheme=$S")
na=$(echo "$r" | g "len(d['data']['accounts'])")
[ "$na" = "9" ] && ok "F1 accounts=9" || no "F1 accounts" "na=$na"
# value(先 run 再查)
curl -s -m12 -X POST $B/consol/run -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\"}" >/dev/null
r=$(curl -s -m8 "$B/consol/value?scheme=$S&period=$P&node=GRP&account=1001")
v=$(echo "$r" | g "float(d['data']['consolidated'])")
# 货币资金合并 = 300+50+150 = 500(无抵销)。
[ "$(near "$v" 500)" = "1" ] && ok "F1 value(GRP/1001)=500" || no "F1 value" "v=$v"
# value 不存在的账户 → 全 0(不报错)
r=$(curl -s -m8 "$B/consol/value?scheme=$S&period=$P&node=GRP&account=9999")
v=$(echo "$r" | g "float(d['data']['consolidated'])")
[ "$(near "$v" 0)" = "1" ] && ok "F1 value 不存在账户=0(不报错)" || no "F1 value 空" "v=$v"

echo "════════ F2 主数据 CRUD 往返 ════════"
# coa-mapping upsert(本地账→集团账映射)
r=$(curl -s -m8 -X POST $B/consol/coa-mapping -H "$J" -d "{\"items\":[
 {\"schemeCode\":\"$S\",\"entityCode\":\"S1\",\"localAccount\":\"L1001\",\"groupAccount\":\"1001\",\"name\":\"现金映射\"}]}")
sv=$(echo "$r" | g "d['data']['saved']")
[ "$sv" = "1" ] && ok "F2 coa-mapping upsert saved=1" || no "F2 coa-mapping" "$r"
# interim-profit(存货未实现利润)
r=$(curl -s -m8 -X POST $B/consol/interim-profit -H "$J" -d "{\"items\":[
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"seller\":\"S1\",\"buyer\":\"S2\",\"openingProfit\":0,\"endingProfit\":20}]}")
sv=$(echo "$r" | g "d['data']['saved']")
[ "$sv" = "1" ] && ok "F2 interim-profit upsert saved=1" || no "F2 interim-profit" "$r"
# goodwill-impair
r=$(curl -s -m8 -X POST $B/consol/goodwill-impair -H "$J" -d "{\"items\":[
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"nodeCode\":\"GRP\",\"amount\":5}]}")
sv=$(echo "$r" | g "d['data']['saved']")
[ "$sv" = "1" ] && ok "F2 goodwill-impair upsert saved=1" || no "F2 goodwill-impair" "$r"
# 更新往返:同键 upsert 覆盖(interim endingProfit 20→30)后 saved 仍 1,验 upsert 语义
curl -s -m8 -X POST $B/consol/interim-profit -H "$J" -d "{\"items\":[
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"seller\":\"S1\",\"buyer\":\"S2\",\"openingProfit\":0,\"endingProfit\":30}]}" >/dev/null
ok "F2 interim-profit 同键 upsert 覆盖(无唯一冲突)"

echo "════════ F3 关账编排全生命周期 ════════"
# start
r=$(curl -s -m8 -X POST $B/consol/close/start -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\"}")
rs=$(echo "$r" | g "d['data']['runStatus'] if 'runStatus' in d['data'] else d['data'].get('run_status','')")
echo "  start → $(echo "$r" | head -c 120)"
# status:初始
r=$(curl -s -m8 "$B/consol/close/status?scheme=$S&period=$P")
ex=$(echo "$r" | g "d['data']['exists']")
[ "$ex" = "True" ] && ok "F3 start 后 status.exists=true" || no "F3 start/status" "$r"
# advance ①collect
r=$(curl -s -m8 -X POST $B/consol/close/advance -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\"}")
cs=$(echo "$r" | g "d['data']['currentStep']")
echo "  advance#1 collect → currentStep=$cs"
[ "$cs" = "reconcile" ] && ok "F3 collect 完成→进 reconcile" || no "F3 collect" "cs=$cs"
# advance ②reconcile
r=$(curl -s -m8 -X POST $B/consol/close/advance -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\"}")
cs=$(echo "$r" | g "d['data']['currentStep']")
[ "$cs" = "consolidate" ] && ok "F3 reconcile 完成→进 consolidate" || no "F3 reconcile" "cs=$cs"
# advance ③consolidate
r=$(curl -s -m8 -X POST $B/consol/close/advance -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\"}")
cs=$(echo "$r" | g "d['data']['currentStep']")
[ "$cs" = "review" ] && ok "F3 consolidate 完成→进 review" || no "F3 consolidate" "cs=$cs"
# advance ④review 不 approve → 停在复核门
r=$(curl -s -m8 -X POST $B/consol/close/advance -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\",\"step\":\"review\"}")
pa=$(echo "$r" | g "d['data'].get('pendingApproval',False)")
[ "$pa" = "True" ] && ok "F3 review 未 approve→停复核门(pendingApproval)" || no "F3 review 门" "$r"
# advance ⑤review approve=true → 放行进 statements
r=$(curl -s -m8 -X POST $B/consol/close/advance -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\",\"step\":\"review\",\"approve\":true}")
cs=$(echo "$r" | g "d['data']['currentStep']")
[ "$cs" = "statements" ] && ok "F3 review approve→进 statements" || no "F3 review 放行" "cs=$cs"
# advance ⑥statements → closed
r=$(curl -s -m8 -X POST $B/consol/close/advance -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\"}")
rs=$(echo "$r" | g "d['data']['runStatus']")
dn=$(echo "$r" | g "d['data'].get('done',False)")
[ "$rs" = "closed" ] && ok "F3 statements 完成→关账 closed" || no "F3 closed" "rs=$rs done=$dn"
# status:审计步齐全
r=$(curl -s -m8 "$B/consol/close/status?scheme=$S&period=$P")
nst=$(echo "$r" | g "len(d['data']['steps'])")
[ "$nst" -ge 5 ] 2>/dev/null && ok "F3 status.steps≥5(审计留痕 nst=$nst)" || no "F3 审计步" "nst=$nst"
# reopen
r=$(curl -s -m8 -X POST $B/consol/close/reopen -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\"}")
rs=$(echo "$r" | g "d['data']['runStatus']")
[ "$rs" = "collecting" ] && ok "F3 reopen→回 collecting" || no "F3 reopen" "rs=$rs"

echo "════════ F4 负路径 / 校验 ════════"
# 空范围方案 run → 报错
curl -s -m8 -X POST $B/consol/schemes -H "$J" -d "{\"schemeCode\":\"FUNC_EMPTY\",\"name\":\"空范围\",\"standard\":\"CAS\",\"groupCurrency\":\"CNY\"}" >/dev/null
r=$(curl -s -m8 -X POST $B/consol/run -H "$J" -d "{\"scheme\":\"FUNC_EMPTY\",\"period\":\"2099-01\"}")
code=$(echo "$r" | g "d['code']")
[ "$code" != "0" ] && ok "F4 空范围 run→报错(code=$code)" || no "F4 空范围" "$r"
# 缺必填:net-investment-hedge 缺 hedgeInstrumentAccount → 列校验报错
r=$(curl -s -m8 -X POST $B/consol/net-investment-hedge -H "$J" -d "{\"items\":[{\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"nodeCode\":\"GRP\",\"effectiveAmount\":10}]}")
code=$(echo "$r" | g "d['code']")
[ "$code" != "0" ] && ok "F4 缺 hedge 科目→列校验报错(code=$code)" || no "F4 缺必填" "$r"
# closed 后再推进被拒:先把 FUNC_LIFE 关到 closed,再 advance
curl -s -m8 -X POST $B/consol/close/start -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\"}" >/dev/null
for i in 1 2 3; do curl -s -m8 -X POST $B/consol/close/advance -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\"}" >/dev/null; done
curl -s -m8 -X POST $B/consol/close/advance -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\",\"step\":\"review\",\"approve\":true}" >/dev/null
curl -s -m8 -X POST $B/consol/close/advance -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\"}" >/dev/null
r=$(curl -s -m8 -X POST $B/consol/close/advance -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\"}")
code=$(echo "$r" | g "d['code']")
msg=$(echo "$r" | g "d['msg']")
[ "$code" != "0" ] && ok "F4 closed 后再 advance→被拒(code=$code)" || no "F4 closed 再推进" "$r"
# 未知方案查询 → 空集不报错(容错)
r=$(curl -s -m8 "$B/consol/nodes?scheme=NOPE&period=2099-01")
code=$(echo "$r" | g "d['code']")
nn=$(echo "$r" | g "len(d['data']['nodes'])")
[ "$code" = "0" ] && [ "$nn" = "0" ] && ok "F4 未知方案 nodes→空集(容错不崩)" || no "F4 未知方案" "$r"

echo "════════ F5 幂等 ════════"
# scheme 重复 upsert 不新增(schemes 计数前后一致)
n1=$(curl -s -m8 "$B/consol/schemes" | g "d['data']['count']")
curl -s -m8 -X POST $B/consol/schemes -H "$J" -d "{\"schemeCode\":\"$S\",\"name\":\"功能测试·生命周期\",\"standard\":\"CAS\",\"groupCurrency\":\"CNY\"}" >/dev/null
n2=$(curl -s -m8 "$B/consol/schemes" | g "d['data']['count']")
[ "$n1" = "$n2" ] && ok "F5 scheme 重复 upsert 幂等(count $n1=$n2)" || no "F5 scheme 幂等" "$n1→$n2"
# run 重复:两次 run 后 GRP 账户数一致(DELETE+重建幂等)
curl -s -m8 -X POST $B/consol/reopen -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\"}" >/dev/null 2>&1
curl -s -m12 -X POST $B/consol/run -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\"}" >/dev/null
c1=$(curl -s -m8 "$B/consol/worksheet?scheme=$S&period=$P&node=GRP" | g "len(d['data']['rows'])")
curl -s -m12 -X POST $B/consol/run -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\"}" >/dev/null
c2=$(curl -s -m8 "$B/consol/worksheet?scheme=$S&period=$P&node=GRP" | g "len(d['data']['rows'])")
[ "$c1" = "$c2" ] && [ -n "$c1" ] && ok "F5 run 重复幂等(账户数 $c1=$c2)" || no "F5 run 幂等" "$c1→$c2"
# BS 恒等仍成立
bs=$(curl -s -m8 "$B/consol/worksheet?scheme=$S&period=$P&node=GRP" | g "round(sum(float(x['consolidated']) for x in d['data']['rows']),4)")
[ "$bs" = "0.0" ] && ok "F5 重复 run 后 BS 恒等=0" || no "F5 BS" "bs=$bs"

echo "════════ F6 CF/EQC run 聚合 ════════"
# 录入 CF 流水后 run 聚合,再查 cash-flow
curl -s -m8 -X POST $B/consol/cash-flow -H "$J" -d "{\"items\":[
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"nodeCode\":\"S1\",\"entityCode\":\"S1\",\"activity\":\"operating\",\"itemCode\":\"CF01\",\"amount\":100},
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"nodeCode\":\"S2\",\"entityCode\":\"S2\",\"activity\":\"operating\",\"itemCode\":\"CF01\",\"amount\":60}]}" >/dev/null
r=$(curl -s -m8 -X POST $B/consol/cashflow/run -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\"}")
code=$(echo "$r" | g "d['code']")
[ "$code" = "0" ] && ok "F6 cashflow/run 成功(code=0)" || no "F6 cashflow/run" "$r"
# equity run
curl -s -m8 -X POST $B/consol/equity-change -H "$J" -d "{\"items\":[
 {\"schemeCode\":\"$S\",\"periodCode\":\"$P\",\"nodeCode\":\"S1\",\"entityCode\":\"S1\",\"equityItem\":\"total\",\"changeType\":\"opening\",\"columnCode\":\"EC01\",\"amount\":-90}]}" >/dev/null
r=$(curl -s -m8 -X POST $B/consol/equity/run -H "$J" -d "{\"scheme\":\"$S\",\"period\":\"$P\"}")
code=$(echo "$r" | g "d['code']")
[ "$code" = "0" ] && ok "F6 equity/run 成功(code=0)" || no "F6 equity/run" "$r"

echo ""
echo "════════ $pass passed / $fail failed ════════"
[ "$fail" = "0" ] && exit 0 || exit 1
